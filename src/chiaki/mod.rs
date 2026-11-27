//! chiaki-ng's libchiaki behind Rust types: errors, the log, and one
//! submodule per part of the library. This is the only module tree that
//! touches the C library.

pub(crate) mod audio;
pub(crate) mod decoder;
pub(crate) mod discovery;
pub(crate) mod registration;
pub(crate) mod session;

#[allow(
    unsafe_code,
    dead_code,
    non_camel_case_types,
    non_snake_case,
    non_upper_case_globals,
    unused_imports,
    unreachable_pub,
    clippy::all,
    clippy::pedantic
)]
pub(crate) mod sys;

use std::ffi::{CStr, CString, c_char, c_void};
use std::io::{self, Write};
use std::ptr;
use std::sync::{Arc, Mutex};

pub(crate) type Result<T> = std::result::Result<T, Error>;

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub(crate) enum Error {
    #[error(transparent)]
    Code(#[from] Code),
    #[error("the {what} may not hold a NUL byte")]
    InteriorNul { what: &'static str },
    #[error("the {what} did not start: {}", log.trim())]
    Startup { what: &'static str, log: String },
    #[error("the video decoder failed: {0}")]
    Video(String),
}

/// An error code the C library returned.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Code(pub(crate) sys::ChiakiErrorCode);

impl std::fmt::Display for Code {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "libchiaki: {}", error_string(*self))
    }
}

impl std::error::Error for Code {}

/// Prepares the library's global state; call it once before anything else here.
pub(crate) fn init() -> Result<()> {
    // SAFETY: chiaki_lib_init takes no arguments and only sets up global state.
    check(unsafe { sys::chiaki_lib_init() })
}

fn check(code: sys::ChiakiErrorCode) -> Result<()> {
    if code == sys::CHIAKI_ERR_SUCCESS {
        Ok(())
    } else {
        Err(Code(code).into())
    }
}

fn error_string(code: Code) -> &'static str {
    // SAFETY: chiaki_error_string returns a constant string for every code.
    unsafe { CStr::from_ptr(sys::chiaki_error_string(code.0)) }
        .to_str()
        .unwrap_or("unknown error")
}

/// The library's own text for why a session ended.
pub(crate) fn quit_reason(reason: sys::ChiakiQuitReason) -> String {
    // SAFETY: chiaki_quit_reason_string returns a constant string for every reason.
    unsafe { text(sys::chiaki_quit_reason_string(reason)) }.unwrap_or_default()
}

pub(crate) struct Log {
    raw: sys::ChiakiLog,
    // We hold the buffer libchiaki writes the log into.
    _buffer: Option<Arc<Mutex<String>>>,
}

// SAFETY: the only address a Log holds is the heap allocation of the buffer a capturing log
// writes into, which moves with the Log, and chiaki reads the log under its own locks.
unsafe impl Send for Log {}
// SAFETY: as for Send; the buffer is behind a mutex.
unsafe impl Sync for Log {}

impl Log {
    /// A log that drops every message, for while katui owns the terminal.
    pub(crate) fn quiet() -> Self {
        Self::with(Some(discard), ptr::null_mut(), None)
    }

    /// A log that writes info, warnings and errors to stderr.
    pub(crate) fn stderr() -> Self {
        Self::with(Some(write_stderr), ptr::null_mut(), None)
    }

    /// A log that keeps every message in the buffer it hands out.
    pub(crate) fn capture() -> (Self, Arc<Mutex<String>>) {
        let kept = Arc::new(Mutex::new(String::new()));
        // the mutex sits at a fixed address for as long as an Arc to it lives
        let address = Arc::as_ptr(&kept).cast_mut().cast::<c_void>();
        let log = Self::with(Some(keep), address, Some(Arc::clone(&kept)));
        (log, kept)
    }

    fn with(
        callback: sys::ChiakiLogCb,
        user: *mut c_void,
        buffer: Option<Arc<Mutex<String>>>,
    ) -> Self {
        let levels = sys::CHIAKI_LOG_INFO | sys::CHIAKI_LOG_WARNING | sys::CHIAKI_LOG_ERROR;
        let mut log = sys::ChiakiLog {
            level_mask: 0,
            cb: None,
            user: ptr::null_mut(),
        };
        // SAFETY: `log` is a live ChiakiLog and the callback matches the signature chiaki_log_init takes.
        unsafe { sys::chiaki_log_init(ptr::from_mut(&mut log), levels, callback, user) };
        Self {
            raw: log,
            _buffer: buffer,
        }
    }

    fn raw(&self) -> *mut sys::ChiakiLog {
        ptr::from_ref(&self.raw).cast_mut()
    }
}

/// A NUL-terminated copy of `value` for the library.
fn c_string(value: &str, what: &'static str) -> Result<CString> {
    CString::new(value).map_err(|_| Error::InteriorNul { what })
}

/// # Safety
/// `pointer` is null or a live, null-terminated string.
unsafe fn text(pointer: *const c_char) -> Option<String> {
    if pointer.is_null() {
        return None;
    }
    // SAFETY: the caller holds the contract above.
    Some(unsafe { CStr::from_ptr(pointer).to_string_lossy().into_owned() })
}

/// # Safety
/// `message` is null or a live, null-terminated string.
unsafe fn log_line(level: sys::ChiakiLogLevel, message: *const c_char) -> String {
    // SAFETY: the caller holds the contract above, and the level only maps to a letter.
    unsafe {
        let letter = char::from(u8::from_ne_bytes(
            sys::chiaki_log_level_char(level).to_ne_bytes(),
        ));
        format!("[{letter}] {}\n", text(message).unwrap_or_default())
    }
}

unsafe extern "C" fn discard(
    _level: sys::ChiakiLogLevel,
    _message: *const c_char,
    _user: *mut c_void,
) {
}

unsafe extern "C" fn write_stderr(
    level: sys::ChiakiLogLevel,
    message: *const c_char,
    _user: *mut c_void,
) {
    // SAFETY: chiaki passes a message that lives for the call.
    let line = unsafe { log_line(level, message) };
    io::stderr().write_all(line.as_bytes()).ok();
}

unsafe extern "C" fn keep(_level: sys::ChiakiLogLevel, message: *const c_char, user: *mut c_void) {
    // SAFETY: `user` is the buffer a capturing log owns, and the message lives for the call.
    let kept = unsafe { &*user.cast::<Mutex<String>>() };
    if let Ok(mut buffer) = kept.lock() {
        // SAFETY: chiaki passes a message that lives for the call.
        buffer.push_str(&unsafe { text(message) }.unwrap_or_default());
        buffer.push('\n');
    }
}

#[cfg(test)]
mod tests {
    use super::sys;
    use super::{Code, Error, check, error_string, init};

    #[test]
    fn only_success_passes_the_check() {
        assert!(check(sys::CHIAKI_ERR_SUCCESS).is_ok());
        let timeout = Code(sys::CHIAKI_ERR_TIMEOUT);
        let error: Error = check(sys::CHIAKI_ERR_TIMEOUT).unwrap_err();
        assert_eq!(error.to_string(), "libchiaki: Timeout");
        assert_eq!(error_string(timeout), "Timeout");
    }

    #[test]
    fn the_library_initializes() {
        assert!(init().is_ok());
    }
}
