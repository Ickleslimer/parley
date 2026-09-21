use std::fs::{self, File, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::process;
use std::sync::atomic::{AtomicU64, Ordering};

use crate::error::{Denial, LaneError};
use crate::schema::FileIdentity;

const BOM: [u8; 3] = [0xEF, 0xBB, 0xBF];
static TEMP_SEQ: AtomicU64 = AtomicU64::new(1);

pub fn strip_bom(bytes: &[u8]) -> &[u8] {
    if bytes.starts_with(&BOM) {
        &bytes[3..]
    } else {
        bytes
    }
}

pub struct StateLock {
    _file: File,
}

pub fn acquire_lock(state_dir: &Path) -> Result<StateLock, LaneError> {
    fs::create_dir_all(state_dir).map_err(|error| {
        LaneError::new(
            Denial::MalformedGrant,
            format!("create lane state dir: {error}"),
        )
    })?;
    refuse_reparse_chain(state_dir)?;
    let path = state_dir.join("grants.lock");
    let file = open_exclusive(&path)?;
    Ok(StateLock { _file: file })
}

pub fn atomic_create_new(destination: &Path, bytes: &[u8]) -> Result<(), LaneError> {
    atomic_move(destination, bytes, false)
}

pub fn atomic_replace(destination: &Path, bytes: &[u8]) -> Result<(), LaneError> {
    atomic_move(destination, bytes, true)
}

pub fn read_limited(path: &Path, max_bytes: usize) -> Result<Vec<u8>, LaneError> {
    let mut file = open_shared_read(path).map_err(|error| {
        if error.kind() == io::ErrorKind::NotFound {
            LaneError::new(Denial::MissingGrant, "grant file is missing")
        } else {
            LaneError::new(Denial::MalformedGrant, format!("read grant: {error}"))
        }
    })?;
    let mut buf = Vec::new();
    let mut chunk = [0_u8; 8192];
    loop {
        let read = io::Read::read(&mut file, &mut chunk).map_err(|error| {
            LaneError::new(Denial::MalformedGrant, format!("read grant: {error}"))
        })?;
        if read == 0 {
            break;
        }
        if buf.len() + read > max_bytes {
            return Err(LaneError::new(
                Denial::MalformedGrant,
                "grant file exceeds bound",
            ));
        }
        buf.extend_from_slice(&chunk[..read]);
    }
    Ok(buf)
}

pub fn file_identity(path: &Path) -> Result<FileIdentity, LaneError> {
    let file = open_identity(path).map_err(|error| {
        if error.kind() == io::ErrorKind::NotFound {
            LaneError::new(Denial::StaleGrant, "grant path is missing")
        } else {
            LaneError::new(Denial::Cwd, format!("open grant path: {error}"))
        }
    })?;
    identity_of(&file)
}

pub fn refuse_reparse_chain(path: &Path) -> Result<(), LaneError> {
    let mut current = PathBuf::new();
    let mut inspect = false;
    for component in path.components() {
        match component {
            std::path::Component::ParentDir | std::path::Component::CurDir => {
                return Err(LaneError::new(
                    Denial::Path,
                    "path contains a traversal component",
                ));
            }
            std::path::Component::Prefix(_) => {
                current.push(component);
            }
            std::path::Component::RootDir | std::path::Component::Normal(_) => {
                current.push(component);
                inspect = true;
            }
        }
        if !inspect {
            continue;
        }
        match fs::symlink_metadata(&current) {
            Ok(metadata) if metadata_is_reparse(&metadata) => {
                return Err(LaneError::new(
                    Denial::Path,
                    "path traverses a reparse point",
                ));
            }
            Ok(_) => {}
            Err(error) if error.kind() == io::ErrorKind::NotFound => break,
            Err(error) => {
                return Err(LaneError::new(
                    Denial::Path,
                    format!("inspect path: {error}"),
                ));
            }
        }
    }
    Ok(())
}

fn atomic_move(destination: &Path, bytes: &[u8], replace: bool) -> Result<(), LaneError> {
    let parent = destination
        .parent()
        .ok_or_else(|| LaneError::new(Denial::MalformedGrant, "grant path has no parent"))?;
    fs::create_dir_all(parent).map_err(|error| {
        LaneError::new(Denial::MalformedGrant, format!("create grant dir: {error}"))
    })?;
    refuse_reparse_chain(parent)?;
    let temp = temp_sibling(parent, "grant");
    let write_result = (|| -> Result<(), LaneError> {
        let mut file = File::create(&temp).map_err(|error| {
            LaneError::new(
                Denial::MalformedGrant,
                format!("create temp grant: {error}"),
            )
        })?;
        file.write_all(bytes).map_err(|error| {
            LaneError::new(Denial::MalformedGrant, format!("write temp grant: {error}"))
        })?;
        file.sync_all().map_err(|error| {
            LaneError::new(Denial::MalformedGrant, format!("flush temp grant: {error}"))
        })?;
        drop(file);
        move_file(&temp, destination, replace)
    })();
    if write_result.is_err() {
        let _ = fs::remove_file(&temp);
    }
    write_result
}

fn temp_sibling(parent: &Path, stem: &str) -> PathBuf {
    let seq = TEMP_SEQ.fetch_add(1, Ordering::Relaxed);
    parent.join(format!(".{stem}.{}.{seq}.tmp", process::id()))
}

fn open_shared_read(path: &Path) -> io::Result<File> {
    let mut options = OpenOptions::new();
    options.read(true);
    apply_share_all(&mut options);
    options.open(path)
}

fn open_exclusive(path: &Path) -> Result<File, LaneError> {
    let mut options = OpenOptions::new();
    options.read(true).write(true).create(true).truncate(false);
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        options.share_mode(0);
    }
    match options.open(path) {
        Ok(file) => Ok(file),
        Err(error) if is_share_violation(&error) => {
            Err(LaneError::new(Denial::Lock, "lane state lock is held"))
        }
        Err(error) => Err(LaneError::new(
            Denial::Lock,
            format!("acquire lane state lock: {error}"),
        )),
    }
}

fn open_identity(path: &Path) -> io::Result<File> {
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        use windows_sys::Win32::Storage::FileSystem::FILE_FLAG_BACKUP_SEMANTICS;

        let mut options = OpenOptions::new();
        options.read(true).custom_flags(FILE_FLAG_BACKUP_SEMANTICS);
        apply_share_all(&mut options);
        options.open(path)
    }
    #[cfg(not(windows))]
    {
        open_shared_read(path)
    }
}

fn apply_share_all(options: &mut OpenOptions) {
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        use windows_sys::Win32::Storage::FileSystem::{
            FILE_SHARE_DELETE, FILE_SHARE_READ, FILE_SHARE_WRITE,
        };
        options.share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE);
    }
    #[cfg(not(windows))]
    {
        let _ = options;
    }
}

fn is_share_violation(error: &io::Error) -> bool {
    matches!(error.raw_os_error(), Some(32) | Some(33))
}

pub(crate) fn metadata_is_reparse(metadata: &fs::Metadata) -> bool {
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        metadata.file_attributes() & 0x400 != 0
    }
    #[cfg(not(windows))]
    {
        metadata.file_type().is_symlink()
    }
}

fn identity_of(file: &File) -> Result<FileIdentity, LaneError> {
    #[cfg(windows)]
    {
        use std::os::windows::io::AsRawHandle;
        use windows_sys::Win32::Storage::FileSystem::{
            GetFileInformationByHandle, BY_HANDLE_FILE_INFORMATION,
        };

        let mut info = unsafe { std::mem::zeroed::<BY_HANDLE_FILE_INFORMATION>() };
        let ok = unsafe { GetFileInformationByHandle(file.as_raw_handle(), &mut info) };
        if ok == 0 {
            return Err(LaneError::new(
                Denial::Cwd,
                format!("read file identity: {}", io::Error::last_os_error()),
            ));
        }
        let file_index = (u64::from(info.nFileIndexHigh) << 32) | u64::from(info.nFileIndexLow);
        Ok(FileIdentity {
            volume_serial: info.dwVolumeSerialNumber.to_string(),
            file_index: file_index.to_string(),
        })
    }
    #[cfg(not(windows))]
    {
        let _ = file;
        Err(LaneError::new(
            Denial::Cwd,
            "file identity requires Windows",
        ))
    }
}

fn move_file(source: &Path, destination: &Path, replace: bool) -> Result<(), LaneError> {
    #[cfg(windows)]
    {
        use windows_sys::Win32::Storage::FileSystem::{
            MoveFileExW, MOVEFILE_REPLACE_EXISTING, MOVEFILE_WRITE_THROUGH,
        };

        let mut flags = MOVEFILE_WRITE_THROUGH;
        if replace {
            flags |= MOVEFILE_REPLACE_EXISTING;
        }
        let source_wide = wide_null(source);
        let destination_wide = wide_null(destination);
        let result = unsafe { MoveFileExW(source_wide.as_ptr(), destination_wide.as_ptr(), flags) };
        if result == 0 {
            let error = io::Error::last_os_error();
            let denial = if !replace && destination.exists() {
                Denial::Duplicate
            } else {
                Denial::MalformedGrant
            };
            return Err(LaneError::new(
                denial,
                format!("replace grant file: {error}"),
            ));
        }
        Ok(())
    }
    #[cfg(not(windows))]
    {
        let _ = (source, destination, replace);
        Err(LaneError::new(
            Denial::MalformedGrant,
            "atomic grant replace requires Windows",
        ))
    }
}

#[cfg(windows)]
fn wide_null(path: &Path) -> Vec<u16> {
    use std::os::windows::ffi::OsStrExt;
    path.as_os_str()
        .encode_wide()
        .chain(std::iter::once(0))
        .collect()
}
