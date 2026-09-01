//! `config/BUCK` — one `config_setting` per configured platform.
//!
//! A renderer over S2's `constraint_labels`, which already implements design
//! §7's conditional-abi rule and already returns labels in the order the
//! output must have them: sorted for a generated platform, and preserved
//! verbatim — in the user's own order — for a `constraints = [...]`
//! override, since that escape hatch's determinism comes from the input's
//! own stable order rather than from any sorting here. No platform logic is
//! written in this file; adding any would put a second copy of that rule
//! where the first one could not see it.

use std::collections::BTreeMap;

use crate::buck::HEADER;
use crate::config::Platform;
use crate::platform::constraints::constraint_labels;

pub fn render(platforms: &BTreeMap<String, Platform>) -> String {
    let mut out = String::from(HEADER);
    // BTreeMap iteration is name order, which is the emitted order.
    for (name, platform) in platforms {
        out.push('\n');
        out.push_str("config_setting(\n");
        out.push_str(&format!(
            "    name = {},\n",
            super::format::starlark_string(name)
        ));
        out.push_str("    constraint_values = [\n");
        for label in constraint_labels(platform, platforms) {
            out.push_str(&format!(
                "        {},\n",
                super::format::starlark_string(&label)
            ));
        }
        out.push_str("    ],\n");
        out.push_str("    visibility = [\"PUBLIC\"],\n");
        out.push_str(")\n");
    }
    out
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use super::*;
    use crate::config::Platform;
    use crate::platform::{Cpu, Libc, Os};

    fn platform(os: Os, cpu: Cpu, libc: Option<Libc>) -> Platform {
        Platform {
            os,
            cpu,
            libc,
            constraints: None,
        }
    }

    fn glibc_only() -> BTreeMap<String, Platform> {
        BTreeMap::from([(
            "linux-x64-gnu".to_string(),
            platform(Os::Linux, Cpu::X64, Some(Libc::Glibc)),
        )])
    }

    #[test]
    fn one_config_setting_per_platform_in_name_order() {
        let mut p = glibc_only();
        p.insert(
            "darwin-arm64".to_string(),
            platform(Os::Darwin, Cpu::Arm64, None),
        );
        let out = render(&p);
        let darwin = out.find("darwin-arm64").unwrap();
        let linux = out.find("linux-x64-gnu").unwrap();
        assert!(darwin < linux, "platforms must be emitted in name order");
        assert_eq!(out.matches("config_setting(").count(), 2);
    }

    #[test]
    fn a_glibc_only_config_emits_no_abi_constraint() {
        // Design §7: the abi constraint is emitted only when it discriminates,
        // so the common case needs zero user wiring.
        let out = render(&glibc_only());
        assert!(!out.contains("abi/constraints"));
        assert!(out.contains("prelude//cpu/constraints:x86_64"));
        assert!(out.contains("prelude//os/constraints:linux"));
    }

    #[test]
    fn a_libc_pair_emits_the_abi_constraint_on_both() {
        let mut p = glibc_only();
        p.insert(
            "linux-x64-musl".to_string(),
            platform(Os::Linux, Cpu::X64, Some(Libc::Musl)),
        );
        let out = render(&p);
        assert!(out.contains("prelude//abi/constraints:gnu"));
        assert!(out.contains("prelude//abi/constraints:musl"));
    }

    #[test]
    fn the_file_opens_with_the_generated_banner_and_is_deterministic() {
        let p = glibc_only();
        assert!(render(&p).starts_with(crate::buck::HEADER));
        assert_eq!(render(&p), render(&p));
    }

    #[test]
    fn every_config_setting_is_public() {
        let out = render(&glibc_only());
        assert_eq!(out.matches(r#"visibility = ["PUBLIC"]"#).count(), 1);
    }
}
