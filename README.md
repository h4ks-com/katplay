<h1 align="center">katplay</h1>

<p align="center">
katplay is a PlayStation Remote Play client that streams your PS5 or PS4 into a terminal window.
</p>

<p align="center">
<a href="#install">Install</a> · <a href="#first-use">First use</a> · <a href="#controls">Controls</a>
</p>

katplay finds the consoles on your network, wakes them from rest mode and lets
you play them with sound, using a game controller or the keyboard. It shows the
picture as real pixels in kitty, Ghostty, WezTerm and iTerm2, as sixel in other
terminals that support sixel, and as colored blocks in every other terminal.
katplay uses libchiaki from [chiaki-ng](https://github.com/chiaki-ng/chiaki-ng)
to connect to the console and decode the video and audio, and
[katui](https://github.com/h4ks-com/katui) to draw the picture.

```console
$ katplay discover 3        # list the consoles that answer within 3 seconds
192.168.1.20     PS5-123                  PS5  ready
192.168.1.21     PS4-Den                  PS4  in rest
```

## Install

katplay runs on macOS 11 or newer and on Linux with glibc 2.35 or newer, on
x86_64 and arm64. Install it with Homebrew:

```sh
brew install h4ks-com/tap/katplay
```

<details>
<summary><b>cargo-binstall</b></summary>

```sh
cargo binstall --git https://github.com/h4ks-com/katplay katplay
```

</details>

Then run `katplay`.

## First use

`katplay` opens the list of consoles on your network. Select a console with
the arrow keys and press <kbd>Enter</kbd>.

If you have not paired the console yet, katplay opens the registration form.
On the console, open Settings, then Remote Play, then Link Device. Type the
address and the 8-digit code that the console shows into the form. Then tell
katplay which PSN account to register, in one of two ways: type the account's
public online id, or sign in to PSN in a browser and paste the address of the
page that PSN shows after you sign in. The console remembers the pairing, so
you only register each console once.

In the list, press <kbd>w</kbd> to wake a console from rest mode,
<kbd>x</kbd> to forget a console, and <kbd>Tab</kbd> to change the video
quality. If you start playing a console that is in rest mode, katplay wakes it
first.

## Controls

You can play with a game controller, with the keyboard, or with both at once.
The buttons on your controller do the same thing they do on the PlayStation's
own controller. Most operating systems take the PS button for themselves, so
to press PS, hold Share and Options together. In the menus, move with the
D-pad or the left stick, select with the bottom face button and go back with
the right face button.

On the keyboard, these keys press these controller buttons:

| Keys | Controller |
| --- | --- |
| `w` `a` `s` `d` | left stick |
| arrow keys | right stick |
| `Shift` + arrow keys | D-pad |
| `i` `j` `k` `l` | triangle, square, cross, circle, in the same layout as on the controller |
| `e` `u` | L1, R1 |
| `r` `o` | L2, R2 |
| `c` `n` | L3, R3 |
| `v` `b` `p` `m` | Share, Options, PS, touchpad |

Press <kbd>Esc</kbd> to open a menu where you can leave the session or put the
console into rest mode. Press <kbd>Ctrl</kbd>+<kbd>C</kbd> to quit.

## From the command line

You can also run each step on its own, for example in a script or a shortcut.
For `<console>`, use the name or the address that `discover` prints.

```sh
katplay discover 5          # list the consoles that answer within 5 seconds
katplay wake <console>      # wake a registered console from rest mode
katplay play <console>      # start playing a console right away
katplay register --host <address> --pin <code> --username <online id>   # pair a console
```

To name the PSN account in `register`, you can use `--link <url>` with the
address of the page PSN shows after you sign in, or `--account-id <id>` with
the account id in base64, in place of `--username`. Add `--name <name>` to
give the console a name, and `--ps4` to register a PS4.

## Build from source

To build katplay yourself, you need Rust 1.90 or newer, which you can install
with [rustup](https://rustup.rs), and [just](https://github.com/casey/just), the
command runner that katplay uses for its build steps.

On macOS, first install Xcode's command line tools with
`xcode-select --install`. Then install the build tools and libraries with
Homebrew:

```sh
brew install just uv cmake ninja git openssl@3 json-c miniupnpc libevent opus nasm protobuf
```

<details>
<summary><b>Debian and Ubuntu</b></summary>

```sh
sudo apt install just build-essential curl xz-utils cmake ninja-build git pkg-config libclang-dev clang \
  libssl-dev libjson-c-dev libminiupnpc-dev libevent-dev libopus-dev \
  nasm libasound2-dev libudev-dev protobuf-compiler
```

Then install [uv](https://docs.astral.sh/uv/getting-started/installation/)
with its installer. If your release has no `just` package, run
`cargo install just`.

</details>

Then clone katplay, build it and run it:

```sh
git clone https://github.com/h4ks-com/katplay
cd katplay
just run
```

The first run downloads chiaki-ng and FFmpeg into `~/.local/share/katplay` and
builds them, which takes a few minutes. After that, katplay starts right away.
To run one of the commands above, put it after `just run`, like
`just run discover 5`.

## License

AGPL-3.0-only, like libchiaki.
