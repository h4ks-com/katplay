//! Pairing a console: the registration that stores a registration key on it,
//! and its result.

use std::ffi::c_void;
use std::mem::zeroed;
use std::ptr;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use super::{Log, Result, c_string, check, sys};

/// What a registration needs: the console's address, the PSN account to
/// register and the link code the console shows.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Request {
    pub host: String,
    pub ps5: bool,
    pub psn_account_id: [u8; 8],
    pub pin: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Outcome {
    /// The console stored the account and handed out what lets a session in.
    Registered {
        regist_key: [u8; 16],
        morning: [u8; 16],
    },
    Failed,
    Canceled,
}

pub(crate) struct Registration {
    regist: Box<sys::ChiakiRegist>,
    // We hold these so the pointers libchiaki keeps stay valid.
    _host: std::ffi::CString,
    _log: Arc<Log>,
    done: Arc<AtomicBool>,
    _callback: Box<Callback>,
}

/// A concrete type, so the library can hold a plain pointer to the closure.
struct Callback(Box<dyn FnMut(Outcome) + Send>, Arc<AtomicBool>);

impl Registration {
    /// Registers the account on the console and calls `on_outcome` once, from
    /// the library's thread.
    pub(crate) fn start(
        request: &Request,
        log: Arc<Log>,
        on_outcome: impl FnMut(Outcome) + Send + 'static,
    ) -> Result<Self> {
        let host = c_string(&request.host, "console address")?;
        let done = Arc::new(AtomicBool::new(false));
        let mut callback = Box::new(Callback(Box::new(on_outcome), Arc::clone(&done)));
        // SAFETY: an all-zero request means no broadcast and no remote registration.
        let rest = unsafe { zeroed::<sys::ChiakiRegistInfo>() };
        let info = sys::ChiakiRegistInfo {
            target: if request.ps5 {
                sys::CHIAKI_TARGET_PS5_1
            } else {
                sys::CHIAKI_TARGET_PS4_10
            },
            host: host.as_ptr(),
            psn_account_id: request.psn_account_id,
            pin: request.pin,
            ..rest
        };
        // SAFETY: chiaki_regist_start expects an all-zero registration and fills it in.
        let mut regist = Box::new(unsafe { zeroed::<sys::ChiakiRegist>() });
        // SAFETY: `regist` is zeroed as required, `info` and `log` are live, and the callback matches.
        check(unsafe {
            sys::chiaki_regist_start(
                ptr::from_mut(&mut *regist),
                log.raw(),
                ptr::from_ref(&info),
                Some(finished),
                ptr::from_mut(&mut *callback).cast(),
            )
        })?;
        Ok(Self {
            regist,
            _host: host,
            _log: log,
            done,
            _callback: callback,
        })
    }
}

impl Drop for Registration {
    fn drop(&mut self) {
        // SAFETY: the registration was started in `start`; stop ends a registration that is still
        // running, and fini waits for its thread and releases it.
        unsafe {
            if !self.done.load(Ordering::Relaxed) {
                sys::chiaki_regist_stop(ptr::from_mut(&mut *self.regist));
            }
            sys::chiaki_regist_fini(ptr::from_mut(&mut *self.regist));
        }
    }
}

unsafe extern "C" fn finished(event: *mut sys::ChiakiRegistEvent, user: *mut c_void) {
    // SAFETY: chiaki passes a live event for the call, and `user` is the `Callback` boxed in `start`.
    unsafe {
        let callback = &mut *user.cast::<Callback>();
        let event = &*event;
        let outcome = match event.type_ {
            sys::CHIAKI_REGIST_EVENT_TYPE_FINISHED_SUCCESS if !event.registered_host.is_null() => {
                // SAFETY: with the success event, chiaki passes the host it registered, live for the call.
                let host = &*event.registered_host;
                Outcome::Registered {
                    regist_key: host
                        .rp_regist_key
                        .map(|byte| u8::from_ne_bytes(byte.to_ne_bytes())),
                    morning: host.rp_key,
                }
            }
            sys::CHIAKI_REGIST_EVENT_TYPE_FINISHED_FAILED => Outcome::Failed,
            _ => Outcome::Canceled,
        };
        callback.1.store(true, Ordering::Relaxed);
        (callback.0)(outcome);
    }
}
