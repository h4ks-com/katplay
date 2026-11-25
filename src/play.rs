//! The session: waking the console, connecting to it, showing its video and
//! sending it the controller.

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use katui::proto::input::{Key, Mods};
use katui::rt::events::Waker;
use katui::rt::gamepad::Pad as Gamepad;
use katui::scene::surface::Surface;

use crate::Error;
use crate::audio::Output;
use crate::chiaki::Log;
use crate::chiaki::audio::{OpusDecoder, Sound};
use crate::chiaki::decoder::VideoDecoder;
use crate::chiaki::session::{ConnectInfo, Event, Pad, Session, VideoProfile};
use crate::console::{Console, Kind, Registration, State};
use crate::pad::{from_gamepads, from_keys};
use crate::settings::{Quality, Settings};
use crate::shell::{Message, Told};

/// How long a console gets to wake before the session gives up on it.
const WAKE_TIMEOUT: Duration = Duration::from_secs(90);
/// How often the controller may be sent, so a burst of keys still ends with
/// the newest state on its way.
const SEND_EVERY: Duration = Duration::from_millis(4);
/// How long a key counts as held when the terminal does not report releases;
/// its repeats keep a held key alive.
const HELD_WITHOUT_RELEASE: Duration = Duration::from_millis(250);
/// How long the console gets to rest before the session is dropped anyway.
const REST_WAIT: Duration = Duration::from_secs(3);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Stage {
    Waking,
    Connecting,
    Live,
}

/// What the shell should do after the session reported something.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Report {
    Nothing,
    /// The console asks for its sign-in PIN; `incorrect` when the last one sent was wrong.
    Pin {
        incorrect: bool,
    },
    /// The session ended, and why.
    Ended(String),
}

struct Held {
    key: Key,
    mods: Mods,
    since: Instant,
}

/// The waker behind "a picture is waiting", shared by two closures.
#[derive(Clone)]
struct Signal(Waker<Message>);

impl Signal {
    fn send(&self) {
        self.0.send(Message::told(Told::Picture));
    }
}

pub(crate) struct Play {
    // We drop the session first so its threads stop before the decoders they call.
    session: Option<Session>,
    _audio: Output,
    opus: OpusDecoder,
    decoder: VideoDecoder,
    log: Arc<Log>,
    registration: Registration,
    /// The video the console was asked for.
    video: VideoProfile,
    /// What the console calls itself, once it said.
    pub(crate) name: String,
    stage: Stage,
    woke_at: Option<Instant>,
    rest_at: Option<Instant>,
    picture: Option<Surface>,
    picture_size: (u32, u32),
    lost: u32,
    shown: u32,
    counted: u32,
    counted_since: Instant,
    held: Vec<Held>,
    gamepads: Vec<Gamepad>,
    sent: Pad,
    sent_at: Option<Instant>,
    releases_seen: bool,
    faults: Arc<Mutex<Vec<String>>>,
}

impl Play {
    /// Start a session with the registered console. A console in rest is
    /// woken first and connected once it answers.
    pub(crate) fn start(
        registration: &Registration,
        settings: Settings,
        console: Option<&Console>,
        log: Arc<Log>,
        waker: &Waker<Message>,
    ) -> Result<Self, Error> {
        let video = settings.video();
        let decoder = decoder(Arc::clone(&log), settings, video, Signal(waker.clone()))?;
        let faults = Arc::new(Mutex::new(Vec::new()));
        let audio = Output::open(Arc::clone(&faults));
        let queue = audio.queue();
        let opus = OpusDecoder::new(Arc::clone(&log), move |sound| match sound {
            Sound::Settings { channels, rate } => queue.set_shape(channels, rate),
            Sound::Frame(samples) => queue.push(&samples),
        });
        let mut play = Self {
            session: None,
            _audio: audio,
            opus,
            decoder,
            log,
            name: registration.name.clone(),
            registration: registration.clone(),
            video,
            stage: Stage::Connecting,
            woke_at: None,
            rest_at: None,
            picture: None,
            picture_size: (0, 0),
            lost: 0,
            shown: 0,
            counted: 0,
            counted_since: Instant::now(),
            held: Vec::new(),
            gamepads: Vec::new(),
            sent: Pad::default(),
            sent_at: None,
            releases_seen: false,
            faults,
        };
        if console.is_some_and(|console| console.state == State::Standby) {
            registration.wake(&play.log)?;
            play.stage = Stage::Waking;
            play.woke_at = Some(Instant::now());
            // Discovery reports only changes, so a console that stays asleep would never
            // let the timeout run out without a clock of its own.
            let deadline = waker.clone();
            std::thread::spawn(move || {
                std::thread::sleep(WAKE_TIMEOUT);
                deadline.send(Message::told(Told::WakeDeadline));
            });
        } else {
            play.connect(video, waker)?;
        }
        Ok(play)
    }

    fn connect(&mut self, video: VideoProfile, waker: &Waker<Message>) -> Result<(), Error> {
        let registration = &self.registration;
        let info = ConnectInfo {
            host: registration.host.clone(),
            ps5: registration.kind == Kind::PS5,
            regist_key: registration.regist_key,
            morning: registration.morning,
            psn_account_id: registration.psn_account_id,
            video,
        };
        let waker = waker.clone();
        let session = Session::connect(
            &info,
            Arc::clone(&self.log),
            &self.decoder,
            &self.opus,
            move |event| {
                waker.send(Message::told(Told::Session(event)));
            },
        )?;
        self.session = Some(session);
        self.stage = Stage::Connecting;
        Ok(())
    }

    pub(crate) fn stage(&self) -> Stage {
        self.stage
    }

    /// Discovery reported the network; whether a console we woke is now
    /// connected. Err once it had its time and never answered.
    pub(crate) fn found(
        &mut self,
        consoles: &[Console],
        waker: &Waker<Message>,
    ) -> Result<bool, Error> {
        if self.stage != Stage::Waking {
            return Ok(false);
        }
        let awake = consoles.iter().any(|console| {
            console.address == self.registration.host && console.state == State::Ready
        });
        if awake {
            self.connect(self.video, waker)?;
            return Ok(true);
        }
        self.check_wake()?;
        Ok(false)
    }

    /// Err once a console we woke had its time and never answered.
    pub(crate) fn check_wake(&self) -> Result<(), Error> {
        if self.stage == Stage::Waking
            && self.woke_at.is_some_and(|at| at.elapsed() >= WAKE_TIMEOUT)
        {
            return Err(Error::Console(format!("{} did not wake up", self.name)));
        }
        Ok(())
    }

    /// Show the newest decoded picture at `size`; whether one was waiting.
    pub(crate) fn show(&mut self, size: (u32, u32)) -> Result<bool, Error> {
        let Some(picture) = self.decoder.pull(size)? else {
            return Ok(false);
        };
        let surface = Surface::from_rgba(size.0, size.1, picture.rgba)
            .ok_or_else(|| Error::Picture("has a size this terminal cannot take".to_owned()))?;
        self.picture = Some(surface);
        self.picture_size = size;
        self.lost += u32::try_from(picture.lost).unwrap_or(0);
        self.counted += 1;
        if self.counted_since.elapsed() >= Duration::from_secs(1) {
            self.shown = self.counted;
            self.counted = 0;
            self.counted_since = Instant::now();
        }
        Ok(true)
    }

    pub(crate) fn picture(&self) -> Option<&Surface> {
        self.picture.as_ref()
    }

    /// Whether the picture shown has the size the terminal takes now.
    pub(crate) fn fits(&self, size: (u32, u32)) -> bool {
        self.picture_size == size
    }

    /// A key went down (`down`) or came up for the controller.
    pub(crate) fn key(&mut self, key: Key, mods: Mods, down: bool, now: Instant) {
        // A terminal reports the modifiers held at release time, so Shift let go before an arrow
        // would leave the arrow held if we matched on both.
        self.held.retain(|held| held.key != key);
        if down {
            self.held.push(Held {
                key,
                mods,
                since: now,
            });
        } else {
            self.releases_seen = true;
        }
    }

    /// Let every key go, for when the terminal lost the focus.
    pub(crate) fn release_keys(&mut self) {
        self.held.clear();
    }

    pub(crate) fn gamepads(&mut self, gamepads: Vec<Gamepad>) {
        self.gamepads = gamepads;
    }

    /// Send the controller the keys and the game controllers make, when it
    /// changed and the last send was a moment ago.
    pub(crate) fn send(&mut self, now: Instant) {
        let Some(session) = self.session.as_mut() else {
            return;
        };
        if !self.releases_seen {
            self.held
                .retain(|held| now.duration_since(held.since) < HELD_WITHOUT_RELEASE);
        }
        let pad = from_keys(self.held.iter().map(|held| (held.key, held.mods)))
            .merge(from_gamepads(&self.gamepads));
        if pad == self.sent || self.sent_at.is_some_and(|at| at.elapsed() < SEND_EVERY) {
            return;
        }
        session.set_controller(pad);
        self.sent = pad;
        self.sent_at = Some(now);
    }

    /// The session reported something; what to do about it.
    pub(crate) fn reported(&mut self, event: Event) -> Report {
        match event {
            Event::Connected => {
                self.stage = Stage::Live;
                // We ask for a keyframe so the first picture decodes whole.
                if let Some(session) = self.session.as_mut() {
                    session.request_keyframe();
                }
                Report::Nothing
            }
            Event::Nickname(name) => {
                self.name = name;
                Report::Nothing
            }
            Event::LoginPin { incorrect } => Report::Pin { incorrect },
            Event::Quit(reason) => Report::Ended(reason),
        }
    }

    /// Answer the console's request for the sign-in PIN.
    pub(crate) fn send_pin(&mut self, pin: &[u8]) {
        if let Some(session) = self.session.as_mut() {
            session.set_login_pin(pin);
        }
    }

    /// Ask the console to rest; the session is dropped once it did, or after its time.
    pub(crate) fn rest(&mut self, now: Instant) {
        if let Some(session) = self.session.as_mut() {
            session.rest();
        }
        self.rest_at = Some(now + REST_WAIT);
    }

    /// Whether the console had its time to rest.
    pub(crate) fn rested(&self, now: Instant) -> bool {
        self.rest_at.is_some_and(|at| now >= at)
    }

    /// The line above the picture.
    pub(crate) fn status(&self, quality: Quality) -> String {
        let stage = match self.stage {
            Stage::Waking => "waking up".to_owned(),
            Stage::Connecting => "connecting".to_owned(),
            Stage::Live if self.shown == 0 => "waiting for the first picture".to_owned(),
            Stage::Live => format!("{} fps, {} lost", self.shown, self.lost),
        };
        let fault = self
            .faults
            .lock()
            .ok()
            .and_then(|faults| faults.first().cloned())
            .map_or_else(
                || "Press Esc to leave".to_owned(),
                |fault| format!("no sound: {fault}"),
            );
        format!("{}  {}  {}  {}", self.name, quality, stage, fault)
    }
}

/// The decoder for the console's video, on the hardware decoder when the
/// settings name one and it starts.
fn decoder(
    log: Arc<Log>,
    settings: Settings,
    video: VideoProfile,
    signal: Signal,
) -> Result<VideoDecoder, Error> {
    let hardware = settings.hardware_decoder();
    let (told, retry) = (signal.clone(), signal);
    let started = VideoDecoder::new(
        Arc::clone(&log),
        video.codec,
        video.max_fps,
        hardware,
        move || told.send(),
    );
    match (started, hardware) {
        (Err(_), Some(_)) => {
            VideoDecoder::new(log, video.codec, video.max_fps, None, move || retry.send())
        }
        (started, _) => started,
    }
    .map_err(Error::from)
}
