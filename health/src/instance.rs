use crate::schema::HealthError;

pub struct InstanceGuard {
    #[cfg(windows)]
    handle: windows_sys::Win32::Foundation::HANDLE,
}

impl Drop for InstanceGuard {
    fn drop(&mut self) {
        #[cfg(windows)]
        {
            use windows_sys::Win32::Foundation::CloseHandle;
            unsafe {
                CloseHandle(self.handle);
            }
        }
    }
}

pub fn acquire(name: &str) -> Result<InstanceGuard, HealthError> {
    #[cfg(windows)]
    {
        acquire_windows(name)
    }
    #[cfg(not(windows))]
    {
        let _ = name;
        Ok(InstanceGuard {})
    }
}

#[cfg(windows)]
fn acquire_windows(name: &str) -> Result<InstanceGuard, HealthError> {
    use std::ffi::OsStr;
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::Foundation::{
        GetLastError, SetLastError, ERROR_ALREADY_EXISTS, ERROR_SUCCESS, INVALID_HANDLE_VALUE,
    };
    use windows_sys::Win32::System::Threading::CreateMutexW;

    let wide: Vec<u16> = OsStr::new(name)
        .encode_wide()
        .chain(std::iter::once(0))
        .collect();
    unsafe { SetLastError(ERROR_SUCCESS) };
    let handle = unsafe { CreateMutexW(std::ptr::null(), 1, wide.as_ptr()) };
    if handle.is_null() || handle == INVALID_HANDLE_VALUE {
        return Err(HealthError::msg(format!(
            "CreateMutexW failed: {}",
            std::io::Error::last_os_error()
        )));
    }
    let last = unsafe { GetLastError() };
    if last == ERROR_ALREADY_EXISTS {
        unsafe {
            windows_sys::Win32::Foundation::CloseHandle(handle);
        }
        return Err(HealthError::msg("parley-health-supervisor already running"));
    }
    Ok(InstanceGuard { handle })
}
