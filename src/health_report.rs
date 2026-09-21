use std::collections::BTreeMap;
use std::env;
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use crate::event_log::{EventReceipt, ExchangeReceipt};
use crate::fsx;
use crate::json::Json;

static REPORT_COUNTER: AtomicU64 = AtomicU64::new(1);

#[derive(Clone, Debug)]
pub(crate) struct HealthReporter {
    inbox: Option<PathBuf>,
}

impl HealthReporter {
    pub(crate) fn from_env() -> Result<Self, String> {
        let Some(value) = env::var_os("PARLEY_HEALTH_INBOX") else {
            return Ok(Self { inbox: None });
        };
        let inbox = PathBuf::from(value);
        if !inbox.is_absolute() {
            return Err(format!(
                "PARLEY_HEALTH_INBOX must be an absolute path: {}",
                inbox.display()
            ));
        }
        Ok(Self { inbox: Some(inbox) })
    }

    pub(crate) fn request_started(&self, receipt: &EventReceipt) -> Result<(), String> {
        self.write("request_started", "parley", None, None, receipt, None)
    }

    pub(crate) fn grok_success(&self, receipt: &ExchangeReceipt) -> Result<(), String> {
        self.write(
            "parley_observation",
            "grok",
            Some("usage_sample"),
            Some(true),
            &receipt.completion,
            None,
        )
    }

    pub(crate) fn turn_failure(
        &self,
        receipt: &ExchangeReceipt,
        class: &str,
    ) -> Result<(), String> {
        self.write(
            "parley_observation",
            "parley",
            Some(class),
            Some(false),
            &receipt.completion,
            None,
        )
    }

    pub(crate) fn logging_failure(&self, request: &EventReceipt) -> Result<(), String> {
        self.write(
            "parley_observation",
            "parley",
            Some("turn_error"),
            Some(false),
            request,
            None,
        )
    }

    pub(crate) fn footer_missing(&self, receipt: &ExchangeReceipt) -> Result<(), String> {
        self.write(
            "policy_diagnostic",
            "parley",
            None,
            None,
            &receipt.completion,
            Some("handoff_footer_missing"),
        )
    }

    pub(crate) fn mcp_stdout_undelivered(&self, receipt: &ExchangeReceipt) -> Result<(), String> {
        self.write(
            "parley_observation",
            "parley",
            Some("mcp_stdout_undelivered"),
            Some(false),
            &receipt.completion,
            None,
        )
    }

    fn write(
        &self,
        kind: &str,
        source: &str,
        class: Option<&str>,
        success: Option<bool>,
        receipt: &EventReceipt,
        diagnostic_code: Option<&str>,
    ) -> Result<(), String> {
        let Some(inbox) = &self.inbox else {
            return Ok(());
        };
        fsx::refuse_if_symlink(inbox)?;
        fs::create_dir_all(inbox)
            .map_err(|error| format!("create health inbox {}: {error}", inbox.display()))?;

        let inbox_id = format!("parley-{kind}-{}", receipt.event_id);
        let mut fields = BTreeMap::new();
        fields.insert("schema_version".to_string(), Json::Number(1.0));
        fields.insert("inbox_id".to_string(), Json::Str(inbox_id.clone()));
        fields.insert("kind".to_string(), Json::Str(kind.to_string()));
        fields.insert(
            "as_of_ms".to_string(),
            Json::Number(receipt.timestamp_ms as f64),
        );
        fields.insert("source".to_string(), Json::Str(source.to_string()));
        if let Some(class) = class {
            fields.insert("class".to_string(), Json::Str(class.to_string()));
        }
        if let Some(success) = success {
            fields.insert("success".to_string(), Json::Bool(success));
        }
        fields.insert(
            "session_id".to_string(),
            receipt
                .session_id
                .clone()
                .map(Json::Str)
                .unwrap_or(Json::Null),
        );
        fields.insert("event_id".to_string(), Json::Str(receipt.event_id.clone()));
        fields.insert(
            "exchange_id".to_string(),
            Json::Str(receipt.exchange_id.clone()),
        );
        if let Some(code) = diagnostic_code {
            fields.insert("diagnostic_code".to_string(), Json::Str(code.to_string()));
        }
        let mut bytes = Json::Object(fields).to_compact_string().into_bytes();
        bytes.push(b'\n');
        write_atomic(inbox, &inbox_id, &bytes)
    }

    #[cfg(test)]
    pub(crate) fn at(inbox: PathBuf) -> Self {
        Self { inbox: Some(inbox) }
    }
}

fn write_atomic(inbox: &Path, inbox_id: &str, bytes: &[u8]) -> Result<(), String> {
    let stamp = timestamp_ms();
    let sequence = REPORT_COUNTER.fetch_add(1, Ordering::Relaxed);
    let destination = inbox.join(format!("{stamp}-{inbox_id}.json"));
    let temporary = inbox.join(format!(
        ".{inbox_id}.{}.{}.tmp",
        std::process::id(),
        sequence
    ));
    fsx::refuse_if_symlink(&destination)?;
    let result = (|| {
        let mut file = OpenOptions::new()
            .create_new(true)
            .write(true)
            .open(&temporary)
            .map_err(|error| {
                format!("create health inbox temp {}: {error}", temporary.display())
            })?;
        file.write_all(bytes)
            .and_then(|_| file.flush())
            .and_then(|_| file.sync_all())
            .map_err(|error| format!("write health inbox temp {}: {error}", temporary.display()))?;
        drop(file);
        fs::rename(&temporary, &destination).map_err(|error| {
            format!(
                "publish health inbox {} -> {}: {error}",
                temporary.display(),
                destination.display()
            )
        })
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temporary);
    }
    result
}

fn timestamp_ms() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn receipt(event_type: &str) -> EventReceipt {
        EventReceipt {
            event_id: "event-1".to_string(),
            exchange_id: "exchange-1".to_string(),
            timestamp_ms: 42,
            event_type: event_type.to_string(),
            session_id: Some("01a06582-d66e-7811-b0c9-0b0266e17903".to_string()),
            logged: true,
        }
    }

    #[test]
    fn writes_only_sanitized_transport_identifiers() {
        let root = env::temp_dir().join(format!(
            "parley-health-report-{}-{}",
            std::process::id(),
            REPORT_COUNTER.fetch_add(1, Ordering::Relaxed)
        ));
        let reporter = HealthReporter::at(root.clone());
        let exchange = ExchangeReceipt {
            request: receipt("request"),
            completion: receipt("response"),
            target: "grok".to_string(),
        };
        reporter.mcp_stdout_undelivered(&exchange).unwrap();
        let file = fs::read_dir(&root)
            .unwrap()
            .flatten()
            .next()
            .unwrap()
            .path();
        let text = fs::read_to_string(file).unwrap();
        let json = Json::parse(&text).unwrap();
        assert_eq!(
            json.get("class").and_then(Json::as_str),
            Some("mcp_stdout_undelivered")
        );
        assert_eq!(
            json.get("exchange_id").and_then(Json::as_str),
            Some("exchange-1")
        );
        for forbidden in ["prompt", "reply", "command", "environment"] {
            assert!(!text.contains(forbidden));
        }
        fs::remove_dir_all(root).unwrap();
    }
}
