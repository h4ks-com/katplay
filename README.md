<h1 align="center">katplay</h1>

<p align="center">
katplay is a PlayStation Remote Play client that streams your PS5 or PS4 into a terminal window.
</p>

<p align="center">
<a href="#install">Install</a> · <a href="#first-use">First use</a> · <a href="#controls">Controls</a>
</p>

katplay finds the consoles on your network, wakes them from rest and plays
them with sound, from a game controller or the keyboard. The picture is real
pixels in kitty, Ghostty, WezTerm and iTerm2, sixel in terminals that support
it, and colored blocks everywhere else.
[chiaki-ng](https://github.com/chiaki-ng/chiaki-ng)'s libchiaki handles the
connection, video and audio, and [katui](https://github.com/h4ks-com/katui)
draws the picture.

```console
$ just run discover 3        # list the consoles that answer within 3 seconds
192.168.1.20     PS5-123                  PS5  ready
192.168.1.21     PS4-Den                  PS4  in rest
```

## Install

katplay runs on macOS and Linux and builds from source. You need Rust 1.90 or
newer, which [rustup](https://rustup.rs) installs, and
[just](https://github.com/casey/just), the command runner the build steps live
in.

On macOS, install Xcode's command line tools with `xcode-select --install`,
then the build tools and the libraries katplay links with Homebrew:

```sh
brew install just uv cmake ninja git openssl@3 json-c miniupnpc libevent opus ffmpeg protobuf
```

<details>
<summary><b>Debian and Ubuntu</b></summary>

```sh
sudo apt install just cmake ninja-build git pkg-config libclang-dev clang \
  libssl-dev libjson-c-dev libminiupnpc-dev libevent-dev libopus-dev \
  libavcodec-dev libavutil-dev libswscale-dev libasound2-dev libudev-dev protobuf-compiler
```

Then install [uv](https://docs.astral.sh/uv/getting-started/installation/)
with its installer. On a release that has no `just` package, run
`cargo install just`.

</details>

Then clone katplay and run it:

```sh
git clone https://github.com/h4ks-com/katplay
cd katplay
just run
```

The first run downloads chiaki-ng into `~/.local/share/katplay` and builds
libchiaki from it, which takes a few minutes. Later runs start right away.

## First use

`just run` opens the list of consoles on your network. Pick one with the arrow
keys and press <kbd>Enter</kbd>.

Picking a console you have not paired yet opens the registration form. On the
console, open Settings, then Remote Play, then Link Device, and type the
address and the 8-digit code it shows. Then name the PSN account to register in
one of two ways: type the account's public online id, or sign in to PSN in a
browser and paste the address of the page the sign-in ends on. The console
keeps the pairing, so you register each console once.

In the list, press <kbd>w</kbd> to wake a console from rest, <kbd>x</kbd> to
forget one, and <kbd>Tab</kbd> to change the video quality. A console in rest
mode also wakes on its own when you start playing it.

## Controls

Play with a game controller, the keyboard, or both at once. A controller's
buttons work as they do on the console. Most systems keep the PS button for
themselves, so hold Share and Options together to press PS. In the menus, move
with the D-pad or the left stick, pick with the bottom face button and go back
with the right one.

On the keyboard:

| Keys | Controller |
| --- | --- |
| `w` `a` `s` `d` | left stick |
| arrow keys | right stick |
| `Shift` + arrow keys | D-pad |
| `i` `j` `k` `l` | triangle, square, cross, circle, laid out as on the pad |
| `e` `u` | L1, R1 |
| `r` `o` | L2, R2 |
| `c` `n` | L3, R3 |
| `v` `b` `p` `m` | Share, Options, PS, touchpad |

Press <kbd>Esc</kbd> to open the menu that leaves the session or puts the
console to rest, and <kbd>Ctrl</kbd>+<kbd>C</kbd> to quit.

## From the command line

Each of these does one job on its own, for scripts and shortcuts. A
`<console>` is the name or the address that `discover` prints.

```sh
just run discover 5          # list the consoles that answer within 5 seconds
just run wake <console>      # wake a registered console from rest
just run play <console>      # start playing a console right away
just run register --host <address> --pin <code> --username <online id>   # pair a console
```

`register` also takes `--link <url>` (the sign-in page address) or
`--account-id <id>` (the account id in base64) in place of `--username`,
`--name <name>` to name the console, and `--ps4` for a PS4.

## License

AGPL-3.0-only, like libchiaki.
