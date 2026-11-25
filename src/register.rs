//! Registering a console: the link code the console shows, the PSN account
//! to register, and the registration itself.

use std::sync::Arc;

use katui::proto::input::{InputEvent, Key};
use katui::ui::{Align, Element};
use katui::widgets::{
    Edit, Focus, TextField, Theme, button, hint, label, page, panel, primary_button, tabs,
    text_field, title,
};

use crate::Error;
use crate::chiaki::registration::Registration;
use crate::chiaki::registration::Request;
use crate::console::Kind;
use crate::psn;
use crate::shell::{Action, Message};

/// The text fields of the form, in the order Tab walks them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Field {
    Address,
    Code,
    Account,
}

/// How the account is found: by the link PSN ends a sign-in on, or by a
/// public online id.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum By {
    Link,
    Name,
}

/// What there is to look the account up with.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Account {
    /// The link the browser ended on after signing in to PSN.
    Link(String),
    /// A public online id.
    Name(String),
}

pub(crate) struct Register {
    address: TextField,
    code: TextField,
    account: TextField,
    by: By,
    focus: Focus<Field>,
    /// The account id, once PSN or the lookup gave it.
    found: Option<[u8; 8]>,
    /// What went wrong, or what is under way.
    status: Option<String>,
    /// The registration asked for, and the one under way (dropping it cancels it).
    pending: Option<Request>,
    under_way: Option<Registration>,
    kind: Kind,
}

impl Register {
    /// The form for a console of `kind` at `address`, when it is known.
    pub(crate) fn new(address: Option<String>, kind: Kind) -> Self {
        Self {
            address: TextField::with_text(address.unwrap_or_default()),
            code: TextField::default(),
            account: TextField::default(),
            by: By::Link,
            focus: Focus::new(Field::Address, [Field::Code, Field::Account]),
            found: None,
            status: None,
            pending: None,
            under_way: None,
            kind,
        }
    }

    pub(crate) fn view(&self) -> Element<Message> {
        let theme = Theme::current();
        let focused = |field| self.focus.is(field);
        let account: Vec<Element<Message>> = match self.by {
            By::Link => vec![
                hint("Sign in to PSN in a browser and paste the link it ends on"),
                text_field(
                    &self.account,
                    &format!("{}?code=...", psn::REDIRECT),
                    focused(Field::Account),
                ),
            ],
            By::Name => vec![
                hint("Or look the account up by its public online id"),
                text_field(&self.account, "online id", focused(Field::Account)),
            ],
        };
        let status = self.status.as_ref().map_or_else(
            || hint("Register with a PSN account that is not signed in at the television"),
            |status| label(status.clone()),
        );
        let actions = Element::row([
            button("Look up the account", Message::ask(Action::LookUp)),
            primary_button("Register", Message::ask(Action::Link)),
            button("Open the sign-in page", Message::ask(Action::OpenSignIn)),
            button("Back", Message::ask(Action::Back)),
        ])
        .gap(theme.cell_width());
        let form = panel(
            [
                title("Register a console"),
                hint("On the console open Settings, Remote Play, Link Device, and copy what it shows"),
                label("Console address"),
                text_field(&self.address, "192.168.1.20", focused(Field::Address)),
                label("Link code"),
                text_field(&self.code, "the 8 digits the console shows", focused(Field::Code)),
                tabs(
                    &["Sign-in link", "Online id"],
                    usize::from(self.by == By::Name),
                    |index| Message::ask(Action::Source(if index == 1 { By::Name } else { By::Link })),
                ),
            ]
            .into_iter()
            .chain(account)
            .chain([status, actions]),
        )
        .width(theme.columns(76.0));
        page([form]).align(Align::Center)
    }

    /// A key for the form; what it asks for.
    pub(crate) fn input(&mut self, event: &InputEvent) -> Option<Action> {
        if self.under_way.is_some() {
            // Dropping the registration cancels it, so a console that never answers
            // cannot hold the form.
            if let InputEvent::Key(Key::Escape, _) = event {
                self.under_way = None;
                self.pending = None;
                return Some(Action::Back);
            }
            return None;
        }
        if self.focus.handle(event) {
            return None;
        }
        if let InputEvent::Key(Key::Escape, _) = event {
            return Some(Action::Back);
        }
        let field = self.focus.current();
        let edit = match field {
            Field::Address => self.address.handle(event),
            Field::Code => self.code.handle(event),
            Field::Account => self.account.handle(event),
        };
        match (edit, field) {
            (Edit::Submitted, Field::Address) => Some(Action::Focus(Field::Code)),
            (Edit::Submitted, Field::Code) => Some(Action::Focus(Field::Account)),
            (Edit::Submitted, Field::Account) if self.found.is_some() => Some(Action::Link),
            (Edit::Submitted, Field::Account) => Some(Action::LookUp),
            (Edit::Changed, _) => {
                if field == Field::Account {
                    self.found = None;
                }
                self.status = None;
                None
            }
            (Edit::Moved | Edit::Ignored, _) => None,
        }
    }

    pub(crate) fn focus(&mut self, field: Field) {
        self.focus.set(field);
    }

    /// The address typed for the console, when it looks like one.
    fn address(&self) -> Option<String> {
        let address = self.address.text().trim();
        (!address.is_empty() && !address.chars().any(char::is_whitespace))
            .then(|| address.to_owned())
    }

    /// Find the account by the online id (`true`) or by the sign-in link.
    pub(crate) fn look_by(&mut self, by: By) {
        self.by = by;
        self.found = None;
        self.status = None;
    }

    /// What to look the account up with; a status is shown instead when there is nothing.
    pub(crate) fn lookup(&mut self) -> Option<Account> {
        let text = self.account.text().trim().to_owned();
        if text.is_empty() {
            self.status = Some("Type the link or the online id first".to_owned());
            return None;
        }
        Some(if self.by == By::Link {
            Account::Link(text)
        } else {
            Account::Name(text)
        })
    }

    pub(crate) fn found(&mut self, account: Result<[u8; 8], Arc<Error>>) {
        self.status = Some(match account {
            Ok(found) => {
                self.found = Some(found);
                "Account found. Press Enter to register".to_owned()
            }
            Err(error) => {
                self.found = None;
                error.to_string()
            }
        });
    }

    /// The registration to start; a status is shown instead when the form is
    /// not ready for one.
    pub(crate) fn request(&mut self) -> Option<Request> {
        let pin = eight_digits(self.code.text());
        let account = self.found;
        self.status = match (self.address(), pin, account) {
            (None, _, _) => Some("Type the console's address".to_owned()),
            (_, None, _) => Some("Type the 8 digits the console shows as the link code".to_owned()),
            (_, _, None) => Some("Look up the account first".to_owned()),
            (Some(host), Some(pin), Some(psn_account_id)) => {
                self.status = Some("Registering...".to_owned());
                self.pending = Some(Request {
                    host,
                    ps5: self.kind == Kind::PS5,
                    psn_account_id,
                    pin,
                });
                return self.pending.clone();
            }
        };
        self.pending = None;
        None
    }

    /// The registration under way, kept so it runs until it reports.
    pub(crate) fn hold(&mut self, registration: Registration) {
        self.under_way = Some(registration);
    }

    pub(crate) fn failed(&mut self, error: impl std::fmt::Display) {
        self.under_way = None;
        self.status = Some(error.to_string());
    }

    /// The registration asked for, once the form was ready for one.
    pub(crate) fn pending(&self) -> Option<&Request> {
        self.pending.as_ref()
    }

    #[cfg(test)]
    pub(crate) fn status(&self) -> Option<&str> {
        self.status.as_deref()
    }
}

/// The eight digits of a link code.
pub(crate) fn eight_digits(text: &str) -> Option<u32> {
    let text = text.trim();
    (text.len() == 8 && text.bytes().all(|byte| byte.is_ascii_digit()))
        .then(|| text.parse().ok())
        .flatten()
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use katui::proto::input::{InputEvent, Key, Mods};

    use super::Register;
    use crate::Error;
    use crate::chiaki::registration::Request;
    use crate::console::Kind;
    use crate::shell::Action;

    const ENTER: InputEvent = InputEvent::Key(Key::Enter, Mods::NONE);

    fn type_into(form: &mut Register, text: &str) {
        for letter in text.chars() {
            form.input(&InputEvent::Key(Key::Char(letter), Mods::NONE));
        }
    }

    #[test]
    fn a_form_needs_an_address_eight_digits_and_an_account() {
        let mut form = Register::new(Some("192.0.2.10".to_owned()), Kind::PS5);
        assert_eq!(form.request(), None);
        assert!(
            form.status
                .as_deref()
                .is_some_and(|status| status.contains("link code"))
        );
        assert_eq!(form.input(&ENTER), Some(Action::Focus(super::Field::Code)));
        form.focus(super::Field::Code);
        type_into(&mut form, "1234567");
        assert_eq!(form.request(), None, "seven digits are not a link code");
        type_into(&mut form, "8");
        form.found(Ok([0, 1, 2, 3, 4, 5, 6, 7]));
        assert_eq!(
            form.request(),
            Some(Request {
                host: "192.0.2.10".to_owned(),
                ps5: true,
                psn_account_id: [0, 1, 2, 3, 4, 5, 6, 7],
                pin: 12_345_678,
            })
        );
    }

    #[test]
    fn a_refused_account_shows_why_and_clears_the_one_found() {
        let mut form = Register::new(None, Kind::PS5);
        form.found(Ok([0, 1, 2, 3, 4, 5, 6, 7]));
        assert!(form.found.as_ref().is_some());
        form.found(Err(Arc::new(Error::Psn(
            "PSN rejected that code".to_owned(),
        ))));
        assert!(form.found.is_none());
        assert_eq!(form.status.as_deref(), Some("psn: PSN rejected that code"));
        assert_eq!(form.lookup(), None, "nothing typed, nothing to look up");
        assert_eq!(form.input(&ENTER), Some(Action::Focus(super::Field::Code)));
        form.focus(super::Field::Code);
        assert_eq!(
            form.input(&ENTER),
            Some(Action::Focus(super::Field::Account))
        );
        form.focus(super::Field::Account);
        type_into(&mut form, "someone");
        assert_eq!(
            form.lookup()
                .map(|account| matches!(account, super::Account::Link(_))),
            Some(true)
        );
        form.look_by(super::By::Name);
        assert_eq!(
            form.lookup()
                .map(|account| matches!(account, super::Account::Name(_))),
            Some(true)
        );
    }

    #[test]
    fn enter_registers_once_the_account_is_found() {
        let mut form = Register::new(Some("192.0.2.10".to_owned()), Kind::PS4);
        form.focus(super::Field::Account);
        type_into(&mut form, "someone");
        assert_eq!(form.input(&ENTER), Some(Action::LookUp));
        form.found(Ok([0, 1, 2, 3, 4, 5, 6, 7]));
        assert_eq!(form.input(&ENTER), Some(Action::Link));
        form.focus(super::Field::Code);
        type_into(&mut form, "12345678");
        assert!(
            form.request().is_some_and(|request| !request.ps5),
            "a PS4 registers as a PS4"
        );
    }
}
