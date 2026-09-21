use std::fs::{self, File, OpenOptions};
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::process;
use std::sync::atomic::{AtomicU64, Ordering};

use crate::schema::HealthError;

static TEMP_SEQ: AtomicU64 = AtomicU64::new(1);

const BOM: [u8; 3] = [0xEF, 0xBB, 0xBF];
const SHARE_VIOLATION: i32 = 32;
const LOCK_VIOLATION: i32 = 33;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct FileIdentity {
    pub volume_serial: u32,
    pub file_index: u64,
}

pub fn strip_bom(bytes: &[u8]) -> &[u8] {
    if bytes.starts_with(&BOM) {
        &bytes[3..]
    } else {
        bytes
    }
}

pub fn is_locked_error(error: &io::Error) -> bool {
    matches!(
        error.raw_os_error(),
        Some(SHARE_VIOLATION) | Some(LOCK_VIOLATION)
    )
}

pub fn open_shared_read(path: &Path) -> io::Result<File> {
    let mut options = OpenOptions::new();
    options.read(true);
    apply_share_mode(&mut options);
    options.open(path)
}

pub fn open_shared_append(path: &Path) -> io::Result<File> {
    let mut options = OpenOptions::new();
    options.create(true).append(true).write(true);
    apply_share_mode(&mut options);
    options.open(path)
}

pub fn read_bounded(path: &Path, max_bytes: usize) -> io::Result<ReadBounded> {
    let mut file = open_shared_read(path)?;
    let mut buf = Vec::new();
    let mut chunk = [0_u8; 8192];
    let mut clipped = false;
    loop {
        let read = file.read(&mut chunk)?;
        if read == 0 {
            break;
        }
        let room = max_bytes.saturating_sub(buf.len());
        if read > room {
            buf.extend_from_slice(&chunk[..room]);
            clipped = true;
            break;
        }
        buf.extend_from_slice(&chunk[..read]);
    }
    Ok(ReadBounded {
        bytes: buf,
        clipped,
    })
}

pub struct ReadBounded {
    pub bytes: Vec<u8>,
    pub clipped: bool,
}

pub fn atomic_write(path: &Path, bytes: &[u8]) -> Result<(), HealthError> {
    let parent = path
        .parent()
        .ok_or_else(|| HealthError::msg("path has no parent"))?;
    fs::create_dir_all(parent)
        .map_err(|error| HealthError::msg(format!("create {}: {error}", parent.display())))?;
    let file_name = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("file");
    let temp_path = temp_sibling(parent, file_name);
    let write_result = (|| -> Result<(), HealthError> {
        let mut file = File::create(&temp_path).map_err(|error| {
            HealthError::msg(format!("create temp {}: {error}", temp_path.display()))
        })?;
        file.write_all(bytes).map_err(|error| {
            HealthError::msg(format!("write temp {}: {error}", temp_path.display()))
        })?;
        file.sync_all().map_err(|error| {
            HealthError::msg(format!("flush temp {}: {error}", temp_path.display()))
        })?;
        drop(file);
        replace_file(&temp_path, path)
    })();
    if write_result.is_err() {
        let _ = fs::remove_file(&temp_path);
    }
    write_result
}

pub fn replace_file(source: &Path, destination: &Path) -> Result<(), HealthError> {
    #[cfg(windows)]
    {
        move_replace(source, destination).map_err(|error| {
            HealthError::msg(format!(
                "replace {} -> {}: {error}",
                source.display(),
                destination.display()
            ))
        })
    }
    #[cfg(not(windows))]
    {
        fs::rename(source, destination).map_err(|error| {
            HealthError::msg(format!(
                "replace {} -> {}: {error}",
                source.display(),
                destination.display()
            ))
        })
    }
}

pub fn file_identity(path: &Path) -> io::Result<FileIdentity> {
    let file = open_shared_read(path)?;
    identity_from_file(&file)
}

fn temp_sibling(parent: &Path, file_name: &str) -> PathBuf {
    let seq = TEMP_SEQ.fetch_add(1, Ordering::Relaxed);
    parent.join(format!(".{file_name}.{}.{seq}.tmp", process::id()))
}

fn apply_share_mode(_options: &mut OpenOptions) {
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        use windows_sys::Win32::Storage::FileSystem::{
            FILE_SHARE_DELETE, FILE_SHARE_READ, FILE_SHARE_WRITE,
        };
        _options.share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE);
    }
}

#[cfg(windows)]
fn identity_from_file(file: &File) -> io::Result<FileIdentity> {
    use std::os::windows::io::AsRawHandle;
    use windows_sys::Win32::Storage::FileSystem::{
        GetFileInformationByHandle, BY_HANDLE_FILE_INFORMATION,
    };

    let mut info = unsafe { std::mem::zeroed::<BY_HANDLE_FILE_INFORMATION>() };
    let ok = unsafe { GetFileInformationByHandle(file.as_raw_handle(), &mut info) };
    if ok == 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(FileIdentity {
        volume_serial: info.dwVolumeSerialNumber,
        file_index: (u64::from(info.nFileIndexHigh) << 32) | u64::from(info.nFileIndexLow),
    })
}

#[cfg(not(windows))]
fn identity_from_file(_file: &File) -> io::Result<FileIdentity> {
    Err(io::Error::new(
        io::ErrorKind::Unsupported,
        "file identity requires Windows",
    ))
}

#[cfg(windows)]
fn move_replace(source: &Path, destination: &Path) -> io::Result<()> {
    use windows_sys::Win32::Storage::FileSystem::{
        MoveFileExW, MOVEFILE_REPLACE_EXISTING, MOVEFILE_WRITE_THROUGH,
    };

    let source = wide_null(source);
    let destination = wide_null(destination);
    let result = unsafe {
        MoveFileExW(
            source.as_ptr(),
            destination.as_ptr(),
            MOVEFILE_REPLACE_EXISTING | MOVEFILE_WRITE_THROUGH,
        )
    };
    if result == 0 {
        Err(io::Error::last_os_error())
    } else {
        Ok(())
    }
}

#[cfg(windows)]
pub fn wide_null(path: &Path) -> Vec<u16> {
    use std::os::windows::ffi::OsStrExt;

    path.as_os_str()
        .encode_wide()
        .chain(std::iter::once(0))
        .collect()
}
