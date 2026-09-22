use std::fs::{self, File, OpenOptions};
use std::io::{self, Read, Write};
use std::path::{Component, Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::thread;
use std::time::Duration;

use crate::schema::MAX_JOURNAL_BYTES;

const SHARE_ALL: u32 = 0x0000_0001 | 0x0000_0002 | 0x0000_0004;
const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x400;

static TEMP_SEQ: AtomicU64 = AtomicU64::new(1);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FailKind {
    Arguments,
    Missing,
    Locked,
    Malformed,
    Stale,
    Capability,
    Process,
    Session,
    FileIdentity,
    Path,
    Expired,
    Cwd,
    Exists,
    Io,
}

impl FailKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Arguments => "arguments",
            Self::Missing => "missing",
            Self::Locked => "locked",
            Self::Malformed => "malformed",
            Self::Stale => "stale",
            Self::Capability => "capability",
            Self::Process => "process",
            Self::Session => "session",
            Self::FileIdentity => "file_identity",
            Self::Path => "path",
            Self::Expired => "expired",
            Self::Cwd => "cwd",
            Self::Exists => "exists",
            Self::Io => "io",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FileIdentity {
    pub volume_serial: u32,
    pub file_index: u64,
}

pub struct ExclusiveLock {
    _file: File,
}

pub fn is_lock_denied(error: &io::Error) -> bool {
    matches!(error.raw_os_error(), Some(32) | Some(33))
}

pub fn valid_identifier(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 128
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b':'))
}

pub fn refuse_reparse_chain(path: &Path) -> Result<(), FailKind> {
    let mut current = PathBuf::new();
    for component in path.components() {
        if matches!(component, Component::ParentDir | Component::CurDir) {
            return Err(FailKind::Path);
        }
        current.push(component.as_os_str());
        if matches!(component, Component::Prefix(_) | Component::RootDir) {
            continue;
        }
        let metadata = match fs::symlink_metadata(&current) {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == io::ErrorKind::NotFound => continue,
            Err(error) if is_lock_denied(&error) => return Err(FailKind::Locked),
            Err(_) => return Err(FailKind::Io),
        };
        if is_reparse(&metadata) {
            return Err(FailKind::Path);
        }
    }
    Ok(())
}

pub fn canonical_existing(path: &Path) -> Result<PathBuf, FailKind> {
    refuse_reparse_chain(path)?;
    match fs::canonicalize(path) {
        Ok(path) => {
            refuse_reparse_chain(&path)?;
            Ok(path)
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => Err(FailKind::Missing),
        Err(error) if is_lock_denied(&error) => Err(FailKind::Locked),
        Err(_) => Err(FailKind::Io),
    }
}

pub fn require_job_dir(state_dir: &Path, job_dir: &Path, job_id: &str) -> Result<(), FailKind> {
    if !valid_identifier(job_id) || job_id.contains(['/', '\\']) {
        return Err(FailKind::Path);
    }
    if !state_dir.is_absolute() || !job_dir.is_absolute() {
        return Err(FailKind::Path);
    }
    let state = canonical_existing(state_dir)?;
    let job = canonical_existing(job_dir)?;
    let expected = canonical_existing(&state.join("jobs").join(job_id))?;
    if job != expected || !job.starts_with(&state) {
        return Err(FailKind::Path);
    }
    Ok(())
}

pub fn file_identity(path: &Path) -> Result<FileIdentity, FailKind> {
    refuse_reparse_chain(path)?;
    let file = open_shared_read(path)?;
    identity_of(&file)
}

pub fn same_file(left: &Path, right: &Path) -> Result<bool, FailKind> {
    Ok(file_identity(left)? == file_identity(right)?)
}

pub fn same_directory(left: &Path, right: &Path) -> Result<bool, FailKind> {
    Ok(canonical_existing(left)? == canonical_existing(right)?)
}

pub fn read_shared(path: &Path, max_bytes: usize) -> Result<Vec<u8>, FailKind> {
    let mut file = open_shared_read(path)?;
    let mut buf = Vec::new();
    let mut chunk = [0_u8; 8192];
    loop {
        let read = file.read(&mut chunk).map_err(map_io)?;
        if read == 0 {
            break;
        }
        if buf.len().saturating_add(read) > max_bytes {
            return Err(FailKind::Malformed);
        }
        buf.extend_from_slice(&chunk[..read]);
    }
    Ok(buf)
}

pub fn read_tail(path: &Path, max_bytes: usize) -> Result<Vec<u8>, FailKind> {
    let mut file = open_shared_read(path)?;
    let length = file.metadata().map_err(map_io)?.len();
    if length > max_bytes as u64 {
        use std::io::Seek;
        file.seek(std::io::SeekFrom::Start(length - max_bytes as u64))
            .map_err(map_io)?;
    }
    let mut buf = Vec::new();
    file.read_to_end(&mut buf).map_err(map_io)?;
    if length > max_bytes as u64 {
        if let Some(index) = buf.iter().position(|byte| *byte == b'\n') {
            buf.drain(..=index);
        } else {
            buf.clear();
        }
    }
    Ok(buf)
}

pub fn read_prefix(path: &Path, max_bytes: usize) -> Result<Vec<u8>, FailKind> {
    let file = open_shared_read(path)?;
    let mut buf = Vec::new();
    std::io::Read::take(file, max_bytes as u64)
        .read_to_end(&mut buf)
        .map_err(map_io)?;
    Ok(buf)
}

pub fn acquire_lock(directory: &Path) -> Result<ExclusiveLock, FailKind> {
    refuse_reparse_chain(directory)?;
    let path = directory.join("lock");
    let mut last = FailKind::Locked;
    for _ in 0..50 {
        match open_exclusive(&path) {
            Ok(file) => return Ok(ExclusiveLock { _file: file }),
            Err(FailKind::Locked) => {
                last = FailKind::Locked;
                thread::sleep(Duration::from_millis(20));
            }
            Err(error) => return Err(error),
        }
    }
    Err(last)
}

pub fn atomic_replace(directory: &Path, file_name: &str, bytes: &[u8]) -> Result<(), FailKind> {
    if file_name.contains(['/', '\\']) || file_name.starts_with('.') {
        return Err(FailKind::Path);
    }
    let destination = directory.join(file_name);
    if !destination.starts_with(directory) {
        return Err(FailKind::Path);
    }
    refuse_reparse_chain(directory)?;
    if destination.exists() {
        refuse_reparse_chain(&destination)?;
    }
    let temporary = directory.join(format!(
        ".snapshot.{}.{}.tmp",
        std::process::id(),
        TEMP_SEQ.fetch_add(1, Ordering::Relaxed)
    ));
    let write_result = (|| {
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temporary)
            .map_err(map_io)?;
        file.write_all(bytes).map_err(map_io)?;
        file.flush().map_err(map_io)?;
        file.sync_all().map_err(map_io)?;
        Ok(())
    })();
    if let Err(error) = write_result {
        let _ = fs::remove_file(&temporary);
        return Err(error);
    }
    match replace_file(&temporary, &destination) {
        Ok(()) => Ok(()),
        Err(error) => {
            let _ = fs::remove_file(&temporary);
            Err(error)
        }
    }
}

pub fn create_new_synced(path: &Path, bytes: &[u8]) -> Result<(), FailKind> {
    if let Some(parent) = path.parent() {
        refuse_reparse_chain(parent)?;
        if !path.starts_with(parent) {
            return Err(FailKind::Path);
        }
    }
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
        .map_err(|error| {
            if error.kind() == io::ErrorKind::AlreadyExists {
                FailKind::Exists
            } else {
                map_io(error)
            }
        })?;
    let result = file
        .write_all(bytes)
        .and_then(|_| file.flush())
        .and_then(|_| file.sync_all())
        .map_err(map_io);
    drop(file);
    if let Err(error) = result {
        let _ = fs::remove_file(path);
        return Err(error);
    }
    Ok(())
}

pub fn bounded_journal_bytes() -> usize {
    MAX_JOURNAL_BYTES
}

fn open_shared_read(path: &Path) -> Result<File, FailKind> {
    let mut options = OpenOptions::new();
    options.read(true);
    apply_share_all(&mut options);
    options.open(path).map_err(map_io)
}

fn open_exclusive(path: &Path) -> Result<File, FailKind> {
    let mut options = OpenOptions::new();
    options.read(true).write(true).create(true).truncate(false);
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        options.share_mode(0);
    }
    options.open(path).map_err(map_io)
}

fn apply_share_all(options: &mut OpenOptions) {
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        options.share_mode(SHARE_ALL);
    }
    #[cfg(not(windows))]
    {
        let _ = options;
    }
}

fn map_io(error: io::Error) -> FailKind {
    if error.kind() == io::ErrorKind::NotFound {
        FailKind::Missing
    } else if error.kind() == io::ErrorKind::AlreadyExists {
        FailKind::Io
    } else if is_lock_denied(&error) {
        FailKind::Locked
    } else {
        FailKind::Io
    }
}

#[cfg(windows)]
fn replace_file(source: &Path, destination: &Path) -> Result<(), FailKind> {
    use std::os::windows::ffi::OsStrExt;

    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn MoveFileExW(existing: *const u16, replacement: *const u16, flags: u32) -> i32;
        fn ReplaceFileW(
            replaced: *const u16,
            replacement: *const u16,
            backup: *const u16,
            flags: u32,
            exclude: *mut std::ffi::c_void,
            reserved: *mut std::ffi::c_void,
        ) -> i32;
    }

    const MOVEFILE_REPLACE_EXISTING: u32 = 0x0000_0001;
    const MOVEFILE_WRITE_THROUGH: u32 = 0x0000_0008;

    let source = source
        .as_os_str()
        .encode_wide()
        .chain(std::iter::once(0))
        .collect::<Vec<_>>();
    let destination = destination
        .as_os_str()
        .encode_wide()
        .chain(std::iter::once(0))
        .collect::<Vec<_>>();
    let destination_exists = destination_path_exists(destination.as_ptr());
    let ok = if destination_exists {
        unsafe {
            ReplaceFileW(
                destination.as_ptr(),
                source.as_ptr(),
                std::ptr::null(),
                0,
                std::ptr::null_mut(),
                std::ptr::null_mut(),
            )
        }
    } else {
        unsafe {
            MoveFileExW(
                source.as_ptr(),
                destination.as_ptr(),
                MOVEFILE_REPLACE_EXISTING | MOVEFILE_WRITE_THROUGH,
            )
        }
    };
    if ok == 0 {
        Err(map_io(io::Error::last_os_error()))
    } else {
        Ok(())
    }
}

#[cfg(windows)]
fn destination_path_exists(path: *const u16) -> bool {
    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn GetFileAttributesW(path: *const u16) -> u32;
    }

    unsafe { GetFileAttributesW(path) != u32::MAX }
}

#[cfg(not(windows))]
fn replace_file(source: &Path, destination: &Path) -> Result<(), FailKind> {
    fs::rename(source, destination).map_err(map_io)
}

fn is_reparse(metadata: &fs::Metadata) -> bool {
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0
    }
    #[cfg(not(windows))]
    {
        metadata.file_type().is_symlink()
    }
}

fn identity_of(file: &File) -> Result<FileIdentity, FailKind> {
    #[cfg(windows)]
    {
        use std::ffi::c_void;
        use std::mem::MaybeUninit;
        use std::os::windows::io::AsRawHandle;

        #[repr(C)]
        struct FileTime {
            low: u32,
            high: u32,
        }

        #[repr(C)]
        struct ByHandleFileInformation {
            attributes: u32,
            creation_time: FileTime,
            last_access_time: FileTime,
            last_write_time: FileTime,
            volume_serial_number: u32,
            file_size_high: u32,
            file_size_low: u32,
            number_of_links: u32,
            file_index_high: u32,
            file_index_low: u32,
        }

        #[link(name = "kernel32")]
        unsafe extern "system" {
            fn GetFileInformationByHandle(file: *mut c_void, information: *mut c_void) -> i32;
        }

        let mut information = MaybeUninit::<ByHandleFileInformation>::uninit();
        let ok = unsafe {
            GetFileInformationByHandle(file.as_raw_handle().cast(), information.as_mut_ptr().cast())
        };
        if ok == 0 {
            return Err(FailKind::Io);
        }
        let information = unsafe { information.assume_init() };
        let file_index =
            (u64::from(information.file_index_high) << 32) | u64::from(information.file_index_low);
        Ok(FileIdentity {
            volume_serial: information.volume_serial_number,
            file_index,
        })
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        let metadata = file.metadata().map_err(|_| FailKind::Io)?;
        Ok(FileIdentity {
            volume_serial: metadata.dev() as u32,
            file_index: metadata.ino(),
        })
    }
    #[cfg(not(any(windows, unix)))]
    {
        let _ = file;
        Err(FailKind::Io)
    }
}

pub fn ancestor_pids(start: u32) -> Vec<u32> {
    let mut ancestors = Vec::new();
    let mut current = start;
    let mut seen = std::collections::BTreeSet::new();
    for _ in 0..16 {
        let Some(parent) = parent_pid(current) else {
            break;
        };
        if parent == 0 || parent == current || !seen.insert(parent) {
            break;
        }
        ancestors.push(parent);
        current = parent;
    }
    ancestors
}

fn parent_pid(pid: u32) -> Option<u32> {
    #[cfg(windows)]
    {
        use std::ffi::c_void;

        #[repr(C)]
        struct ProcessBasicInformation {
            exit_status: i32,
            _pad0: u32,
            peb_base_address: *mut c_void,
            affinity_mask: usize,
            base_priority: i32,
            _pad1: u32,
            unique_process_id: usize,
            inherited_from_unique_process_id: usize,
        }

        #[link(name = "kernel32")]
        unsafe extern "system" {
            fn GetCurrentProcess() -> *mut c_void;
            fn OpenProcess(access: u32, inherit: i32, pid: u32) -> *mut c_void;
            fn CloseHandle(handle: *mut c_void) -> i32;
        }

        #[link(name = "ntdll")]
        unsafe extern "system" {
            fn NtQueryInformationProcess(
                process: *mut c_void,
                class: u32,
                information: *mut c_void,
                length: u32,
                returned: *mut u32,
            ) -> i32;
        }

        const PROCESS_QUERY_LIMITED_INFORMATION: u32 = 0x1000;
        unsafe {
            let current = pid == std::process::id();
            let handle = if current {
                GetCurrentProcess()
            } else {
                OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid)
            };
            if handle.is_null() {
                return None;
            }
            let mut info = std::mem::zeroed::<ProcessBasicInformation>();
            let mut returned = 0_u32;
            let status = NtQueryInformationProcess(
                handle,
                0,
                &mut info as *mut ProcessBasicInformation as *mut c_void,
                std::mem::size_of::<ProcessBasicInformation>() as u32,
                &mut returned,
            );
            if !current {
                CloseHandle(handle);
            }
            if status != 0 {
                return None;
            }
            let parent = info.inherited_from_unique_process_id;
            u32::try_from(parent).ok().filter(|value| *value != 0)
        }
    }
    #[cfg(not(windows))]
    {
        let _ = pid;
        None
    }
}
