# katplay

katplay is PlayStation Remote Play for the terminal, built on
[chiaki-ng](https://github.com/chiaki-ng/chiaki-ng) and
[katui](https://github.com/h4ks-com/katui). chiaki-ng speaks to the console,
and katui draws the picture as real pixels in kitty, Ghostty, WezTerm and
iTerm2, as sixel in terminals that have it, and as colored blocks everywhere
else.

## Build

You need Rust 1.90 or newer, and `just`, `uv`, CMake, Ninja and Git on your
`PATH`.

The libraries libchiaki links come from your package manager. On macOS:

```sh
brew install just uv cmake ninja git openssl@3 json-c miniupnpc libevent opus ffmpeg protobuf
```

On Debian and Ubuntu:

```sh
sudo apt install just cmake ninja-build git pkg-config libclang-dev clang \
  libssl-dev libjson-c-dev libminiupnpc-dev libevent-dev libopus-dev \
  libavcodec-dev libavutil-dev libswscale-dev libasound2-dev protobuf-compiler
```

Then, in a clone of this repository:

```sh
just run
```

The first run clones chiaki-ng into `~/.local/share/katplay` and builds its
protocol library, which takes a few minutes.

## Use

`just run` lists the consoles on your network. Pick one with the arrow keys
and press <kbd>Enter</kbd> to play it. A console you have not paired yet opens
the registration form: on the console open Settings, Remote Play, Link
Device, and type the address and the code it shows. Press <kbd>r</kbd> to
register, <kbd>w</kbd> to wake a console in rest mode, <kbd>x</kbd> to forget
one, and <kbd>Tab</kbd> to change the quality.

While you play, the keyboard is the controller:

| Keys | Controller |
| --- | --- |
| <kbd>W</kbd> <kbd>A</kbd> <kbd>S</kbd> <kbd>D</kbd> | left stick |
| arrow keys | right stick |
| <kbd>Shift</kbd> + arrow keys | direction pad |
| <kbd>I</kbd> <kbd>J</kbd> <kbd>K</kbd> <kbd>L</kbd> | triangle, square, cross, circle |
| <kbd>E</kbd> <kbd>U</kbd> | L1, R1 |
| <kbd>R</kbd> <kbd>O</kbd> | L2, R2 |
| <kbd>C</kbd> <kbd>N</kbd> | L3, R3 |
| <kbd>V</kbd> <kbd>B</kbd> <kbd>P</kbd> <kbd>M</kbd> | share, options, PS, touchpad |

A game controller works too. In the menus the direction pad moves, the
bottom face button picks and the right one goes back. Most systems keep a
controller's PS button for themselves, so hold Share and Options together to
press PS. Press <kbd>Esc</kbd> to leave the session and
<kbd>Ctrl</kbd>+<kbd>C</kbd> to quit.

The same works from the command line:

```sh
just run discover 5
just run wake <console>
just run play <console>
just run register --host <address> --pin <code> --username <online id>
```

## Status

katplay is early. Discovery works. Playing a session is written and untested
against a console.

## License

AGPL-3.0-only. katplay links libchiaki, which is AGPL-3.0-only too.
