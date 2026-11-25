//! A Remote Play session: what to ask the console for, the controller it
//! reads, and what it reports while it runs.

use std::ffi::{c_char, c_void};
use std::mem::zeroed;
use std::ptr;
use std::sync::Arc;

use super::audio::OpusDecoder;
use super::decoder::VideoDecoder;
use super::{Log, Result, c_string, check, quit_reason, sys, text};

/// The encoding the console sends the video in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Codec {
    H264,
    H265,
    H265Hdr,
}

impl Codec {
    pub(super) fn raw(self) -> sys::ChiakiCodec {
        match self {
            Self::H264 => sys::CHIAKI_CODEC_H264,
            Self::H265 => sys::CHIAKI_CODEC_H265,
            Self::H265Hdr => sys::CHIAKI_CODEC_H265_HDR,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Resolution {
    P360,
    P540,
    P720,
    P1080,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum FrameRate {
    Fps30,
    Fps60,
}

/// The video the console is asked for. `bitrate` 0 leaves the choice to the console.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct VideoProfile {
    pub width: u32,
    pub height: u32,
    pub max_fps: u32,
    pub bitrate: u32,
    pub codec: Codec,
}

impl VideoProfile {
    pub(crate) fn preset(resolution: Resolution, fps: FrameRate, codec: Codec) -> Self {
        // SAFETY: chiaki_connect_video_profile_preset only writes the profile it is given.
        let mut raw = unsafe { zeroed::<sys::ChiakiConnectVideoProfile>() };
        // SAFETY: both arguments are the enum values of the presets.
        unsafe {
            sys::chiaki_connect_video_profile_preset(
                ptr::from_mut(&mut raw),
                match resolution {
                    Resolution::P360 => sys::CHIAKI_VIDEO_RESOLUTION_PRESET_360p,
                    Resolution::P540 => sys::CHIAKI_VIDEO_RESOLUTION_PRESET_540p,
                    Resolution::P720 => sys::CHIAKI_VIDEO_RESOLUTION_PRESET_720p,
                    Resolution::P1080 => sys::CHIAKI_VIDEO_RESOLUTION_PRESET_1080p,
                },
                match fps {
                    FrameRate::Fps30 => sys::CHIAKI_VIDEO_FPS_PRESET_30,
                    FrameRate::Fps60 => sys::CHIAKI_VIDEO_FPS_PRESET_60,
                },
            );
        }
        raw.codec = codec.raw();
        Self {
            width: raw.width,
            height: raw.height,
            max_fps: raw.max_fps,
            bitrate: raw.bitrate,
            codec,
        }
    }

    fn raw(self) -> sys::ChiakiConnectVideoProfile {
        sys::ChiakiConnectVideoProfile {
            width: self.width,
            height: self.height,
            max_fps: self.max_fps,
            bitrate: self.bitrate,
            codec: self.codec.raw(),
        }
    }
}

/// Everything a session needs to be let in: the console's address, its
/// registration and the video to ask for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ConnectInfo {
    pub host: String,
    pub ps5: bool,
    pub regist_key: [u8; 16],
    pub morning: [u8; 16],
    pub psn_account_id: [u8; 8],
    pub video: VideoProfile,
}

/// The console's buttons, as bit flags.
pub(crate) mod button {
    use super::sys;

    pub(crate) const CROSS: u32 = sys::CHIAKI_CONTROLLER_BUTTON_CROSS;
    pub(crate) const CIRCLE: u32 = sys::CHIAKI_CONTROLLER_BUTTON_MOON;
    pub(crate) const SQUARE: u32 = sys::CHIAKI_CONTROLLER_BUTTON_BOX;
    pub(crate) const TRIANGLE: u32 = sys::CHIAKI_CONTROLLER_BUTTON_PYRAMID;
    pub(crate) const LEFT: u32 = sys::CHIAKI_CONTROLLER_BUTTON_DPAD_LEFT;
    pub(crate) const RIGHT: u32 = sys::CHIAKI_CONTROLLER_BUTTON_DPAD_RIGHT;
    pub(crate) const UP: u32 = sys::CHIAKI_CONTROLLER_BUTTON_DPAD_UP;
    pub(crate) const DOWN: u32 = sys::CHIAKI_CONTROLLER_BUTTON_DPAD_DOWN;
    pub(crate) const L1: u32 = sys::CHIAKI_CONTROLLER_BUTTON_L1;
    pub(crate) const R1: u32 = sys::CHIAKI_CONTROLLER_BUTTON_R1;
    pub(crate) const L3: u32 = sys::CHIAKI_CONTROLLER_BUTTON_L3;
    pub(crate) const R3: u32 = sys::CHIAKI_CONTROLLER_BUTTON_R3;
    pub(crate) const OPTIONS: u32 = sys::CHIAKI_CONTROLLER_BUTTON_OPTIONS;
    pub(crate) const SHARE: u32 = sys::CHIAKI_CONTROLLER_BUTTON_SHARE;
    pub(crate) const TOUCHPAD: u32 = sys::CHIAKI_CONTROLLER_BUTTON_TOUCHPAD;
    pub(crate) const PS: u32 = sys::CHIAKI_CONTROLLER_BUTTON_PS;
}

/// How far a stick or a trigger is pushed.
pub(crate) const FULL: i16 = 0x7fff;

/// One controller as the console sees it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) struct Pad {
    /// A mask of the flags in [`button`].
    pub buttons: u32,
    pub left_trigger: u8,
    pub right_trigger: u8,
    /// Each stick from `-FULL` to `FULL`, right and down positive.
    pub left: (i16, i16),
    pub right: (i16, i16),
}

impl Pad {
    fn raw(self) -> sys::ChiakiControllerState {
        // SAFETY: chiaki_controller_state_set_idle expects a writable state and fills it in.
        let mut state: sys::ChiakiControllerState = unsafe { zeroed() };
        // SAFETY: `state` is live and writable.
        unsafe { sys::chiaki_controller_state_set_idle(ptr::from_mut(&mut state)) };
        state.buttons = self.buttons;
        state.l2_state = self.left_trigger;
        state.r2_state = self.right_trigger;
        state.left_x = self.left.0;
        state.left_y = self.left.1;
        state.right_x = self.right.0;
        state.right_y = self.right.1;
        state
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Event {
    Connected,
    /// The console asks for the sign-in PIN of the account; `incorrect` when the last one sent was wrong.
    LoginPin {
        incorrect: bool,
    },
    Nickname(String),
    /// The session ended, and why.
    Quit(String),
}

/// The highest packet loss we report to the console, as a fraction. The console lowers the
/// bitrate from what we report, and libchiaki caps the report at this, so zero would pin the
/// stream at full bitrate on a lossy link.
const PACKET_LOSS_MAX: f64 = 0.05;

pub(crate) struct Session {
    inner: Box<sys::ChiakiSession>,
    // We hold these so the pointers libchiaki keeps stay valid.
    _log: Arc<Log>,
    _on_event: Box<Box<dyn FnMut(Event) + Send>>,
}

impl Session {
    /// Connects to the console in `info` and runs until the session is dropped.
    /// Video goes through `video`, sound through `audio`, and `on_event` runs
    /// on the library's thread.
    pub(crate) fn connect(
        info: &ConnectInfo,
        log: Arc<Log>,
        video: &VideoDecoder,
        audio: &OpusDecoder,
        on_event: impl FnMut(Event) + Send + 'static,
    ) -> Result<Self> {
        let host = c_string(&info.host, "console address")?;
        let mut on_event: Box<Box<dyn FnMut(Event) + Send>> = Box::new(Box::new(on_event));
        // SAFETY: an all-zero connect info means no hole punching and no auto registration.
        let rest = unsafe { zeroed::<sys::ChiakiConnectInfo>() };
        let mut connect = sys::ChiakiConnectInfo {
            ps5: info.ps5,
            host: host.as_ptr(),
            // c_char is signed on some platforms, so we let the cast wrap there.
            #[allow(clippy::cast_possible_wrap)]
            regist_key: info.regist_key.map(|byte| byte as c_char),
            morning: info.morning,
            video_profile: info.video.raw(),
            video_profile_auto_downgrade: true,
            enable_idr_on_fec_failure: true,
            packet_loss_max: PACKET_LOSS_MAX,
            psn_account_id: info.psn_account_id,
            ..rest
        };
        // SAFETY: chiaki_session_init expects an all-zero session and fills it in.
        let mut session = Box::new(unsafe { zeroed::<sys::ChiakiSession>() });
        // SAFETY: `session` is zeroed as required, `connect` is filled in above and `log` is live.
        check(unsafe {
            sys::chiaki_session_init(
                ptr::from_mut(&mut *session),
                ptr::from_mut(&mut connect),
                log.raw(),
            )
        })?;
        session.event_cb = Some(event);
        session.event_cb_user = ptr::from_mut(&mut *on_event).cast();
        session.video_sample_cb = Some(sys::chiaki_ffmpeg_decoder_video_sample_cb);
        session.video_sample_cb_user = video.raw().cast();
        session.audio_sink = audio.sink();
        // SAFETY: the session is initialized with its callbacks above.
        if let Err(error) =
            check(unsafe { sys::chiaki_session_start(ptr::from_mut(&mut *session)) })
        {
            // SAFETY: the session was initialized, so fini releases it.
            unsafe { sys::chiaki_session_fini(ptr::from_mut(&mut *session)) };
            return Err(error);
        }
        Ok(Self {
            inner: session,
            _log: log,
            _on_event: on_event,
        })
    }

    pub(crate) fn set_controller(&mut self, pad: Pad) {
        let mut state = pad.raw();
        // SAFETY: the session is started, and the function locks the session itself.
        unsafe {
            sys::chiaki_session_set_controller_state(
                ptr::from_mut(&mut *self.inner),
                ptr::from_mut(&mut state),
            )
        };
    }

    pub(crate) fn set_login_pin(&mut self, pin: &[u8]) {
        // SAFETY: the session is started, and the function copies `pin` before it returns.
        unsafe {
            sys::chiaki_session_set_login_pin(
                ptr::from_mut(&mut *self.inner),
                pin.as_ptr(),
                pin.len(),
            )
        };
    }

    /// Asks the console for a keyframe, the place a decoder can start from.
    pub(crate) fn request_keyframe(&mut self) {
        // SAFETY: the session is started, and the function only queues a request.
        unsafe { sys::chiaki_session_request_idr(ptr::from_mut(&mut *self.inner)) };
    }

    /// Asks the console to go to rest.
    pub(crate) fn rest(&mut self) {
        // SAFETY: the session is started, and the function only queues a request.
        unsafe { sys::chiaki_session_goto_bed(ptr::from_mut(&mut *self.inner)) };
    }
}

impl Drop for Session {
    fn drop(&mut self) {
        // SAFETY: the session was started in `connect`; stop ends its thread, join waits for it, fini releases it.
        unsafe {
            sys::chiaki_session_stop(ptr::from_mut(&mut *self.inner));
            sys::chiaki_session_join(ptr::from_mut(&mut *self.inner));
            sys::chiaki_session_fini(ptr::from_mut(&mut *self.inner));
        }
    }
}

unsafe extern "C" fn event(event: *mut sys::ChiakiEvent, user: *mut c_void) {
    // SAFETY: chiaki passes a live event for the call, and `user` is the closure boxed in `connect`.
    unsafe {
        let on_event = &mut *user.cast::<Box<dyn FnMut(Event) + Send>>();
        let event = &*event;
        let data = &event.__bindgen_anon_1;
        let forwarded = match event.type_ {
            sys::CHIAKI_EVENT_CONNECTED => Some(Event::Connected),
            sys::CHIAKI_EVENT_LOGIN_PIN_REQUEST => Some(Event::LoginPin {
                incorrect: data.login_pin_request.pin_incorrect,
            }),
            sys::CHIAKI_EVENT_QUIT => Some(Event::Quit(
                text(data.quit.reason_str).unwrap_or_else(|| quit_reason(data.quit.reason)),
            )),
            sys::CHIAKI_EVENT_NICKNAME_RECEIVED => {
                text(data.server_nickname.as_ptr()).map(Event::Nickname)
            }
            _ => None,
        };
        if let Some(forwarded) = forwarded {
            (*on_event)(forwarded);
        }
    }
}
