use std::sync::{Mutex, MutexGuard};

use serde::Serialize;

use crate::event_engine::{
    EventEngine, WidgetBrowserSnapshot, WidgetExchange, WidgetSelectionState, WidgetSnapshot,
};

#[derive(Debug)]
pub struct WidgetBrowser {
    inner: Mutex<BrowserState>,
}

#[derive(Debug)]
struct BrowserState {
    follow_live: bool,
    anchor: Option<ExchangeAnchor>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WidgetOpenSelection {
    pub event_key: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct ExchangeAnchor {
    exchange_key: String,
    source_identity: String,
    session_id: String,
    exchange_id: String,
    request_event_id: Option<String>,
    completion_event_id: Option<String>,
}

enum Resolved {
    Empty,
    Selected(usize),
    Missing,
    Ambiguous,
}

impl WidgetBrowser {
    pub fn new() -> Self {
        Self {
            inner: Mutex::new(BrowserState {
                follow_live: true,
                anchor: None,
            }),
        }
    }

    pub fn snapshot(&self, engine: &EventEngine) -> WidgetBrowserSnapshot {
        let mut state = self.lock();
        let exchanges = engine.widget_exchanges();
        project(&mut state, &exchanges)
    }

    pub fn older(&self, engine: &EventEngine) -> WidgetBrowserSnapshot {
        let mut state = self.lock();
        let exchanges = engine.widget_exchanges();
        if let Resolved::Selected(index) =
            resolve(state.follow_live, state.anchor.as_ref(), &exchanges)
        {
            if let Some(next) = exchanges.get(index.saturating_add(1)) {
                state.follow_live = false;
                state.anchor = Some(ExchangeAnchor::from_exchange(next));
            }
        }
        project(&mut state, &exchanges)
    }

    pub fn newer(&self, engine: &EventEngine) -> WidgetBrowserSnapshot {
        let mut state = self.lock();
        let exchanges = engine.widget_exchanges();
        if let Resolved::Selected(index) =
            resolve(state.follow_live, state.anchor.as_ref(), &exchanges)
        {
            if index == 0 {
                state.follow_live = true;
            } else {
                let next = &exchanges[index - 1];
                state.follow_live = index == 1;
                state.anchor = Some(ExchangeAnchor::from_exchange(next));
            }
        }
        project(&mut state, &exchanges)
    }

    pub fn live(&self, engine: &EventEngine) -> WidgetBrowserSnapshot {
        let mut state = self.lock();
        state.follow_live = true;
        let exchanges = engine.widget_exchanges();
        project(&mut state, &exchanges)
    }

    pub fn displayed_event(&self, engine: &EventEngine) -> Result<WidgetOpenSelection, String> {
        let state = self.lock();
        let exchanges = engine.widget_exchanges();
        let resolved = if state.anchor.is_some() {
            resolve(false, state.anchor.as_ref(), &exchanges)
        } else {
            resolve(state.follow_live, None, &exchanges)
        };
        let index = match resolved {
            Resolved::Selected(index) => index,
            Resolved::Empty => return Err("no widget exchange is available to open".to_string()),
            Resolved::Missing => {
                return Err("the displayed widget exchange is no longer available".to_string())
            }
            Resolved::Ambiguous => {
                return Err("the displayed widget exchange is ambiguous".to_string())
            }
        };
        let exchange = &exchanges[index].summary;
        let event = exchange
            .completion
            .as_ref()
            .or(exchange.request.as_ref())
            .ok_or_else(|| "the displayed widget exchange contains no event".to_string())?;
        Ok(WidgetOpenSelection {
            event_key: event.event_key.clone(),
        })
    }

    fn lock(&self) -> MutexGuard<'_, BrowserState> {
        self.inner
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }
}

impl Default for WidgetBrowser {
    fn default() -> Self {
        Self::new()
    }
}

impl ExchangeAnchor {
    fn from_exchange(exchange: &WidgetExchange) -> Self {
        Self {
            exchange_key: exchange.summary.exchange_key.clone(),
            source_identity: exchange.source_identity.clone(),
            session_id: exchange.summary.session_id.clone(),
            exchange_id: exchange.summary.exchange_id.clone(),
            request_event_id: event_id(&exchange.summary.request),
            completion_event_id: event_id(&exchange.summary.completion),
        }
    }
}

fn project(state: &mut BrowserState, exchanges: &[WidgetExchange]) -> WidgetBrowserSnapshot {
    let total = exchanges.len() as u64;
    match resolve(state.follow_live, state.anchor.as_ref(), exchanges) {
        Resolved::Empty => WidgetBrowserSnapshot {
            follow_live: true,
            selection_state: WidgetSelectionState::Empty,
            position: 0,
            total: 0,
            has_older: false,
            has_newer: false,
            newer_count: 0,
            widget: WidgetSnapshot::empty(),
        },
        Resolved::Missing => unresolved(state.follow_live, total, WidgetSelectionState::Missing),
        Resolved::Ambiguous => unresolved(false, total, WidgetSelectionState::Ambiguous),
        Resolved::Selected(index) => {
            let exchange = &exchanges[index];
            state.anchor = Some(ExchangeAnchor::from_exchange(exchange));
            let position = index as u64;
            WidgetBrowserSnapshot {
                follow_live: state.follow_live,
                selection_state: WidgetSelectionState::Selected,
                position,
                total,
                has_older: index.saturating_add(1) < exchanges.len(),
                has_newer: index > 0,
                newer_count: position,
                widget: WidgetSnapshot::from_exchange(&exchange.summary),
            }
        }
    }
}

fn unresolved(
    follow_live: bool,
    total: u64,
    selection_state: WidgetSelectionState,
) -> WidgetBrowserSnapshot {
    WidgetBrowserSnapshot {
        follow_live,
        selection_state,
        position: 0,
        total,
        has_older: false,
        has_newer: false,
        newer_count: 0,
        widget: WidgetSnapshot::empty(),
    }
}

fn resolve(
    follow_live: bool,
    anchor: Option<&ExchangeAnchor>,
    exchanges: &[WidgetExchange],
) -> Resolved {
    if exchanges.is_empty() {
        return if follow_live {
            Resolved::Empty
        } else {
            Resolved::Missing
        };
    }
    if follow_live {
        return Resolved::Selected(0);
    }
    let Some(anchor) = anchor else {
        return Resolved::Missing;
    };
    let key_hits = indexes(exchanges, |exchange| {
        exchange.summary.exchange_key == anchor.exchange_key
    });
    if key_hits.len() > 1 {
        return Resolved::Ambiguous;
    }
    if let Some(index) = key_hits.first().copied() {
        let exchange = &exchanges[index];
        return if same_conversation(anchor, exchange) {
            Resolved::Selected(index)
        } else {
            Resolved::Ambiguous
        };
    }
    recover_replaced(anchor, exchanges)
}

fn recover_replaced(anchor: &ExchangeAnchor, exchanges: &[WidgetExchange]) -> Resolved {
    let in_source = indexes(exchanges, |exchange| {
        exchange.source_identity == anchor.source_identity
    });
    let exchange_hits = in_source
        .iter()
        .copied()
        .filter(|index| {
            let summary = &exchanges[*index].summary;
            summary.exchange_id == anchor.exchange_id && summary.session_id == anchor.session_id
        })
        .collect::<Vec<_>>();
    let request_hits = hits_for_event(
        &in_source,
        exchanges,
        anchor.request_event_id.as_deref(),
        true,
    );
    let completion_hits = hits_for_event(
        &in_source,
        exchanges,
        anchor.completion_event_id.as_deref(),
        false,
    );
    if exchange_hits.len() > 1 || request_hits.len() > 1 || completion_hits.len() > 1 {
        return Resolved::Ambiguous;
    }
    if exchange_hits.is_empty() {
        return if request_hits.is_empty() && completion_hits.is_empty() {
            Resolved::Missing
        } else {
            Resolved::Ambiguous
        };
    }
    let index = exchange_hits[0];
    if request_hits.first().is_some_and(|hit| *hit != index)
        || completion_hits.first().is_some_and(|hit| *hit != index)
    {
        return Resolved::Ambiguous;
    }
    if !recorded_ids_compatible(anchor, &exchanges[index]) {
        return Resolved::Missing;
    }
    Resolved::Selected(index)
}

fn hits_for_event(
    in_source: &[usize],
    exchanges: &[WidgetExchange],
    event_id: Option<&str>,
    request: bool,
) -> Vec<usize> {
    let Some(event_id) = event_id else {
        return Vec::new();
    };
    in_source
        .iter()
        .copied()
        .filter(|index| {
            let summary = &exchanges[*index].summary;
            let preview = if request {
                summary.request.as_ref()
            } else {
                summary.completion.as_ref()
            };
            preview.is_some_and(|message| message.event_id == event_id)
        })
        .collect()
}

fn same_conversation(anchor: &ExchangeAnchor, exchange: &WidgetExchange) -> bool {
    exchange.source_identity == anchor.source_identity
        && exchange.summary.session_id == anchor.session_id
        && exchange.summary.exchange_id == anchor.exchange_id
}

fn recorded_ids_compatible(anchor: &ExchangeAnchor, exchange: &WidgetExchange) -> bool {
    id_compatible(
        anchor.request_event_id.as_deref(),
        event_id(&exchange.summary.request).as_deref(),
    ) && id_compatible(
        anchor.completion_event_id.as_deref(),
        event_id(&exchange.summary.completion).as_deref(),
    )
}

fn id_compatible(anchored: Option<&str>, current: Option<&str>) -> bool {
    match (anchored, current) {
        (None, _) => true,
        (Some(expected), Some(actual)) => expected == actual,
        (Some(_), None) => false,
    }
}

fn event_id(preview: &Option<crate::event_engine::MessagePreview>) -> Option<String> {
    preview.as_ref().map(|message| message.event_id.clone())
}

fn indexes(
    exchanges: &[WidgetExchange],
    mut matches: impl FnMut(&WidgetExchange) -> bool,
) -> Vec<usize> {
    exchanges
        .iter()
        .enumerate()
        .filter_map(|(index, exchange)| matches(exchange).then_some(index))
        .collect()
}

#[cfg(test)]
mod tests {
    use std::fs::{self, OpenOptions};
    use std::io::Write;
    use std::os::windows::fs::OpenOptionsExt;
    use std::path::{Path, PathBuf};
    use std::sync::atomic::{AtomicU64, Ordering};

    use serde_json::{json, Value};
    use windows_sys::Win32::Storage::FileSystem::{
        FILE_SHARE_DELETE, FILE_SHARE_READ, FILE_SHARE_WRITE,
    };

    use super::*;
    use crate::event_engine::EventType;

    static SEQ: AtomicU64 = AtomicU64::new(1);

    struct TempLog {
        path: PathBuf,
    }

    impl TempLog {
        fn new(label: &str) -> Self {
            let path = std::env::temp_dir().join(format!(
                "parley-viewer-widget-browser-{}-{}-{label}.jsonl",
                std::process::id(),
                SEQ.fetch_add(1, Ordering::Relaxed)
            ));
            let _ = fs::remove_file(&path);
            Self { path }
        }

        fn path(&self) -> &Path {
            &self.path
        }

        fn write_lines(&self, lines: &[String]) {
            let mut body = String::new();
            for line in lines {
                body.push_str(line);
                body.push('\n');
            }
            fs::write(&self.path, body).unwrap();
        }

        fn append_line(&self, line: &str) {
            let mut file = OpenOptions::new()
                .create(true)
                .append(true)
                .share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE)
                .open(&self.path)
                .unwrap();
            file.write_all(line.as_bytes()).unwrap();
            file.write_all(b"\n").unwrap();
            file.flush().unwrap();
        }
    }

    impl Drop for TempLog {
        fn drop(&mut self) {
            let _ = fs::remove_file(&self.path);
        }
    }

    fn event_line(
        event_type: &str,
        event_id: &str,
        exchange_id: &str,
        session_id: &str,
        content: Option<&str>,
        timestamp_ms: u64,
    ) -> String {
        let (status, duration, error) = match event_type {
            "request" => ("started", Value::Null, Value::Null),
            "error" => ("failed", json!(9), json!("boom")),
            _ => ("ok", json!(9), Value::Null),
        };
        json!({
            "schema_version": 1,
            "event_type": event_type,
            "event_id": event_id,
            "exchange_id": exchange_id,
            "timestamp_ms": timestamp_ms,
            "source": "codex",
            "target": "grok",
            "cwd": "C:\\worker",
            "session_id": session_id,
            "session_action": "new",
            "content": content,
            "status": status,
            "duration_ms": duration,
            "error": error
        })
        .to_string()
    }

    fn request(event_id: &str, exchange_id: &str, content: &str, timestamp_ms: u64) -> String {
        event_line(
            "request",
            event_id,
            exchange_id,
            "session-1",
            Some(content),
            timestamp_ms,
        )
    }

    fn response(event_id: &str, exchange_id: &str, content: &str, timestamp_ms: u64) -> String {
        event_line(
            "response",
            event_id,
            exchange_id,
            "session-1",
            Some(content),
            timestamp_ms,
        )
    }

    fn error(event_id: &str, exchange_id: &str, timestamp_ms: u64) -> String {
        event_line(
            "error",
            event_id,
            exchange_id,
            "session-1",
            None,
            timestamp_ms,
        )
    }

    fn load(log: &TempLog, lines: &[String]) -> EventEngine {
        log.write_lines(lines);
        let engine = EventEngine::new();
        engine.set_source(Some(log.path().to_path_buf())).unwrap();
        engine.poll();
        engine
    }

    fn replace_with(log: &TempLog, engine: &EventEngine, lines: &[String]) {
        fs::remove_file(log.path()).unwrap();
        assert!(engine.poll());
        log.write_lines(lines);
        assert!(engine.poll());
    }

    #[test]
    fn widget_browser_is_send_sync() {
        fn assert_send_sync<T: Send + Sync>() {}
        assert_send_sync::<WidgetBrowser>();
    }

    #[test]
    fn empty_state_stays_live_and_motion_does_not_invent_a_selection() {
        let engine = EventEngine::new();
        let browser = WidgetBrowser::new();
        let snapshot = browser.snapshot(&engine);
        assert_eq!(snapshot.selection_state, WidgetSelectionState::Empty);
        assert!(snapshot.follow_live);
        assert_eq!(snapshot.total, 0);
        assert!(!snapshot.has_older);
        assert!(!snapshot.has_newer);
        assert!(snapshot.widget.exchange_id.is_none());

        let older = browser.older(&engine);
        assert_eq!(older.selection_state, WidgetSelectionState::Empty);
        assert!(older.follow_live);
        let newer = browser.newer(&engine);
        assert!(newer.follow_live);
        assert_eq!(newer.selection_state, WidgetSelectionState::Empty);
    }

    #[test]
    fn live_follows_the_newest_and_older_leaves_live_only_when_an_older_exchange_exists() {
        let log = TempLog::new("live");
        let engine = load(
            &log,
            &[
                request("req-old", "ex-old", "older task", 10),
                response("res-old", "ex-old", "older reply", 11),
                request("req-new", "ex-new", "newer task", 20),
            ],
        );
        let browser = WidgetBrowser::new();
        let live = browser.snapshot(&engine);
        assert!(live.follow_live);
        assert_eq!(live.selection_state, WidgetSelectionState::Selected);
        assert_eq!(live.position, 0);
        assert_eq!(live.total, 2);
        assert_eq!(live.widget.exchange_id.as_deref(), Some("ex-new"));
        assert_eq!(
            live.widget.pending_label.as_deref(),
            Some("Request logged; no response event yet")
        );
        assert!(live.has_older);
        assert!(!live.has_newer);
        assert_eq!(live.newer_count, 0);
        assert_eq!(live.widget.request.as_ref().unwrap().speaker, "codex");
        assert_eq!(live.widget.request.as_ref().unwrap().recipient, "grok");

        let only = TempLog::new("one");
        let one = load(&only, &[request("req-only", "ex-only", "alone", 1)]);
        let single = WidgetBrowser::new();
        let stayed = single.older(&one);
        assert!(stayed.follow_live);
        assert_eq!(stayed.position, 0);
        assert_eq!(stayed.widget.exchange_id.as_deref(), Some("ex-only"));
        assert!(!stayed.has_older);

        let older = browser.older(&engine);
        assert!(!older.follow_live);
        assert_eq!(older.position, 1);
        assert_eq!(older.widget.exchange_id.as_deref(), Some("ex-old"));
        assert!(older.has_newer);
        assert_eq!(older.newer_count, 1);
        assert!(!older.has_older);
        assert_eq!(
            older.widget.completion.as_ref().unwrap().excerpt,
            "older reply"
        );
        assert_eq!(older.widget.completion.as_ref().unwrap().speaker, "grok");

        let bounded = browser.older(&engine);
        assert_eq!(bounded.position, 1);
        assert_eq!(bounded.widget.exchange_id.as_deref(), Some("ex-old"));

        let step = browser.newer(&engine);
        assert!(step.follow_live);
        assert_eq!(step.position, 0);
        assert_eq!(step.widget.exchange_id.as_deref(), Some("ex-new"));
        assert_eq!(step.newer_count, 0);
    }

    #[test]
    fn newer_steps_toward_newest_and_resumes_live_only_at_position_zero() {
        let log = TempLog::new("steps");
        let engine = load(
            &log,
            &[
                request("req-c", "ex-c", "oldest", 10),
                request("req-b", "ex-b", "middle", 20),
                request("req-a", "ex-a", "newest", 30),
            ],
        );
        let browser = WidgetBrowser::new();
        browser.older(&engine);
        let oldest = browser.older(&engine);
        assert!(!oldest.follow_live);
        assert_eq!(oldest.position, 2);
        assert_eq!(oldest.widget.exchange_id.as_deref(), Some("ex-c"));
        assert_eq!(oldest.newer_count, 2);
        assert!(!oldest.has_older);

        let middle = browser.newer(&engine);
        assert!(!middle.follow_live);
        assert_eq!(middle.position, 1);
        assert_eq!(middle.widget.exchange_id.as_deref(), Some("ex-b"));
        assert_eq!(middle.newer_count, 1);

        let resumed = browser.newer(&engine);
        assert!(resumed.follow_live);
        assert_eq!(resumed.position, 0);
        assert_eq!(resumed.widget.exchange_id.as_deref(), Some("ex-a"));
    }

    #[test]
    fn historical_selection_survives_ordinary_appends() {
        let log = TempLog::new("append");
        let engine = load(
            &log,
            &[
                request("req-old", "ex-old", "hold this", 10),
                request("req-new", "ex-new", "current", 20),
            ],
        );
        let browser = WidgetBrowser::new();
        let selected = browser.older(&engine);
        assert_eq!(selected.widget.exchange_id.as_deref(), Some("ex-old"));
        assert!(selected.widget.completion.is_none());

        log.append_line(&request("req-later", "ex-later", "arrived", 30));
        log.append_line(&response("res-old", "ex-old", "reply arrived", 12));
        assert!(engine.poll());

        let snapshot = browser.snapshot(&engine);
        assert!(!snapshot.follow_live);
        assert_eq!(snapshot.selection_state, WidgetSelectionState::Selected);
        assert_eq!(snapshot.widget.exchange_id.as_deref(), Some("ex-old"));
        assert_eq!(snapshot.position, 2);
        assert_eq!(snapshot.total, 3);
        assert_eq!(snapshot.newer_count, 2);
        assert_eq!(
            snapshot.widget.completion.as_ref().unwrap().excerpt,
            "reply arrived"
        );
        assert!(snapshot.widget.pending_label.is_none());
        assert_ne!(snapshot.widget.exchange_id.as_deref(), Some("ex-later"));
    }

    #[test]
    fn open_uses_the_last_rendered_anchor_until_the_next_snapshot() {
        let log = TempLog::new("open-rendered");
        let engine = load(
            &log,
            &[
                request("req-old", "ex-old", "older task", 10),
                response("res-old", "ex-old", "older reply", 11),
            ],
        );
        let browser = WidgetBrowser::new();
        let rendered = browser.snapshot(&engine);
        assert_eq!(rendered.widget.exchange_id.as_deref(), Some("ex-old"));
        assert_eq!(
            browser.displayed_event(&engine).unwrap().event_key,
            rendered.widget.completion.as_ref().unwrap().event_key
        );

        log.append_line(&request("req-new", "ex-new", "newest task", 20));
        assert!(engine.poll());
        assert_eq!(
            browser.displayed_event(&engine).unwrap().event_key,
            rendered.widget.completion.as_ref().unwrap().event_key
        );

        let refreshed = browser.snapshot(&engine);
        assert_eq!(refreshed.widget.exchange_id.as_deref(), Some("ex-new"));
        assert_eq!(
            browser.displayed_event(&engine).unwrap().event_key,
            refreshed.widget.request.as_ref().unwrap().event_key
        );
    }

    #[test]
    fn source_replacement_recovers_the_same_raw_identity() {
        let lines = vec![
            request("req-old", "ex-old", "hold this", 10),
            response("res-old", "ex-old", "same reply", 11),
            request("req-new", "ex-new", "current", 20),
        ];
        let log = TempLog::new("replace-exact");
        let engine = load(&log, &lines);
        let browser = WidgetBrowser::new();
        let selected = browser.older(&engine);
        let old_key = selected.widget.exchange_key.clone().unwrap();
        assert_eq!(selected.widget.exchange_id.as_deref(), Some("ex-old"));

        replace_with(&log, &engine, &lines);
        assert!(engine.status().generation >= 2);

        let recovered = browser.snapshot(&engine);
        assert_eq!(recovered.selection_state, WidgetSelectionState::Selected);
        assert!(!recovered.follow_live);
        assert_eq!(recovered.widget.exchange_id.as_deref(), Some("ex-old"));
        assert_eq!(recovered.widget.session_id.as_deref(), Some("session-1"));
        assert_eq!(
            recovered.widget.request.as_ref().unwrap().event_id,
            "req-old"
        );
        assert_eq!(
            recovered.widget.completion.as_ref().unwrap().event_id,
            "res-old"
        );
        assert_eq!(
            recovered.widget.completion.as_ref().unwrap().excerpt,
            "same reply"
        );
        assert_ne!(
            recovered.widget.exchange_key.as_deref(),
            Some(old_key.as_str())
        );
        assert_eq!(recovered.position, 1);
    }

    #[test]
    fn missing_identity_does_not_jump_to_another_exchange_or_source() {
        let original = vec![
            request("req-old", "ex-old", "hold this", 10),
            response("res-old", "ex-old", "reply", 11),
            request("req-new", "ex-new", "current", 20),
        ];
        let log = TempLog::new("missing");
        let engine = load(&log, &original);
        let browser = WidgetBrowser::new();
        browser.older(&engine);

        replace_with(
            &log,
            &engine,
            &[request("req-other", "ex-other", "different", 40)],
        );
        let missing = browser.snapshot(&engine);
        assert_eq!(missing.selection_state, WidgetSelectionState::Missing);
        assert!(!missing.follow_live);
        assert!(missing.widget.exchange_id.is_none());
        assert_eq!(missing.total, 1);
        assert!(!missing.has_older);
        assert!(!missing.has_newer);
        assert_eq!(
            browser.older(&engine).selection_state,
            WidgetSelectionState::Missing
        );
        assert_eq!(
            browser.newer(&engine).selection_state,
            WidgetSelectionState::Missing
        );

        let copy = TempLog::new("missing-copy");
        copy.write_lines(&original);
        engine.set_sources(vec![copy.path().to_path_buf()]).unwrap();
        engine.poll();
        let still_missing = browser.snapshot(&engine);
        assert_eq!(still_missing.selection_state, WidgetSelectionState::Missing);
        assert!(still_missing.widget.exchange_id.is_none());

        let recovered = browser.live(&engine);
        assert!(recovered.follow_live);
        assert_eq!(recovered.selection_state, WidgetSelectionState::Selected);
        assert_eq!(recovered.widget.exchange_id.as_deref(), Some("ex-new"));
    }

    #[test]
    fn ambiguous_identity_does_not_jump() {
        let log = TempLog::new("ambiguous");
        let engine = load(
            &log,
            &[
                request("req-old", "ex-old", "hold this", 10),
                response("res-old", "ex-old", "reply", 11),
                request("req-new", "ex-new", "current", 20),
            ],
        );
        let browser = WidgetBrowser::new();
        browser.older(&engine);

        replace_with(
            &log,
            &engine,
            &[
                request("req-old", "ex-old", "hold this", 10),
                response("res-other", "ex-old", "different completion", 11),
                request("req-moved", "ex-moved", "elsewhere", 12),
                response("res-old", "ex-moved", "reply", 13),
                request("req-new", "ex-new", "current", 20),
            ],
        );
        let ambiguous = browser.snapshot(&engine);
        assert_eq!(ambiguous.selection_state, WidgetSelectionState::Ambiguous);
        assert!(!ambiguous.follow_live);
        assert!(ambiguous.widget.exchange_id.is_none());
        assert!(ambiguous.total >= 2);
        assert_eq!(
            browser.older(&engine).selection_state,
            WidgetSelectionState::Ambiguous
        );
        let live = browser.live(&engine);
        assert!(live.follow_live);
        assert_eq!(live.widget.exchange_id.as_deref(), Some("ex-new"));
    }

    #[test]
    fn pending_and_error_records_keep_existing_widget_semantics() {
        let long = "é".repeat(500);
        let log = TempLog::new("semantics");
        let engine = load(
            &log,
            &[
                request("req-err", "ex-err", "fails", 10),
                error("err-1", "ex-err", 11),
                response("res-long", "ex-long", &long, 15),
                request("req-wait", "ex-wait", "still waiting", 30),
            ],
        );
        let browser = WidgetBrowser::new();
        let pending = browser.snapshot(&engine);
        assert_eq!(pending.widget.exchange_id.as_deref(), Some("ex-wait"));
        assert_eq!(
            pending.widget.pending_label.as_deref(),
            Some("Request logged; no response event yet")
        );
        assert!(pending.widget.completion.is_none());

        let long_exchange = browser.older(&engine);
        assert_eq!(long_exchange.widget.exchange_id.as_deref(), Some("ex-long"));
        let excerpt = &long_exchange.widget.completion.as_ref().unwrap().excerpt;
        assert_eq!(excerpt.chars().count(), 420);
        assert!(long.starts_with(excerpt));
        assert_eq!(
            long_exchange.widget.completion.as_ref().unwrap().event_type,
            EventType::Response
        );

        let failed = browser.older(&engine);
        assert_eq!(failed.widget.exchange_id.as_deref(), Some("ex-err"));
        let completion = failed.widget.completion.as_ref().unwrap();
        assert_eq!(completion.event_type, EventType::Error);
        assert_eq!(completion.excerpt, "boom");
        assert_eq!(completion.speaker, "codex");
        assert!(failed.widget.pending_label.is_none());
    }

    #[test]
    fn duplicate_raw_ids_in_another_source_do_not_steal_historical_selection() {
        let first = TempLog::new("source-a");
        let second = TempLog::new("source-b");
        first.write_lines(&[
            request("req-shared", "ex-shared", "from-a", 10),
            response("res-shared", "ex-shared", "reply-a", 11),
            request("req-live", "ex-live", "live-a", 40),
        ]);
        second.write_lines(&[
            request("req-shared", "ex-shared", "from-b", 10),
            response("res-shared", "ex-shared", "reply-b", 30),
        ]);
        let engine = EventEngine::new();
        engine
            .set_sources(vec![
                first.path().to_path_buf(),
                second.path().to_path_buf(),
            ])
            .unwrap();
        engine.poll();
        let browser = WidgetBrowser::new();
        let live = browser.snapshot(&engine);
        assert_eq!(live.widget.exchange_id.as_deref(), Some("ex-live"));
        assert_eq!(live.widget.request.as_ref().unwrap().excerpt, "live-a");

        let historical = browser.older(&engine);
        assert_eq!(historical.widget.exchange_id.as_deref(), Some("ex-shared"));
        assert_eq!(
            historical.widget.completion.as_ref().unwrap().excerpt,
            "reply-b"
        );

        browser.older(&engine);
        let pinned = browser.snapshot(&engine);
        assert_eq!(pinned.widget.request.as_ref().unwrap().excerpt, "from-a");
        assert_eq!(
            pinned.widget.completion.as_ref().unwrap().excerpt,
            "reply-a"
        );
        assert!(!pinned.follow_live);
    }
}
