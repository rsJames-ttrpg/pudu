//! Does a package's npm platform field admit a given platform?

/// Does a package's npm platform field admit `current`?
///
/// A port of pnpm's `checkList` (`@pnpm/package-is-installable`), evaluated
/// for a single `current` value because pudu considers one platform at a
/// time. `field` is the raw list from the lockfile with negation intact;
/// `None` is an absent field.
///
/// The final rule — `matched || negations == list.len()` — is pnpm's, and
/// carries two consequences worth stating because they are not what a
/// reader expects:
///
/// * A list mixing negative and positive entries requires an explicit
///   positive hit. `["!win32", "darwin"]` does not admit linux.
/// * An empty list admits everything, since `0 == 0`.
///
/// pnpm additionally discards non-string list entries before matching.
/// YAML gives pudu a `Vec<String>`, so a non-string entry is rejected by
/// serde long before reaching here; the divergence is unreachable and is
/// noted only so a reader comparing the two implementations is not left
/// wondering.
pub fn admits(field: Option<&[String]>, current: &str) -> bool {
    let Some(list) = field else { return true };

    // `any` is special only as a singleton — `["any", "darwin"]` is an
    // ordinary two-entry positive list.
    if list.len() == 1 && list[0] == "any" {
        return true;
    }

    let mut matched = false;
    let mut negations = 0usize;

    for entry in list {
        if let Some(body) = entry.strip_prefix('!') {
            if body == current {
                return false;
            }
            negations += 1;
        } else if entry == current {
            matched = true;
        }
    }

    matched || negations == list.len()
}

use crate::config::Platform;
use crate::lock::types::PackageMeta;

/// Does a package survive on a platform? All three axes must admit.
///
/// The `libc` axis is skipped entirely when the platform declares no libc.
/// That reproduces pnpm's own behaviour on a machine with no detectable
/// libc — a Mac, where `detect-libc` reports `unknown` and the axis is
/// never checked. See spec §3 and survey §1.
///
/// Each axis matches npm's vocabulary: `linux`/`darwin`/`win32`,
/// `x64`/`arm64`, `glibc`/`musl`. Note this is `Libc::as_npm` (`glibc`) and
/// NOT `Libc::short` (`gnu`), which is the Buck spelling used only by
/// constraint labels and generated platform names.
pub fn admits_platform(meta: &PackageMeta, platform: &Platform) -> bool {
    admits(meta.os.as_deref(), platform.os.as_npm())
        && admits(meta.cpu.as_deref(), platform.cpu.as_npm())
        && match platform.libc {
            Some(libc) => admits(meta.libc.as_deref(), libc.as_npm()),
            None => true,
        }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Convenience: build the `Option<&[String]>` shape `admits` takes.
    fn list(items: &[&str]) -> Vec<String> {
        items.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn absent_field_admits_everything() {
        assert!(admits(None, "linux"));
        assert!(admits(None, "wasm32"));
    }

    #[test]
    fn positive_entry_admits_only_itself() {
        assert!(admits(Some(&list(&["linux"])), "linux"));
        assert!(!admits(Some(&list(&["darwin"])), "linux"));
    }

    #[test]
    fn negation_excludes_its_own_value() {
        assert!(admits(Some(&list(&["!win32"])), "linux"));
        assert!(!admits(Some(&list(&["!win32"])), "win32"));
    }

    #[test]
    fn all_negative_list_admits_anything_it_does_not_name() {
        assert!(admits(Some(&list(&["!win32", "!darwin"])), "linux"));
        assert!(!admits(Some(&list(&["!win32", "!darwin"])), "darwin"));
    }

    /// pnpm's rule is `matched || negations == list.len()`. A list mixing
    /// negative and positive entries therefore requires an explicit
    /// positive hit: negations only ever subtract, they never widen.
    /// `["!win32", "darwin"]` does NOT mean "anything but win32".
    #[test]
    fn mixed_list_requires_an_explicit_positive_hit() {
        assert!(
            !admits(Some(&list(&["!win32", "darwin"])), "linux"),
            "a mixed list must not admit a value no positive entry names"
        );
        assert!(admits(Some(&list(&["!win32", "linux"])), "linux"));
        assert!(!admits(Some(&list(&["!win32", "linux"])), "win32"));
    }

    /// `any` is special ONLY as a singleton list. In any other position it
    /// is an ordinary token that matches nothing.
    #[test]
    fn any_is_special_only_as_a_singleton() {
        assert!(admits(Some(&list(&["any"])), "linux"));
        assert!(
            !admits(Some(&list(&["any", "darwin"])), "linux"),
            "`any` alongside another entry is an ordinary token"
        );
    }

    #[test]
    fn empty_list_admits_everything() {
        // `matched=false, negations=0, len=0` satisfies `negations == len`.
        assert!(admits(Some(&[]), "linux"));
    }

    /// Unknown tokens are ordinary positives that match nothing. They must
    /// never error: the committed fixture alone carries seven `os` and
    /// seven `cpu` values outside pudu's enums.
    #[test]
    fn unknown_tokens_match_nothing_and_never_panic() {
        assert!(!admits(Some(&list(&["wasm32"])), "x64"));
        assert!(!admits(Some(&list(&["openharmony"])), "linux"));
        assert!(admits(Some(&list(&["loong64", "x64"])), "x64"));
    }

    #[test]
    fn negation_of_an_unknown_token_still_admits() {
        assert!(admits(Some(&list(&["!openharmony"])), "linux"));
    }

    /// A bare `!` has an empty body, which equals no platform value.
    #[test]
    fn bare_bang_is_a_negation_of_the_empty_string() {
        assert!(admits(Some(&list(&["!"])), "linux"));
    }

    // These tests live in `platform::matching`, so `use super::*` brings in
    // this module's items — not the parent module's enums, which must be
    // named explicitly.
    use crate::config::Platform;
    use crate::lock::types::{PackageMeta, Resolution};
    use crate::platform::{Cpu, Libc, Os};

    /// A `PackageMeta` carrying only the three platform axes; every other
    /// field takes its default.
    fn meta(os: Option<&[&str]>, cpu: Option<&[&str]>, libc: Option<&[&str]>) -> PackageMeta {
        PackageMeta {
            resolution: Resolution::Integrity {
                integrity: "sha512-test".to_string(),
            },
            engines: Default::default(),
            os: os.map(list),
            cpu: cpu.map(list),
            libc: libc.map(list),
            has_bin: false,
            deprecated: None,
            peer_dependencies: Default::default(),
            peer_dependencies_meta: Default::default(),
            bundled_dependencies: Vec::new(),
        }
    }

    fn platform(os: Os, cpu: Cpu, libc: Option<Libc>) -> Platform {
        Platform {
            os,
            cpu,
            libc,
            constraints: None,
        }
    }

    #[test]
    fn all_three_axes_must_admit() {
        let p = platform(Os::Linux, Cpu::X64, Some(Libc::Glibc));
        assert!(admits_platform(
            &meta(Some(&["linux"]), Some(&["x64"]), None),
            &p
        ));
        assert!(!admits_platform(
            &meta(Some(&["darwin"]), Some(&["x64"]), None),
            &p
        ));
        assert!(!admits_platform(
            &meta(Some(&["linux"]), Some(&["arm64"]), None),
            &p
        ));
        assert!(!admits_platform(
            &meta(Some(&["linux"]), Some(&["x64"]), Some(&["musl"])),
            &p
        ));
    }

    #[test]
    fn a_package_with_no_platform_fields_survives_every_platform() {
        for p in [
            platform(Os::Linux, Cpu::X64, Some(Libc::Glibc)),
            platform(Os::Darwin, Cpu::Arm64, None),
            platform(Os::Win32, Cpu::X64, None),
        ] {
            assert!(admits_platform(&meta(None, None, None), &p));
        }
    }

    /// pnpm evaluates the libc axis only when a libc is detectable, which it
    /// is not on macOS — so a Mac never checks libc, whatever a package
    /// declares. A platform with no configured libc reproduces that.
    #[test]
    fn libc_axis_is_skipped_when_the_platform_declares_none() {
        let mac = platform(Os::Darwin, Cpu::Arm64, None);
        assert!(admits_platform(
            &meta(Some(&["darwin"]), Some(&["arm64"]), Some(&["musl"])),
            &mac
        ));
        assert!(admits_platform(
            &meta(Some(&["darwin"]), Some(&["arm64"]), Some(&["glibc"])),
            &mac
        ));
    }

    #[test]
    fn libc_axis_discriminates_when_the_platform_declares_one() {
        let gnu = platform(Os::Linux, Cpu::X64, Some(Libc::Glibc));
        let musl = platform(Os::Linux, Cpu::X64, Some(Libc::Musl));
        let m = meta(Some(&["linux"]), Some(&["x64"]), Some(&["musl"]));
        assert!(!admits_platform(&m, &gnu));
        assert!(admits_platform(&m, &musl));
    }

    /// The npm spelling is `glibc`, not `gnu` — `gnu` is the *Buck* spelling.
    /// Matching against `Libc::short()` here would silently prune every
    /// glibc-gated package.
    #[test]
    fn libc_matches_the_npm_spelling_not_the_buck_one() {
        let gnu = platform(Os::Linux, Cpu::X64, Some(Libc::Glibc));
        assert!(admits_platform(&meta(None, None, Some(&["glibc"])), &gnu));
        assert!(!admits_platform(&meta(None, None, Some(&["gnu"])), &gnu));
    }
}
