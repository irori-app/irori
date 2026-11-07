//! The operating system and CPU an engine build is for, and the keys
//! manifests use for them (`linux-x86_64`, `macos-aarch64`, `windows-x86_64`).

use std::fmt;

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Os {
    Linux,
    Macos,
    Windows,
}

impl Os {
    pub fn current() -> Self {
        if cfg!(target_os = "macos") {
            Os::Macos
        } else if cfg!(windows) {
            Os::Windows
        } else {
            Os::Linux
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Os::Linux => "linux",
            Os::Macos => "macos",
            Os::Windows => "windows",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Host {
    pub os: Os,
    /// `x86_64` or `aarch64`.
    pub arch: &'static str,
}

impl Host {
    pub fn current() -> Self {
        Self {
            os: Os::current(),
            arch: if cfg!(target_arch = "aarch64") {
                "aarch64"
            } else {
                "x86_64"
            },
        }
    }

    /// The manifest key of this host.
    pub fn key(self) -> String {
        format!("{}-{}", self.os.as_str(), self.arch)
    }

    /// The host whose builds this one runs when a project publishes none
    /// for it: Windows on Arm runs x64 programs through its own emulation,
    /// and most emulators (and every libretro core) ship for x64 only.
    pub fn fallback(self) -> Option<Host> {
        (self.os == Os::Windows && self.arch == "aarch64").then_some(Host {
            os: Os::Windows,
            arch: "x86_64",
        })
    }
}

/// A readable name for a manifest host key: `linux-x86_64` is
/// "Linux (x86-64)". Product names aren't translated; an unknown key is
/// shown as it is.
pub fn label(key: &str) -> String {
    let Some((os, arch)) = key.split_once('-') else {
        return key.to_string();
    };
    let os = match os {
        "linux" => "Linux",
        "macos" => "macOS",
        "windows" => "Windows",
        _ => return key.to_string(),
    };
    let arch = match arch {
        "x86_64" => "x86-64",
        "aarch64" => "Arm64",
        other => other,
    };
    format!("{os} ({arch})")
}

impl fmt::Display for Host {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.key())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn labels_host_keys() {
        assert_eq!(label("linux-x86_64"), "Linux (x86-64)");
        assert_eq!(label("macos-aarch64"), "macOS (Arm64)");
        assert_eq!(label("haiku-x86"), "haiku-x86");
    }
}
