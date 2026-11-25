//! The console's sound: libchiaki's Opus decoder handing PCM to whoever plays it.

use std::ffi::c_void;
use std::mem::zeroed;
use std::ptr;
use std::sync::Arc;

use super::{Log, sys};

/// What the decoder makes of the console's audio.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Sound {
    /// The sound changed shape: this many channels at this many frames a second.
    Settings { channels: u32, rate: u32 },
    /// Interleaved samples of 16 bits, as many as `Settings` says.
    Frame(Vec<i16>),
}

/// What the decoder's callbacks reach through `cb_user`. Opus counts a
/// frame per channel, so we keep the channel count the settings gave us.
struct Listener {
    channels: usize,
    on_sound: Box<dyn FnMut(Sound) + Send>,
}

pub(crate) struct OpusDecoder {
    decoder: Box<sys::ChiakiOpusDecoder>,
    // We hold these so the pointers libchiaki keeps stay valid.
    _log: Arc<Log>,
    _listener: Box<Listener>,
}

impl OpusDecoder {
    /// Calls `on_sound` on the session's thread for as long as the decoder lives.
    pub(crate) fn new(log: Arc<Log>, on_sound: impl FnMut(Sound) + Send + 'static) -> Self {
        let mut listener = Box::new(Listener {
            channels: 0,
            on_sound: Box::new(on_sound),
        });
        // SAFETY: chiaki_opus_decoder_init expects an all-zero decoder and fills it in.
        let mut decoder = Box::new(unsafe { zeroed::<sys::ChiakiOpusDecoder>() });
        // SAFETY: `decoder` is zeroed as required and `log` is live.
        unsafe { sys::chiaki_opus_decoder_init(ptr::from_mut(&mut *decoder), log.raw()) };
        decoder.settings_cb = Some(settings);
        decoder.frame_cb = Some(frame);
        decoder.cb_user = ptr::from_mut(&mut *listener).cast();
        Self {
            decoder,
            _log: log,
            _listener: listener,
        }
    }

    /// Where the session hands the encoded audio to.
    pub(crate) fn sink(&self) -> sys::ChiakiAudioSink {
        let mut sink = sys::ChiakiAudioSink {
            user: ptr::null_mut(),
            header_cb: None,
            frame_cb: None,
        };
        // SAFETY: the decoder is initialized and `sink` is live and writable; the function only copies out.
        unsafe { sys::chiaki_opus_decoder_get_sink(self.raw(), ptr::from_mut(&mut sink)) };
        sink
    }

    fn raw(&self) -> *mut sys::ChiakiOpusDecoder {
        ptr::from_ref(&*self.decoder).cast_mut()
    }
}

impl Drop for OpusDecoder {
    fn drop(&mut self) {
        // SAFETY: the decoder was initialized in `new`.
        unsafe { sys::chiaki_opus_decoder_fini(self.raw()) };
    }
}

unsafe extern "C" fn settings(channels: u32, rate: u32, user: *mut c_void) {
    // SAFETY: `user` is the listener boxed in `new`, which lives until the decoder is dropped.
    let listener = unsafe { &mut *user.cast::<Listener>() };
    listener.channels = usize::try_from(channels).unwrap_or(0);
    (listener.on_sound)(Sound::Settings { channels, rate });
}

unsafe extern "C" fn frame(samples: *mut i16, count: usize, user: *mut c_void) {
    // SAFETY: `user` is the listener boxed in `new`, which lives until the decoder is dropped.
    let listener = unsafe { &mut *user.cast::<Listener>() };
    // SAFETY: chiaki passes `count` samples for each of the channels its settings named, live
    // for the length of the call.
    let copied = unsafe { std::slice::from_raw_parts(samples, count * listener.channels) }.to_vec();
    (listener.on_sound)(Sound::Frame(copied));
}
