//! Finding the consoles on the network, and waking a sleeping one.

use std::ffi::c_void;
use std::mem::{size_of, zeroed};
use std::ptr;
use std::sync::Arc;

use super::{Log, Result, c_string, check, sys, text};
use crate::console::{Console, Kind, State};

const HOSTS_MAX: usize = 16;
const HOST_DROP_PINGS: u64 = 3;
const PING_MS: u64 = 500;

/// Watches the network for consoles and hands every change to the callback on
/// the library's own thread, until the discovery is dropped.
pub(crate) struct Discovery {
    service: Box<sys::ChiakiDiscoveryService>,
    // We hold these so the pointers libchiaki keeps stay valid.
    _log: Arc<Log>,
    _callback: Box<Callback>,
}

/// A concrete type, so the library can hold a plain pointer to the closure.
struct Callback(Box<dyn FnMut(Vec<Console>) + Send>);

// SAFETY: only the owning thread calls into the service, chiaki reaches it from its own thread
// under the service's lock, and the closure it calls is Send.
unsafe impl Send for Discovery {}

impl Discovery {
    pub(crate) fn start(
        log: Arc<Log>,
        on_change: impl FnMut(Vec<Console>) + Send + 'static,
    ) -> Result<Self> {
        let mut callback = Box::new(Callback(Box::new(on_change)));
        let mut send_addr = broadcast_address();
        // SAFETY: chiaki_discovery_service_init expects an all-zero service and fills it in.
        let mut service = Box::new(unsafe { zeroed::<sys::ChiakiDiscoveryService>() });
        // SAFETY: an all-zero options struct is what chiaki's own discovery manager starts from.
        let empty = unsafe { zeroed::<sys::ChiakiDiscoveryServiceOptions>() };
        let options = sys::ChiakiDiscoveryServiceOptions {
            hosts_max: HOSTS_MAX,
            host_drop_pings: HOST_DROP_PINGS,
            ping_ms: PING_MS,
            send_addr: ptr::from_mut(&mut send_addr),
            send_addr_size: size_of::<libc::sockaddr_in>(),
            cb: Some(changed),
            // We box the callback so its pointer stays valid however the structs move.
            cb_user: ptr::from_mut(&mut *callback).cast(),
            ..empty
        };
        // SAFETY: `service` is zeroed as required, `options` is filled in above, and chiaki only copies it.
        check(unsafe {
            sys::chiaki_discovery_service_init(
                ptr::from_mut(&mut *service),
                ptr::from_ref(&options).cast_mut(),
                log.raw(),
            )
        })?;
        Ok(Self {
            service,
            _log: log,
            _callback: callback,
        })
    }
}

impl Drop for Discovery {
    fn drop(&mut self) {
        // SAFETY: `start` initialized the service, and nothing touches it after this.
        unsafe { sys::chiaki_discovery_service_fini(ptr::from_mut(&mut *self.service)) };
    }
}

/// Asks the console at `host` to wake from rest. `credential` is the
/// registration key read as a hex number.
pub(crate) fn wake(log: &Log, host: &str, credential: u64, ps5: bool) -> Result<()> {
    let host = c_string(host, "console address")?;
    // SAFETY: `log` and `host` are live, and a NULL discovery makes the function use a temporary one.
    check(unsafe {
        sys::chiaki_discovery_wakeup(log.raw(), ptr::null_mut(), host.as_ptr(), credential, ps5)
    })
}

/// Where the discovery pings go out from: the global IPv4 broadcast address,
/// the way chiaki's own manager starts. The service fills in the port.
fn broadcast_address() -> sys::sockaddr_storage {
    // SAFETY: sockaddr_in is a plain struct of integers, so all zeros is a valid unset address.
    let unset: libc::sockaddr_in = unsafe { zeroed() };
    let address = libc::sockaddr_in {
        sin_family: libc::AF_INET as libc::sa_family_t,
        sin_addr: libc::in_addr {
            s_addr: libc::INADDR_BROADCAST,
        },
        ..unset
    };
    // SAFETY: sockaddr_storage is a plain struct of integers and at least as large as sockaddr_in.
    let mut storage: sys::sockaddr_storage = unsafe { zeroed() };
    // SAFETY: the two structs do not overlap and the copy reads exactly one sockaddr_in.
    unsafe {
        ptr::copy_nonoverlapping(
            ptr::from_ref(&address).cast::<u8>(),
            ptr::from_mut(&mut storage).cast::<u8>(),
            size_of::<libc::sockaddr_in>(),
        );
    }
    storage
}

/// # Safety
/// `host` is a live host chiaki handed to the discovery callback.
unsafe fn console(host: &sys::ChiakiDiscoveryHost) -> Option<Console> {
    // SAFETY: the caller holds the contract above, and every call only reads the host.
    unsafe {
        Some(Console {
            address: text(host.host_addr)?,
            name: text(host.host_name).unwrap_or_default(),
            kind: Kind::of(sys::chiaki_discovery_host_is_ps5(
                ptr::from_ref(host).cast_mut(),
            )),
            state: state(host.state),
            running_app: text(host.running_app_name),
        })
    }
}

fn state(raw: sys::ChiakiDiscoveryHostState) -> State {
    match raw {
        sys::CHIAKI_DISCOVERY_HOST_STATE_READY => State::Ready,
        sys::CHIAKI_DISCOVERY_HOST_STATE_STANDBY => State::Standby,
        _ => State::Unknown,
    }
}

unsafe extern "C" fn changed(
    hosts: *mut sys::ChiakiDiscoveryHost,
    count: usize,
    user: *mut c_void,
) {
    // SAFETY: `user` is the `Callback` boxed in `start`, which lives until the service is dropped.
    let callback = unsafe { &mut *user.cast::<Callback>() };
    let consoles = (0..count)
        .filter_map(|index| {
            // SAFETY: chiaki passes `count` live hosts.
            unsafe { console(&*hosts.add(index)) }
        })
        .collect();
    (callback.0)(consoles);
}

#[cfg(test)]
mod tests {
    use std::ptr;

    use super::{broadcast_address, state, sys};
    use crate::console::State;

    #[test]
    fn every_discovery_state_has_its_own_variant() {
        assert_eq!(
            state(sys::CHIAKI_DISCOVERY_HOST_STATE_UNKNOWN),
            State::Unknown
        );
        assert_eq!(state(sys::CHIAKI_DISCOVERY_HOST_STATE_READY), State::Ready);
        assert_eq!(
            state(sys::CHIAKI_DISCOVERY_HOST_STATE_STANDBY),
            State::Standby
        );
    }

    #[test]
    fn discovery_pings_the_broadcast_address() {
        let storage = broadcast_address();
        // SAFETY: `broadcast_address` wrote one sockaddr_in into the storage.
        let address = unsafe { &*ptr::from_ref(&storage).cast::<libc::sockaddr_in>() };
        assert_eq!(i32::from(address.sin_family), libc::AF_INET);
        assert_eq!(address.sin_addr.s_addr, libc::INADDR_BROADCAST);
        assert_eq!(address.sin_port, 0, "the service picks the port");
    }
}
