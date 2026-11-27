//! A console on the network, what discovery reports about it, and the
//! registration that lets a session in.

use std::fmt;
use std::fmt::Write as _;
use std::fs::{OpenOptions, Permissions};
use std::io::Write as _;
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::path::PathBuf;

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD as BASE64;

use crate::chiaki::Log;
use crate::chiaki::discovery;
use crate::chiaki::registration::Request;
use crate::settings::data_dir;
use crate::{Error, io_error};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Kind {
    PS4,
    PS5,
}

impl Kind {
    fn from_name(name: &str) -> Option<Self> {
        match name {
            "ps4" => Some(Self::PS4),
            "ps5" => Some(Self::PS5),
            _ => None,
        }
    }

    fn name(self) -> &'static str {
        match self {
            Self::PS4 => "ps4",
            Self::PS5 => "ps5",
        }
    }

    pub(crate) fn of(ps5: bool) -> Self {
        if ps5 { Self::PS5 } else { Self::PS4 }
    }
}

impl fmt::Display for Kind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::PS4 => "PS4",
            Self::PS5 => "PS5",
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum State {
    Unknown,
    Ready,
    Standby,
}

impl fmt::Display for State {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Unknown => "not found",
            Self::Ready => "ready",
            Self::Standby => "in rest",
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Console {
    pub(crate) address: String,
    pub(crate) name: String,
    pub(crate) kind: Kind,
    pub(crate) state: State,
    pub(crate) running_app: Option<String>,
}

/// A console that stored the account: what lets a session in.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Registration {
    pub(crate) name: String,
    pub(crate) host: String,
    pub(crate) kind: Kind,
    pub(crate) regist_key: [u8; 16],
    pub(crate) morning: [u8; 16],
    pub(crate) psn_account_id: [u8; 8],
}

impl Registration {
    /// The file name of a registration: its name in lowercase letters,
    /// digits and dashes.
    pub(crate) fn id(name: &str) -> String {
        let id: String = name
            .to_lowercase()
            .chars()
            .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
            .collect();
        let id = id.trim_matches('-').to_owned();
        if id.is_empty() || id.len() > 56 {
            "console".to_owned()
        } else {
            id
        }
    }

    /// The registration the console granted for `request`, named `name` or after its kind and
    /// address, in the form its file is named.
    pub(crate) fn granted(
        request: &Request,
        regist_key: [u8; 16],
        morning: [u8; 16],
        name: Option<&str>,
    ) -> Self {
        let kind = Kind::of(request.ps5);
        let last_octet = request.host.rsplit('.').next().unwrap_or(&request.host);
        let name = name.map_or_else(|| format!("{kind}-{last_octet}"), str::to_owned);
        Self {
            name: Self::id(&name),
            host: request.host.clone(),
            kind,
            regist_key,
            morning,
            psn_account_id: request.psn_account_id,
        }
    }

    /// Whether `name` picks this registration: its name, its file name or its address.
    pub(crate) fn answers_to(&self, name: &str) -> bool {
        self.name == name || Self::id(&self.name) == name || self.host == name
    }

    /// Send the console the packet that wakes it from rest.
    pub(crate) fn wake(&self, log: &Log) -> Result<(), Error> {
        let credential = self.wakeup_credential().ok_or_else(|| Error::Credentials {
            name: self.name.clone(),
            reason: "has a registration key that cannot wake it: register it again",
        })?;
        discovery::wake(log, &self.host, credential, self.kind == Kind::PS5)?;
        Ok(())
    }

    /// The credential a wakeup packet carries: the registration key, which
    /// is a hex number stored as its digits.
    pub(crate) fn wakeup_credential(&self) -> Option<u64> {
        let digits: String = self
            .regist_key
            .iter()
            .take_while(|byte| **byte != 0)
            .map(|byte| char::from(*byte))
            .collect();
        u64::from_str_radix(&digits, 16).ok()
    }

    /// Read the credentials file of the console called `name`: `host`,
    /// `kind`, `regist_key`, `morning` and `account_id` lines.
    pub(crate) fn parse(name: &str, text: &str) -> Result<Self, Error> {
        let fields: Vec<(&str, &str)> = text
            .lines()
            .filter_map(|line| line.trim_end().split_once('='))
            .collect();
        let field = |wanted: &str| {
            fields
                .iter()
                .find_map(|(key, value)| (*key == wanted).then_some(*value))
        };
        let credentials = |reason| Error::Credentials {
            name: name.to_owned(),
            reason,
        };
        Ok(Self {
            name: name.to_owned(),
            host: field("host")
                .ok_or_else(|| credentials("has no host"))?
                .to_owned(),
            kind: field("kind")
                .and_then(Kind::from_name)
                .ok_or_else(|| credentials("has no kind of ps5 or ps4"))?,
            regist_key: unhex(field("regist_key").ok_or_else(|| credentials("has no regist_key"))?)
                .ok_or_else(|| credentials("has a regist_key that is not 16 bytes of hex"))?,
            morning: unhex(field("morning").ok_or_else(|| credentials("has no morning"))?)
                .ok_or_else(|| credentials("has a morning that is not 16 bytes of hex"))?,
            psn_account_id: field("account_id")
                .and_then(|account| BASE64.decode(account).ok())
                .and_then(|bytes| bytes.try_into().ok())
                .unwrap_or([0; 8]),
        })
    }

    pub(crate) fn to_text(&self) -> String {
        format!(
            "host={}\nkind={}\nregist_key={}\nmorning={}\naccount_id={}\n",
            self.host,
            self.kind.name(),
            hex(&self.regist_key),
            hex(&self.morning),
            BASE64.encode(self.psn_account_id)
        )
    }
}

fn hex(bytes: &[u8]) -> String {
    let mut text = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        let _ = write!(text, "{byte:02x}");
    }
    text
}

fn unhex(text: &str) -> Option<[u8; 16]> {
    if !text.is_ascii() || text.len() != 32 {
        return None;
    }
    let mut bytes = [0; 16];
    for (byte, digits) in bytes.iter_mut().zip(text.as_bytes().chunks(2)) {
        *byte = u8::from_str_radix(std::str::from_utf8(digits).ok()?, 16).ok()?;
    }
    Some(bytes)
}

/// The registrations katplay knows, one credentials file each in the data
/// directory, named after the console.
#[derive(Debug)]
pub(crate) struct Store {
    /// The folder katplay keeps its files in; the registrations sit in a `consoles` folder inside.
    root: PathBuf,
}

impl Store {
    /// The store in the data directory, created when it is missing.
    pub(crate) fn open() -> Result<Self, Error> {
        Self::at(&data_dir())
    }

    /// A store under `root`, created when it is missing.
    pub(crate) fn at(root: &std::path::Path) -> Result<Self, Error> {
        std::fs::create_dir_all(root.join("consoles")).map_err(|source| io_error(root, source))?;
        Ok(Self {
            root: root.to_path_buf(),
        })
    }

    /// The folder the store lives in, with the settings beside it.
    pub(crate) fn root(&self) -> &std::path::Path {
        &self.root
    }

    pub(crate) fn settings_path(&self) -> PathBuf {
        self.root.join("settings.toml")
    }

    fn dir(&self) -> PathBuf {
        self.root.join("consoles")
    }

    /// Every registration, by name, and the files that would not read.
    pub(crate) fn load(&self) -> (Vec<Registration>, Vec<Error>) {
        let mut files: Vec<(String, PathBuf)> = Vec::new();
        let Ok(entries) = std::fs::read_dir(self.dir()) else {
            return (Vec::new(), Vec::new());
        };
        for entry in entries.flatten() {
            let path = entry.path();
            let console = path
                .extension()
                .is_some_and(|extension| extension == "console");
            if let (true, Some(name)) = (console, path.file_stem().and_then(|stem| stem.to_str())) {
                files.push((name.to_owned(), path));
            }
        }
        files.sort();
        let mut registrations = Vec::new();
        let mut problems = Vec::new();
        for (name, path) in files {
            match std::fs::read_to_string(&path) {
                Ok(text) => match Registration::parse(&name, &text) {
                    Ok(registration) => registrations.push(registration),
                    Err(error) => problems.push(error),
                },
                Err(source) => problems.push(io_error(&path, source)),
            }
        }
        (registrations, problems)
    }

    /// Write `registration` readable by its owner only, in place of any registration for the
    /// same address, and under its id or the first free one after it; the registration as saved.
    pub(crate) fn save(&self, registration: &Registration) -> Result<Registration, Error> {
        let (existing, _) = self.load();
        for old in existing.iter().filter(|old| old.host == registration.host) {
            self.remove(&old.name)?;
        }
        let taken: Vec<&str> = existing
            .iter()
            .filter(|other| other.host != registration.host)
            .map(|other| other.name.as_str())
            .collect();
        let id = Registration::id(&registration.name);
        let name = std::iter::once(id.clone())
            .chain((2..taken.len() + 2).map(|number| format!("{id}-{number}")))
            .find(|name| !taken.contains(&name.as_str()))
            .unwrap_or(id);
        let saved = Registration {
            name,
            ..registration.clone()
        };
        let path = self.path(&saved.name);
        let fail = |source| io_error(&path, source);
        let mut file = OpenOptions::new()
            .create(true)
            .truncate(true)
            .write(true)
            .mode(0o600)
            .open(&path)
            .map_err(fail)?;
        // The mode only applies to a new file, so we tighten one that was already there.
        file.set_permissions(Permissions::from_mode(0o600))
            .map_err(fail)?;
        file.write_all(saved.to_text().as_bytes()).map_err(fail)?;
        Ok(saved)
    }

    /// Delete the registration `load` gave the name `name`.
    pub(crate) fn remove(&self, name: &str) -> Result<(), Error> {
        let path = self.path(name);
        match std::fs::remove_file(&path) {
            Err(source) if source.kind() != std::io::ErrorKind::NotFound => {
                Err(io_error(&path, source))
            }
            _ => Ok(()),
        }
    }

    /// The file of the registration named `name`. A name is a file stem from `load` or an id
    /// from `Registration::id`, so it never holds a path separator.
    fn path(&self, name: &str) -> PathBuf {
        self.dir().join(format!("{name}.console"))
    }
}

#[cfg(test)]
mod tests {
    use std::os::unix::fs::PermissionsExt;

    use super::{Kind, Registration, State, Store, hex, unhex};
    use crate::chiaki::registration::Request;

    const FILE: &str = "host=192.0.2.10\nkind=ps5\nregist_key=30313233616263640000000000000000\nmorning=000102030405060708090a0b0c0d0e0f\naccount_id=AAECAwQFBgc=\n";

    #[test]
    fn kinds_and_states_read_as_people_say_them() {
        assert_eq!(Kind::PS5.to_string(), "PS5");
        assert_eq!(State::Standby.to_string(), "in rest");
    }

    #[test]
    fn a_credentials_file_reads_and_writes_back_the_same() {
        let registration = Registration::parse("PS5-123", FILE).expect("it parses");
        assert_eq!(registration.host, "192.0.2.10");
        assert_eq!(registration.kind, Kind::PS5);
        assert_eq!(registration.psn_account_id, [0, 1, 2, 3, 4, 5, 6, 7]);
        assert_eq!(registration.wakeup_credential(), Some(0x0123_abcd));
        assert_eq!(registration.to_text(), FILE);
        let again = Registration {
            name: "again".to_owned(),
            ..registration.clone()
        };
        assert_eq!(
            Registration::parse("again", &registration.to_text()).ok(),
            Some(again)
        );
    }

    #[test]
    fn an_incomplete_file_names_what_it_lacks() {
        let error = Registration::parse("broken", "host=10.0.0.1\n")
            .unwrap_err()
            .to_string();
        assert_eq!(error, "broken: has no kind of ps5 or ps4");
        let error = Registration::parse("broken", "host=10.0.0.1\nkind=ps5\n")
            .unwrap_err()
            .to_string();
        assert_eq!(error, "broken: has no regist_key");
        let error = Registration::parse(
            "broken",
            "host=10.0.0.1\nkind=ps5\nregist_key=zz\nmorning=00\n",
        )
        .unwrap_err()
        .to_string();
        assert_eq!(
            error,
            "broken: has a regist_key that is not 16 bytes of hex"
        );
    }

    #[test]
    fn ids_are_lower_case_words_and_hex_round_trips() {
        assert_eq!(Registration::id("PS5-123"), "ps5-123");
        assert_eq!(Registration::id("  "), "console");
        assert_eq!(hex(&[0x12, 0xab, 0x00]), "12ab00");
        assert_eq!(unhex("12AB00"), None, "a key is 16 bytes, so 32 digits");
        let key = "00ff".repeat(8);
        assert_eq!(hex(&unhex(&key).unwrap_or_default()), key);
        assert_eq!(unhex("12"), None);
    }

    #[test]
    fn a_saved_registration_is_private_and_loads_until_removed() {
        let dir = std::env::temp_dir().join(format!("katplay-store-{}", std::process::id()));
        let store = Store::at(&dir).expect("the folder is writable");
        let registration = Registration::parse("katplay-test", FILE).expect("it parses");
        store.save(&registration).expect("it saves");
        let (loaded, problems) = store.load();
        assert!(problems.is_empty(), "{problems:?}");
        assert!(loaded.contains(&registration));
        let path = dir.join("consoles").join("katplay-test.console");
        let mode = std::fs::metadata(&path)
            .expect("the file exists")
            .permissions()
            .mode()
            & 0o777;
        assert_eq!(mode, 0o600, "only its owner may read it");
        store.remove("katplay-test").expect("it removes");
        assert!(!path.exists());
        assert!(
            store.remove("katplay-test").is_ok(),
            "removing twice is fine"
        );
        std::fs::remove_dir_all(&dir).expect("the test cleans up");
    }

    #[test]
    fn a_file_written_elsewhere_is_forgotten_and_tightened_when_saved_over() {
        let dir = std::env::temp_dir().join(format!("katplay-elsewhere-{}", std::process::id()));
        let store = Store::at(&dir).expect("the folder is writable");
        let path = dir.join("consoles").join("Living Room.console");
        std::fs::write(&path, FILE).expect("the file writes");
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644))
            .expect("the mode sets");
        let (loaded, _) = store.load();
        let name = loaded.first().map(|registration| registration.name.clone());
        assert_eq!(name.as_deref(), Some("Living Room"));
        store.remove("Living Room").expect("it removes");
        assert!(!path.exists(), "forget finds the file load named");

        let saved = dir.join("consoles").join("ps5-123.console");
        std::fs::write(&saved, FILE).expect("the file writes");
        std::fs::set_permissions(&saved, std::fs::Permissions::from_mode(0o644))
            .expect("the mode sets");
        let registration = Registration::parse("PS5-123", FILE).expect("it parses");
        store.save(&registration).expect("it saves");
        let mode = std::fs::metadata(&saved)
            .expect("the file exists")
            .permissions()
            .mode()
            & 0o777;
        assert_eq!(mode, 0o600, "saving over a file makes it private");
        std::fs::remove_dir_all(&dir).expect("the test cleans up");
    }

    #[test]
    fn a_granted_registration_is_named_after_its_kind_and_address() {
        let request = Request {
            host: "192.0.2.10".to_owned(),
            ps5: false,
            psn_account_id: [0; 8],
            pin: 12_345_678,
        };
        let unnamed = Registration::granted(&request, [0; 16], [0; 16], None);
        assert_eq!((unnamed.name.as_str(), unnamed.kind), ("ps4-10", Kind::PS4));
        let named = Registration::granted(&request, [0; 16], [0; 16], Some("Living Room"));
        assert_eq!(named.name, "living-room");
        assert!(named.answers_to("192.0.2.10"));
    }

    #[test]
    fn a_save_replaces_its_address_and_never_overwrites_another_console() {
        let dir = std::env::temp_dir().join(format!("katplay-clash-{}", std::process::id()));
        let store = Store::at(&dir).expect("the folder is writable");
        let request = |host: &str| Request {
            host: host.to_owned(),
            ps5: true,
            psn_account_id: [0; 8],
            pin: 12_345_678,
        };
        let first = Registration::granted(&request("192.0.2.10"), [1; 16], [0; 16], Some("居間"));
        let second = Registration::granted(&request("192.0.2.11"), [2; 16], [0; 16], Some("寝室"));
        let first = store.save(&first).expect("it saves");
        let second = store.save(&second).expect("it saves");
        assert_ne!(first.name, second.name, "two consoles keep two files");

        std::fs::write(dir.join("consoles").join("Living Room.console"), FILE)
            .expect("the file writes");
        let again = Registration::granted(
            &request("192.0.2.10"),
            [3; 16],
            [0; 16],
            Some("Living Room"),
        );
        store.save(&again).expect("it saves");
        let (loaded, _) = store.load();
        let for_first: Vec<&Registration> = loaded
            .iter()
            .filter(|registration| registration.host == "192.0.2.10")
            .collect();
        assert_eq!(for_first.len(), 1, "one registration per address");
        assert_eq!(
            for_first
                .first()
                .map(|registration| registration.regist_key),
            Some([3; 16])
        );
        std::fs::remove_dir_all(&dir).expect("the test cleans up");
    }
}
