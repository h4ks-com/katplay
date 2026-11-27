//! Builds libchiaki from the chiaki-ng checkout the justfile fetches, binds
//! the parts of it katplay calls, and links what CMake resolved for it.

use std::error::Error;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::{env, io};

type Result<T> = std::result::Result<T, Box<dyn Error>>;

const PROBE: &str = "katplay_chiaki_probe";

fn main() -> Result<()> {
    for variable in [
        "CHIAKI_PATH",
        "CHIAKI_PYTHON",
        "CHIAKI_OPENSSL",
        "FFMPEG_PREFIX",
    ] {
        println!("cargo::rerun-if-env-changed={variable}");
    }
    // bindgen names only the headers it read, which turns off cargo's rerun on any change, so we
    // name the CMake scope ourselves.
    println!("cargo::rerun-if-changed=chiaki");
    let out = PathBuf::from(env::var_os("OUT_DIR").ok_or("OUT_DIR is unset")?);
    let hint = "is unset: run `just chiaki` first";
    let source = PathBuf::from(env::var_os("CHIAKI_PATH").ok_or(format!("CHIAKI_PATH {hint}"))?);
    let python = env::var_os("CHIAKI_PYTHON").ok_or(format!("CHIAKI_PYTHON {hint}"))?;
    let ffmpeg = env::var_os("FFMPEG_PREFIX").ok_or(format!("FFMPEG_PREFIX {hint}"))?;

    let mut cmake = cmake::Config::new("chiaki");
    cmake
        .generator("Ninja")
        .profile("Release")
        .define("CHIAKI_PATH", &source)
        .define("PYTHON_EXECUTABLE", python)
        .define("FFMPEG_PREFIX", &ffmpeg);
    if let Some(openssl) = env::var_os("CHIAKI_OPENSSL").filter(|value| !value.is_empty()) {
        cmake.define("OPENSSL_ROOT_DIR", openssl);
    }
    let prefix = cmake.build();
    let build = find(&prefix, "build.ninja")?
        .parent()
        .ok_or("build.ninja sits in no directory")?
        .to_path_buf();

    bind(&source, Path::new(&ffmpeg), &build, &out)?;
    link(&build)
}

fn bind(source: &Path, ffmpeg: &Path, build: &Path, out: &Path) -> Result<()> {
    let config = find(build, "include/chiaki/config.h")?;
    let include = config
        .parent()
        .and_then(Path::parent)
        .ok_or("chiaki's config.h sits in no include directory")?;

    bindgen::Builder::default()
        .header("chiaki/bindings.h")
        .clang_arg(format!("-I{}", source.join("lib/include").display()))
        .clang_arg(format!("-I{}", include.display()))
        .clang_arg(format!("-I{}", ffmpeg.join("include").display()))
        .clang_arg(format!(
            "-I{}",
            source.join("third-party/curl/include").display()
        ))
        .size_t_is_usize(true)
        .layout_tests(false)
        .generate_comments(false)
        .parse_callbacks(Box::new(bindgen::CargoCallbacks::new()))
        .prepend_enum_name(false)
        .allowlist_type(r"^Chiaki\w*$|^chiaki_\w+$")
        .allowlist_function("^chiaki_.*")
        .allowlist_var("^CHIAKI_.*")
        .allowlist_type("^AVFrame$|^AVRational$|^AVPixelFormat$|^SwsContext$")
        .allowlist_function(
            "^av_frame_(alloc|free|unref)$|^av_hwframe_transfer_data$|^sws_(getContext|scale|freeContext)$",
        )
        .allowlist_var(r"^AV_PIX_FMT_\w+$|^SWS_BILINEAR$")
        .generate()
        .map_err(|_| io::Error::other("bindgen could not read chiaki's headers"))?
        .write_to_file(out.join("chiaki.rs"))?;
    Ok(())
}

enum Argument {
    Skip,
    SkipValue,
    Flag(String),
    FlagValue(String),
}

/// Prints one `cargo::rustc-link-arg` per item of the link line CMake gave the
/// probe, so katplay links libchiaki with exactly the libraries CMake resolved
/// for it, in that order.
fn link(build: &Path) -> Result<()> {
    let commands = Command::new("ninja")
        .arg("-C")
        .arg(build)
        .args(["-t", "commands", PROBE])
        .output()?;
    if !commands.status.success() {
        return Err(format!("ninja could not list the commands that build {PROBE}").into());
    }
    let text = String::from_utf8_lossy(&commands.stdout);
    let line = text
        .lines()
        .rfind(|line| line.contains(&format!("-o {PROBE}")))
        .ok_or_else(|| format!("ninja listed no link command for {PROBE}"))?;
    let tokens = line
        .split_whitespace()
        .filter(|token| !matches!(*token, ":" | "&&"))
        .skip(1)
        .map(str::to_owned)
        .collect::<Vec<_>>();
    let tokens = expand(build, tokens)?;

    let mut emit_next = false;
    let mut skip_next = false;
    for token in &tokens {
        if skip_next {
            skip_next = false;
            continue;
        }
        if emit_next {
            emit_next = false;
            println!("cargo::rustc-link-arg={token}");
            continue;
        }
        match classify(build, token) {
            Argument::Skip => {}
            Argument::SkipValue => skip_next = true,
            Argument::Flag(flag) => println!("cargo::rustc-link-arg={flag}"),
            Argument::FlagValue(flag) => {
                println!("cargo::rustc-link-arg={flag}");
                emit_next = true;
            }
        }
    }
    Ok(())
}

fn classify(build: &Path, token: &str) -> Argument {
    let probe_only = [
        "-mmacosx-version-min",
        "-O",
        "-D",
        "-g",
        "-std",
        "-w",
        "-ffunction-sections",
        "-fdata-sections",
        "-fPIC",
        "--target",
        "-Wl,--dependency-file",
        "-pthread",
        "-Wl,-rpath",
    ]
    .iter()
    .any(|prefix| token.starts_with(prefix));
    let object = Path::new(token)
        .extension()
        .is_some_and(|extension| extension == "o");
    match token {
        _ if object || probe_only => Argument::Skip,
        "-o" | "-arch" | "-isysroot" => Argument::SkipValue,
        "-framework" | "-weak_framework" | "-reexport_framework" => {
            Argument::FlagValue(token.into())
        }
        flag if flag.starts_with('-') => Argument::Flag(flag.into()),
        // ninja runs in the build directory, so its paths are relative to it
        file if build.join(file).is_file() => {
            Argument::Flag(build.join(file).display().to_string())
        }
        library => Argument::Flag(format!("-l{library}")),
    }
}

/// Replaces every `@file` with the tokens the file holds.
fn expand(build: &Path, tokens: Vec<String>) -> Result<Vec<String>> {
    let mut expanded = Vec::with_capacity(tokens.len());
    for token in tokens {
        match token.strip_prefix('@') {
            Some(file) => {
                let contents = fs::read_to_string(build.join(file))?;
                expanded.extend(contents.split_whitespace().map(str::to_owned));
            }
            None => expanded.push(token),
        }
    }
    Ok(expanded)
}

/// The first file below `root` whose path ends in `suffix`.
fn find(root: &Path, suffix: &str) -> Result<PathBuf> {
    fn visit(dir: &Path, suffix: &str) -> Option<PathBuf> {
        let entries = fs::read_dir(dir).ok()?;
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                if let Some(found) = visit(&path, suffix) {
                    return Some(found);
                }
            } else if path.to_string_lossy().ends_with(suffix) {
                return Some(path);
            }
        }
        None
    }
    visit(root, suffix)
        .ok_or_else(|| format!("no file under {} ends in {suffix}", root.display()).into())
}
