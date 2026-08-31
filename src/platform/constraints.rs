//! Mapping a configured platform to Buck2 constraint labels.

use std::collections::BTreeMap;

use crate::config::Platform;
use crate::platform::{Cpu, Libc, Os};

/// The Buck2 constraint labels a platform selects on.
///
/// Generated labels are returned sorted, so the emitted `constraint_values`
/// list is deterministic without the caller sorting. A `constraints = [...]`
/// override is returned verbatim in the user's own order (§5.3).
///
/// `all` is the full configured platform set, needed because the abi
/// constraint is conditional on the *set*, not on this platform alone.
pub fn constraint_labels(platform: &Platform, all: &BTreeMap<String, Platform>) -> Vec<String> {
    // The escape hatch replaces the generated labels wholesale, including
    // any abi label the rule below would have added. `os`/`cpu`/`libc`
    // continue to drive npm field matching; only emission is overridden.
    if let Some(overrides) = &platform.constraints {
        return overrides.clone();
    }

    let mut labels = vec![
        os_label(platform.os).to_string(),
        cpu_label(platform.cpu).to_string(),
    ];

    if let Some(libc) = platform.libc
        && abi_discriminates(platform, all)
    {
        labels.push(abi_label(libc).to_string());
    }

    labels.sort();
    labels
}

/// Does the abi constraint distinguish this platform from another configured
/// one?
///
/// `prelude//platforms:default` derives its configuration from `host_info()`
/// and sets only cpu and os; nothing sets an abi constraint by default. So
/// the abi label is emitted only when it discriminates — when some other
/// configured platform shares this one's os and cpu but declares a
/// *different* libc. A glibc-only configuration, the common case, therefore
/// needs zero user wiring (design §7).
///
/// A platform need not exclude itself from this scan: it shares its own os,
/// cpu and libc, so it can never be its own discriminator.
fn abi_discriminates(platform: &Platform, all: &BTreeMap<String, Platform>) -> bool {
    let Some(libc) = platform.libc else {
        return false;
    };
    all.values().any(|other| {
        other.os == platform.os
            && other.cpu == platform.cpu
            && other.libc.is_some_and(|l| l != libc)
    })
}

/// npm's `darwin` is the prelude's `macos`, and `win32` is `windows` —
/// neither is a pass-through.
fn os_label(os: Os) -> &'static str {
    match os {
        Os::Linux => "prelude//os/constraints:linux",
        Os::Darwin => "prelude//os/constraints:macos",
        Os::Win32 => "prelude//os/constraints:windows",
    }
}

fn cpu_label(cpu: Cpu) -> &'static str {
    match cpu {
        Cpu::X64 => "prelude//cpu/constraints:x86_64",
        Cpu::Arm64 => "prelude//cpu/constraints:arm64",
    }
}

/// npm's `glibc` is the prelude's `gnu`.
fn abi_label(libc: Libc) -> &'static str {
    match libc {
        Libc::Glibc => "prelude//abi/constraints:gnu",
        Libc::Musl => "prelude//abi/constraints:musl",
    }
}

#[cfg(test)]
mod tests {
    // `use super::*` already brings in `Platform`, `BTreeMap` and the
    // `Os`/`Cpu`/`Libc` enums from this module's own imports.
    use super::*;

    fn p(os: Os, cpu: Cpu, libc: Option<Libc>) -> Platform {
        Platform {
            os,
            cpu,
            libc,
            constraints: None,
        }
    }

    fn only(platform: Platform) -> (Platform, BTreeMap<String, Platform>) {
        let all = BTreeMap::from([("solo".to_string(), platform.clone())]);
        (platform, all)
    }

    #[test]
    fn maps_os_and_cpu_to_prelude_labels() {
        let (plat, all) = only(p(Os::Linux, Cpu::X64, None));
        assert_eq!(
            constraint_labels(&plat, &all),
            vec![
                "prelude//cpu/constraints:x86_64".to_string(),
                "prelude//os/constraints:linux".to_string(),
            ]
        );
    }

    /// npm's vocabulary and the prelude's differ on two of seven values.
    /// A "simplification" that lowercased the npm name would break both.
    #[test]
    fn npm_and_prelude_vocabularies_differ_on_darwin_and_glibc() {
        let (plat, all) = only(p(Os::Darwin, Cpu::Arm64, None));
        let labels = constraint_labels(&plat, &all);
        assert!(
            labels.contains(&"prelude//os/constraints:macos".to_string()),
            "npm `darwin` is the prelude's `macos`: {labels:?}"
        );
        assert!(!labels.iter().any(|l| l.contains("darwin")), "{labels:?}");

        // `glibc` is the prelude's `gnu`; exercised via the abi rule below.
        let all = BTreeMap::from([
            (
                "linux-x64-gnu".to_string(),
                p(Os::Linux, Cpu::X64, Some(Libc::Glibc)),
            ),
            (
                "linux-x64-musl".to_string(),
                p(Os::Linux, Cpu::X64, Some(Libc::Musl)),
            ),
        ]);
        let labels = constraint_labels(&all["linux-x64-gnu"], &all);
        assert!(
            labels.contains(&"prelude//abi/constraints:gnu".to_string()),
            "npm `glibc` is the prelude's `gnu`: {labels:?}"
        );
        assert!(!labels.iter().any(|l| l.contains("glibc")), "{labels:?}");
    }

    #[test]
    fn maps_win32_to_windows() {
        let (plat, all) = only(p(Os::Win32, Cpu::X64, None));
        assert!(
            constraint_labels(&plat, &all).contains(&"prelude//os/constraints:windows".to_string())
        );
    }

    #[test]
    fn labels_are_sorted() {
        let all = BTreeMap::from([
            (
                "linux-x64-gnu".to_string(),
                p(Os::Linux, Cpu::X64, Some(Libc::Glibc)),
            ),
            (
                "linux-x64-musl".to_string(),
                p(Os::Linux, Cpu::X64, Some(Libc::Musl)),
            ),
        ]);
        let labels = constraint_labels(&all["linux-x64-gnu"], &all);
        let mut sorted = labels.clone();
        sorted.sort();
        assert_eq!(labels, sorted, "emitted constraint_values must be sorted");
    }

    /// A glibc-only configuration needs zero user wiring: the prelude's
    /// default platform sets os and cpu from `host_info()` and nothing sets
    /// an abi constraint, so emitting one would fail to match.
    #[test]
    fn glibc_only_configuration_emits_no_abi_constraint() {
        let all = BTreeMap::from([
            (
                "linux-x64-gnu".to_string(),
                p(Os::Linux, Cpu::X64, Some(Libc::Glibc)),
            ),
            ("darwin-arm64".to_string(), p(Os::Darwin, Cpu::Arm64, None)),
        ]);
        for name in ["linux-x64-gnu", "darwin-arm64"] {
            let labels = constraint_labels(&all[name], &all);
            assert!(
                !labels.iter().any(|l| l.contains("abi")),
                "{name} must gain no abi constraint: {labels:?}"
            );
        }
    }

    /// When two platforms share os+cpu and differ in libc, the abi
    /// constraint discriminates — and BOTH gain it, not just the musl one.
    #[test]
    fn gnu_plus_musl_emits_the_abi_constraint_on_both() {
        let all = BTreeMap::from([
            (
                "linux-x64-gnu".to_string(),
                p(Os::Linux, Cpu::X64, Some(Libc::Glibc)),
            ),
            (
                "linux-x64-musl".to_string(),
                p(Os::Linux, Cpu::X64, Some(Libc::Musl)),
            ),
        ]);
        assert!(
            constraint_labels(&all["linux-x64-gnu"], &all)
                .contains(&"prelude//abi/constraints:gnu".to_string())
        );
        assert!(
            constraint_labels(&all["linux-x64-musl"], &all)
                .contains(&"prelude//abi/constraints:musl".to_string())
        );
    }

    /// The abi rule is keyed on os+cpu. Two platforms differing in libc but
    /// also in cpu do not discriminate each other.
    #[test]
    fn differing_libc_on_a_different_cpu_does_not_discriminate() {
        let all = BTreeMap::from([
            (
                "linux-x64-gnu".to_string(),
                p(Os::Linux, Cpu::X64, Some(Libc::Glibc)),
            ),
            (
                "linux-arm64-musl".to_string(),
                p(Os::Linux, Cpu::Arm64, Some(Libc::Musl)),
            ),
        ]);
        for name in ["linux-x64-gnu", "linux-arm64-musl"] {
            let labels = constraint_labels(&all[name], &all);
            assert!(
                !labels.iter().any(|l| l.contains("abi")),
                "{name}: {labels:?}"
            );
        }
    }

    #[test]
    fn a_platform_with_no_libc_never_gains_an_abi_constraint() {
        let all = BTreeMap::from([
            ("linux-x64".to_string(), p(Os::Linux, Cpu::X64, None)),
            (
                "linux-x64-musl".to_string(),
                p(Os::Linux, Cpu::X64, Some(Libc::Musl)),
            ),
        ]);
        // The no-libc platform should never gain an abi constraint
        let labels = constraint_labels(&all["linux-x64"], &all);
        assert!(!labels.iter().any(|l| l.contains("abi")), "{labels:?}");
        // The musl platform should also not gain an abi constraint when paired with a no-libc platform,
        // because a no-libc platform is not specifying a libc and should not be considered discriminating
        let labels = constraint_labels(&all["linux-x64-musl"], &all);
        assert!(
            !labels.iter().any(|l| l.contains("abi")),
            "no-libc platform should not discriminate: {labels:?}"
        );
    }

    #[test]
    fn constraints_override_replaces_generated_labels_entirely() {
        let plat = Platform {
            os: Os::Linux,
            cpu: Cpu::X64,
            libc: Some(Libc::Glibc),
            constraints: Some(vec![
                "ovr_config//os:linux".to_string(),
                "ovr_config//cpu:x86_64".to_string(),
            ]),
        };
        let all = BTreeMap::from([
            ("corp-linux".to_string(), plat.clone()),
            (
                "linux-x64-musl".to_string(),
                p(Os::Linux, Cpu::X64, Some(Libc::Musl)),
            ),
        ]);
        let labels = constraint_labels(&plat, &all);
        // Verbatim, in the user's order — not sorted, and with no abi label
        // even though the platform set would otherwise discriminate.
        assert_eq!(
            labels,
            vec![
                "ovr_config//os:linux".to_string(),
                "ovr_config//cpu:x86_64".to_string(),
            ]
        );
        assert!(!labels.iter().any(|l| l.starts_with("prelude//")));
    }

    #[test]
    fn an_empty_constraints_override_is_honoured_as_written() {
        let plat = Platform {
            os: Os::Linux,
            cpu: Cpu::X64,
            libc: None,
            constraints: Some(Vec::new()),
        };
        let all = BTreeMap::from([("bare".to_string(), plat.clone())]);
        assert!(
            constraint_labels(&plat, &all).is_empty(),
            "an explicit empty list is a request, not an absence"
        );
    }
}
