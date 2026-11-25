data := env("XDG_DATA_HOME", env("HOME") + "/.local/share") + "/katplay"

# the chiaki-ng release libchiaki is built from; the one place it is named
export CHIAKI_REF := "v1.10.1"

# where `just chiaki` keeps the checkout, and the Python environment nanopb's generator runs on
export CHIAKI_VENV := env("CHIAKI_VENV", data + "/nanopb")
export CHIAKI_PATH := env("CHIAKI_PATH", data + "/chiaki-ng")
export CHIAKI_PYTHON := env("CHIAKI_PYTHON", CHIAKI_VENV + "/bin/python3")

# libchiaki's bundled curl does not compile against OpenSSL 4, so macOS pins 3
export CHIAKI_OPENSSL := if os() == "macos" { `brew --prefix openssl@3` } else { "" }

default:
    @just --list

# clone chiaki-ng at CHIAKI_REF with the submodules libchiaki builds from, once per release
chiaki:
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
