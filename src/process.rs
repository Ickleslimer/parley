use std::io::Read;
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::sync::{Arc, Mutex, MutexGuard, OnceLock};
use std::thread;
use std::time::{Duration, Instant};

use crate::harness::Invocation;

#[derive(Clone, Default)]
pub(crate) struct CancellationToken {
    cancelled: Arc<AtomicBool>,
}

impl CancellationToken {
    #[allow(dead_code)]
    pub(crate) fn cancel(&self) {
        self.cancelled.store(true, Ordering::Release);
    }

    pub(crate) fn is_cancelled(&self) -> bool {
        self.cancelled.load(Ordering::Acquire)
    }
}

pub(crate) trait SpawnObserver: Send + Sync {
    fn process_created(&self, _process_id: u32) -> Result<(), String> {
        Ok(())
    }

    fn process_resumed(&self, _process_id: u32) -> Result<(), String> {
        Ok(())
    }
}

struct NoopSpawnObserver;

impl SpawnObserver for NoopSpawnObserver {}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum OutputStream {
    Stdout,
    Stderr,
}

pub(crate) trait OutputObserver: Send + Sync {
    /// Observe one exact byte chunk. Returning `true` retains the chunk in the
    /// captured stream; returning `false` discards it after this call.
    fn observe(&self, _stream: OutputStream, _chunk: &[u8]) -> Result<bool, String> {
        Ok(true)
    }

    /// Pauses the overall execution clock while an independently verified
    /// helper wait is active. Idle accounting remains independent.
    fn pause_overall_clock(&self) -> bool {
        false
    }

    /// Supplies a caller-owned terminal deadline such as acknowledgement
    /// expiry. The process runner never infers one from silence.
    fn timeout_reason(&self) -> Option<String> {
        None
    }
}

struct NoopOutputObserver;

impl OutputObserver for NoopOutputObserver {}

/// Some agent CLIs keep a single, migration-locked local state store and wedge
/// when two instances run at once. Antigravity's `agy` is the known case: two
/// concurrent `agy` processes deadlock on their shared conversations DB
/// migration ("Waiting for migrations to complete..."), which hangs a `fuse`
/// panel that lists `agy` more than once. Serialize those commands within this
/// process so panelists queue instead of deadlocking. Cross-process contention
/// (an `agy` already running outside `par`) is out of our hands.
fn exclusive_guard(command: &str) -> Option<MutexGuard<'static, ()>> {
    static AGY_LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    let mutex = match command {
        "agy" => AGY_LOCK.get_or_init(|| Mutex::new(())),
        _ => return None,
    };
    // A panicked holder poisons the lock; recover the guard rather than
    // propagate — the child that panicked is already gone.
    Some(
        mutex
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()),
    )
}

/// Run an invocation with inherited stdio and **replace this process** by
/// exiting with the child's status. Used for interactive launches and `mcp
/// connect` where `par` has nothing more to do once the child finishes.
pub(crate) fn run_invocation(
    invocation: Invocation,
    cwd: Option<&str>,
    inherit_stdin: bool,
) -> Result<(), String> {
    let status = run_invocation_status(invocation, cwd, inherit_stdin)?;
    std::process::exit(status.code().unwrap_or(1));
}

/// Run an invocation with inherited stdio and **return** its exit status instead
/// of exiting, so the caller can do post-run work (e.g. record telemetry)
/// before terminating.
pub(crate) fn run_invocation_status(
    invocation: Invocation,
    cwd: Option<&str>,
    inherit_stdin: bool,
) -> Result<std::process::ExitStatus, String> {
    let mut command = Command::new(&invocation.command);
    command
        .args(&invocation.args)
        .envs(&invocation.env)
        .stdin(if inherit_stdin {
            Stdio::inherit()
        } else {
            Stdio::null()
        })
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit());

    if let Some(cwd) = cwd {
        command.current_dir(cwd);
    }

    command
        .status()
        .map_err(|error| format!("failed to start {}: {error}", invocation.command))
}

/// Output captured from a child process (used by `par ask` / the MCP
/// `ask_agent` tool, where one agent's reply is the value, not a stream).
pub(crate) struct Captured {
    pub stdout: String,
    pub stderr: String,
    pub success: bool,
    /// True when the watchdog killed the child for exceeding a time budget
    /// (overall or idle/no-output). The captured streams hold whatever arrived
    /// before the kill.
    pub timed_out: bool,
    pub cancelled: bool,
}

impl Captured {
    /// The agent's usable reply, or an error explaining why there is none.
    ///
    /// A clean exit with **empty stdout** is treated as a failure, not an empty
    /// answer: some CLIs (notably opencode) print the error to stderr and still
    /// exit 0, so keying only on the exit status would hand back a blank
    /// "success" — which reads to the caller as a hung or broken agent. For
    /// ask/fuse an empty reply is never useful, so surface the stderr instead.
    pub(crate) fn reply(&self) -> Result<String, String> {
        let stdout = self.stdout.trim();
        if self.success && !stdout.is_empty() {
            return Ok(stdout.to_string());
        }
        Err(self.failure_message())
    }

    /// A concise reason the call did not yield a reply.
    pub(crate) fn failure_message(&self) -> String {
        let stderr = concise_error(&self.stderr);
        if self.cancelled {
            return match stderr {
                Some(err) => format!("cancelled with no complete reply: {err}"),
                None => "cancelled before a complete reply was produced".to_string(),
            };
        }
        if self.timed_out {
            return match stderr {
                Some(err) => format!("timed out with no complete reply: {err}"),
                None => "timed out with no reply within the time budget".to_string(),
            };
        }
        if let Some(err) = stderr {
            return err;
        }
        let stdout = self.stdout.trim();
        if !stdout.is_empty() {
            return stdout.to_string();
        }
        "exited without producing any output".to_string()
    }
}

/// Pull a concise, human-readable error out of a captured stderr stream:
/// prefer the last line that looks like an error message, else the last
/// non-empty line. ANSI escape codes are stripped so the result is plain text.
fn concise_error(stderr: &str) -> Option<String> {
    let mut last_nonempty: Option<String> = None;
    let mut last_errorish: Option<String> = None;
    for raw in stderr.lines() {
        let line = strip_ansi(raw);
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        let lower = trimmed.to_ascii_lowercase();
        if lower.contains("error") || lower.contains("not found") || lower.contains("failed") {
            last_errorish = Some(trimmed.to_string());
        }
        last_nonempty = Some(trimmed.to_string());
    }
    last_errorish.or(last_nonempty)
}

/// Remove ANSI/VT100 escape sequences (`ESC [ ... <final>`) from a string.
fn strip_ansi(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    let mut chars = input.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '\x1b' {
            // Consume a CSI sequence: ESC [ params/intermediates final-byte.
            if chars.peek() == Some(&'[') {
                chars.next();
                for e in chars.by_ref() {
                    // Final byte of a CSI sequence is in the range @..~.
                    if ('\u{40}'..='\u{7e}').contains(&e) {
                        break;
                    }
                }
            }
            continue;
        }
        out.push(c);
    }
    out
}

/// Run an invocation to completion, capturing stdout/stderr instead of
/// inheriting them. stdin is closed so the child cannot block on a prompt.
pub(crate) fn capture_invocation(
    invocation: Invocation,
    cwd: Option<&str>,
) -> Result<Captured, String> {
    capture_invocation_timeout(invocation, cwd, Timeouts::disabled())
}

/// Time budgets for a captured run. `overall` caps total wall-clock; `idle`
/// caps the gap between bytes of output (a hung agent that emits nothing).
/// Either may be zero to disable that bound.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Timeouts {
    pub overall: Duration,
    pub idle: Duration,
}

impl Timeouts {
    pub(crate) fn disabled() -> Self {
        Self {
            overall: Duration::ZERO,
            idle: Duration::ZERO,
        }
    }

    /// Resolve the default budgets for headless captured calls (ask/fuse/solve)
    /// from the environment. `PARLEY_TIMEOUT` and `PARLEY_IDLE_TIMEOUT` are in
    /// seconds; `0` disables that bound. Defaults: 600s overall, 180s idle.
    pub(crate) fn from_env() -> Self {
        let overall = env_secs("PARLEY_TIMEOUT").unwrap_or(600);
        let idle = env_secs("PARLEY_IDLE_TIMEOUT").unwrap_or(180);
        Self {
            overall: Duration::from_secs(overall),
            idle: Duration::from_secs(idle),
        }
    }
}

fn env_secs(name: &str) -> Option<u64> {
    std::env::var(name).ok().and_then(|v| v.trim().parse().ok())
}

/// Run an invocation captured, with a watchdog that kills the child if it
/// exceeds the overall budget or stops producing output for longer than the
/// idle budget. Output is streamed into buffers off-thread so a wedged child
/// can't deadlock a full pipe.
pub(crate) fn capture_invocation_timeout(
    invocation: Invocation,
    cwd: Option<&str>,
    timeouts: Timeouts,
) -> Result<Captured, String> {
    capture_invocation_timeout_controlled(
        invocation,
        cwd,
        timeouts,
        &CancellationToken::default(),
        &NoopSpawnObserver,
    )
}

pub(crate) fn capture_invocation_timeout_controlled(
    invocation: Invocation,
    cwd: Option<&str>,
    timeouts: Timeouts,
    cancellation: &CancellationToken,
    observer: &dyn SpawnObserver,
) -> Result<Captured, String> {
    capture_invocation_timeout_controlled_observed(
        invocation,
        cwd,
        timeouts,
        cancellation,
        observer,
        &NoopOutputObserver,
    )
}

pub(crate) fn capture_invocation_timeout_controlled_observed(
    invocation: Invocation,
    cwd: Option<&str>,
    timeouts: Timeouts,
    cancellation: &CancellationToken,
    observer: &dyn SpawnObserver,
    output_observer: &dyn OutputObserver,
) -> Result<Captured, String> {
    // Held for the whole child run for commands that can't run concurrently
    // (see `exclusive_guard`); a no-op for everything else.
    let _exclusive = exclusive_guard(&invocation.command);

    let mut child = match spawn_child(&invocation, cwd, observer)? {
        SpawnedChild::Running(child) => child,
        SpawnedChild::StartedFailure(message) => {
            return Ok(Captured {
                stdout: String::new(),
                stderr: message,
                success: false,
                timed_out: false,
                cancelled: false,
            })
        }
    };

    let mut out_buf = Vec::<u8>::new();
    let mut err_buf = Vec::<u8>::new();
    let (event_tx, event_rx) = mpsc::channel::<ReaderEvent>();

    let out_reader = child
        .take_stdout()
        .map(|pipe| spawn_reader(pipe, OutputStream::Stdout, event_tx.clone()));
    let err_reader = child
        .take_stderr()
        .map(|pipe| spawn_reader(pipe, OutputStream::Stderr, event_tx.clone()));
    // Drop our own sender so the channel disconnects once both readers finish.
    drop(event_tx);

    let started = Instant::now();
    let mut active_checkpoint = started;
    let mut active_elapsed = Duration::ZERO;
    let mut last_activity = started;
    let mut timed_out = false;
    let mut cancelled = false;
    // Wait at idle granularity (or a short tick when only an overall bound is
    // set), reacting to output heartbeats and process exit.
    let tick = pick_tick(timeouts);
    let mut lifecycle_error = None;
    let status = loop {
        let now = Instant::now();
        if !output_observer.pause_overall_clock() {
            active_elapsed = active_elapsed.saturating_add(now.duration_since(active_checkpoint));
        }
        active_checkpoint = now;
        match child.try_wait() {
            Ok(Some(exit_code)) => {
                if let Err(error) = child.terminate_remaining_tree() {
                    lifecycle_error = Some(format!(
                        "contained process tree cleanup after root exit failed: {error}"
                    ));
                }
                break Some(exit_code);
            }
            Ok(None) => {}
            Err(error) => {
                let termination = child.terminate().err();
                let reaping = child.wait().err();
                lifecycle_error = Some(lifecycle_failure(
                    &format!("indeterminate child wait: {error}"),
                    termination,
                    reaping,
                ));
                break None;
            }
        }
        if cancellation.is_cancelled() {
            let termination = child.terminate().err();
            cancelled = true;
            match child.wait() {
                Ok(status) => {
                    if let Some(error) = termination {
                        lifecycle_error = Some(format!(
                            "cancelled process termination reported an error before reap: {error}"
                        ));
                    }
                    break Some(status);
                }
                Err(error) => {
                    lifecycle_error = Some(lifecycle_failure(
                        "cancelled process could not be reaped",
                        termination,
                        Some(error),
                    ));
                    break None;
                }
            }
        }
        if let Some(reason) = output_observer.timeout_reason() {
            let termination = child.terminate().err();
            timed_out = true;
            match child.wait() {
                Ok(status) => {
                    lifecycle_error = Some(match termination {
                        Some(error) => format!("{reason}; termination also failed: {error}"),
                        None => reason,
                    });
                    break Some(status);
                }
                Err(error) => {
                    lifecycle_error = Some(lifecycle_failure(&reason, termination, Some(error)));
                    break None;
                }
            }
        }
        if !timeouts.overall.is_zero() && active_elapsed >= timeouts.overall {
            let termination = child.terminate().err();
            timed_out = true;
            match child.wait() {
                Ok(status) => {
                    if let Some(error) = termination {
                        lifecycle_error = Some(format!(
                            "timed-out process termination reported an error before reap: {error}"
                        ));
                    }
                    break Some(status);
                }
                Err(error) => {
                    lifecycle_error = Some(lifecycle_failure(
                        "timed-out process could not be reaped",
                        termination,
                        Some(error),
                    ));
                    break None;
                }
            }
        }
        match event_rx.recv_timeout(tick) {
            Ok(event) => {
                match observe_reader_event(event, output_observer, &mut out_buf, &mut err_buf) {
                    Ok(()) => {
                        last_activity = Instant::now();
                        continue;
                    }
                    Err(error) => {
                        let termination = child.terminate().err();
                        let reaping = child.wait().err();
                        lifecycle_error = Some(lifecycle_failure(
                            &format!("output observer failed: {error}"),
                            termination,
                            reaping,
                        ));
                        break None;
                    }
                }
            }
            Err(mpsc::RecvTimeoutError::Disconnected) => {
                // A child can close its inherited streams before it exits. Keep
                // the watchdog active rather than switching to an unbounded
                // blocking wait; no further output can reset the idle timer.
                if idle_expired(last_activity, timeouts.idle) {
                    let termination = child.terminate().err();
                    timed_out = true;
                    match child.wait() {
                        Ok(status) => {
                            if let Some(error) = termination {
                                lifecycle_error = Some(format!(
                                    "idle-timed-out process termination reported an error before reap: {error}"
                                ));
                            }
                            break Some(status);
                        }
                        Err(error) => {
                            lifecycle_error = Some(lifecycle_failure(
                                "idle-timed-out process could not be reaped",
                                termination,
                                Some(error),
                            ));
                            break None;
                        }
                    }
                }
                thread::sleep(tick);
            }
            Err(mpsc::RecvTimeoutError::Timeout) => {
                if idle_expired(last_activity, timeouts.idle) {
                    // Approximation: no heartbeat within the idle window. Good
                    // enough — a steadily-emitting child keeps resetting it.
                    let termination = child.terminate().err();
                    timed_out = true;
                    match child.wait() {
                        Ok(status) => {
                            if let Some(error) = termination {
                                lifecycle_error = Some(format!(
                                    "idle-timed-out process termination reported an error before reap: {error}"
                                ));
                            }
                            break Some(status);
                        }
                        Err(error) => {
                            lifecycle_error = Some(lifecycle_failure(
                                "idle-timed-out process could not be reaped",
                                termination,
                                Some(error),
                            ));
                            break None;
                        }
                    }
                }
            }
        }
    };
    if let Some(handle) = out_reader {
        let _ = handle.join();
    }
    if let Some(handle) = err_reader {
        let _ = handle.join();
    }

    for event in event_rx.try_iter() {
        if let Err(error) = observe_reader_event(event, output_observer, &mut out_buf, &mut err_buf)
        {
            lifecycle_error.get_or_insert_with(|| format!("output observer failed: {error}"));
            break;
        }
    }

    let stdout = String::from_utf8_lossy(&out_buf).into_owned();
    let mut stderr = String::from_utf8_lossy(&err_buf).into_owned();
    if let Some(error) = &lifecycle_error {
        if !stderr.is_empty() && !stderr.ends_with('\n') {
            stderr.push('\n');
        }
        stderr.push_str(error);
    }
    let success = lifecycle_error.is_none() && !timed_out && !cancelled && status == Some(0);

    Ok(Captured {
        stdout,
        stderr,
        success,
        timed_out,
        cancelled,
    })
}

enum SpawnedChild {
    Running(RunningChild),
    StartedFailure(String),
}

enum RunningChild {
    Standard(Child),
    #[cfg(windows)]
    Contained(crate::winjob::ContainedChild),
}

impl RunningChild {
    fn take_stdout(&mut self) -> Option<Box<dyn Read + Send>> {
        match self {
            Self::Standard(child) => child
                .stdout
                .take()
                .map(|pipe| Box::new(pipe) as Box<dyn Read + Send>),
            #[cfg(windows)]
            Self::Contained(child) => child.take_stdout(),
        }
    }

    fn take_stderr(&mut self) -> Option<Box<dyn Read + Send>> {
        match self {
            Self::Standard(child) => child
                .stderr
                .take()
                .map(|pipe| Box::new(pipe) as Box<dyn Read + Send>),
            #[cfg(windows)]
            Self::Contained(child) => child.take_stderr(),
        }
    }

    fn try_wait(&mut self) -> Result<Option<u32>, String> {
        match self {
            Self::Standard(child) => child
                .try_wait()
                .map(|status| status.map(|status| status.code().unwrap_or(1) as u32))
                .map_err(|error| format!("wait for child: {error}")),
            #[cfg(windows)]
            Self::Contained(child) => child.try_wait(),
        }
    }

    fn wait(&mut self) -> Result<u32, String> {
        match self {
            Self::Standard(child) => child
                .wait()
                .map(|status| status.code().unwrap_or(1) as u32)
                .map_err(|error| format!("wait for child: {error}")),
            #[cfg(windows)]
            Self::Contained(child) => child.wait(),
        }
    }

    fn terminate(&mut self) -> Result<(), String> {
        match self {
            Self::Standard(child) => child
                .kill()
                .map_err(|error| format!("terminate child: {error}")),
            #[cfg(windows)]
            Self::Contained(child) => child.terminate(),
        }
    }

    fn terminate_remaining_tree(&mut self) -> Result<(), String> {
        match self {
            Self::Standard(_) => Ok(()),
            #[cfg(windows)]
            Self::Contained(child) => child.terminate(),
        }
    }
}

fn lifecycle_failure(
    primary: &str,
    termination: Option<String>,
    reaping: Option<String>,
) -> String {
    let mut message = primary.to_string();
    if let Some(error) = termination {
        message.push_str(&format!("; termination failed: {error}"));
    }
    if let Some(error) = reaping {
        message.push_str(&format!("; reap failed: {error}"));
    }
    message
}

fn spawn_child(
    invocation: &Invocation,
    cwd: Option<&str>,
    observer: &dyn SpawnObserver,
) -> Result<SpawnedChild, String> {
    #[cfg(windows)]
    if should_use_job_object(invocation) {
        return match crate::winjob::spawn(
            invocation,
            cwd,
            |process_id| observer.process_created(process_id),
            |process_id| observer.process_resumed(process_id),
        ) {
            Ok(child) => Ok(SpawnedChild::Running(RunningChild::Contained(child))),
            Err(error) if error.started => Ok(SpawnedChild::StartedFailure(error.message)),
            Err(error) => Err(format!(
                "failed to start {}: {}",
                invocation.command, error.message
            )),
        };
    }

    let mut command = Command::new(&invocation.command);
    command
        .args(&invocation.args)
        .envs(&invocation.env)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    if let Some(cwd) = cwd {
        command.current_dir(cwd);
    }
    let mut child = command
        .spawn()
        .map_err(|error| format!("failed to start {}: {error}", invocation.command))?;
    if let Err(error) = observer
        .process_created(child.id())
        .and_then(|()| observer.process_resumed(child.id()))
    {
        let _ = child.kill();
        let _ = child.wait();
        return Ok(SpawnedChild::StartedFailure(format!(
            "spawn observer failed: {error}"
        )));
    }
    Ok(SpawnedChild::Running(RunningChild::Standard(child)))
}

#[cfg(windows)]
fn should_use_job_object(invocation: &Invocation) -> bool {
    #[cfg(test)]
    if invocation
        .env
        .get("PARLEY_TEST_FORCE_JOB_OBJECT")
        .is_some_and(|value| value == "1")
    {
        return true;
    }
    let is_grok = std::path::Path::new(&invocation.command)
        .file_name()
        .and_then(|name| name.to_str())
        .is_some_and(|name| {
            name.eq_ignore_ascii_case("grok") || name.eq_ignore_ascii_case("grok.exe")
        });
    let is_locked = invocation.env.contains_key("PARLEY_GROK_LOCKED_VERSION")
        || std::env::var_os("PARLEY_GROK_LOCKED_VERSION").is_some();
    is_grok && is_locked
}

/// How often the watchdog wakes to re-check budgets. Bounded so an overall-only
/// budget is still enforced promptly, and never longer than the idle window.
fn pick_tick(timeouts: Timeouts) -> Duration {
    let mut tick = Duration::from_millis(250);
    if !timeouts.idle.is_zero() && timeouts.idle < tick {
        tick = timeouts.idle;
    }
    tick
}

fn idle_expired(last_activity: Instant, idle: Duration) -> bool {
    !idle.is_zero() && last_activity.elapsed() >= idle
}

fn spawn_reader<R: Read + Send + 'static>(
    mut pipe: R,
    stream: OutputStream,
    events: mpsc::Sender<ReaderEvent>,
) -> thread::JoinHandle<()> {
    thread::spawn(move || {
        let mut chunk = [0u8; 4096];
        loop {
            match pipe.read(&mut chunk) {
                Ok(0) | Err(_) => break,
                Ok(n) => {
                    // A dead receiver just means the watchdog already moved on.
                    if events
                        .send(ReaderEvent {
                            stream,
                            bytes: chunk[..n].to_vec(),
                        })
                        .is_err()
                    {
                        break;
                    }
                }
            }
        }
    })
}

struct ReaderEvent {
    stream: OutputStream,
    bytes: Vec<u8>,
}

fn observe_reader_event(
    event: ReaderEvent,
    observer: &dyn OutputObserver,
    stdout: &mut Vec<u8>,
    stderr: &mut Vec<u8>,
) -> Result<(), String> {
    if observer.observe(event.stream, &event.bytes)? {
        match event.stream {
            OutputStream::Stdout => stdout.extend_from_slice(&event.bytes),
            OutputStream::Stderr => stderr.extend_from_slice(&event.bytes),
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::harness::Invocation;

    fn inv(command: &str, args: &[&str]) -> Invocation {
        Invocation::new(command, args.iter().map(|s| s.to_string()).collect())
    }

    #[test]
    fn captures_stdout_without_timeout() {
        let out = capture_invocation(inv("printf", &["hello"]), None).unwrap();
        assert_eq!(out.stdout, "hello");
        assert!(out.success);
        assert!(!out.timed_out);
    }

    #[test]
    fn overall_timeout_kills_a_hung_child() {
        let timeouts = Timeouts {
            overall: Duration::from_millis(300),
            idle: Duration::ZERO,
        };
        let out = capture_invocation_timeout(inv("sleep", &["10"]), None, timeouts).unwrap();
        assert!(out.timed_out);
        assert!(!out.success);
    }

    #[test]
    fn idle_timeout_kills_a_silent_child() {
        let timeouts = Timeouts {
            overall: Duration::ZERO,
            idle: Duration::from_millis(300),
        };
        let out = capture_invocation_timeout(inv("sleep", &["10"]), None, timeouts).unwrap();
        assert!(out.timed_out);
    }

    #[test]
    fn idle_timeout_is_measured_from_last_activity() {
        let idle = Duration::from_millis(100);
        let stale = Instant::now() - Duration::from_millis(200);
        assert!(idle_expired(stale, idle));
        assert!(!idle_expired(Instant::now(), idle));
        assert!(!idle_expired(stale, Duration::ZERO));
    }

    #[test]
    fn output_heartbeats_reset_the_idle_watchdog() {
        let timeouts = Timeouts {
            overall: Duration::from_secs(3),
            idle: Duration::from_millis(800),
        };
        let out = capture_invocation_timeout(
            inv(
                "sh",
                &["-c", "printf a; sleep 0.45; printf b; sleep 0.45; printf c"],
            ),
            None,
            timeouts,
        )
        .unwrap();
        assert_eq!(out.stdout, "abc");
        assert!(out.success);
        assert!(!out.timed_out);
    }

    struct DroppingObserver {
        seen: Mutex<Vec<u8>>,
    }

    impl OutputObserver for DroppingObserver {
        fn observe(&self, _stream: OutputStream, chunk: &[u8]) -> Result<bool, String> {
            self.seen
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .extend_from_slice(chunk);
            Ok(false)
        }
    }

    #[test]
    fn output_observer_can_consume_bytes_without_retaining_raw_payloads() {
        let observer = DroppingObserver {
            seen: Mutex::new(Vec::new()),
        };
        let output = capture_invocation_timeout_controlled_observed(
            inv("printf", &["private-stream"]),
            None,
            Timeouts {
                overall: Duration::from_secs(2),
                idle: Duration::from_secs(2),
            },
            &CancellationToken::default(),
            &NoopSpawnObserver,
            &observer,
        )
        .unwrap();
        assert!(output.success);
        assert!(output.stdout.is_empty());
        assert!(output.stderr.is_empty());
        assert_eq!(
            observer
                .seen
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .as_slice(),
            b"private-stream"
        );
    }

    struct RejectingObserver;

    impl OutputObserver for RejectingObserver {
        fn observe(&self, _stream: OutputStream, _chunk: &[u8]) -> Result<bool, String> {
            Err("sanitizer rejected stream".to_string())
        }
    }

    #[test]
    fn observer_failure_reaps_started_child_and_returns_terminal_capture() {
        let output = capture_invocation_timeout_controlled_observed(
            inv("sh", &["-c", "printf x; sleep 10"]),
            None,
            Timeouts {
                overall: Duration::from_secs(3),
                idle: Duration::ZERO,
            },
            &CancellationToken::default(),
            &NoopSpawnObserver,
            &RejectingObserver,
        )
        .unwrap();
        assert!(!output.success);
        assert!(!output.timed_out);
        assert!(output.stderr.contains("output observer failed"));
        assert!(output.stderr.contains("sanitizer rejected stream"));
    }

    struct PausingObserver;

    impl OutputObserver for PausingObserver {
        fn pause_overall_clock(&self) -> bool {
            true
        }
    }

    #[test]
    fn verified_pause_suspends_only_the_overall_execution_clock() {
        let observer = PausingObserver;
        let output = capture_invocation_timeout_controlled_observed(
            inv("sleep", &["0.25"]),
            None,
            Timeouts {
                overall: Duration::from_millis(150),
                idle: Duration::ZERO,
            },
            &CancellationToken::default(),
            &NoopSpawnObserver,
            &observer,
        )
        .unwrap();
        assert!(output.success, "{}", output.failure_message());
        assert!(!output.timed_out);
    }

    struct ExpiredDecisionWindow;

    impl OutputObserver for ExpiredDecisionWindow {
        fn timeout_reason(&self) -> Option<String> {
            Some("acknowledgement decision window expired".to_string())
        }
    }

    #[test]
    fn observer_owned_deadline_terminates_and_reaps_the_exact_child() {
        let output = capture_invocation_timeout_controlled_observed(
            inv("sleep", &["10"]),
            None,
            Timeouts {
                overall: Duration::from_secs(5),
                idle: Duration::ZERO,
            },
            &CancellationToken::default(),
            &NoopSpawnObserver,
            &ExpiredDecisionWindow,
        )
        .unwrap();
        assert!(output.timed_out);
        assert!(!output.success);
        assert!(output
            .stderr
            .contains("acknowledgement decision window expired"));
    }

    #[cfg(windows)]
    #[test]
    fn contained_root_exit_terminates_background_descendants_before_reader_join() {
        let mut invocation = Invocation::new(
            std::env::current_exe().unwrap().to_string_lossy(),
            vec![
                "--exact".to_string(),
                "process::tests::contained_background_helper".to_string(),
                "--ignored".to_string(),
                "--nocapture".to_string(),
                "--test-threads=1".to_string(),
            ],
        );
        invocation
            .env
            .insert("PARLEY_TEST_FORCE_JOB_OBJECT".to_string(), "1".to_string());
        let started = Instant::now();
        let output = capture_invocation_timeout_controlled(
            invocation,
            None,
            Timeouts {
                overall: Duration::from_secs(5),
                idle: Duration::from_secs(5),
            },
            &CancellationToken::default(),
            &NoopSpawnObserver,
        )
        .unwrap();
        assert!(
            started.elapsed() < Duration::from_secs(3),
            "elapsed={:?} success={} timed_out={} stderr={} stdout={}",
            started.elapsed(),
            output.success,
            output.timed_out,
            output.stderr,
            output.stdout
        );
        assert!(
            output.success,
            "timed_out={} cancelled={} stderr={} stdout={}",
            output.timed_out, output.cancelled, output.stderr, output.stdout
        );
    }

    #[cfg(windows)]
    #[test]
    #[ignore]
    #[allow(clippy::zombie_processes)]
    fn contained_background_helper() {
        Command::new("ping.exe")
            .args(["-n", "30", "127.0.0.1"])
            .spawn()
            .unwrap();
    }

    #[test]
    fn completes_before_timeout() {
        let timeouts = Timeouts {
            overall: Duration::from_secs(5),
            idle: Duration::from_secs(5),
        };
        let out = capture_invocation_timeout(inv("printf", &["done"]), None, timeouts).unwrap();
        assert_eq!(out.stdout, "done");
        assert!(out.success);
        assert!(!out.timed_out);
    }

    fn captured(stdout: &str, stderr: &str, success: bool, timed_out: bool) -> Captured {
        Captured {
            stdout: stdout.to_string(),
            stderr: stderr.to_string(),
            success,
            timed_out,
            cancelled: false,
        }
    }

    #[test]
    fn reply_returns_trimmed_stdout_on_success() {
        let out = captured("  PONG\n", "", true, false);
        assert_eq!(out.reply().unwrap(), "PONG");
    }

    #[test]
    fn reply_treats_empty_stdout_as_failure_even_when_exit_zero() {
        // opencode's failure mode: prints the error to stderr, exits 0.
        let out = captured(
            "",
            "\u{1b}[91m\u{1b}[1mError: \u{1b}[0mModel not found: opencode/glm-5.2.",
            true,
            false,
        );
        let err = out.reply().unwrap_err();
        assert_eq!(err, "Error: Model not found: opencode/glm-5.2.");
    }

    #[test]
    fn reply_surfaces_timeout() {
        let out = captured("partial", "", false, true);
        assert!(out.reply().unwrap_err().contains("timed out"));
    }

    #[test]
    fn reply_falls_back_to_generic_when_silent() {
        let out = captured("", "", false, false);
        assert_eq!(
            out.reply().unwrap_err(),
            "exited without producing any output"
        );
    }

    #[test]
    fn concise_error_prefers_the_error_line_over_a_stack_trace() {
        let stderr =
            "SomeError: boom\n    at foo (x.js:1:2)\n    at bar (y.js:3:4)\nError: real message";
        assert_eq!(
            concise_error(stderr).as_deref(),
            Some("Error: real message")
        );
    }

    #[test]
    fn strip_ansi_removes_color_codes() {
        assert_eq!(
            strip_ansi("\u{1b}[91m\u{1b}[1mError:\u{1b}[0m x"),
            "Error: x"
        );
    }
}
