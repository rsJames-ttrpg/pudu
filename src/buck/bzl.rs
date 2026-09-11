//! `pudu.bzl` — the generated macro file.
//!
//! Static text. Nothing is interpolated, so this file is byte-identical in
//! every project and carries none of `format.rs`'s escaping risk.

const BODY: &str = r#"
load("@prelude//:rules.bzl", "http_archive")

# `NodeToolchainInfo` is defined in `toolchains.bzl`, which `pudu init` writes
# into this same directory. That file is user-owned ("Safe to edit"), so this
# load is a contract between a generated file and a user-editable one.
load(":toolchains.bzl", "NodeToolchainInfo")

def npm_package(name, url, sha256, size, root, bin = {}, visibility = None):
    """One registry tarball, extracted and verified by Buck.

    The archive is deliberately NOT stripped. The buck2 prelude interpolates
    `strip_prefix` unquoted into a shell command, and an archive root is
    third-party data that can contain a space: `@types/node` unpacks to
    `node v22.20`, which tar then reads as two arguments. The root is exposed
    as the `[root]` sub-target instead, which is a pure artifact projection
    and never reaches a shell.

    `sha512` is not passed: Buck2 verifies sha1 or sha256 only. The lockfile's
    sha512 is verified by `pudu vendor` and recorded in packages.toml.
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

_BUILD_TREE = '''
const fs = require("fs");
const path = require("path");

const [, , manifestPath, outDir] = process.argv;
const manifest = JSON.parse(fs.readFileSync(manifestPath, "utf8"));

// Hardlink rather than copy: this is pnpm's own strategy, and it makes a
// tree cost O(entries) instead of O(bytes). The inode is shared with
// buck2's extraction output, so nothing may ever write into the tree.
// linkSync fails across a filesystem boundary; copying is the fallback.
function place(from, to) {
  const st = fs.lstatSync(from);
  if (st.isDirectory()) {
    fs.mkdirSync(to, { recursive: true });
    for (const entry of fs.readdirSync(from)) {
      place(path.join(from, entry), path.join(to, entry));
    }
  } else if (st.isSymbolicLink()) {
    place(fs.realpathSync(from), to);
  } else {
    try {
      fs.linkSync(from, to);
    } catch (e) {
      fs.copyFileSync(from, to);
    }
  }
}

for (const [dest, src] of Object.entries(manifest.copies)) {
  const d = path.join(outDir, dest);
  fs.mkdirSync(path.dirname(d), { recursive: true });
  place(Array.isArray(src) ? src.join("") : src, d);
}

for (const [dest, target] of Object.entries(manifest.links)) {
  const d = path.join(outDir, dest);
  fs.mkdirSync(path.dirname(d), { recursive: true });
  fs.symlinkSync(Array.isArray(target) ? target.join("") : target, d);
}
'''

def _node_modules_tree_impl(ctx):
    """One importer's node_modules, shaped exactly like pnpm's.

    This cannot be a `filegroup`. `copy = False` calls `symlinked_dir`, which
    makes every store leaf a symlink out of the tree; Node realpaths through
    those and sibling lookup escapes with `Cannot find module`. `copy = True`
    fails oppositely, flattening the symlink pnpm's isolation depends on. The
    real shape needs real directories and intra-tree relative symlinks from
    one action, which no `filegroup` mode provides.

    Inputs arrive as a JSON manifest, so no package name, archive root or bin
    path is ever interpolated into a command line.
    """
    out = ctx.actions.declare_output("node_modules", dir = True)
    builder = ctx.actions.write("build_tree.js", _BUILD_TREE)
    copies = {
        dest: dep[DefaultInfo].default_outputs[0]
        for dest, dep in ctx.attrs.copies.items()
    }
    manifest = ctx.actions.write_json(
        "tree_manifest.json",
        {"copies": copies, "links": ctx.attrs.links},
        with_inputs = True,
    )
    node = ctx.attrs._node_toolchain[NodeToolchainInfo].node
    ctx.actions.run(
        cmd_args(node, builder, manifest, out.as_output()),
        category = "node_modules_tree",
    )
    return [DefaultInfo(default_output = out)]

node_modules_tree = rule(
    impl = _node_modules_tree_impl,
    attrs = {
        "copies": attrs.dict(attrs.string(), attrs.dep(), default = {}),
        "links": attrs.dict(attrs.string(), attrs.string(), default = {}),
        "_node_toolchain": attrs.toolchain_dep(default = "toolchains//:node"),
    },
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
    fn the_macro_never_passes_strip_prefix() {
        // Spec §1.1. The prelude interpolates strip_prefix unquoted into a
        // shell script, and `@types/node` unpacks to `node v22.20`. If this
        // ever fails, 18 packages in the 400-package fixture stop building.
        //
        // Asserts on the attribute assignment, not the bare word: the
        // docstring names `strip_prefix` deliberately, so that a maintainer
        // grepping for it finds the reason it is absent.
        assert!(
            !render().contains("strip_prefix ="),
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
        // time and kept in packages.toml for audit. As above, the docstring
        // mentions it; only the attribute is forbidden.
        assert!(!render().contains("sha512 ="));
    }

    #[test]
    fn the_macro_documents_why_strip_prefix_is_absent() {
        // The absence is the whole point of the design; an undocumented
        // absence invites a future maintainer to add it back.
        let out = render();
        assert!(out.contains("strip_prefix"), "the docstring must name it");
        assert!(out.contains("node v22.20"), "and give the concrete case");
    }

    #[test]
    fn the_file_is_constant() {
        // No interpolation of any kind, so there is no escaping risk here and
        // the file is byte-identical across every project.
        assert_eq!(render(), render());
    }

    #[test]
    fn the_tree_rule_is_defined() {
        let s = render();
        assert!(s.contains("node_modules_tree = rule("));
        assert!(s.contains("ctx.actions.declare_output(\"node_modules\", dir = True)"));
    }

    #[test]
    fn the_tree_rule_takes_node_from_the_toolchain() {
        let s = render();
        assert!(s.contains("load(\":toolchains.bzl\", \"NodeToolchainInfo\")"));
        assert!(s.contains("attrs.toolchain_dep(default = \"toolchains//:node\")"));
    }

    /// Package names, archive roots and bin paths reach the builder as JSON,
    /// never as command-line arguments. This is what retires the `strip_prefix`
    /// hazard class (S4 §1.4) by construction rather than by escaping.
    #[test]
    fn tree_inputs_travel_as_a_manifest_not_as_arguments() {
        let s = render();
        assert!(s.contains("ctx.actions.write_json("));
        assert!(s.contains("with_inputs = True"));
    }

    /// pnpm's own strategy, and what makes a tree cost O(entries) rather than
    /// O(bytes). The copy fallback covers a cross-filesystem buck-out.
    #[test]
    fn the_builder_hardlinks_with_a_copy_fallback() {
        let s = render();
        assert!(s.contains("fs.linkSync"));
        assert!(s.contains("fs.copyFileSync"));
    }

    #[test]
    fn the_macro_documents_why_the_tree_is_not_a_filegroup() {
        let s = render();
        assert!(
            s.contains("filegroup"),
            "the docstring must say why the obvious rule does not work, \
             or the next reader will try it again"
        );
    }
}
