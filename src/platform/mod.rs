//! Platform axes, spelled the way npm spells them.
//!
//! S0 defines the types; S2 adds npm-field matching and the mapping to Buck2
//! constraint labels.

use serde::{Deserialize, Serialize};

pub mod matching;

pub use matching::admits;

/// Operating system, using npm's `os` field vocabulary.
///
/// `Win32` is representable even though Windows is a v1 non-goal: npm packages
/// declare `os: ["win32"]`, so S2 must parse it, and config validation rejects
/// a win32 *platform* with a helpful message rather than a serde error.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Os {
    Linux,
    Darwin,
    Win32,
}

/// CPU architecture, using npm's `cpu` field vocabulary.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Cpu {
    X64,
    Arm64,
}

/// C standard library, using npm's `libc` field vocabulary. Linux only.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Libc {
    Glibc,
    Musl,
}

impl Os {
    pub fn as_npm(&self) -> &'static str {
        match self {
            Os::Linux => "linux",
            Os::Darwin => "darwin",
            Os::Win32 => "win32",
        }
    }
}

impl Cpu {
    pub fn as_npm(&self) -> &'static str {
        match self {
            Cpu::X64 => "x64",
            Cpu::Arm64 => "arm64",
        }
    }
}

impl Libc {
    pub fn as_npm(&self) -> &'static str {
        match self {
            Libc::Glibc => "glibc",
            Libc::Musl => "musl",
        }
    }

    /// Short form used in generated platform names (`linux-x64-gnu`).
    pub fn short(&self) -> &'static str {
        match self {
            Libc::Glibc => "gnu",
            Libc::Musl => "musl",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Debug, serde::Deserialize)]
    struct Holder {
        os: Os,
        cpu: Cpu,
        libc: Option<Libc>,
    }

    #[test]
    fn deserializes_npm_spellings() {
        let h: Holder = toml::from_str(
            r#"os = "linux"
cpu = "x64"
libc = "musl"
"#,
        )
        .unwrap();
        assert_eq!(h.os, Os::Linux);
        assert_eq!(h.cpu, Cpu::X64);
        assert_eq!(h.libc, Some(Libc::Musl));
    }

    #[test]
    fn win32_is_representable() {
        let h: Holder = toml::from_str("os = \"win32\"\ncpu = \"x64\"\n").unwrap();
        assert_eq!(h.os, Os::Win32);
        assert_eq!(h.libc, None);
    }

    #[test]
    fn unknown_os_lists_valid_values() {
        let err = toml::from_str::<Holder>("os = \"solaris\"\ncpu = \"x64\"\n").unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("linux"), "should list valid values: {msg}");
    }

    #[test]
    fn npm_spellings_round_trip() {
        assert_eq!(Os::Darwin.as_npm(), "darwin");
        assert_eq!(Os::Win32.as_npm(), "win32");
        assert_eq!(Cpu::Arm64.as_npm(), "arm64");
    }

    #[test]
    fn libc_as_npm_spells_glibc_and_musl() {
        assert_eq!(Libc::Glibc.as_npm(), "glibc");
        assert_eq!(Libc::Musl.as_npm(), "musl");
    }

    #[test]
    fn libc_short_form_feeds_generated_platform_names() {
        // `short()` feeds names like `linux-x64-gnu`: glibc is spelled
        // "gnu" (not "glibc"), musl keeps its own name.
        assert_eq!(Libc::Glibc.short(), "gnu");
        assert_eq!(Libc::Musl.short(), "musl");
    }
}
