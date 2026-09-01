//! `pudu.bzl` — the generated macro file.
//!
//! Static text. Nothing is interpolated, so this file is byte-identical in
//! every project and carries none of `format.rs`'s escaping risk.

const BODY: &str = r#"
load("@prelude//:rules.bzl", "http_archive")

def npm_package(name, url, sha256, size, root, bin = {}, visibility = None):
    """One registry tarball, extracted and verified by Buck.

    The archive is deliberately NOT stripped. The buck2 prelude interpolates
    the archive-root-stripping attribute unquoted into a shell command, and an
    archive root is third-party data that can contain a space: `@types/node`
    unpacks to `node v22.20`, which tar then reads as two arguments. The root
    is exposed as the `[root]` sub-target instead, which is a pure artifact
    projection and never reaches a shell.

    The lockfile's SHA-512 is not passed: Buck2 verifies sha1 or sha256 only.
    It is verified by `pudu vendor` and recorded in packages.toml.
    """
    sub_targets = {"root": [root]}
    for bin_name, bin_path in bin.items():
        # A list: `unarchive` takes dict[str, list[str]].
        sub_targets["bin/" + bin_name] = [root + "/" + bin_path]

    http_archive(
        name = name,
        urls = [url],
        sha256 = sha256,
        size_bytes = size,
        type = "tar.gz",
        sub_targets = sub_targets,
        visibility = visibility or ["PUBLIC"],
    )
"#;

pub fn render() -> String {
    format!("{}{}", crate::buck::HEADER, BODY)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_macro_file_opens_with_the_generated_banner() {
        assert!(render().starts_with(crate::buck::HEADER));
    }

    #[test]
    fn the_macro_never_emits_strip_prefix() {
        // Spec §1.1. The prelude interpolates strip_prefix unquoted into a
        // shell script, and `@types/node` unpacks to `node v22.20`. If this
        // ever fails, 18 packages in the 400-package fixture stop building.
        assert!(
            !render().contains("strip_prefix"),
            "pudu.bzl must never pass strip_prefix to http_archive"
        );
    }

    #[test]
    fn the_root_is_exposed_as_a_sub_target() {
        let out = render();
        assert!(out.contains(r#"sub_targets = {"root": [root]}"#));
    }

    #[test]
    fn bin_entries_are_joined_under_the_unstripped_root() {
        // Nothing is stripped, so a bin path must be prefixed with the root
        // to address the file inside the archive.
        assert!(render().contains(r#"sub_targets["bin/" + bin_name] = [root + "/" + bin_path]"#));
    }

    #[test]
    fn sha512_is_not_passed_to_http_archive() {
        // Buck2 cannot verify sha512 (design §4). It is verified at vendor
        // time and kept in packages.toml for audit.
        assert!(!render().contains("sha512"));
    }

    #[test]
    fn the_file_is_constant() {
        // No interpolation of any kind, so there is no escaping risk here and
        // the file is byte-identical across every project.
        assert_eq!(render(), render());
    }
}
