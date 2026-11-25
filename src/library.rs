//! The first screen: the consoles katplay is registered with, and the others
//! on the network.

use katui::proto::input::{InputEvent, Key};
use katui::ui::{Align, Element};
use katui::widgets::{
    ListState, Theme, button, dialog, hint, key_hints, label, list, page, panel, primary_button,
    title,
};

use crate::console::{Console, Registration};
use crate::settings::Quality;
use crate::shell::{Action, Message};

/// How many consoles the list shows at once.
const VISIBLE: usize = 10;

/// One console in the list.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Row {
    pub name: String,
    pub detail: String,
    pub registered: bool,
}

#[derive(Debug, Default)]
pub(crate) struct Library {
    selection: ListState,
    /// Whether the dialog asking to forget the selected console is open.
    forgetting: bool,
}

impl Library {
    pub(crate) fn selected(&self) -> usize {
        self.selection.selected()
    }

    pub(crate) fn resize(&mut self, rows: usize) {
        self.selection.resize(rows, VISIBLE);
    }

    pub(crate) fn forgetting(&self) -> bool {
        self.forgetting
    }

    pub(crate) fn ask_to_forget(&mut self) {
        self.forgetting = true;
    }

    pub(crate) fn cancel_forget(&mut self) {
        self.forgetting = false;
    }

    pub(crate) fn view(
        &self,
        rows: &[Row],
        quality: Quality,
        notice: Option<&str>,
    ) -> Element<Message> {
        let consoles = list(&self.selection, |index, _| {
            let text = match rows.get(index) {
                Some(row) => format!("{:<20} {}", row.name, row.detail),
                None => String::new(),
            };
            label(text)
        });
        let body = if rows.is_empty() {
            hint(
                "Turn on the PlayStation and connect it to this network, then press r to register it",
            )
        } else {
            consoles
        };
        let notice = notice.map(|notice| label(notice.to_owned()));
        let card = panel(
            [
                title("katplay"),
                body,
                label(format!("Video: {quality}. Press Tab to change it")),
            ]
            .into_iter()
            .chain(notice)
            .chain([key_hints(&[
                ("Enter", "play or register"),
                ("r", "register a console"),
                ("w", "wake"),
                ("x", "forget"),
                ("Ctrl+C", "quit"),
            ])]),
        )
        .width(Theme::current().columns(68.0));
        let forget = self.forgetting.then(|| {
            dialog(
                "Forget this console?",
                label(
                    "The registration stays on the console; pairing it again needs a new link code",
                ),
                [
                    button("Keep it", Message::ask(Action::CancelForget)),
                    primary_button("Forget it", Message::ask(Action::Forget)),
                ],
                Message::ask(Action::CancelForget),
            )
        });
        page([card].into_iter().chain(forget)).align(Align::Center)
    }

    /// A key for the list; what it asks for.
    pub(crate) fn input(&mut self, event: &InputEvent, rows: &[Row]) -> Option<Action> {
        let registered = rows.get(self.selected()).is_some_and(|row| row.registered);
        if self.forgetting {
            return match event {
                InputEvent::Key(Key::Escape, _) => Some(Action::CancelForget),
                InputEvent::Key(Key::Enter, _) if registered => Some(Action::Forget),
                _ => None,
            };
        }
        self.selection.handle(event);
        match event {
            InputEvent::Key(Key::Enter, _) => Some(if registered {
                Action::Pick
            } else {
                Action::Register
            }),
            InputEvent::Key(Key::Char('r'), _) => Some(Action::Register),
            InputEvent::Key(Key::Char('x'), _) if registered => Some(Action::Forget),
            InputEvent::Key(Key::Char('w'), _) if registered => Some(Action::Wake),
            InputEvent::Key(Key::Tab, _) => Some(Action::NextQuality),
            _ => None,
        }
    }
}
/// The rows of the list: the registrations with what the network says about
/// them, then the consoles on the network that have no registration.
pub(crate) fn rows(registrations: &[Registration], discovered: &[Console]) -> Vec<Row> {
    let mut rows: Vec<Row> = registrations
        .iter()
        .map(|registration| {
            let state = discovered
                .iter()
                .find(|console| console.address == registration.host)
                .map_or_else(
                    || "not on the network".to_owned(),
                    |console| console.state.to_string(),
                );
            Row {
                name: registration.name.clone(),
                detail: format!("{}  {}  {}", registration.kind, registration.host, state),
                registered: true,
            }
        })
        .collect();
    let mut found: Vec<Row> = discovered
        .iter()
        .filter(|console| {
            !registrations
                .iter()
                .any(|registration| registration.host == console.address)
        })
        .map(|console| Row {
            name: console.name.clone(),
            detail: format!("{}  {}  not registered", console.kind, console.address),
            registered: false,
        })
        .collect();
    found.sort_by(|a, b| a.name.cmp(&b.name));
    rows.append(&mut found);
    rows
}
