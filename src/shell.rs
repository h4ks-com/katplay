//! The katui app: one event loop for every screen. The menus are widgets,
//! drawn as one picture; the session's video goes through the pixel
//! presenter below the status row and shrinks when the terminal falls behind.

use std::io;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use katui::draw::{TextEngine, bundled_fonts};
use katui::present::session;
use katui::present::surface::{PatchedKitty, PixelScreen};
use katui::proto::input::{Button, InputEvent, Key, Mods, MouseAction, Pointer};
use katui::proto::probe::{Capabilities, CellPixels};
use katui::rt::app::{App, AppEvent, Control, Delivery};
use katui::rt::events::Waker;
use katui::rt::gamepad::{self, Pad as Gamepad};
use katui::rt::stash::local_stash;
use katui::rt::tty::cell_size;
use katui::scene::CellSize;
use katui::scene::placement::CellRect;
use katui::ui::{Align, Element, Ui, UiOptions};
use katui::widgets::{
    Focus, TextField, Theme, button, hint, label, page, panel, primary_button, text_field, title,
};

use crate::chiaki::Log;
use crate::chiaki::discovery::Discovery;
use crate::chiaki::registration::{Outcome, Registration as Pairing};
use crate::chiaki::session::Event;
use crate::console::{Console, Kind, Registration, Store};
use crate::governor::Governor;
use crate::library::{self, Library, Row};
use crate::pad;
use crate::play::{Play, Report, Stage};
use crate::psn;
use crate::register::{Account, By, Field, Register};
use crate::settings::Settings;
use crate::status::{ROWS, Status};
use crate::{Error, io_error};

/// The first image id the pixel presenter hands out.
const BASE_IMAGE: u32 = 1_000;
/// The picture a terminal is asked to take per frame at most, the size the
/// katui video player settles on.
const MOST_PIXELS: (u32, u32) = (960, 540);
/// The render scale the governor picks from; it halves under a multiplexer,
/// which carries every byte inline.
const SCALE_RANGE: (f64, f64) = (0.25, 1.0);
pub(crate) const FRAMES_PER_SECOND: u32 = 60;

/// What a widget or a key asks for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Action {
    Pick,
    Register,
    Forget,
    CancelForget,
    Wake,
    NextQuality,
    Back,
    Focus(Field),
    Source(By),
    OpenSignIn,
    LookUp,
    Link,
    Leave,
    Rest,
    Keep,
    SubmitPin,
}

/// What a thread of the app tells the loop.
#[derive(Debug, Clone)]
pub(crate) enum Told {
    Consoles(Vec<Console>),
    Picture,
    Session(Event),
    Gamepads,
    Registered(Outcome),
    Account(Result<[u8; 8], Arc<Error>>),
    Notice(String),
    /// A console we woke had its time to answer.
    WakeDeadline,
}

#[derive(Debug, Clone)]
pub(crate) enum Message {
    Ask(Action),
    Told(Told),
}

impl Message {
    pub(crate) fn ask(action: Action) -> Self {
        Self::Ask(action)
    }

    pub(crate) fn told(told: Told) -> Self {
        Self::Told(told)
    }
}

enum Screen {
    Library(Library),
    Register(Box<Register>),
    Play,
    /// The console asks for the PIN of the account's sign-in.
    Pin {
        typed: TextField,
        complaint: Option<String>,
    },
    /// The menu Esc opens while a session runs, with the choice the keys are on and the
    /// screen "Keep playing" goes back to.
    Leave {
        choice: Focus<Action>,
        back: Box<Screen>,
    },
    /// The session ended; the text says why.
    Ended(String),
}

pub(crate) struct Shell {
    caps: Capabilities,
    cell: CellPixels,
    ui: Ui<Message>,
    presenter: PixelScreen,
    pointer: Pointer,
    status: Status,
    screen: Screen,
    /// The session; there while the screen is `Play`, `Pin` or `Leave`.
    play: Option<Box<Play>>,
    store: Store,
    registrations: Vec<Registration>,
    settings: Settings,
    log: Arc<Log>,
    discovered: Vec<Console>,
    /// The newest state of the game controllers, which their thread replaces.
    gamepads: Arc<Mutex<Option<Vec<Gamepad>>>>,
    /// The controllers as the last snapshot left them, so a menu sees each press once.
    held_pads: Vec<Gamepad>,
    waker: Option<Waker<Message>>,
    governor: Governor,
    /// The size the session's pictures are scaled to, as the terminal last sized them.
    frame: (u32, u32),
    size: CellSize,
    /// Whether the terminal was asked to report key releases for this session.
    releases_asked: bool,
    notice: Option<String>,
    offline: bool,
    /// The console to play as soon as the app starts.
    eager: Option<String>,
}

impl Shell {
    /// The app over `store` and `settings`, listing `registrations`.
    pub(crate) fn new(
        caps: Capabilities,
        store: Store,
        settings: Settings,
        registrations: Vec<Registration>,
        offline: bool,
    ) -> Result<Self, Error> {
        let cell = cell_size(&caps);
        let engine = TextEngine::new(bundled_fonts()).map_err(|source| {
            io_error(&store.root().join("katui fonts"), io::Error::other(source))
        })?;
        let presenter = if caps.kitty {
            let mut presenter = PatchedKitty::with_tiles(BASE_IMAGE, cell, 64, 0);
            if let Some(stash) = local_stash(&caps) {
                presenter = presenter.with_stash(stash);
            }
            PixelScreen::new(&caps, cell, presenter)
        } else {
            PixelScreen::picture(&caps, cell)
        };
        let halve = |scale: f64| {
            if caps.multiplexed() {
                scale / 2.0
            } else {
                scale
            }
        };
        let (min, max) = (halve(SCALE_RANGE.0), halve(SCALE_RANGE.1));
        let mut library = Library::default();
        library.resize(library::rows(&registrations, &[]).len());
        let pointer = Pointer::new(caps.cell, cell);
        Ok(Self {
            caps,
            cell,
            ui: Ui::new(engine, UiOptions::default()),
            presenter,
            pointer,
            status: Status::default(),
            screen: Screen::Library(library),
            play: None,
            store,
            registrations,
            settings,
            log: Arc::new(Log::quiet()),
            discovered: Vec::new(),
            gamepads: Arc::default(),
            held_pads: Vec::new(),
            waker: None,
            governor: Governor::new(Duration::from_secs(1) / FRAMES_PER_SECOND, min, max),
            frame: (0, 0),
            size: CellSize {
                columns: 0,
                rows: 0,
            },
            releases_asked: false,
            notice: None,
            offline,
            eager: None,
        })
    }

    /// Show a line on the first screen, for what went wrong.
    pub(crate) fn notice(&mut self, text: impl Into<String>) {
        self.notice = Some(text.into());
    }

    /// Play the console registered under `name` as soon as the app starts.
    pub(crate) fn play(&mut self, name: &str) {
        self.eager = Some(name.to_owned());
    }

    fn registration(&self, name: &str) -> Option<Registration> {
        self.registrations
            .iter()
            .find(|registration| registration.answers_to(name))
            .cloned()
    }

    fn start_play(&mut self, registration: &Registration) -> Result<(), Error> {
        let waker = self.waker.clone().ok_or_else(|| {
            Error::Console("the console cannot be played before the app starts".to_owned())
        })?;
        let console = self
            .discovered
            .iter()
            .find(|console| console.address == registration.host)
            .cloned();
        let play = Play::start(
            registration,
            self.settings,
            console.as_ref(),
            Arc::clone(&self.log),
            &waker,
        )?;
        self.play = Some(Box::new(play));
        self.screen = Screen::Play;
        self.releases_asked = false;
        self.status.invalidate();
        self.frame = self.frame_size();
        Ok(())
    }

    /// End the session, and show why on its own screen when there is a reason.
    fn end_play(&mut self, why: Option<String>) {
        self.play = None;
        self.screen = match why {
            Some(why) => Screen::Ended(why),
            None => Screen::Library(Library::default()),
        };
        self.resize_library();
        self.status.invalidate();
    }

    fn resize_library(&mut self) {
        let rows = library::rows(&self.registrations, &self.discovered).len();
        if let Screen::Library(library) = &mut self.screen {
            library.resize(rows);
        }
    }

    fn reload(&mut self) {
        let (registrations, problems) = self.store.load();
        self.registrations = registrations;
        self.notice = problems.first().map(std::string::ToString::to_string);
        self.resize_library();
    }

    /// The size the session's pictures are scaled to.
    fn frame_size(&self) -> (u32, u32) {
        let area = video_area(self.size, self.cell);
        frame_size(
            (
                u32::from(area.columns) * u32::from(self.cell.width),
                u32::from(area.rows) * u32::from(self.cell.height),
            ),
            self.caps.multiplexed(),
            self.governor.scale(),
        )
    }

    /// The row the list has selected.
    fn selected(&self) -> Option<Row> {
        match &self.screen {
            Screen::Library(library) => library::rows(&self.registrations, &self.discovered)
                .get(library.selected())
                .cloned(),
            _ => None,
        }
    }

    fn act(&mut self, action: Action) -> Control {
        match action {
            Action::Pick => self.pick(),
            Action::Register => {
                let row = self.selected().filter(|row| !row.registered);
                self.open_register(row.as_ref());
            }
            Action::Forget => self.forget(),
            Action::CancelForget => {
                if let Screen::Library(library) = &mut self.screen {
                    library.cancel_forget();
                }
            }
            Action::Wake => self.wake(),
            Action::NextQuality => {
                self.settings.quality = self.settings.quality.next();
                if let Err(error) = self.settings.save(&self.store.settings_path()) {
                    self.notice = Some(error.to_string());
                }
            }
            Action::Back => {
                self.screen = Screen::Library(Library::default());
                self.resize_library();
            }
            Action::Leave => self.end_play(None),
            Action::Rest => {
                if let Some(play) = self.play.as_mut() {
                    play.rest(Instant::now());
                }
                self.screen = Screen::Play;
            }
            Action::Keep => self.keep_playing(),
            Action::SubmitPin => self.submit_pin(),
            form => self.form_action(form),
        }
        Control::Redraw
    }

    fn form_action(&mut self, action: Action) {
        match action {
            Action::Focus(field) => {
                if let Screen::Register(form) = &mut self.screen {
                    form.focus(field);
                }
            }
            Action::Source(by) => {
                if let Screen::Register(form) = &mut self.screen {
                    form.look_by(by);
                }
            }
            Action::OpenSignIn => open_link(&psn::sign_in_url()),
            Action::LookUp => self.look_up(),
            Action::Link => self.link(),
            _ => {}
        }
    }

    fn open_register(&mut self, row: Option<&Row>) {
        let console = row.and_then(|row| {
            self.discovered
                .iter()
                .find(|console| console.name == row.name)
        });
        let form = Register::new(
            console.map(|console| console.address.clone()),
            console.map_or(Kind::PS5, |console| console.kind),
        );
        self.screen = Screen::Register(Box::new(form));
    }

    /// Close the leave menu onto the screen it opened over, and hand the session back the game
    /// controllers the menu held as released.
    fn keep_playing(&mut self) {
        if let Screen::Leave { back, .. } = std::mem::replace(&mut self.screen, Screen::Play) {
            self.screen = *back;
        }
        if let (Screen::Play, Some(play)) = (&self.screen, self.play.as_mut()) {
            play.gamepads(self.held_pads.clone());
        }
    }

    /// Play the selected console, or open the form for one that has no registration.
    fn pick(&mut self) {
        match self.selected() {
            Some(Row {
                registered: true,
                name,
                ..
            }) => {
                if let Some(registration) = self.registration(&name)
                    && let Err(error) = self.start_play(&registration)
                {
                    self.notice = Some(error.to_string());
                }
            }
            Some(row) => self.open_register(Some(&row)),
            None => {}
        }
    }

    /// Forget the selected console once the dialog asking for it is open.
    fn forget(&mut self) {
        if let Screen::Library(library) = &mut self.screen {
            if library.forgetting() {
                if let Some(row) =
                    library::rows(&self.registrations, &self.discovered).get(library.selected())
                    && let Err(error) = self.store.remove(&row.name)
                {
                    self.notice = Some(error.to_string());
                }
                library.cancel_forget();
                self.reload();
            } else {
                library.ask_to_forget();
            }
        }
    }

    /// Send the typed PIN when it has four digits; complain otherwise.
    fn submit_pin(&mut self) {
        let pin = match &mut self.screen {
            Screen::Pin { typed, complaint } => {
                let digits: Vec<u8> = typed.text().bytes().filter(u8::is_ascii_digit).collect();
                *complaint = (digits.len() != 4)
                    .then(|| "Type the 4 digits of the account's sign-in PIN".to_owned());
                (digits, complaint.is_none())
            }
            _ => (Vec::new(), false),
        };
        if pin.1 {
            if let Some(play) = self.play.as_mut() {
                play.send_pin(&pin.0);
            }
            self.screen = Screen::Play;
        }
    }

    /// Ask the selected console to wake, on a thread since it takes a moment.
    fn wake(&mut self) {
        let Some(registration) = self
            .selected()
            .filter(|row| row.registered)
            .and_then(|row| self.registration(&row.name))
        else {
            return;
        };
        let (log, told) = (Arc::clone(&self.log), self.waker.clone());
        std::thread::spawn(move || {
            if let (Err(error), Some(told)) = (registration.wake(&log), told) {
                told.send(Message::told(Told::Notice(error.to_string())));
            }
        });
    }

    fn look_up(&mut self) {
        let (account, told) = match (&mut self.screen, self.waker.clone()) {
            (Screen::Register(form), told) => (form.lookup(), told),
            _ => return,
        };
        let Some(account) = account else {
            return;
        };
        std::thread::spawn(move || {
            let found = match account {
                Account::Link(link) => psn::account_id_from_redirect(&link),
                Account::Name(name) => psn::account_id_of(&name),
            };
            if let Some(told) = told {
                told.send(Message::told(Told::Account(found.map_err(Arc::new))));
            }
        });
    }

    fn link(&mut self) {
        let (request, told) = match (&mut self.screen, self.waker.clone()) {
            (Screen::Register(form), told) => (form.request(), told),
            _ => return,
        };
        let (Some(request), Some(told)) = (request, told) else {
            return;
        };
        let log = Arc::clone(&self.log);
        let started = Pairing::start(&request, log, move |outcome| {
            told.send(Message::told(Told::Registered(outcome)));
        });
        if let Screen::Register(form) = &mut self.screen {
            match started {
                Ok(pairing) => form.hold(pairing),
                Err(error) => form.failed(error),
            }
        }
    }

    /// A registration ended: store what the console gave, or say why it refused.
    fn registered(&mut self, outcome: Outcome) -> Control {
        let Screen::Register(form) = &mut self.screen else {
            return Control::Wait;
        };
        let stored = match outcome {
            Outcome::Registered {
                regist_key,
                morning,
            } => match form.pending() {
                Some(request) => {
                    let name = self
                        .discovered
                        .iter()
                        .find(|console| console.address == request.host)
                        .map(|console| console.name.as_str());
                    let registration = Registration::granted(request, regist_key, morning, name);
                    self.store.save(&registration)
                }
                None => Err(Error::Console(
                    "the registration lost its request".to_owned(),
                )),
            },
            Outcome::Failed => Err(Error::Refused),
            Outcome::Canceled => return Control::Wait,
        };
        match stored {
            Ok(registration) => {
                let name = registration.name.clone();
                self.reload();
                self.notice = Some(format!("Registered {name}. Press Enter to play"));
                self.screen = Screen::Library(Library::default());
                self.resize_library();
            }
            Err(error) => {
                if let Screen::Register(form) = &mut self.screen {
                    form.failed(error);
                }
            }
        }
        Control::Redraw
    }

    fn input(&mut self, event: &InputEvent) -> Control {
        if matches!(event, InputEvent::Key(Key::Char('c'), mods) if mods.ctrl) {
            return Control::Quit;
        }
        let action = match &mut self.screen {
            Screen::Play => return self.play_input(event),
            Screen::Pin { .. } => return self.pin_input(event),
            Screen::Leave { .. } => return self.leave_input(event),
            Screen::Ended(_) => {
                return match event {
                    InputEvent::Mouse(mouse) => self.click(mouse),
                    InputEvent::Key(Key::Escape | Key::Enter, _) => self.act(Action::Back),
                    _ => Control::Redraw,
                };
            }
            Screen::Library(library) => match &event {
                InputEvent::Mouse(mouse) => return self.click(mouse),
                other => {
                    library.input(other, &library::rows(&self.registrations, &self.discovered))
                }
            },
            Screen::Register(form) => match &event {
                InputEvent::Mouse(mouse) => return self.click(mouse),
                other => form.input(other),
            },
        };
        match action {
            Some(action) => self.act(action),
            None => Control::Redraw,
        }
    }

    /// A key while the picture shows. Esc opens the menu; every other key
    /// is for the controller.
    fn play_input(&mut self, event: &InputEvent) -> Control {
        if matches!(event, InputEvent::Key(Key::Escape, _)) {
            if let Some(play) = self.play.as_mut() {
                play.release_keys();
            }
            self.open_leave_menu();
            return Control::Redraw;
        }
        let now = Instant::now();
        if let Some(play) = self.play.as_mut() {
            match event {
                InputEvent::Key(key, mods) | InputEvent::KeyRepeat(key, mods) => {
                    play.key(*key, *mods, true, now);
                }
                InputEvent::KeyUp(key, mods) => play.key(*key, *mods, false, now),
                InputEvent::Focus(false) => play.release_keys(),
                _ => {}
            }
        }
        Control::Redraw
    }

    /// The leave menu's choices with their labels: resting the console is one once the
    /// session is live.
    fn leave_choices(&self) -> Vec<(Action, &'static str)> {
        let live = self
            .play
            .as_ref()
            .is_some_and(|play| play.stage() == Stage::Live);
        let rest = live.then_some((Action::Rest, "Rest the console and leave"));
        std::iter::once((Action::Keep, "Keep playing"))
            .chain(rest)
            .chain([(Action::Leave, "Leave")])
            .collect()
    }

    /// The leave menu's keys over its choices as they are now, on `current` when it is one.
    fn leave_focus(&self, current: Action) -> Focus<Action> {
        let mut choice = Focus::new(
            Action::Keep,
            self.leave_choices()
                .into_iter()
                .skip(1)
                .map(|(action, _)| action),
        );
        choice.set(current);
        choice
    }

    fn open_leave_menu(&mut self) {
        let back = std::mem::replace(&mut self.screen, Screen::Play);
        self.screen = Screen::Leave {
            choice: self.leave_focus(Action::Keep),
            back: Box::new(back),
        };
    }

    fn leave_input(&mut self, event: &InputEvent) -> Control {
        let current = match &self.screen {
            Screen::Leave { choice, .. } => choice.current(),
            _ => return Control::Wait,
        };
        // The session can turn live while the menu is open, which adds a choice.
        let fresh = self.leave_focus(current);
        let Screen::Leave { choice, .. } = &mut self.screen else {
            return Control::Wait;
        };
        *choice = fresh;
        match event {
            InputEvent::Mouse(mouse) => return self.click(mouse),
            InputEvent::Key(Key::Escape, _) => return self.act(Action::Keep),
            InputEvent::Key(Key::Enter, _) => {
                let picked = choice.current();
                return self.act(picked);
            }
            InputEvent::Key(Key::Left | Key::Up, _) => choice.previous(),
            InputEvent::Key(Key::Right | Key::Down, _) => choice.next(),
            other => {
                choice.handle(other);
            }
        }
        Control::Redraw
    }

    fn pin_input(&mut self, event: &InputEvent) -> Control {
        match &event {
            InputEvent::Mouse(mouse) => return self.click(mouse),
            InputEvent::Key(Key::Escape, _) => self.open_leave_menu(),
            InputEvent::Key(Key::Enter, _) => return self.act(Action::SubmitPin),
            _ => {
                if let Screen::Pin { typed, complaint } = &mut self.screen {
                    typed.handle(event);
                    *complaint = None;
                }
            }
        }
        Control::Redraw
    }

    /// A mouse event on a widget; what the widget asks for.
    fn click(&mut self, mouse: &katui::proto::input::MouseEvent) -> Control {
        let cell = self.cell;
        let (x, y) = self.pointer.locate(mouse, cell);
        if let MouseAction::Release(_) = mouse.action {
            self.ui.release();
            return Control::Wait;
        }
        let asked = match mouse.action {
            MouseAction::Press(Button::Left) => self.ui.click(x, y),
            MouseAction::Drag(Button::Left) => self.ui.drag(x, y),
            MouseAction::WheelUp | MouseAction::WheelDown => {
                self.ui.wheel(x, y, mouse.action == MouseAction::WheelDown)
            }
            _ => None,
        };
        match asked {
            Some(Message::Ask(action)) => self.act(action),
            _ => Control::Wait,
        }
    }

    fn told(&mut self, told: Told) -> Control {
        match told {
            Told::Consoles(consoles) => self.consoles(consoles),
            Told::Picture => self.picture(),
            Told::Session(event) => return self.session(event),
            Told::Gamepads => return self.gamepads_moved(),
            Told::Registered(outcome) => return self.registered(outcome),
            Told::Account(found) => {
                if let Screen::Register(form) = &mut self.screen {
                    form.found(found);
                }
            }
            Told::Notice(notice) => self.notice = Some(notice),
            Told::WakeDeadline => {
                if let Some(Err(error)) = self.play.as_ref().map(|play| play.check_wake()) {
                    self.end_play(Some(error.to_string()));
                }
            }
        }
        Control::Redraw
    }

    /// The stream gets the controllers while it shows; every other screen
    /// reads them as menu keys, and the console sees them released.
    fn gamepads_moved(&mut self) -> Control {
        let Some(pads) = self.gamepads.lock().ok().and_then(|mut slot| slot.take()) else {
            return Control::Wait;
        };
        let before = std::mem::replace(&mut self.held_pads, pads);
        let streaming = matches!(self.screen, Screen::Play);
        if let Some(play) = self.play.as_mut() {
            play.gamepads(if streaming {
                self.held_pads.clone()
            } else {
                Vec::new()
            });
            play.send(Instant::now());
        }
        if streaming {
            return Control::Wait;
        }
        let keys = pad::menu_keys(&before, &self.held_pads);
        if keys.is_empty() {
            return Control::Wait;
        }
        for key in keys {
            if matches!(self.input(&InputEvent::Key(key, Mods::NONE)), Control::Quit) {
                return Control::Quit;
            }
        }
        Control::Redraw
    }

    fn consoles(&mut self, consoles: Vec<Console>) {
        self.discovered = consoles;
        self.resize_library();
        if let (Some(waker), Some(play)) = (self.waker.clone(), self.play.as_mut())
            && play.stage() == Stage::Waking
        {
            match play.found(&self.discovered, &waker) {
                Err(error) => self.end_play(Some(error.to_string())),
                Ok(true) => self.frame = self.frame_size(),
                Ok(false) => {}
            }
        }
    }

    fn picture(&mut self) {
        let frame = self.frame;
        if let Some(play) = self.play.as_mut()
            && let Err(error) = play.show(frame)
        {
            self.end_play(Some(error.to_string()));
        }
    }

    fn session(&mut self, event: Event) -> Control {
        let Some(play) = self.play.as_mut() else {
            return Control::Wait;
        };
        let name = play.name.clone();
        match play.reported(event) {
            Report::Nothing => {}
            Report::Pin { incorrect } => {
                self.screen = Screen::Pin {
                    typed: TextField::default(),
                    complaint: incorrect
                        .then(|| "Type the PIN again: that one was wrong".to_owned()),
                }
            }
            Report::Ended(why) => {
                self.end_play(Some(if why.is_empty() {
                    format!("{name} ended the session")
                } else {
                    why
                }));
            }
        }
        Control::Redraw
    }

    fn view(&self) -> Element<Message> {
        let gap = Theme::current().cell_width();
        match &self.screen {
            Screen::Library(library) => library.view(
                &library::rows(&self.registrations, &self.discovered),
                self.settings.quality,
                self.notice.as_deref(),
            ),
            Screen::Register(form) => form.view(),
            Screen::Pin { typed, complaint } => page([panel(
                [
                    title("Sign in to the console"),
                    label("Type the 4 digits of the account's sign-in PIN"),
                    text_field(typed, "PIN", true),
                ]
                .into_iter()
                .chain(std::iter::once(complaint.as_ref().map_or_else(
                    || hint("Press Enter to send it"),
                    |complaint| label(complaint.clone()),
                )))
                .chain(std::iter::once(
                    Element::row([
                        primary_button("Send the PIN", Message::ask(Action::SubmitPin)),
                        button("Leave the session", Message::ask(Action::Leave)),
                    ])
                    .gap(gap),
                )),
            )])
            .align(Align::Center),
            Screen::Leave { choice, .. } => {
                let actions = self.leave_choices().into_iter().map(|(action, text)| {
                    // katui's buttons have no focused look, so we draw the one the keys are on
                    // as the primary button.
                    if choice.is(action) {
                        primary_button(text, Message::ask(action))
                    } else {
                        button(text, Message::ask(action))
                    }
                });
                page([panel([
                    title("Leave the session?"),
                    Element::row(actions).gap(gap),
                ])])
                .align(Align::Center)
            }
            Screen::Ended(why) => page([panel([
                title("The session ended"),
                label(why.clone()),
                button("Back to the consoles", Message::ask(Action::Back)),
            ])])
            .align(Align::Center),
            Screen::Play => page(std::iter::empty()),
        }
    }

    fn draw_play(&mut self, out: &mut Vec<u8>, size: CellSize) -> io::Result<Control> {
        self.frame = self.frame_size();
        let line = self
            .play
            .as_ref()
            .map_or_else(String::new, |play| play.status(self.settings.quality));
        self.status.set(line);
        self.status.draw(out, size.columns)?;
        let area = video_area(size, self.cell);
        if let Some(play) = self.play.as_ref()
            && let Some(picture) = play.picture().filter(|_| play.fits(self.frame))
        {
            self.presenter.render_frame_in(out, picture, area)?;
        }
        Ok(Control::Wait)
    }

    fn draw_widgets(&mut self, out: &mut Vec<u8>, size: CellSize) -> io::Result<()> {
        let root = Theme::dark(self.cell).scope(|_| self.view());
        let rows = self.presenter.rows_shown(size.rows);
        let pixels = (
            u32::from(size.columns) * u32::from(self.cell.width),
            u32::from(rows) * u32::from(self.cell.height),
        );
        let output = self.ui.render(&root, pixels, self.cell);
        if let Some(picture) = output.flatten(self.cell, pixels) {
            self.presenter
                .render_frame(out, &picture, size.columns, size.rows)?;
        }
        Ok(())
    }

    /// Send everything again: the terminal may have lost what was shown.
    fn repaint(&mut self) -> Control {
        self.presenter.repaint();
        self.status.invalidate();
        Control::Redraw
    }
}

impl App for Shell {
    type Message = Message;

    fn start(&mut self, waker: Waker<Message>) {
        self.waker = Some(waker.clone());
        if let Some(name) = self.eager.take() {
            match self.registration(&name) {
                Some(registration) => {
                    if let Err(error) = self.start_play(&registration) {
                        self.notice = Some(error.to_string());
                    }
                }
                None => self.notice = Some(Error::NotRegistered(name).to_string()),
            }
        }
        if self.offline {
            return;
        }
        let log = Arc::new(Log::quiet());
        let found = waker.clone();
        std::thread::spawn(move || {
            // the discovery lives as long as the app, so its thread parks here
            if Discovery::start(log, move |consoles| {
                found.send(Message::told(Told::Consoles(consoles)));
            })
            .is_ok()
            {
                std::thread::park();
            }
        });
        let latest = Arc::clone(&self.gamepads);
        let pads = waker;
        // a controller reports far faster than we draw, so we keep only its
        // newest state and wake the loop once for it
        let _ = gamepad::watch(move |now| {
            let Ok(mut slot) = latest.lock() else {
                return false;
            };
            let waiting = slot.replace(now.to_vec()).is_some();
            drop(slot);
            waiting || pads.send(Message::told(Told::Gamepads))
        });
    }

    fn update(&mut self, event: AppEvent<Message>) -> Control {
        match event {
            AppEvent::Input(input) => self.input(&input),
            AppEvent::Message(Message::Ask(action)) => self.act(action),
            AppEvent::Message(Message::Told(told)) => self.told(told),
            AppEvent::Resized(size) => {
                self.size = size;
                self.cell = cell_size(&self.caps);
                self.presenter.set_cell(self.cell);
                self.frame = self.frame_size();
                self.repaint()
            }
            AppEvent::Repaint => self.repaint(),
        }
    }

    fn draw(&mut self, out: &mut Vec<u8>, size: CellSize) -> io::Result<Control> {
        self.pointer.resize(size.columns, size.rows);
        self.size = size;
        if self.play.is_some() && !self.releases_asked {
            session::report_key_releases(out)?;
            self.releases_asked = true;
        }
        let control = if matches!(self.screen, Screen::Play) {
            self.draw_play(out, size)?
        } else {
            self.draw_widgets(out, size)?;
            Control::Wait
        };
        let now = Instant::now();
        if let Some(play) = self.play.as_mut() {
            play.send(now);
            if play.rested(now) {
                self.end_play(None);
                return Ok(Control::Redraw);
            }
        }
        Ok(control)
    }

    /// Let the governor judge how long the picture took to reach the terminal.
    fn delivered(&mut self, delivery: &Delivery) {
        if matches!(self.screen, Screen::Play)
            && self
                .governor
                .delivered(delivery.delivered, Instant::now())
                .is_some()
        {
            self.frame = self.frame_size();
        }
    }
}

/// Open `link` in the browser of this machine.
fn open_link(link: &str) {
    let opener = if cfg!(target_os = "macos") {
        "open"
    } else {
        "xdg-open"
    };
    let _child = std::process::Command::new(opener).arg(link).spawn();
}

/// The cells the picture gets: the screen below the status row, cut to the
/// 16:9 of the console's video and centred in what is left.
fn video_area(size: CellSize, cell: CellPixels) -> CellRect {
    let (wide, high) = (
        u32::from(size.columns),
        u32::from(size.rows.saturating_sub(ROWS)),
    );
    let (cell_w, cell_h) = (u32::from(cell.width).max(1), u32::from(cell.height).max(1));
    let rows_if_full_width = wide * cell_w * 9 / (16 * cell_h);
    let (wide, high) = if rows_if_full_width <= high {
        (wide, rows_if_full_width)
    } else {
        (high * cell_h * 16 / (9 * cell_w), high)
    };
    let (wide, high) = (wide.max(1), high.max(1));
    CellRect {
        x: u16::try_from((u32::from(size.columns) - wide) / 2).unwrap_or(0),
        y: ROWS
            + u16::try_from((u32::from(size.rows.saturating_sub(ROWS)) - high) / 2).unwrap_or(0),
        columns: u16::try_from(wide).unwrap_or(u16::MAX),
        rows: u16::try_from(high).unwrap_or(u16::MAX),
    }
}

/// The size of the picture for an area of `area` pixels: the area's shape,
/// shrunk to what the terminal takes `FRAMES_PER_SECOND` times a second and
/// to `scale`, with whole even sides.
#[allow(
    clippy::cast_sign_loss,
    reason = "a pixel count is positive by construction"
)]
fn frame_size(area: (u32, u32), multiplexed: bool, scale: f64) -> (u32, u32) {
    let shrink = f64::from(if multiplexed { 2 } else { 1 });
    let (across, down) = (f64::from(area.0.max(1)), f64::from(area.1.max(1)));
    let fit = (f64::from(MOST_PIXELS.0) / shrink / across)
        .min(f64::from(MOST_PIXELS.1) / shrink / down)
        .min(1.0);
    let side = |pixels: f64| ((pixels * fit * scale).round() as u32).max(2) & !1;
    (side(across), side(down))
}

#[cfg(test)]
mod tests {
    use katui::proto::input::{InputEvent, Key, Mods};
    use katui::proto::probe::Capabilities;
    use katui::rt::headless::Headless;
    use katui::scene::CellSize;
    use katui::widgets::TextField;

    use super::{Action, Screen, Shell, frame_size, video_area};
    use crate::chiaki::registration::Outcome;
    use crate::console::{Console, Kind, Registration, State, Store};
    use crate::library;
    use crate::settings::{Quality, Settings};
    use crate::shell::{Message, ROWS, Told};

    const SIZE: CellSize = CellSize {
        columns: 100,
        rows: 32,
    };
    const CELL: katui::proto::probe::CellPixels = katui::proto::probe::CellPixels {
        width: 10,
        height: 20,
    };
    const FILE: &str = "host=192.0.2.10\nkind=ps5\nregist_key=30313233616263640000000000000000\nmorning=000102030405060708090a0b0c0d0e0f\naccount_id=AAECAwQFBgc=\n";

    fn shell(test: &str, registrations: Vec<Registration>) -> Shell {
        let caps = Capabilities {
            kitty: true,
            ..Capabilities::default()
        };
        let root = std::env::temp_dir().join(format!("katplay-{test}-{}", std::process::id()));
        let store = Store::at(&root).expect("a store in a temporary folder");
        Shell::new(caps, store, Settings::default(), registrations, true).expect("the fonts parse")
    }

    fn frame(driver: &mut Headless<Shell>) -> usize {
        driver
            .frame()
            .expect("drawing works")
            .map_or(0, |frame| frame.len())
    }

    #[test]
    fn the_list_shows_the_registrations_and_the_keys_walk_through_the_screens() {
        let registration = Registration::parse("PS5-123", FILE).expect("it parses");
        let mut driver = Headless::start(shell("list", vec![registration]), SIZE);
        assert!(frame(&mut driver) > 0, "the list goes out as a picture");
        driver.type_bytes(b"\x1b[B");
        driver.type_bytes(b"r");
        assert!(frame(&mut driver) > 0, "the form draws");
        for key in [b"10.0.0.5".as_slice(), b"\r", b"12345678", b"\r"] {
            driver.type_bytes(key);
        }
        assert!(frame(&mut driver) > 0);
        driver.type_bytes(b"\x1b");
        assert!(
            matches!(driver.app().screen, Screen::Library(_)),
            "Esc leaves the form"
        );
        driver.type_bytes(b"\t");
        assert_eq!(
            driver.app().settings.quality,
            Quality::P1080,
            "Tab moves to the next quality"
        );
        assert!(
            driver.app().store.settings_path().is_file(),
            "the choice is kept"
        );
        driver.type_bytes(b"\x03");
        assert!(driver.quit(), "ctrl c quits");
        std::fs::remove_dir_all(driver.app().store.root()).expect("the test cleans up");
    }

    #[test]
    fn the_keys_reach_every_choice_of_the_leave_menu() {
        let mut app = shell("leave", vec![]);
        app.screen = Screen::Pin {
            typed: TextField::default(),
            complaint: None,
        };
        app.input(&InputEvent::Key(Key::Escape, Mods::NONE));
        app.input(&InputEvent::Key(Key::Escape, Mods::NONE));
        assert!(
            matches!(app.screen, Screen::Pin { .. }),
            "keeping on goes back to the PIN the console waits for"
        );
        app.open_leave_menu();
        app.input(&InputEvent::Key(Key::Right, Mods::NONE));
        assert!(
            matches!(&app.screen, Screen::Leave { choice, .. } if choice.is(Action::Leave)),
            "the arrow moves to Leave"
        );
        app.input(&InputEvent::Key(Key::Enter, Mods::NONE));
        assert!(matches!(app.screen, Screen::Library(_)), "Enter leaves");
        std::fs::remove_dir_all(app.store.root()).expect("the test cleans up");
    }

    #[test]
    fn a_refused_registration_says_what_to_do_and_a_notice_is_shown() {
        let mut driver = Headless::start(shell("refused", vec![]), SIZE);
        frame(&mut driver);
        driver.type_bytes(b"r");
        let waker = driver.app().waker.clone().expect("the app started");
        waker.send(Message::told(Told::Registered(Outcome::Failed)));
        driver.deliver_messages();
        let status = match &driver.app().screen {
            Screen::Register(form) => form.status().map(str::to_owned),
            _ => None,
        };
        assert!(status.is_some_and(|status| status.contains("Link Device again")));
        waker.send(Message::told(Told::Notice("waking failed".to_owned())));
        driver.deliver_messages();
        assert_eq!(driver.app().notice.as_deref(), Some("waking failed"));
        driver.type_bytes(b"\x03");
        assert!(driver.quit());
        std::fs::remove_dir_all(driver.app().store.root()).expect("the test cleans up");
    }

    #[test]
    fn the_picture_keeps_the_video_shape_below_the_status_row() {
        let area = video_area(SIZE, CELL);
        assert_eq!(
            (area.x, area.y, area.columns, area.rows),
            (0, ROWS + 1, 100, 28),
            "1000 pixels wide is 28 rows of 16:9"
        );
        let flat = CellSize {
            columns: 100,
            rows: 10,
        };
        let area = video_area(flat, CELL);
        assert_eq!(
            (area.x, area.y, area.columns, area.rows),
            (34, ROWS, 32, 9),
            "a short screen narrows the picture to its height"
        );
        assert_eq!(frame_size((1000, 560), false, 1.0), (960, 538));
        assert_eq!(frame_size((1000, 560), true, 1.0), (480, 268));
        let (wide, high) = frame_size((800, 450), false, 0.5);
        assert_eq!((wide, high), (400, 224));
        assert_eq!(frame_size((0, 0), false, 1.0), (2, 2));
    }

    #[test]
    fn the_rows_list_the_registrations_then_the_other_consoles() {
        let registration = Registration::parse("PS5-123", FILE).expect("it parses");
        let console = |name: &str, address: &str, state: State| Console {
            name: name.to_owned(),
            address: address.to_owned(),
            kind: Kind::PS5,
            state,
            running_app: None,
        };
        let discovered = vec![
            console("PS5-999", "10.0.0.9", State::Ready),
            console("PS5-123", "192.0.2.10", State::Standby),
            console("PS5-111", "10.0.0.1", State::Ready),
        ];
        let rows = library::rows(std::slice::from_ref(&registration), &discovered);
        assert_eq!(rows.len(), 3);
        assert_eq!(rows[0].name, "PS5-123");
        assert!(rows[0].registered && rows[0].detail.ends_with("in rest"));
        assert_eq!(
            (rows[1].name.as_str(), rows[2].name.as_str()),
            ("PS5-111", "PS5-999"),
            "the others sort by name"
        );
        assert!(!rows[1].registered);
    }
}
