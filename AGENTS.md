# katplay: agent guide

PlayStation Remote Play in the terminal: chiaki-ng holds the session, decodes
the video and the audio, and katui draws the pixels over the kitty graphics
protocol.

## Layout

- `src/main.rs`: arguments, the error enum, the `play`, `discover`, `wake`
  and `register` commands
- `src/shell.rs`: the katui `App`: one loop at 60 fps that draws every screen
- `src/library.rs`, `src/register.rs`: the console list and the registration
  form
- `src/play.rs`: a session: wake, connect, show pictures, send the controller
- `src/pad.rs`: the controller made from keys and game controllers
- `src/audio.rs`: the thread that owns the output device, and its queue
- `src/console.rs`: a console, its registration file and the store of them
- `src/psn.rs`: the PSN sign-in link and the account id lookups
- `src/settings.rs`, `src/status.rs`: the settings file and the status row
- `src/chiaki/`: libchiaki behind Rust types: errors, the log, discovery,
  registration, the session and the video and audio decoders
- `chiaki/`: the CMake scope that builds libchiaki from the chiaki-ng
  checkout, and the list of headers bindgen reads

`just chiaki` clones chiaki-ng at `CHIAKI_REF` into the data directory
(`CHIAKI_PATH` overrides it) with the submodules libchiaki builds from, and
makes the Python that nanopb's generator runs on. Every build recipe runs it
first, after `just ffmpeg` builds FFmpeg `FFMPEG_VERSION` as static libraries
with only the decoders a console sends. `build.rs` reads the link line CMake
gives a probe executable, so Rust links what CMake resolved and a new libchiaki
dependency needs no change here. CMake looks only for static libraries, so the
binary runs on a machine that has none of them installed.

`just release patch` (or minor, major) bumps the version, tags it and pushes;
the release workflow then attaches one archive per platform, which Homebrew
and cargo-binstall install. Start the release workflow by hand to build the
archives without publishing them.

katui comes from crates.io. To work against a local katui checkout, add a
`[patch.crates-io]` section to a `.cargo/config.toml` you keep untracked.

## Commands (the justfile is the single source of truth)

- `just check`: clippy with CI's denials; run after every change
- `just ci`: fmt check, clippy, tests, cargo deny, typos, complexity and
  duplication; must pass before any commit
- `just hooks`: install the prek git hooks, once per clone
- `just test`: unit tests
- `just discover <seconds>`: scan the network for consoles and print them
- `just run [<command>]`: build katplay and run it, like `just run` for the
  console list or `just run play <console>`

## The loop

1. Keep the change small; one concern per commit.
2. `just check` after every change, `just ci` before committing.
3. Review every non-trivial change before committing. Fix Critical and
   Important findings first.
4. Commit messages: one conventional line, `<type>: <lowercase imperative
   summary>` with types feat, fix, perf, refactor, test, docs, chore, ci,
   build, revert. No body.

## Rules

- No `unwrap`, `panic!` or `todo!` in non-test code; clippy denies them.
- `unsafe` is denied. The one exception is `src/chiaki/`, the libchiaki
  boundary, and every block there says why it holds. A C callback never
  panics: it hands its data on and returns.
- Errors: `chiaki::Error` for the library's codes, `Error` in `src/main.rs`
  at the top. No `Result<_, String>` seams.
- Cognitive complexity over 20 fails clippy: the function grew a second job,
  split it.
- katplay stays a thin shell. The session, the decoders and the protocol
  belong to libchiaki; pixels, terminal input and presenters belong to katui.
  When katplay needs something katui lacks, add it to katui.
- Unit tests need no terminal, no console and no network.
- `CHIAKI_REF` in the justfile is the one place a libchiaki release is named.
  To move to another release, change it and run `just check`.
- libchiaki is AGPL-3.0-only and katplay links it, so katplay is
  AGPL-3.0-only as well.
- Comments explain why, never what; default to zero. No change history.
- Prose in docs, commits and comments: declarative, terse, no em dashes.
- UI text is imperative and names the action the reader takes.
- Nothing personal in the repo: no real names, handles, emails, local
  machine paths or assistant attribution in code, docs, fixtures, commit
  messages or CI.
- Do not weaken a gate to make it pass. Fix the code.

## Secrets

The repo holds none. Keep it that way. A console's registration is a
credential: it never belongs in the repo, a log or a test fixture.
