use std::collections::BTreeMap;
use std::env;
use std::ffi::{c_void, OsStr, OsString};
use std::fs::File;
use std::io::Read;
use std::mem::{size_of, zeroed};
use std::os::windows::ffi::OsStrExt;
use std::os::windows::io::FromRawHandle;
use std::path::{Path, PathBuf};
use std::ptr::{null, null_mut};

use crate::harness::Invocation;

type Handle = *mut c_void;

const INVALID_HANDLE_VALUE: Handle = -1_isize as Handle;
const HANDLE_FLAG_INHERIT: u32 = 0x0000_0001;
const STARTF_USESTDHANDLES: u32 = 0x0000_0100;
const CREATE_SUSPENDED: u32 = 0x0000_0004;
const CREATE_UNICODE_ENVIRONMENT: u32 = 0x0000_0400;
const GENERIC_READ: u32 = 0x8000_0000;
const FILE_SHARE_READ: u32 = 0x0000_0001;
const FILE_SHARE_WRITE: u32 = 0x0000_0002;
const OPEN_EXISTING: u32 = 3;
const FILE_ATTRIBUTE_NORMAL: u32 = 0x0000_0080;
const JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE: u32 = 0x0000_2000;
const JOB_OBJECT_EXTENDED_LIMIT_INFORMATION_CLASS: i32 = 9;
const WAIT_OBJECT_0: u32 = 0;
const WAIT_TIMEOUT: u32 = 0x0000_0102;
const INFINITE: u32 = 0xffff_ffff;

#[repr(C)]
struct SecurityAttributes {
    length: u32,
    security_descriptor: *mut c_void,
    inherit_handle: i32,
}

#[repr(C)]
struct StartupInfoW {
    cb: u32,
    reserved: *mut u16,
    desktop: *mut u16,
    title: *mut u16,
    x: u32,
    y: u32,
    x_size: u32,
    y_size: u32,
    x_count_chars: u32,
    y_count_chars: u32,
    fill_attribute: u32,
    flags: u32,
    show_window: u16,
    reserved2_size: u16,
    reserved2: *mut u8,
    stdin: Handle,
    stdout: Handle,
    stderr: Handle,
}

#[repr(C)]
struct ProcessInformation {
    process: Handle,
    thread: Handle,
    process_id: u32,
    thread_id: u32,
}

#[repr(C)]
#[derive(Default)]
struct JobObjectBasicLimitInformation {
    per_process_user_time_limit: i64,
    per_job_user_time_limit: i64,
    limit_flags: u32,
    minimum_working_set_size: usize,
    maximum_working_set_size: usize,
    active_process_limit: u32,
    affinity: usize,
    priority_class: u32,
    scheduling_class: u32,
}

#[repr(C)]
#[derive(Default)]
struct IoCounters {
    read_operation_count: u64,
    write_operation_count: u64,
    other_operation_count: u64,
    read_transfer_count: u64,
    write_transfer_count: u64,
    other_transfer_count: u64,
}

#[repr(C)]
#[derive(Default)]
struct JobObjectExtendedLimitInformation {
    basic_limit_information: JobObjectBasicLimitInformation,
    io_info: IoCounters,
    process_memory_limit: usize,
    job_memory_limit: usize,
    peak_process_memory_used: usize,
    peak_job_memory_used: usize,
}

#[link(name = "kernel32")]
extern "system" {
    fn CreatePipe(
        read_pipe: *mut Handle,
        write_pipe: *mut Handle,
        attributes: *mut SecurityAttributes,
        size: u32,
    ) -> i32;
    fn SetHandleInformation(handle: Handle, mask: u32, flags: u32) -> i32;
    fn CreateFileW(
        file_name: *const u16,
        desired_access: u32,
        share_mode: u32,
        attributes: *mut SecurityAttributes,
        creation_disposition: u32,
        flags_and_attributes: u32,
        template_file: Handle,
    ) -> Handle;
    fn CreateJobObjectW(attributes: *const c_void, name: *const u16) -> Handle;
    fn SetInformationJobObject(
        job: Handle,
        information_class: i32,
        information: *const c_void,
        information_length: u32,
    ) -> i32;
    fn CreateProcessW(
        application_name: *const u16,
        command_line: *mut u16,
        process_attributes: *const c_void,
        thread_attributes: *const c_void,
        inherit_handles: i32,
        creation_flags: u32,
        environment: *mut c_void,
        current_directory: *const u16,
        startup_info: *mut StartupInfoW,
        process_information: *mut ProcessInformation,
    ) -> i32;
    fn AssignProcessToJobObject(job: Handle, process: Handle) -> i32;
    fn ResumeThread(thread: Handle) -> u32;
    fn TerminateJobObject(job: Handle, exit_code: u32) -> i32;
    fn TerminateProcess(process: Handle, exit_code: u32) -> i32;
    fn WaitForSingleObject(handle: Handle, milliseconds: u32) -> u32;
    fn GetExitCodeProcess(process: Handle, exit_code: *mut u32) -> i32;
    fn CloseHandle(handle: Handle) -> i32;
    fn GetLastError() -> u32;
}

pub(crate) struct SpawnError {
    pub(crate) message: String,
    pub(crate) started: bool,
}

pub(crate) struct ContainedChild {
    process: Handle,
    job: Handle,
    stdout: Option<File>,
    stderr: Option<File>,
    terminated: bool,
}

impl ContainedChild {
    pub(crate) fn take_stdout(&mut self) -> Option<Box<dyn Read + Send>> {
        self.stdout
            .take()
            .map(|file| Box::new(file) as Box<dyn Read + Send>)
    }

    pub(crate) fn take_stderr(&mut self) -> Option<Box<dyn Read + Send>> {
        self.stderr
            .take()
            .map(|file| Box::new(file) as Box<dyn Read + Send>)
    }

    pub(crate) fn try_wait(&self) -> Result<Option<u32>, String> {
        let wait = unsafe { WaitForSingleObject(self.process, 0) };
        match wait {
            WAIT_TIMEOUT => Ok(None),
            WAIT_OBJECT_0 => self.exit_code().map(Some),
            _ => Err(last_error("WaitForSingleObject")),
        }
    }

    pub(crate) fn wait(&self) -> Result<u32, String> {
        let wait = unsafe { WaitForSingleObject(self.process, INFINITE) };
        if wait != WAIT_OBJECT_0 {
            return Err(last_error("WaitForSingleObject"));
        }
        self.exit_code()
    }

    pub(crate) fn terminate(&mut self) -> Result<(), String> {
        if self.terminated {
            return Ok(());
        }
        self.terminated = true;
        if unsafe { TerminateJobObject(self.job, 1) } == 0 {
            let job_error = last_error("TerminateJobObject");
            close(self.job);
            self.job = null_mut();
            if unsafe { TerminateProcess(self.process, 1) } == 0 {
                return Err(format!(
                    "{job_error}; closing the kill-on-close job and terminating the root also failed: {}",
                    last_error("TerminateProcess")
                ));
            }
            return Err(format!(
                "{job_error}; closed the kill-on-close job and terminated the root as fallback"
            ));
        }
        Ok(())
    }

    fn exit_code(&self) -> Result<u32, String> {
        let mut code = 0_u32;
        if unsafe { GetExitCodeProcess(self.process, &mut code) } == 0 {
            return Err(last_error("GetExitCodeProcess"));
        }
        Ok(code)
    }
}

impl Drop for ContainedChild {
    fn drop(&mut self) {
        unsafe {
            CloseHandle(self.process);
            if !self.job.is_null() {
                CloseHandle(self.job);
            }
        }
    }
}

pub(crate) fn spawn<F, G>(
    invocation: &Invocation,
    cwd: Option<&str>,
    process_created: F,
    process_resumed: G,
) -> Result<ContainedChild, SpawnError>
where
    F: FnOnce(u32) -> Result<(), String>,
    G: FnOnce(u32) -> Result<(), String>,
{
    let executable = resolve_executable(&invocation.command).map_err(|message| SpawnError {
        message,
        started: false,
    })?;
    let mut application = wide_null(executable.as_os_str());
    let mut command_line = build_command_line(&executable, &invocation.args);
    let mut environment = build_environment(&invocation.env);
    let cwd = cwd
        .map(PathBuf::from)
        .or_else(|| env::current_dir().ok())
        .ok_or_else(|| SpawnError {
            message: "failed to resolve child working directory".to_string(),
            started: false,
        })?;
    let current_directory = wide_null(cwd.as_os_str());

    let mut attributes = SecurityAttributes {
        length: size_of::<SecurityAttributes>() as u32,
        security_descriptor: null_mut(),
        inherit_handle: 1,
    };
    let (stdout_read, stdout_write) =
        create_pipe(&mut attributes).map_err(|message| SpawnError {
            message,
            started: false,
        })?;
    let (stderr_read, stderr_write) = match create_pipe(&mut attributes) {
        Ok(pair) => pair,
        Err(message) => {
            close(stdout_read);
            close(stdout_write);
            return Err(SpawnError {
                message,
                started: false,
            });
        }
    };
    let nul_name = wide_null(OsStr::new("NUL"));
    let stdin = unsafe {
        CreateFileW(
            nul_name.as_ptr(),
            GENERIC_READ,
            FILE_SHARE_READ | FILE_SHARE_WRITE,
            &mut attributes,
            OPEN_EXISTING,
            FILE_ATTRIBUTE_NORMAL,
            null_mut(),
        )
    };
    if stdin == INVALID_HANDLE_VALUE {
        close(stdout_read);
        close(stdout_write);
        close(stderr_read);
        close(stderr_write);
        return Err(SpawnError {
            message: last_error("CreateFileW(NUL)"),
            started: false,
        });
    }

    let job = unsafe { CreateJobObjectW(null(), null()) };
    if job.is_null() {
        close(stdout_read);
        close(stdout_write);
        close(stderr_read);
        close(stderr_write);
        close(stdin);
        return Err(SpawnError {
            message: last_error("CreateJobObjectW"),
            started: false,
        });
    }
    let mut limits = JobObjectExtendedLimitInformation::default();
    limits.basic_limit_information.limit_flags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
    if unsafe {
        SetInformationJobObject(
            job,
            JOB_OBJECT_EXTENDED_LIMIT_INFORMATION_CLASS,
            &limits as *const _ as *const c_void,
            size_of::<JobObjectExtendedLimitInformation>() as u32,
        )
    } == 0
    {
        close(stdout_read);
        close(stdout_write);
        close(stderr_read);
        close(stderr_write);
        close(stdin);
        close(job);
        return Err(SpawnError {
            message: last_error("SetInformationJobObject"),
            started: false,
        });
    }

    let mut startup: StartupInfoW = unsafe { zeroed() };
    startup.cb = size_of::<StartupInfoW>() as u32;
    startup.flags = STARTF_USESTDHANDLES;
    startup.stdin = stdin;
    startup.stdout = stdout_write;
    startup.stderr = stderr_write;
    let mut process_info: ProcessInformation = unsafe { zeroed() };
    let created = unsafe {
        CreateProcessW(
            application.as_mut_ptr(),
            command_line.as_mut_ptr(),
            null(),
            null(),
            1,
            CREATE_SUSPENDED | CREATE_UNICODE_ENVIRONMENT,
            environment.as_mut_ptr() as *mut c_void,
            current_directory.as_ptr(),
            &mut startup,
            &mut process_info,
        )
    };
    close(stdout_write);
    close(stderr_write);
    close(stdin);
    if created == 0 {
        close(stdout_read);
        close(stderr_read);
        close(job);
        return Err(SpawnError {
            message: last_error("CreateProcessW"),
            started: false,
        });
    }

    let fail_started = |message: String| -> SpawnError {
        unsafe {
            TerminateProcess(process_info.process, 1);
            WaitForSingleObject(process_info.process, INFINITE);
            CloseHandle(process_info.thread);
            CloseHandle(process_info.process);
            CloseHandle(job);
            CloseHandle(stdout_read);
            CloseHandle(stderr_read);
        }
        SpawnError {
            message,
            started: true,
        }
    };

    if unsafe { AssignProcessToJobObject(job, process_info.process) } == 0 {
        return Err(fail_started(last_error("AssignProcessToJobObject")));
    }
    if let Err(error) = process_created(process_info.process_id) {
        return Err(fail_started(format!(
            "process-created observer failed: {error}"
        )));
    }
    if unsafe { ResumeThread(process_info.thread) } == u32::MAX {
        return Err(fail_started(last_error("ResumeThread")));
    }
    close(process_info.thread);
    if let Err(error) = process_resumed(process_info.process_id) {
        unsafe {
            TerminateJobObject(job, 1);
            WaitForSingleObject(process_info.process, INFINITE);
            CloseHandle(process_info.process);
            CloseHandle(job);
            CloseHandle(stdout_read);
            CloseHandle(stderr_read);
        }
        return Err(SpawnError {
            message: format!("process-resumed observer failed: {error}"),
            started: true,
        });
    }

    let stdout = unsafe { File::from_raw_handle(stdout_read) };
    let stderr = unsafe { File::from_raw_handle(stderr_read) };
    Ok(ContainedChild {
        process: process_info.process,
        job,
        stdout: Some(stdout),
        stderr: Some(stderr),
        terminated: false,
    })
}

fn create_pipe(attributes: &mut SecurityAttributes) -> Result<(Handle, Handle), String> {
    let mut read = null_mut();
    let mut write = null_mut();
    if unsafe { CreatePipe(&mut read, &mut write, attributes, 0) } == 0 {
        return Err(last_error("CreatePipe"));
    }
    if unsafe { SetHandleInformation(read, HANDLE_FLAG_INHERIT, 0) } == 0 {
        close(read);
        close(write);
        return Err(last_error("SetHandleInformation"));
    }
    Ok((read, write))
}

fn resolve_executable(command: &str) -> Result<PathBuf, String> {
    let path = Path::new(command);
    if path.components().count() > 1 || path.is_absolute() {
        return path
            .canonicalize()
            .map_err(|error| format!("resolve executable {command}: {error}"));
    }
    let extensions = if path.extension().is_some() {
        vec![String::new()]
    } else {
        env::var("PATHEXT")
            .unwrap_or_else(|_| ".COM;.EXE;.BAT;.CMD".to_string())
            .split(';')
            .map(str::to_string)
            .collect()
    };
    let mut roots = Vec::new();
    if let Ok(current) = env::current_dir() {
        roots.push(current);
    }
    if let Some(value) = env::var_os("PATH") {
        roots.extend(env::split_paths(&value));
    }
    for root in roots {
        for extension in &extensions {
            let candidate = root.join(format!("{command}{extension}"));
            if candidate.is_file() {
                return candidate.canonicalize().map_err(|error| {
                    format!("resolve executable {}: {error}", candidate.display())
                });
            }
        }
    }
    Err(format!("executable not found on PATH: {command}"))
}

fn build_command_line(executable: &Path, args: &[String]) -> Vec<u16> {
    let mut command = quote_windows_arg(&executable.to_string_lossy());
    for arg in args {
        command.push(' ');
        command.push_str(&quote_windows_arg(arg));
    }
    wide_null(OsStr::new(&command))
}

fn quote_windows_arg(value: &str) -> String {
    if !value.is_empty()
        && !value
            .chars()
            .any(|character| character.is_whitespace() || character == '"')
    {
        return value.to_string();
    }
    let mut quoted = String::from("\"");
    let mut slashes = 0;
    for character in value.chars() {
        match character {
            '\\' => slashes += 1,
            '"' => {
                quoted.push_str(&"\\".repeat(slashes * 2 + 1));
                quoted.push('"');
                slashes = 0;
            }
            _ => {
                quoted.push_str(&"\\".repeat(slashes));
                slashes = 0;
                quoted.push(character);
            }
        }
    }
    quoted.push_str(&"\\".repeat(slashes * 2));
    quoted.push('"');
    quoted
}

fn build_environment(overrides: &BTreeMap<String, String>) -> Vec<u16> {
    let mut values = env::vars_os().collect::<Vec<(OsString, OsString)>>();
    for (key, value) in overrides {
        values.retain(|(existing, _)| !existing.to_string_lossy().eq_ignore_ascii_case(key));
        values.push((OsString::from(key), OsString::from(value)));
    }
    values.sort_by_key(|(key, _)| key.to_string_lossy().to_ascii_lowercase());
    let mut block = Vec::new();
    for (key, value) in values {
        block.extend(key.encode_wide());
        block.push(b'=' as u16);
        block.extend(value.encode_wide());
        block.push(0);
    }
    block.push(0);
    block
}

fn wide_null(value: &OsStr) -> Vec<u16> {
    value.encode_wide().chain(std::iter::once(0)).collect()
}

fn last_error(operation: &str) -> String {
    format!("{operation} failed with Windows error {}", unsafe {
        GetLastError()
    })
}

fn close(handle: Handle) {
    if !handle.is_null() && handle != INVALID_HANDLE_VALUE {
        unsafe {
            CloseHandle(handle);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Mutex};
    use std::thread;
    use std::time::{Duration, Instant};

    #[link(name = "kernel32")]
    extern "system" {
        fn OpenProcess(desired_access: u32, inherit_handle: i32, process_id: u32) -> Handle;
        fn CreateToolhelp32Snapshot(flags: u32, process_id: u32) -> Handle;
        fn Process32FirstW(snapshot: Handle, entry: *mut ProcessEntry32W) -> i32;
        fn Process32NextW(snapshot: Handle, entry: *mut ProcessEntry32W) -> i32;
    }

    const SYNCHRONIZE: u32 = 0x0010_0000;
    const TH32CS_SNAPPROCESS: u32 = 0x0000_0002;

    #[repr(C)]
    struct ProcessEntry32W {
        size: u32,
        usage: u32,
        process_id: u32,
        default_heap_id: usize,
        module_id: u32,
        threads: u32,
        parent_process_id: u32,
        priority_class_base: i32,
        flags: u32,
        exe_file: [u16; 260],
    }

    #[test]
    fn windows_argument_quoting_preserves_spaces_quotes_and_trailing_slashes() {
        assert_eq!(quote_windows_arg("plain"), "plain");
        assert_eq!(quote_windows_arg("two words"), "\"two words\"");
        assert_eq!(quote_windows_arg("a\"b"), "\"a\\\"b\"");
        assert_eq!(
            quote_windows_arg("C:\\path with space\\"),
            "\"C:\\path with space\\\\\""
        );
    }

    #[test]
    fn contained_spawn_observes_assignment_before_resume_and_captures_output() {
        let events = Arc::new(Mutex::new(Vec::new()));
        let created_events = Arc::clone(&events);
        let resumed_events = Arc::clone(&events);
        let invocation = Invocation::new(
            "cmd.exe",
            vec![
                "/D".to_string(),
                "/C".to_string(),
                "echo contained".to_string(),
            ],
        );
        let mut child = spawn(
            &invocation,
            None,
            move |process_id| {
                created_events.lock().unwrap().push(("created", process_id));
                Ok(())
            },
            move |process_id| {
                resumed_events.lock().unwrap().push(("resumed", process_id));
                Ok(())
            },
        )
        .map_err(|error| error.message)
        .unwrap();
        let mut output = String::new();
        child
            .take_stdout()
            .unwrap()
            .read_to_string(&mut output)
            .unwrap();
        assert_eq!(child.wait().unwrap(), 0);
        assert_eq!(output.trim(), "contained");
        let events = events.lock().unwrap();
        assert_eq!(events.len(), 2);
        assert_eq!(events[0].0, "created");
        assert_eq!(events[1].0, "resumed");
        assert_eq!(events[0].1, events[1].1);
    }

    fn descendant_invocation() -> Invocation {
        Invocation::new(
            "cmd.exe",
            vec![
                "/D".to_string(),
                "/S".to_string(),
                "/C".to_string(),
                "cmd.exe /D /S /C ping.exe -t 127.0.0.1".to_string(),
            ],
        )
    }

    fn spawn_with_descendant() -> (ContainedChild, Vec<u32>) {
        let root_pid = Arc::new(Mutex::new(None));
        let created_pid = Arc::clone(&root_pid);
        let child = spawn(
            &descendant_invocation(),
            None,
            move |process_id| {
                *created_pid.lock().unwrap() = Some(process_id);
                Ok(())
            },
            |_| Ok(()),
        )
        .map_err(|error| error.message)
        .unwrap();
        let root_pid = root_pid.lock().unwrap().unwrap();
        let started = Instant::now();
        loop {
            let descendants = descendants_of(root_pid);
            if !descendants.is_empty() {
                return (child, descendants);
            }
            assert!(
                started.elapsed() < Duration::from_secs(3),
                "contained root {root_pid} did not spawn a descendant"
            );
            thread::sleep(Duration::from_millis(25));
        }
    }

    fn descendants_of(root: u32) -> Vec<u32> {
        let snapshot = unsafe { CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0) };
        assert_ne!(snapshot, INVALID_HANDLE_VALUE);
        let mut entry: ProcessEntry32W = unsafe { zeroed() };
        entry.size = size_of::<ProcessEntry32W>() as u32;
        let mut pairs = Vec::new();
        let mut ok = unsafe { Process32FirstW(snapshot, &mut entry) };
        while ok != 0 {
            pairs.push((entry.process_id, entry.parent_process_id));
            ok = unsafe { Process32NextW(snapshot, &mut entry) };
        }
        close(snapshot);
        let mut found = Vec::new();
        let mut frontier = vec![root];
        while let Some(parent) = frontier.pop() {
            for (process_id, parent_process_id) in &pairs {
                if *parent_process_id == parent && !found.contains(process_id) {
                    found.push(*process_id);
                    frontier.push(*process_id);
                }
            }
        }
        found
    }

    fn process_running(process_id: u32) -> bool {
        let handle = unsafe { OpenProcess(SYNCHRONIZE, 0, process_id) };
        if handle.is_null() {
            return false;
        }
        let running = unsafe { WaitForSingleObject(handle, 0) } == WAIT_TIMEOUT;
        close(handle);
        running
    }

    fn wait_until_stopped(process_id: u32) {
        let started = Instant::now();
        while process_running(process_id) && started.elapsed() < Duration::from_secs(3) {
            thread::sleep(Duration::from_millis(25));
        }
        assert!(
            !process_running(process_id),
            "process {process_id} survived job teardown"
        );
    }

    #[test]
    fn terminate_kills_the_descendant_tree() {
        let (mut parent, descendants) = spawn_with_descendant();
        assert!(descendants.iter().all(|pid| process_running(*pid)));
        parent.terminate().unwrap();
        let _ = parent.wait().unwrap();
        for descendant in descendants {
            wait_until_stopped(descendant);
        }
    }

    #[test]
    fn closing_the_owner_job_handle_kills_descendants() {
        let (parent, descendants) = spawn_with_descendant();
        assert!(descendants.iter().all(|pid| process_running(*pid)));
        drop(parent);
        for descendant in descendants {
            wait_until_stopped(descendant);
        }
    }
}
