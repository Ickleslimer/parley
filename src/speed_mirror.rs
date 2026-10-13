use std::io::{Read, Seek, SeekFrom};
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

use crate::ask::AskRequest;
use crate::context::{
    open_shared_read, refuse_reparse_chain, resolve_codex_rollout, resolve_codex_sessions_root,
    CodexHomeEnv,
};
use crate::json::Json;
use crate::policy::{is_uuid, RuntimePolicy};

const MAX_METADATA: u64 = 2048;
const MAX_TAIL: u64 = 8 * 1024 * 1024;

pub(crate) fn wants_fast(request: &AskRequest, policy: &RuntimePolicy, automatic: bool) -> bool {
    if !automatic || request.harness != "grok" {
        return false;
    }
    let Some(directory) = policy.codex_speed_dir() else {
        return false;
    };
    let Some(context) = request
        .context
        .as_ref()
        .filter(|context| context.harness == "codex" && is_uuid(&context.session))
    else {
        return false;
    };
    let Some(turn) = current_turn(&context.session) else {
        return false;
    };
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis() as u64)
        .unwrap_or(0);
    read_fast(directory, &context.session, &turn, now, host_alive).unwrap_or(false)
}

fn current_turn(session: &str) -> Option<String> {
    let root = resolve_codex_sessions_root(&CodexHomeEnv::from_process()).ok()?;
    let source = resolve_codex_rollout(&root, session).ok()?;
    refuse_reparse_chain(&source.path).ok()?;
    let mut file = open_shared_read(&source.path).ok()?;
    let size = file.metadata().ok()?.len();
    let offset = size.saturating_sub(MAX_TAIL);
    file.seek(SeekFrom::Start(offset)).ok()?;
    let mut bytes = Vec::new();
    file.take(MAX_TAIL).read_to_end(&mut bytes).ok()?;
    latest_turn(&bytes, offset > 0)
}

fn latest_turn(bytes: &[u8], partial_front: bool) -> Option<String> {
    let end = bytes.iter().rposition(|byte| *byte == b'\n')?;
    let start = if partial_front {
        bytes.iter().position(|byte| *byte == b'\n')? + 1
    } else {
        0
    };
    let mut latest = None;
    for line in bytes.get(start..end)?.split(|byte| *byte == b'\n') {
        if line.len() > 65536 {
            continue;
        }
        let Ok(text) = std::str::from_utf8(line) else {
            continue;
        };
        let Ok(record) = Json::parse_exact(text.trim_start_matches('\u{feff}')) else {
            continue;
        };
        if record.get("type").and_then(Json::as_str) != Some("turn_context") {
            continue;
        }
        latest = record
            .get("payload")
            .and_then(|payload| payload.get("turn_id"))
            .and_then(Json::as_str)
            .filter(|identifier| is_uuid(identifier))
            .map(str::to_string);
    }
    latest
}

fn read_json(path: &Path) -> Option<Json> {
    refuse_reparse_chain(path).ok()?;
    let file = open_shared_read(path).ok()?;
    if !file.metadata().ok()?.is_file() || file.metadata().ok()?.len() > MAX_METADATA {
        return None;
    }
    let mut content = String::new();
    file.take(MAX_METADATA + 1)
        .read_to_string(&mut content)
        .ok()?;
    if content.len() as u64 > MAX_METADATA {
        return None;
    }
    Json::parse_exact(&content).ok()
}

fn number(value: &Json, key: &str) -> Option<u64> {
    let value = value.get(key)?.as_number()?;
    (value.is_finite() && value >= 0.0 && value.fract() == 0.0 && value <= 9_007_199_254_740_991.0)
        .then_some(value as u64)
}

fn read_fast(
    directory: &Path,
    session: &str,
    turn: &str,
    now: u64,
    alive: impl Fn(u32, u64) -> bool,
) -> Option<bool> {
    if !is_uuid(session) || !is_uuid(turn) {
        return None;
    }
    let host = read_json(&directory.join("host.json"))?;
    let latch = read_json(&directory.join(format!("{session}.json")))?;
    let instance = host.get("instance_id")?.as_str()?;
    let observed = number(&host, "observed_at_ms")?;
    let born = number(&host, "host_started_ms")?;
    let pid = u32::try_from(number(&host, "host_pid")?).ok()?;
    let latched = number(&latch, "observed_at_ms")?;
    if number(&host, "schema_version") != Some(1)
        || number(&latch, "schema_version") != Some(1)
        || host.get("available")?.as_bool() != Some(true)
        || !is_uuid(instance)
        || pid == 0
        || observed < now.saturating_sub(90_000)
        || observed > now.saturating_add(1000)
        || born == 0
        || born > observed
        || latched < born
        || latched > now.saturating_add(1000)
        || latch.get("instance_id")?.as_str()? != instance
        || latch.get("thread_id")?.as_str()? != session
        || latch.get("turn_id")?.as_str()? != turn
        || !alive(pid, born)
    {
        return None;
    }
    let after = read_json(&directory.join("host.json"))?;
    if after.get("instance_id")?.as_str()? != instance
        || number(&after, "host_pid") != Some(pid as u64)
        || after.get("available")?.as_bool() != Some(true)
    {
        return None;
    }
    match latch.get("speed")?.as_str()? {
        "fast" => Some(true),
        "standard" | "unknown" => Some(false),
        _ => None,
    }
}

#[cfg(windows)]
fn host_alive(pid: u32, born: u64) -> bool {
    use std::ffi::c_void;
    #[repr(C)]
    #[derive(Default)]
    struct FileTime {
        low: u32,
        high: u32,
    }
    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn OpenProcess(access: u32, inherit: i32, pid: u32) -> *mut c_void;
        fn GetExitCodeProcess(process: *mut c_void, code: *mut u32) -> i32;
        fn GetProcessTimes(
            process: *mut c_void,
            created: *mut FileTime,
            exited: *mut FileTime,
            kernel: *mut FileTime,
            user: *mut FileTime,
        ) -> i32;
        fn CloseHandle(handle: *mut c_void) -> i32;
    }
    let handle = unsafe { OpenProcess(0x1000, 0, pid) };
    if handle.is_null() {
        return false;
    }
    let mut code = 0;
    let mut created = FileTime::default();
    let mut exited = FileTime::default();
    let mut kernel = FileTime::default();
    let mut user = FileTime::default();
    let live = unsafe { GetExitCodeProcess(handle, &mut code) } != 0
        && code == 259
        && unsafe { GetProcessTimes(handle, &mut created, &mut exited, &mut kernel, &mut user) }
            != 0;
    unsafe { CloseHandle(handle) };
    let milliseconds = (((created.high as u64) << 32) | created.low as u64) / 10_000;
    live && milliseconds
        .saturating_sub(11_644_473_600_000)
        .abs_diff(born)
        <= 10_000
}

#[cfg(not(windows))]
fn host_alive(_pid: u32, _born: u64) -> bool {
    false
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    const SESSION: &str = "11111111-1111-4111-8111-111111111111";
    const TURN: &str = "22222222-2222-4222-8222-222222222222";
    const HOST: &str = "33333333-3333-4333-8333-333333333333";

    #[test]
    fn complete_metadata_only_and_current_turn_binding() {
        let text = format!("\u{feff}{{\"type\":\"turn_context\",\"payload\":{{\"turn_id\":\"{TURN}\"}}}}\r\n{{\"type\":\"response_item\",\"payload\":{{\"turn_id\":\"{HOST}\"}}}}\n{{\"type\":\"turn_context\",\"payload\":");
        assert_eq!(latest_turn(text.as_bytes(), false).as_deref(), Some(TURN));
        assert_eq!(latest_turn(b"partial\n", true), None);
        assert_eq!(
            latest_turn(
                b"{\"type\":\"turn_context\",\"payload\":{\"turn_id\":\"bad\"}}\n",
                false
            ),
            None
        );
    }

    #[test]
    fn stale_wrong_identity_and_missing_status_fail_to_standard_without_writes() {
        let directory = std::env::temp_dir().join(format!(
            "parley-speed-test-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir(&directory).unwrap();
        let host = format!("{{\"schema_version\":1,\"instance_id\":\"{HOST}\",\"host_pid\":1,\"host_started_ms\":1000,\"observed_at_ms\":100000,\"available\":true}}");
        let latch = format!("{{\"schema_version\":1,\"instance_id\":\"{HOST}\",\"thread_id\":\"{SESSION}\",\"turn_id\":\"{TURN}\",\"speed\":\"fast\",\"observed_at_ms\":5000}}");
        fs::write(directory.join("host.json"), &host).unwrap();
        fs::write(directory.join(format!("{SESSION}.json")), &latch).unwrap();
        assert_eq!(
            read_fast(&directory, SESSION, TURN, 100000, |_, _| true),
            Some(true)
        );
        assert_eq!(
            read_fast(&directory, SESSION, HOST, 100000, |_, _| true),
            None
        );
        assert_eq!(
            read_fast(&directory, SESSION, TURN, 190001, |_, _| true),
            None
        );
        assert_eq!(
            read_fast(&directory, SESSION, TURN, 100000, |_, _| false),
            None
        );
        assert_eq!(read_fast(&directory, HOST, TURN, 100000, |_, _| true), None);
        assert_eq!(
            fs::read_to_string(directory.join("host.json")).unwrap(),
            host
        );
        assert_eq!(
            fs::read_to_string(directory.join(format!("{SESSION}.json"))).unwrap(),
            latch
        );
        for replacement in [
            latch.replace("\"fast\"", "\"standard\""),
            latch.replace("\"fast\"", "\"unknown\""),
        ] {
            fs::write(directory.join(format!("{SESSION}.json")), replacement).unwrap();
            assert_eq!(
                read_fast(&directory, SESSION, TURN, 100000, |_, _| true),
                Some(false)
            );
        }
    }
}
