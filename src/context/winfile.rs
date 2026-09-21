//! Std-only Windows shared opens, exclusive locks, and stable file identity.

use std::fs::{File, OpenOptions};
use std::io;
use std::path::{Path, PathBuf};

use super::error::{ContextError, ErrorKind};

pub(crate) const SHARE_READ: u32 = 0x0000_0001;
pub(crate) const SHARE_WRITE: u32 = 0x0000_0002;
pub(crate) const SHARE_DELETE: u32 = 0x0000_0004;
pub(crate) const SHARE_ALL: u32 = SHARE_READ | SHARE_WRITE | SHARE_DELETE;

const SHARE_VIOLATION: i32 = 32;
const LOCK_VIOLATION: i32 = 33;

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub(crate) struct SourceIdentity {
    pub volume_serial: u32,
    pub file_index: u64,
    pub canonical_path: String,
}

impl SourceIdentity {
    pub(crate) fn same_file(&self, other: &Self) -> bool {
        self.volume_serial == other.volume_serial && self.file_index == other.file_index
    }
}

pub(crate) fn is_lock_denied(error: &io::Error) -> bool {
    matches!(
        error.raw_os_error(),
        Some(SHARE_VIOLATION) | Some(LOCK_VIOLATION)
    )
}

pub(crate) fn open_shared_read(path: &Path) -> io::Result<File> {
    let mut options = OpenOptions::new();
    options.read(true);
    apply_share_all(&mut options);
    options.open(path)
}

pub(crate) fn open_shared_read_write(path: &Path) -> io::Result<File> {
    let mut options = OpenOptions::new();
    options.create(true).truncate(false).read(true).write(true);
    apply_share_all(&mut options);
    options.open(path)
}

pub(crate) fn open_exclusive_lock(path: &Path) -> Result<File, ContextError> {
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        match OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .share_mode(0)
            .open(path)
        {
            Ok(file) => Ok(file),
            Err(error) if is_lock_denied(&error) => Err(ContextError::new(
                ErrorKind::Locked,
                format!("context key lock is held: {}", path.display()),
            )),
            Err(error) => Err(error.into()),
        }
    }
    #[cfg(not(windows))]
    {
        let _ = path;
        Err(ContextError::unsupported(
            "exclusive context lock requires Windows share_mode(0)",
        ))
    }
}

pub(crate) fn canonical_path(path: &Path) -> PathBuf {
    std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf())
}

pub(crate) fn refuse_reparse_chain(path: &Path) -> Result<(), ContextError> {
    let mut current = PathBuf::new();
    for component in path.components() {
        current.push(component.as_os_str());
        let metadata = match std::fs::symlink_metadata(&current) {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == io::ErrorKind::NotFound => continue,
            Err(error) => return Err(error.into()),
        };
        if is_reparse(&metadata) {
            return Err(ContextError::new(
                ErrorKind::Io,
                format!(
                    "refusing context path through a reparse point: {}",
                    current.display()
                ),
            ));
        }
    }
    Ok(())
}

#[cfg(windows)]
fn is_reparse(metadata: &std::fs::Metadata) -> bool {
    use std::os::windows::fs::MetadataExt;
    metadata.file_attributes() & 0x400 != 0
}

#[cfg(not(windows))]
fn is_reparse(metadata: &std::fs::Metadata) -> bool {
    metadata.file_type().is_symlink()
}

pub(crate) fn source_identity(file: &File, path: &Path) -> Result<SourceIdentity, ContextError> {
    let canonical_path = canonical_path(path).to_string_lossy().into_owned();
    identity_from_file(file, canonical_path)
}

fn identity_from_file(file: &File, canonical_path: String) -> Result<SourceIdentity, ContextError> {
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
            fn GetFileInformationByHandle(
                file: *mut c_void,
                information: *mut ByHandleFileInformation,
            ) -> i32;
        }

        let mut information = MaybeUninit::<ByHandleFileInformation>::uninit();
        let ok = unsafe {
            GetFileInformationByHandle(file.as_raw_handle().cast(), information.as_mut_ptr())
        };
        if ok == 0 {
            return Err(io::Error::last_os_error().into());
        }
        let information = unsafe { information.assume_init() };
        let volume_serial = information.volume_serial_number;
        let file_index =
            (u64::from(information.file_index_high) << 32) | u64::from(information.file_index_low);
        Ok(SourceIdentity {
            volume_serial,
            file_index,
            canonical_path,
        })
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        let meta = file.metadata()?;
        Ok(SourceIdentity {
            volume_serial: meta.dev() as u32,
            file_index: meta.ino(),
            canonical_path,
        })
    }
    #[cfg(not(any(windows, unix)))]
    {
        let _ = file;
        Err(ContextError::unsupported(format!(
            "source identity is unsupported on this platform ({canonical_path})"
        )))
    }
}

fn apply_share_all(_options: &mut OpenOptions) {
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        _options.share_mode(SHARE_ALL);
    }
}
