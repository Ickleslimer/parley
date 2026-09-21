use crate::schema::HealthError;

pub const SUPERVISOR_MUTEX: &str = r"Local\ParleyHealthSupervisor";
pub const SUPERVISOR_SHUTDOWN_EVENT: &str = r"Local\ParleyHealthSupervisorShutdown";

pub struct InstanceGuard {
    #[cfg(windows)]
    handle: windows_sys::Win32::Foundation::HANDLE,
}

pub struct ShutdownEvent {
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

impl Drop for ShutdownEvent {
    fn drop(&mut self) {
        #[cfg(windows)]
        unsafe {
            windows_sys::Win32::Foundation::CloseHandle(self.handle);
        }
    }
}

impl ShutdownEvent {
    pub fn create(name: &str) -> Result<Self, HealthError> {
        #[cfg(windows)]
        {
            create_shutdown_event_windows(name)
        }
        #[cfg(not(windows))]
        {
            let _ = name;
            Ok(Self {})
        }
    }

    pub fn is_signaled(&self) -> bool {
        #[cfg(windows)]
        {
            use windows_sys::Win32::Foundation::WAIT_OBJECT_0;
            use windows_sys::Win32::System::Threading::WaitForSingleObject;
            unsafe { WaitForSingleObject(self.handle, 0) == WAIT_OBJECT_0 }
        }
        #[cfg(not(windows))]
        {
            false
        }
    }
}

pub fn signal_shutdown(name: &str) -> Result<(), HealthError> {
    #[cfg(windows)]
    {
        signal_shutdown_windows(name)
    }
    #[cfg(not(windows))]
    {
        let _ = name;
        Err(HealthError::msg(
            "supervisor shutdown signaling is Windows-only",
        ))
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

#[cfg(windows)]
fn create_shutdown_event_windows(name: &str) -> Result<ShutdownEvent, HealthError> {
    use windows_sys::Win32::System::Threading::CreateEventW;

    let wide = wide_null(name);
    let handle = unsafe { CreateEventW(std::ptr::null(), 1, 0, wide.as_ptr()) };
    if handle.is_null() {
        Err(HealthError::msg(format!(
            "CreateEventW failed: {}",
            std::io::Error::last_os_error()
        )))
    } else {
        Ok(ShutdownEvent { handle })
    }
}

#[cfg(windows)]
fn signal_shutdown_windows(name: &str) -> Result<(), HealthError> {
    use windows_sys::Win32::Foundation::CloseHandle;
    use windows_sys::Win32::System::Threading::{
        OpenEventW, SetEvent, EVENT_MODIFY_STATE, SYNCHRONIZATION_SYNCHRONIZE,
    };

    let wide = wide_null(name);
    let handle = unsafe {
        OpenEventW(
            EVENT_MODIFY_STATE | SYNCHRONIZATION_SYNCHRONIZE,
            0,
            wide.as_ptr(),
        )
    };
    if handle.is_null() {
        return Err(HealthError::msg(format!(
            "parley-health-supervisor is not running: {}",
            std::io::Error::last_os_error()
        )));
    }
    let result = unsafe { SetEvent(handle) };
    unsafe { CloseHandle(handle) };
    if result == 0 {
        Err(HealthError::msg(format!(
            "SetEvent failed: {}",
            std::io::Error::last_os_error()
        )))
    } else {
        Ok(())
    }
}

#[cfg(windows)]
fn wide_null(value: &str) -> Vec<u16> {
    use std::os::windows::ffi::OsStrExt;

    std::ffi::OsStr::new(value)
        .encode_wide()
        .chain(std::iter::once(0))
        .collect()
}

#[cfg(all(test, windows))]
mod tests {
    use super::*;

    #[test]
    fn named_shutdown_event_can_be_signaled() {
        let name = format!(
            r"Local\ParleyHealthShutdownTest-{}-{}",
            std::process::id(),
            crate::schema::now_ms()
        );
        let event = ShutdownEvent::create(&name).unwrap();
        assert!(!event.is_signaled());
        signal_shutdown(&name).unwrap();
        assert!(event.is_signaled());
    }
}
