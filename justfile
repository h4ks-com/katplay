data := env("XDG_DATA_HOME", env("HOME") + "/.local/share") + "/katplay"

# the chiaki-ng release libchiaki is built from; the one place it is named
export CHIAKI_REF := "v1.10.1"

# where `just chiaki` keeps the checkout, and the Python environment nanopb's generator runs on
export CHIAKI_VENV := env("CHIAKI_VENV", data + "/nanopb")
export CHIAKI_PATH := env("CHIAKI_PATH", data + "/chiaki-ng")
export CHIAKI_PYTHON := env("CHIAKI_PYTHON", CHIAKI_VENV + "/bin/python3")

# libchiaki's bundled curl does not compile against OpenSSL 4, so macOS pins 3
export CHIAKI_OPENSSL := if os() == "macos" { `brew --prefix openssl@3` } else { "" }

# the FFmpeg release katplay links statically, built with only the decoders a console sends
FFMPEG_VERSION := "9.1"
export FFMPEG_PREFIX := env("FFMPEG_PREFIX", data + "/ffmpeg-" + FFMPEG_VERSION)

default:
    @just --list

# build FFmpeg FFMPEG_VERSION into FFMPEG_PREFIX as static libraries, once per release
ffmpeg:
    #!/usr/bin/env bash
    set -euo pipefail
    if [ -f "$FFMPEG_PREFIX/lib/libavcodec.a" ]; then
        echo "FFmpeg {{FFMPEG_VERSION}} is in $FFMPEG_PREFIX"
        exit 0
    fi
    source=$(mktemp -d)
    trap 'rm -rf "$source"' EXIT
    curl --fail --silent --show-error --location \
        "https://ffmpeg.org/releases/ffmpeg-{{FFMPEG_VERSION}}.tar.xz" | tar -xJ -C "$source" --strip-components 1
    hardware=()
    if [ "$(uname)" = Darwin ]; then
        hardware=(--enable-videotoolbox --enable-hwaccel=h264_videotoolbox,hevc_videotoolbox)
    fi
    cd "$source"
    ./configure --prefix="$FFMPEG_PREFIX" --enable-static --disable-shared --enable-pic \
        --disable-everything --disable-autodetect --disable-programs --disable-doc \
        --disable-avdevice --disable-avfilter --disable-swresample --disable-network \
        --enable-decoder=h264,hevc --enable-parser=h264,hevc "${hardware[@]}"
    make -j "$(getconf _NPROCESSORS_ONLN)" install
    echo "FFmpeg {{FFMPEG_VERSION}} is in $FFMPEG_PREFIX"

# clone chiaki-ng at CHIAKI_REF with the submodules libchiaki builds from, once per release
chiaki: ffmpeg
    #!/usr/bin/env bash
    set -euo pipefail
    ready() {
        [ -d "$CHIAKI_PATH/.git" ] \
            && [ "$(git -C "$CHIAKI_PATH" describe --tags --exact-match HEAD 2>/dev/null)" = "$CHIAKI_REF" ] \
            && "$CHIAKI_PYTHON" -c 'import google.protobuf' 2>/dev/null
    }
    if ready; then
        echo "chiaki-ng $CHIAKI_REF is in $CHIAKI_PATH"
        exit 0
    fi
    if [ ! -d "$CHIAKI_PATH/.git" ] \
        || [ "$(git -C "$CHIAKI_PATH" describe --tags --exact-match HEAD 2>/dev/null)" != "$CHIAKI_REF" ]; then
        mkdir -p "$(dirname "$CHIAKI_PATH")"
        rm -rf "$CHIAKI_PATH"
        git -c advice.detachedHead=false clone --depth 1 --branch "$CHIAKI_REF" --quiet \
            https://github.com/chiaki-ng/chiaki-ng.git "$CHIAKI_PATH"
        git -C "$CHIAKI_PATH" submodule update --init --depth 1 --quiet \
            third-party/nanopb third-party/jerasure third-party/gf-complete third-party/curl
    fi
    uv venv --clear --quiet "$CHIAKI_VENV"
    uv pip install --python "$CHIAKI_PYTHON" --quiet protobuf
    echo "chiaki-ng $CHIAKI_REF is in $CHIAKI_PATH"

# clippy with ci's denials, after every change
check: chiaki
    cargo clippy --all-targets -- -D warnings

# apply clippy's and rustfmt's fixes
fix: chiaki
    cargo clippy --fix --allow-dirty --allow-staged --all-targets
    cargo fmt --all

fmt:
    cargo fmt --all

fmt-check:
    cargo fmt --all -- --check

test: chiaki
    cargo test

# licenses, advisories and sources, from deny.toml
deny:
    cargo deny check

typos:
    typos

# cyclomatic complexity, length and parameter count of every function
complexity:
    uvx --from lizard==1.26.0 lizard src build.rs -l rust -C 15 -L 100 -a 8 -w

# copy-paste duplication, from .jscpd.json
dupes:
    npx --yes jscpd@4 src/

# install the git hooks, once per clone
hooks:
    prek install --hook-type pre-commit --hook-type commit-msg

# the gate every commit passes
ci: fmt-check check test deny typos complexity dupes

# scan the network for consoles for a few seconds and print them, like `just discover 5`
discover seconds="3": chiaki
    cargo run --quiet -- discover {{seconds}}

# build katplay and run it, like `just run` for the console list or `just run play PS5-123`
run *arguments: chiaki
    cargo run --quiet -- {{arguments}}

# the version Cargo.toml names
version:
    @sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml | head -1

# fail unless `tag` names the crate version, like v0.1.0
release-check tag:
    test {{quote(tag)}} = "v$(just version)"

# build the release binary for `target` and pack it with the readme and license, like `just package aarch64-apple-darwin`
package target: chiaki
    cargo build --release --locked --target {{quote(target)}}
    tar -czf katplay-{{target}}.tar.gz -C target/{{target}}/release katplay -C "$PWD" README.md LICENSE

# bump the version, pass the gate, commit, tag and push, which starts the release workflow,
# like `just release patch` (or minor, major)
release level:
    #!/usr/bin/env bash
    set -euo pipefail
    test -z "$(git status --porcelain)" || { echo "commit or stash your changes first"; exit 1; }
    test "$(git branch --show-current)" = main || { echo "release from main"; exit 1; }
    IFS=. read -r major minor patch <<< "$(just version)"
    case {{quote(level)}} in
      major) next="$((major + 1)).0.0" ;;
      minor) next="$major.$((minor + 1)).0" ;;
      patch) next="$major.$minor.$((patch + 1))" ;;
      *) echo "say major, minor or patch"; exit 1 ;;
    esac
    current=$(just version)
    perl -pi -e "s/^version = \"\Q$current\E\"/version = \"$next\"/" Cargo.toml
    cargo update --workspace --quiet
    just release-check "v$next"
    git add -A
    just ci
    git commit -m "chore: release $next"
    git tag "v$next"
    git push origin main "v$next"
