//! Arguments, the error enum and the wiring between the console and the terminal.

#[allow(unsafe_code)]
mod chiaki;

mod audio;
mod console;
mod governor;
mod library;
mod pad;
mod play;
mod psn;
mod register;
mod settings;
mod shell;
mod status;

use std::fmt::Display;
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::sync::{Arc, Mutex, mpsc};
use std::thread;
use std::time::Duration;
use std::{env, io, io::Write};

use katui::rt::app::run_app;
use katui::rt::tty::probe_default;

use crate::chiaki::Log;
use crate::chiaki::discovery::Discovery;
use crate::chiaki::registration::{Outcome, Registration as Pairing, Request};
use crate::console::{Registration, Store};
use crate::settings::Settings;
use crate::shell::{FRAMES_PER_SECOND, Shell};

const DEFAULT_SCAN_SECONDS: u64 = 3;
/// How long a link code stays valid on the console.
const REGISTRATION_TIME: Duration = Duration::from_secs(90);
const USAGE: &str = "katplay [play <console> | discover [<seconds>] | wake <console> | register --host <address> --pin <code> (--link <url> | --username <id> | --account-id <id>) [--name <name>] [--ps4]]";

#[derive(Debug, thiserror::Error)]
pub(crate) enum Error {
    #[error(transparent)]
    Chiaki(#[from] chiaki::Error),
    #[error(transparent)]
    Terminal(#[from] katui::rt::RtError),
    #[error("{path}: {source}")]
    Io {
        path: PathBuf,
        source: std::io::Error,
    },
    #[error("{name}: {reason}")]
    Credentials { name: String, reason: &'static str },
    #[error("{path}: {reason}")]
    Settings { path: PathBuf, reason: String },
    #[error("psn: {0}")]
    Psn(String),
    #[error("audio: {0}")]
    Audio(String),
    #[error("the picture {0}")]
    Picture(String),
    #[error("{0}")]
    Console(String),
    #[error("no console is registered as {0}: press r in the list to register it")]
    NotRegistered(String),
    #[error(
        "the console refused the registration: open Link Device again for a fresh code and check the account"
    )]
    Refused,
    #[error("the console did not answer the registration in time: open Link Device and try again")]
    NoAnswer,
    #[error("usage: {0}")]
    Usage(&'static str),
}

pub(crate) fn io_error(path: &Path, source: std::io::Error) -> Error {
    Error::Io {
        path: path.to_path_buf(),
        source,
    }
}

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => report(&error),
    }
}

fn run() -> Result<(), Error> {
    chiaki::init()?;
    let mut arguments = env::args().skip(1);
    let command = arguments.next().unwrap_or_default();
    let rest: Vec<String> = arguments.collect();
    match command.as_str() {
        "" => open(None),
        "play" => open(rest.first().cloned()),
        "discover" => scan(rest.first().map(String::as_str)),
        "wake" => wake(rest.first().map(String::as_str)),
        "register" => pair(&rest),
        _ => Err(Error::Usage(USAGE)),
    }
}

/// Open the console list, or play `console` straight away.
fn open(console: Option<String>) -> Result<(), Error> {
    let caps = probe_default();
    let store = Store::open()?;
    let settings = Settings::load(&store.settings_path())?;
    let (registrations, problems) = store.load();
    let mut shell = Shell::new(caps.clone(), store, settings, registrations, false)?;
    if let Some(problem) = problems.first() {
        shell.notice(problem.to_string());
    }
    if let Some(console) = console {
        shell.play(&console);
    }
    run_app(&mut shell, io::stdout(), &caps, FRAMES_PER_SECOND)?;
    Ok(())
}

#[expect(
    clippy::print_stdout,
    reason = "discover prints what it found for people and scripts; the app itself only draws"
)]
fn scan(seconds: Option<&str>) -> Result<(), Error> {
    let seconds = match seconds {
        None => DEFAULT_SCAN_SECONDS,
        Some(text) => text.parse().map_err(|_| Error::Usage(USAGE))?,
    };
    let found = Arc::new(Mutex::new(Vec::new()));
    let reported = Arc::clone(&found);
    let log = Arc::new(Log::stderr());
    let discovery = Discovery::start(Arc::clone(&log), move |consoles| {
        if let Ok(mut reported) = reported.lock() {
            *reported = consoles;
        }
    })?;
    thread::sleep(Duration::from_secs(seconds));
    drop(discovery);

    let mut consoles = found.lock().map(|found| found.clone()).unwrap_or_default();
    consoles.sort_by(|a, b| a.name.cmp(&b.name));
    for console in &consoles {
        println!(
            "{:<16} {:<24} {:<4} {:<10} {}",
            console.address,
            console.name,
            console.kind,
            console.state,
            console.running_app.as_deref().unwrap_or_default()
        );
    }
    Ok(())
}

fn wake(name: Option<&str>) -> Result<(), Error> {
    let name = name.ok_or(Error::Usage(USAGE))?;
    let store = Store::open()?;
    let (registrations, problems) = store.load();
    let registration = registrations
        .iter()
        .find(|registration| registration.answers_to(name))
        .ok_or_else(|| {
            problems
                .into_iter()
                .next()
                .unwrap_or_else(|| Error::NotRegistered(name.to_owned()))
        })?;
    registration.wake(&Log::stderr())?;
    say(format!(
        "Asked {} to wake up; give it a moment before playing",
        registration.name
    ));
    Ok(())
}

fn pair(arguments: &[String]) -> Result<(), Error> {
    let request = pairing_request(arguments)?;
    let (regist_key, morning) = wait_for_pairing(&request)?;
    let registration = Registration::granted(
        &request,
        regist_key,
        morning,
        flag_value(arguments, "--name"),
    );
    let registration = Store::open()?.save(&registration)?;
    say(format!(
        "Registered {}. Start it with: katplay play {}",
        registration.name, registration.name
    ));
    Ok(())
}

fn flag_value<'a>(arguments: &'a [String], flag: &str) -> Option<&'a str> {
    arguments
        .iter()
        .position(|argument| argument == flag)
        .and_then(|at| arguments.get(at + 1))
        .map(String::as_str)
}

fn pairing_request(arguments: &[String]) -> Result<Request, Error> {
    let value = |flag| flag_value(arguments, flag);
    let host = value("--host").ok_or(Error::Usage(USAGE))?.to_owned();
    let pin = value("--pin")
        .and_then(register::eight_digits)
        .ok_or(Error::Usage(USAGE))?;
    let psn_account_id = match (value("--link"), value("--username"), value("--account-id")) {
        (Some(link), None, _) => psn::account_id_from_redirect(link)?,
        (None, Some(name), _) => psn::account_id_of(name)?,
        (None, None, Some(id)) => psn::decode_account_id(id).ok_or(Error::Usage(USAGE))?,
        _ => return Err(Error::Usage(USAGE)),
    };
    Ok(Request {
        host,
        ps5: !arguments.iter().any(|argument| argument == "--ps4"),
        psn_account_id,
        pin,
    })
}

fn wait_for_pairing(request: &Request) -> Result<([u8; 16], [u8; 16]), Error> {
    let (report, reported) = mpsc::channel();
    let pairing = Pairing::start(request, Arc::new(Log::stderr()), move |outcome| {
        // The receiver is gone once we stopped waiting, and then nobody needs the outcome.
        report.send(outcome).ok();
    })?;
    let outcome = reported.recv_timeout(REGISTRATION_TIME);
    // Dropping a registration that is still running cancels it.
    drop(pairing);
    match outcome {
        Ok(Outcome::Registered {
            regist_key,
            morning,
        }) => Ok((regist_key, morning)),
        Ok(Outcome::Failed) => Err(Error::Refused),
        _ => Err(Error::NoAnswer),
    }
}

#[expect(
    clippy::print_stdout,
    reason = "the commands report how they went for people and scripts; the app itself only draws"
)]
fn say(text: impl Display) {
    println!("{text}");
}

fn report(error: &Error) -> ExitCode {
    io::stderr()
        .write_all(format!("katplay: {error}\n").as_bytes())
        .ok();
    ExitCode::FAILURE
}
