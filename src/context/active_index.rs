use std::path::{Path, PathBuf};

use super::error::ContextError;

pub(super) fn active_rollout(home: &Path, session: &str) -> Result<Option<PathBuf>, ContextError> {
    for entry in std::fs::read_dir(home)? {
        let name = entry?.file_name();
        if name
            .to_str()
            .and_then(|name| name.strip_prefix("state_"))
            .and_then(|name| name.strip_suffix(".sqlite"))
            .and_then(|version| version.parse::<u32>().ok())
            .is_some_and(|version| version > 5)
        {
            return Err(ContextError::unsupported("newer Codex state index requires a reviewed reader; refusing a stale state_5 pointer"));
        }
    }
    let database = home.join("state_5.sqlite");
    if !database.try_exists()? {
        return Ok(None);
    }
    super::winfile::refuse_reparse_chain(&database)?;
    platform::lookup(&database, session).map(Some)
}

#[cfg(all(test, windows))]
pub(crate) fn with_fixture(database: &Path, rows: &[(&str, &Path)], check: impl FnOnce()) {
    platform::with_fixture(database, rows, check);
}

#[cfg(not(windows))]
mod platform {
    use super::*;

    pub(super) fn lookup(_: &Path, _: &str) -> Result<PathBuf, ContextError> {
        Err(ContextError::unsupported(
            "native Codex active-rollout index requires Windows system SQLite",
        ))
    }
}

#[cfg(windows)]
mod platform {
    use std::ffi::{c_char, c_int, c_void, CString};
    use std::ptr;

    use super::*;

    type Open =
        unsafe extern "C" fn(*const c_char, *mut *mut c_void, c_int, *const c_char) -> c_int;
    type Close = unsafe extern "C" fn(*mut c_void) -> c_int;
    type Prepare = unsafe extern "C" fn(
        *mut c_void,
        *const c_char,
        c_int,
        *mut *mut c_void,
        *mut *const c_char,
    ) -> c_int;
    type Bind = unsafe extern "C" fn(
        *mut c_void,
        c_int,
        *const c_char,
        c_int,
        Option<unsafe extern "C" fn(*mut c_void)>,
    ) -> c_int;
    type Step = unsafe extern "C" fn(*mut c_void) -> c_int;
    type Column = unsafe extern "C" fn(*mut c_void, c_int) -> *const u8;
    type ColumnInt = unsafe extern "C" fn(*mut c_void, c_int) -> c_int;
    type Timeout = unsafe extern "C" fn(*mut c_void, c_int) -> c_int;

    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn LoadLibraryExW(name: *const u16, file: *mut c_void, flags: u32) -> *mut c_void;
        fn GetProcAddress(module: *mut c_void, name: *const c_char) -> *mut c_void;
        fn FreeLibrary(module: *mut c_void) -> i32;
    }

    struct Library(*mut c_void);

    impl Drop for Library {
        fn drop(&mut self) {
            unsafe { FreeLibrary(self.0) };
        }
    }

    impl Library {
        fn load() -> Result<Self, ContextError> {
            let name: Vec<u16> = "winsqlite3.dll\0".encode_utf16().collect();
            let module = unsafe { LoadLibraryExW(name.as_ptr(), ptr::null_mut(), 0x800) };
            if module.is_null() {
                return Err(ContextError::unsupported(
                    "Windows system SQLite unavailable",
                ));
            }
            Ok(Self(module))
        }

        fn symbol(&self, name: &std::ffi::CStr) -> Result<*mut c_void, ContextError> {
            let address = unsafe { GetProcAddress(self.0, name.as_ptr()) };
            if address.is_null() {
                return Err(ContextError::unsupported(
                    "Windows system SQLite API unavailable",
                ));
            }
            Ok(address)
        }
    }

    struct Connection {
        handle: *mut c_void,
        close: Close,
    }

    impl Drop for Connection {
        fn drop(&mut self) {
            unsafe { (self.close)(self.handle) };
        }
    }

    struct Statement {
        handle: *mut c_void,
        finalize: Close,
    }

    impl Drop for Statement {
        fn drop(&mut self) {
            unsafe { (self.finalize)(self.handle) };
        }
    }

    fn checked(code: c_int) -> Result<(), ContextError> {
        if code == 0 {
            Ok(())
        } else {
            Err(ContextError::state(format!(
                "Codex active-rollout index read failed (SQLite {code})"
            )))
        }
    }

    #[cfg(test)]
    pub(super) fn with_fixture(database: &Path, rows: &[(&str, &Path)], check: impl FnOnce()) {
        let library = Library::load().unwrap();
        let open: Open =
            unsafe { std::mem::transmute(library.symbol(c"sqlite3_open_v2").unwrap()) };
        let close: Close =
            unsafe { std::mem::transmute(library.symbol(c"sqlite3_close").unwrap()) };
        type Execute = unsafe extern "C" fn(
            *mut c_void,
            *const c_char,
            *mut c_void,
            *mut c_void,
            *mut *mut c_char,
        ) -> c_int;
        let execute: Execute =
            unsafe { std::mem::transmute(library.symbol(c"sqlite3_exec").unwrap()) };
        let filename = CString::new(database.to_str().unwrap()).unwrap();
        let mut handle = ptr::null_mut();
        checked(unsafe { open(filename.as_ptr(), &mut handle, 6, ptr::null()) }).unwrap();
        let connection = Connection { handle, close };
        let mut sql = "PRAGMA journal_mode=WAL; CREATE TABLE threads (id TEXT, rollout_path TEXT);"
            .to_string();
        for (session, path) in rows {
            sql.push_str(&format!(
                "INSERT INTO threads VALUES ('{}','{}');",
                session.replace('\'', "''"),
                path.to_str().unwrap().replace('\'', "''")
            ));
        }
        let sql = CString::new(sql).unwrap();
        checked(unsafe {
            execute(
                connection.handle,
                sql.as_ptr(),
                ptr::null_mut(),
                ptr::null_mut(),
                ptr::null_mut(),
            )
        })
        .unwrap();
        check();
    }

    pub(super) fn lookup(database: &Path, session: &str) -> Result<PathBuf, ContextError> {
        let library = Library::load()?;
        let open: Open = unsafe { std::mem::transmute(library.symbol(c"sqlite3_open_v2")?) };
        let close: Close = unsafe { std::mem::transmute(library.symbol(c"sqlite3_close")?) };
        let prepare: Prepare =
            unsafe { std::mem::transmute(library.symbol(c"sqlite3_prepare_v2")?) };
        let bind: Bind = unsafe { std::mem::transmute(library.symbol(c"sqlite3_bind_text")?) };
        let step: Step = unsafe { std::mem::transmute(library.symbol(c"sqlite3_step")?) };
        let column: Column =
            unsafe { std::mem::transmute(library.symbol(c"sqlite3_column_text")?) };
        let column_type: ColumnInt =
            unsafe { std::mem::transmute(library.symbol(c"sqlite3_column_type")?) };
        let column_bytes: ColumnInt =
            unsafe { std::mem::transmute(library.symbol(c"sqlite3_column_bytes")?) };
        let timeout: Timeout =
            unsafe { std::mem::transmute(library.symbol(c"sqlite3_busy_timeout")?) };
        let finalize: Close = unsafe { std::mem::transmute(library.symbol(c"sqlite3_finalize")?) };
        let filename = CString::new(
            database
                .to_string_lossy()
                .strip_prefix(r"\\?\")
                .unwrap_or(&database.to_string_lossy())
                .as_bytes(),
        )
        .map_err(|_| ContextError::malformed("invalid Codex index path"))?;
        let parameter =
            CString::new(session).map_err(|_| ContextError::malformed("invalid source session"))?;
        let mut handle = ptr::null_mut();
        let code = unsafe { open(filename.as_ptr(), &mut handle, 1, ptr::null()) };
        let connection = Connection { handle, close };
        checked(code)?;
        checked(unsafe { timeout(connection.handle, 1000) })?;
        let mut statement = ptr::null_mut();
        let code = unsafe {
            prepare(
                connection.handle,
                c"SELECT rollout_path FROM threads WHERE id = ?1".as_ptr(),
                -1,
                &mut statement,
                ptr::null_mut(),
            )
        };
        let statement = Statement {
            handle: statement,
            finalize,
        };
        checked(code)?;
        checked(unsafe { bind(statement.handle, 1, parameter.as_ptr(), -1, None) })?;
        if unsafe { step(statement.handle) } != 100 {
            return Err(ContextError::state(
                "Codex active-rollout index has no readable unique thread row",
            ));
        }
        if unsafe { column_type(statement.handle, 0) } != 3 {
            return Err(ContextError::malformed(
                "Codex active rollout path is not text",
            ));
        }
        let bytes = unsafe { column_bytes(statement.handle, 0) };
        let pointer = unsafe { column(statement.handle, 0) };
        if !(1..=32768).contains(&bytes) || pointer.is_null() {
            return Err(ContextError::malformed(
                "Codex active rollout path is invalid",
            ));
        }
        let value =
            std::str::from_utf8(unsafe { std::slice::from_raw_parts(pointer, bytes as usize) })
                .map_err(|_| ContextError::malformed("Codex active rollout path is not UTF-8"))?;
        if value.contains('\0') {
            return Err(ContextError::malformed(
                "Codex active rollout path contains NUL",
            ));
        }
        let path = PathBuf::from(value);
        if unsafe { step(statement.handle) } != 101 {
            return Err(ContextError::state(
                "Codex active-rollout index is ambiguous or unreadable",
            ));
        }
        Ok(path)
    }
}
