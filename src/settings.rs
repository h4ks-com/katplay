//! What katplay remembers between runs, in one TOML file in the data
//! directory.

use std::path::{Path, PathBuf};
use std::{env, fmt, fs};

use serde::{Deserialize, Serialize};

use crate::chiaki::session::{Codec, FrameRate, Resolution, VideoProfile};
use crate::{Error, io_error};

/// Where katplay keeps its files: `katplay` under the XDG data directory.
pub(crate) fn data_dir() -> PathBuf {
    let base = match env::var_os("XDG_DATA_HOME").filter(|value| !value.is_empty()) {
        Some(data) => PathBuf::from(data),
        None => env::var_os("HOME").map_or_else(
            || PathBuf::from("."),
            |home| PathBuf::from(home).join(".local/share"),
        ),
    };
    base.join("katplay")
}

/// The video the console is asked for, by name.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub(crate) enum Quality {
    #[serde(rename = "360p60")]
    P360,
    #[serde(rename = "540p60")]
    P540,
    #[serde(rename = "720p60")]
    #[default]
    P720,
    #[serde(rename = "1080p60")]
    P1080,
    #[serde(rename = "1080p30")]
    P1080Slow,
}

impl Quality {
    pub(crate) const ALL: [Self; 5] = [
        Self::P360,
        Self::P540,
        Self::P720,
        Self::P1080,
        Self::P1080Slow,
    ];

    pub(crate) fn next(self) -> Self {
        let at = Self::ALL
            .iter()
            .position(|quality| *quality == self)
            .unwrap_or(0);
        Self::ALL
            .get((at + 1) % Self::ALL.len())
            .copied()
            .unwrap_or(self)
    }

    pub(crate) fn video(self, codec: Codec) -> VideoProfile {
        match self {
            Self::P360 => VideoProfile::preset(Resolution::P360, FrameRate::Fps60, codec),
            Self::P540 => VideoProfile::preset(Resolution::P540, FrameRate::Fps60, codec),
            Self::P720 => VideoProfile::preset(Resolution::P720, FrameRate::Fps60, codec),
            Self::P1080 => VideoProfile::preset(Resolution::P1080, FrameRate::Fps60, codec),
            Self::P1080Slow => VideoProfile::preset(Resolution::P1080, FrameRate::Fps30, codec),
        }
    }
}

impl fmt::Display for Quality {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::P360 => "360p60",
            Self::P540 => "540p60",
            Self::P720 => "720p60",
            Self::P1080 => "1080p60",
            Self::P1080Slow => "1080p30",
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub(crate) enum Encoding {
    #[default]
    H264,
    H265,
    H265Hdr,
}

impl Encoding {
    fn codec(self) -> Codec {
        match self {
            Self::H264 => Codec::H264,
            Self::H265 => Codec::H265,
            Self::H265Hdr => Codec::H265Hdr,
        }
    }
}

/// Whether the console's video is decoded by the hardware decoder of this
/// machine, when it has one.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub(crate) enum Decode {
    #[default]
    Auto,
    Software,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(default, deny_unknown_fields)]
pub(crate) struct Settings {
    pub quality: Quality,
    pub encoding: Encoding,
    pub decode: Decode,
}

impl Settings {
    /// The settings in the file `from`, or the defaults when it is missing.
    pub(crate) fn load(from: &Path) -> Result<Self, Error> {
        let text = match fs::read_to_string(from) {
            Ok(text) => text,
            Err(source) if source.kind() == std::io::ErrorKind::NotFound => {
                return Ok(Self::default());
            }
            Err(source) => return Err(io_error(from, source)),
        };
        toml::from_str(&text).map_err(|source| Error::Settings {
            path: from.to_path_buf(),
            reason: source.to_string(),
        })
    }

    /// Write the settings to `to`.
    pub(crate) fn save(self, to: &Path) -> Result<(), Error> {
        let text = toml::to_string(&self).map_err(|source| Error::Settings {
            path: to.to_path_buf(),
            reason: source.to_string(),
        })?;
        if let Some(parent) = to.parent() {
            fs::create_dir_all(parent).map_err(|source| io_error(to, source))?;
        }
        fs::write(to, text.as_bytes()).map_err(|source| io_error(to, source))
    }

    pub(crate) fn video(self) -> VideoProfile {
        self.quality.video(self.encoding.codec())
    }

    /// The hardware decoder to ask for, by the name `FFmpeg` gives it.
    pub(crate) fn hardware_decoder(self) -> Option<&'static str> {
        if self.decode == Decode::Software {
            return None;
        }
        Some(if cfg!(target_os = "macos") {
            "videotoolbox"
        } else {
            "vaapi"
        })
    }
}

#[cfg(test)]
mod tests {
    use super::{Encoding, Quality, Settings};

    #[test]
    fn settings_round_trip_and_refuse_what_they_do_not_know() {
        let defaults = Settings::default();
        let path =
            std::env::temp_dir().join(format!("katplay-settings-{}.toml", std::process::id()));
        assert!(
            Settings::load(&path).is_ok_and(|loaded| loaded == defaults),
            "a missing file is the defaults"
        );
        let text = toml::to_string(&defaults).expect("settings serialize");
        assert_eq!(
            text,
            "quality = \"720p60\"\nencoding = \"h264\"\ndecode = \"auto\"\n"
        );
        toml::from_str::<Settings>(&text).expect("they parse back");
        assert!(
            toml::from_str::<Settings>("quality = \"4k\"\n").is_err(),
            "an unknown quality is refused"
        );
        assert!(
            toml::from_str::<Settings>("").is_ok_and(|loaded| loaded == defaults),
            "an empty file is the defaults"
        );
        let custom = toml::from_str::<Settings>("decode = \"software\"\nquality = \"1080p30\"\n")
            .expect("known keys");
        assert!(custom.hardware_decoder().is_none());
        custom.save(&path).expect("the file is written");
        assert!(
            Settings::load(&path).is_ok_and(|loaded| loaded == custom),
            "what was saved loads again"
        );
        std::fs::remove_file(&path).expect("the test cleans up");
        assert_eq!(Quality::P1080.next(), Quality::P1080Slow);
        assert_eq!(Quality::P1080Slow.next(), Quality::P360, "the list wraps");
        assert_eq!(Quality::P720.video(Encoding::H265.codec()).width, 1280);
        assert_eq!(custom.video().max_fps, 30);
    }
}
