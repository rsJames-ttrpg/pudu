//! `BUCK` — one `npm_package` per package table entry.
//!
//! Facts are inline rather than loaded from a generated data file
//! (TD-S4-01, decided against in spec §10): `packages.toml` is already the
//! committed, reviewable hash record, so a `vendor.bzl` would put the same
//! several hundred hashes in a third generated file and add a Starlark writer
//! whose escaping must track a reader.

use std::collections::BTreeMap;

use crate::buck::HEADER;
use crate::buck::format::{starlark_string, validate_bin_name};
use crate::buck::store;
use crate::error::BuckError;
use crate::lock::snapshot_key::target_name;
use crate::packages::Entry;

/// An importer path as a Buck target-name stem.
///
/// The root importer is `.`, which is not a name; everything else is a
/// workspace-relative directory path, and `/` is not legal in a target name.
pub fn importer_target_name(importer: &str) -> String {
    if importer == "." {
        "root".to_string()
    } else {
        importer.replace('/', "_")
    }
}

pub fn render(
    entries: &BTreeMap<String, Entry>,
    trees: &BTreeMap<String, store::Tree>,
    third_party_label: &str,
) -> Result<String, BuckError> {
    let mut out = String::from(HEADER);
    out.push('\n');
    out.push_str(&format!(
        "load(\"//{third_party_label}:pudu.bzl\", \"node_modules_tree\", \"npm_package\")\n"
    ));

    // BTreeMap iteration is key order — lexicographic by `name@version`,
    // the same order packages.toml is written in.
    for (key, e) in entries {
        for name in e.bin.keys() {
            validate_bin_name(key, name)?;
        }

        out.push_str("\nnpm_package(\n");
        out.push_str(&format!(
            "    name = {},\n",
            starlark_string(&target_name(key))
        ));
        out.push_str(&format!("    url = {},\n", starlark_string(&e.url)));
        out.push_str(&format!("    sha256 = {},\n", starlark_string(&e.sha256)));
        out.push_str(&format!("    size = {},\n", e.size));
        out.push_str(&format!("    root = {},\n", starlark_string(&e.root)));
        if !e.bin.is_empty() {
            let rendered: Vec<String> = e
                .bin
                .iter()
                .map(|(k, v)| format!("{}: {}", starlark_string(k), starlark_string(v)))
                .collect();
            out.push_str(&format!("    bin = {{{}}},\n", rendered.join(", ")));
        }
        out.push_str(")\n");
    }

    // One `node_modules_tree` per importer, in `BTreeMap` order (which is
    // importer-path order). Two importers can flatten to the same target
    // name (`a/b` and `a_b`), which would produce a duplicate-target error
    // from buck2 naming neither importer — caught here instead.
    let mut seen: BTreeMap<String, String> = BTreeMap::new();
    for (importer, tree) in trees {
        let name = importer_target_name(importer);
        if let Some(first) = seen.get(&name) {
            return Err(BuckError::ImporterNameCollision {
                target: name,
                first: first.clone(),
                second: importer.clone(),
            });
        }
        seen.insert(name.clone(), importer.clone());

        out.push_str("\nnode_modules_tree(\n");
        out.push_str(&format!(
            "    name = {},\n",
            starlark_string(&format!("{name}_node_modules"))
        ));
        if !tree.copies.is_empty() {
            out.push_str("    copies = {\n");
            for (dest, key) in &tree.copies {
                out.push_str(&format!(
                    "        {}: {},\n",
                    starlark_string(dest),
                    starlark_string(&format!("//{third_party_label}:{}[root]", target_name(key)))
                ));
            }
            out.push_str("    },\n");
        }
        if !tree.links.is_empty() {
            out.push_str("    links = {\n");
            for (dest, target) in &tree.links {
                out.push_str(&format!(
                    "        {}: {},\n",
                    starlark_string(dest),
                    starlark_string(target)
                ));
            }
            out.push_str("    },\n");
        }
        out.push_str(")\n");
    }

    Ok(out)
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use super::*;

    fn entry(url: &str, root: &str, bin: &[(&str, &str)]) -> Entry {
        Entry {
            url: url.to_string(),
            sha512: "sha512-AAAA".to_string(),
            sha256: "0123456789abcdef".to_string(),
            size: 1234,
            root: root.to_string(),
            bin: bin
                .iter()
                .map(|(k, v)| (k.to_string(), v.to_string()))
                .collect(),
            has_install_script: false,
        }
    }

    fn one(key: &str, e: Entry) -> BTreeMap<String, Entry> {
        BTreeMap::from([(key.to_string(), e)])
    }

    #[test]
    fn the_file_opens_with_the_banner_and_loads_the_macro() {
        let t = one("left-pad@1.3.0", entry("https://r/lp.tgz", "package", &[]));
        let out = render(&t, &BTreeMap::new(), "third-party/js").unwrap();
        assert!(out.starts_with(crate::buck::HEADER));
        assert!(
            out.contains(
                r#"load("//third-party/js:pudu.bzl", "node_modules_tree", "npm_package")"#
            )
        );
    }

    #[test]
    fn the_load_label_follows_the_configured_third_party_dir() {
        let t = one("left-pad@1.3.0", entry("https://r/lp.tgz", "package", &[]));
        let out = render(&t, &BTreeMap::new(), "vendor/npm").unwrap();
        assert!(
            out.contains(r#"load("//vendor/npm:pudu.bzl", "node_modules_tree", "npm_package")"#)
        );
    }

    #[test]
    fn a_scoped_name_is_mangled_through_target_name() {
        // pnpm's virtual-store convention: `/` becomes `+`. Reusing
        // `target_name` keeps package targets and S5's `.pnpm` paths spelled
        // identically. buck2 accepts the raw form too, so nothing but this
        // test stops the convention diverging.
        let t = one(
            "@types/node@22.20.0",
            entry("https://r/node.tgz", "node v22.20", &[]),
        );
        let out = render(&t, &BTreeMap::new(), "third-party/js").unwrap();
        assert!(out.contains(r#"name = "@types+node@22.20.0","#), "{out}");
        assert!(!out.contains("@types/node@22.20.0"));
    }

    #[test]
    fn a_space_in_the_root_is_emitted_as_a_quoted_literal() {
        let t = one(
            "@types/node@22.20.0",
            entry("https://r/node.tgz", "node v22.20", &[]),
        );
        let out = render(&t, &BTreeMap::new(), "third-party/js").unwrap();
        assert!(out.contains(r#"root = "node v22.20","#));
        assert!(!out.contains("strip_prefix"));
    }

    #[test]
    fn bin_is_omitted_when_empty_and_sorted_when_present() {
        let empty = one("left-pad@1.3.0", entry("https://r/lp.tgz", "package", &[]));
        assert!(
            !render(&empty, &BTreeMap::new(), "third-party/js")
                .unwrap()
                .contains("bin =")
        );

        let two = one(
            "tool@1.0.0",
            entry(
                "https://r/t.tgz",
                "package",
                &[("zed", "z.js"), ("abe", "a.js")],
            ),
        );
        let out = render(&two, &BTreeMap::new(), "third-party/js").unwrap();
        let abe = out.find("\"abe\"").unwrap();
        let zed = out.find("\"zed\"").unwrap();
        assert!(abe < zed, "bin entries must be sorted");
    }

    #[test]
    fn packages_are_emitted_in_key_order() {
        let mut t = one("zeta@1.0.0", entry("https://r/z.tgz", "package", &[]));
        t.insert(
            "alpha@1.0.0".to_string(),
            entry("https://r/a.tgz", "package", &[]),
        );
        let out = render(&t, &BTreeMap::new(), "third-party/js").unwrap();
        assert!(out.find("alpha@1.0.0").unwrap() < out.find("zeta@1.0.0").unwrap());
    }

    #[test]
    fn two_versions_of_one_package_get_two_targets() {
        let mut t = one("semver@7.6.3", entry("https://r/s7.tgz", "package", &[]));
        t.insert(
            "semver@6.3.1".to_string(),
            entry("https://r/s6.tgz", "package", &[]),
        );
        let out = render(&t, &BTreeMap::new(), "third-party/js").unwrap();
        assert_eq!(out.matches("npm_package(").count(), 2);
        assert!(out.contains(r#"name = "semver@6.3.1","#));
        assert!(out.contains(r#"name = "semver@7.6.3","#));
    }

    #[test]
    fn an_unaddressable_bin_name_fails_naming_the_package() {
        let t = one(
            "tool@1.0.0",
            entry("https://r/t.tgz", "package", &[("bang!", "b.js")]),
        );
        let err = render(&t, &BTreeMap::new(), "third-party/js").unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("tool@1.0.0"), "{msg}");
        assert!(msg.contains("bang!"), "{msg}");
    }

    #[test]
    fn rendering_is_deterministic() {
        let mut t = one("zeta@1.0.0", entry("https://r/z.tgz", "package", &[]));
        t.insert(
            "alpha@1.0.0".to_string(),
            entry("https://r/a.tgz", "package", &[("x", "x.js")]),
        );
        assert_eq!(
            render(&t, &BTreeMap::new(), "third-party/js").unwrap(),
            render(&t, &BTreeMap::new(), "third-party/js").unwrap()
        );
    }

    #[test]
    fn sha512_and_has_install_script_do_not_reach_the_output() {
        // sha512 is unverifiable by Buck2 (design §4); has_install_script is
        // S6's gate, not an http_archive attribute.
        let t = one("left-pad@1.3.0", entry("https://r/lp.tgz", "package", &[]));
        let out = render(&t, &BTreeMap::new(), "third-party/js").unwrap();
        assert!(!out.contains("sha512"));
        assert!(!out.contains("has_install_script"));
    }

    fn tree_of(links: &[(&str, &str)], copies: &[(&str, &str)]) -> store::Tree {
        store::Tree {
            copies: copies
                .iter()
                .map(|(d, k)| (d.to_string(), k.to_string()))
                .collect(),
            links: links
                .iter()
                .map(|(d, t)| (d.to_string(), t.to_string()))
                .collect(),
        }
    }

    #[test]
    fn the_root_importer_is_named_root() {
        assert_eq!(importer_target_name("."), "root");
    }

    #[test]
    fn a_nested_importer_flattens_its_path() {
        assert_eq!(importer_target_name("packages/legacy"), "packages_legacy");
    }

    #[test]
    fn a_tree_target_is_emitted_per_importer() {
        let trees = BTreeMap::from([(
            ".".to_string(),
            tree_of(
                &[("left-pad", ".pnpm/left-pad@1.3.0/node_modules/left-pad")],
                &[(
                    ".pnpm/left-pad@1.3.0/node_modules/left-pad",
                    "left-pad@1.3.0",
                )],
            ),
        )]);
        let out = render(&BTreeMap::new(), &trees, "third-party/js").unwrap();
        assert!(out.contains("node_modules_tree(\n    name = \"root_node_modules\","));
        assert!(out.contains("\"left-pad\": \".pnpm/left-pad@1.3.0/node_modules/left-pad\""));
        assert!(out.contains(
            "\".pnpm/left-pad@1.3.0/node_modules/left-pad\": \
             \"//third-party/js:left-pad@1.3.0[root]\""
        ));
    }

    #[test]
    fn the_tree_macro_is_loaded() {
        let trees = BTreeMap::from([(".".to_string(), tree_of(&[], &[]))]);
        let out = render(&BTreeMap::new(), &trees, "third-party/js").unwrap();
        assert!(out.contains("\"node_modules_tree\""));
    }

    /// Two importers can flatten to the same target name. Emitting both would
    /// produce a duplicate-target error from buck2 naming neither importer.
    #[test]
    fn colliding_importer_names_are_rejected() {
        let trees = BTreeMap::from([
            ("a/b".to_string(), tree_of(&[], &[])),
            ("a_b".to_string(), tree_of(&[], &[])),
        ]);
        let err = render(&BTreeMap::new(), &trees, "third-party/js").unwrap_err();
        assert!(matches!(err, BuckError::ImporterNameCollision { .. }));
    }
}
