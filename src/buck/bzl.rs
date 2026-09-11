//! `pudu.bzl` — the generated macro file.
//!
//! Static text. Nothing is interpolated, so this file is byte-identical in
//! every project and carries none of `format.rs`'s escaping risk.

const BODY: &str = r##"
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

_BUILD_APP = '''
const fs = require("fs");
const path = require("path");

const [, , manifestPath, outDir] = process.argv;
const manifest = JSON.parse(fs.readFileSync(manifestPath, "utf8"));

// First-party sources are copied, never hardlinked. A hardlink would share
// an inode with a file in the user's working tree, so an in-place write
// would mutate a build output directly. The store's inodes are different in
// kind: they belong to immutable buck-out extractions.
for (const [dest, src] of Object.entries(manifest.copies)) {
  const d = path.join(outDir, dest);
  fs.mkdirSync(path.dirname(d), { recursive: true });
  fs.copyFileSync(Array.isArray(src) ? src.join("") : src, d);
}

for (const [dest, target] of Object.entries(manifest.links)) {
  const d = path.join(outDir, dest);
  fs.mkdirSync(path.dirname(d), { recursive: true });
  fs.symlinkSync(Array.isArray(target) ? target.join("") : target, d);
}
'''

def _node_app_dir(ctx):
    """The runnable directory: first-party sources plus one node_modules link.

    Node finds `node_modules` by walking up from the main module's directory,
    so the sources and the store have to meet. They meet here. The sources
    are real files, so relative `require`s between them work; `node_modules`
    is a single symlink into the tree, and Node realpaths it *into* the
    genuine store where `.pnpm` isolation holds.
    """
    tree = ctx.attrs.node_modules[DefaultInfo].default_outputs[0]
    app = ctx.actions.declare_output("app", dir = True)
    builder = ctx.actions.write("build_app.js", _BUILD_APP)
    copies = {src.short_path: src for src in ctx.attrs.srcs}
    manifest = ctx.actions.write_json(
        "app_manifest.json",
        {
            "copies": copies,
            # `relative_to = (app, 0)` places the link relative to the app
            # directory itself. One level off and the link dangles, which
            # builds clean and fails only when Node runs.
            "links": {"node_modules": cmd_args(tree, relative_to = (app, 0))},
        },
        with_inputs = True,
    )
    node = ctx.attrs._node_toolchain[NodeToolchainInfo].node
    ctx.actions.run(
        cmd_args(node, builder, manifest, app.as_output()),
        category = "node_app",
    )
    return app, tree

def _node_launcher(ctx, app):
    node = ctx.attrs._node_toolchain[NodeToolchainInfo].node
    return ctx.actions.write(
        "run.sh",
        cmd_args(
            "#!/bin/sh",
            "set -e",
            cmd_args(
                "exec",
                node,
                cmd_args(app, format = "{}/" + ctx.attrs.main),
                "\"$@\"",
                delimiter = " ",
            ),
            "",
            delimiter = "\n",
        ),
        is_executable = True,
        with_inputs = True,
    )

def _node_binary_impl(ctx):
    app, tree = _node_app_dir(ctx)
    launcher = _node_launcher(ctx, app)

    # The tree is reached through a symlink that leaves the app output, so
    # buck2 has no structural edge to it. It must be named in both places or
    # it is not materialized and the link dangles.
    return [
        DefaultInfo(default_output = launcher, other_outputs = [app, tree]),
        RunInfo(args = cmd_args(launcher, hidden = [app, tree])),
    ]

def _node_test_impl(ctx):
    app, tree = _node_app_dir(ctx)
    launcher = _node_launcher(ctx, app)
    return [
        DefaultInfo(default_output = launcher, other_outputs = [app, tree]),
        RunInfo(args = cmd_args(launcher, hidden = [app, tree])),
        ExternalRunnerTestInfo(
            type = "node",
            command = [cmd_args(launcher, hidden = [app, tree])],
        ),
    ]

_NODE_ATTRS = {
    "main": attrs.string(),
    "node_modules": attrs.dep(),
    "srcs": attrs.list(attrs.source(), default = []),
    "_node_toolchain": attrs.toolchain_dep(default = "toolchains//:node"),
}

node_binary = rule(impl = _node_binary_impl, attrs = _NODE_ATTRS)

node_test = rule(impl = _node_test_impl, attrs = _NODE_ATTRS)
"##;

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

    #[test]
    fn node_binary_is_defined() {
        let s = render();
        assert!(s.contains("node_binary = rule("));
        assert!(s.contains("node_test = rule("));
    }

    /// The tree is reached through a symlink that leaves the app output, which
    /// is a dependency buck2 does not track structurally. Absent from either
    /// place, buck2 does not materialize it and the link dangles — and a
    /// dangling link builds clean (S5 design §1.3).
    #[test]
    fn node_binary_keeps_the_tree_materialized() {
        let s = render();
        assert!(s.contains("other_outputs = [app, tree]"));
        assert!(s.contains("hidden = [app, tree]"));
    }

    #[test]
    fn node_binary_places_the_tree_link_relative_to_the_app_directory() {
        let s = render();
        assert!(s.contains("relative_to = (app, 0)"));
    }

    #[test]
    fn node_test_reports_itself_to_the_test_runner() {
        let s = render();
        assert!(s.contains("ExternalRunnerTestInfo"));
    }

    /// First-party sources are copied, never hardlinked: a hardlink would share
    /// an inode with a file the user is editing, so an in-place write would
    /// mutate a build output.
    #[test]
    fn node_binary_documents_why_first_party_sources_are_copied() {
        let s = render();
        assert!(s.contains("hardlink"));
    }
}
