//! Differential test against the live npm registry.
//!
//! `#[ignore]`d: it downloads a few hundred tarballs. CI runs it in its own
//! job (`vendor-oracle`); locally, `cargo test --test vendor_oracle --
//! --ignored`.
//!
//! The expectation comes from `oracle/manifests.json`, computed by
//! `capture-manifests.mjs` from the registry's own manifests using a
//! JavaScript port of pnpm's rules. Where the tarball and the manifest
//! disagree, the tarball wins and the disagreement is the finding — the
//! oracle is a cross-check, not the truth.
//!
//! One disagreement is known, verified, and permanently excluded rather than
//! chased: `fsevents@2.3.3`'s registry manifest reports `scripts.install`
//! (and `gypfile: true`), but its published tarball — hash-verified against
//! `dist.integrity` — contains neither an `install` script nor a
//! `binding.gyp`; it ships a prebuilt `fsevents.node` instead. Real pnpm's
//! `pkgRequiresBuild` reads the manifest and file index back from the
//! extracted tarball (see `pnpm11/worker/src/start.ts`), never the registry
//! API, so it would agree with pudu's tarball-derived `false`, not with this
//! oracle. The registry's packument is simply stale for this one field on
//! this one version. See the header of `capture-manifests.mjs` for the full
//! trail.

mod common;

use std::collections::{BTreeMap, BTreeSet};

#[derive(serde::Deserialize)]
struct OracleEntry {
    key: String,
    url: String,
    /// `null` where the manifest cannot answer — a `directories.bin`
    /// package. None exist in this fixture; the field exists so one would be
    /// skipped rather than silently mis-asserted.
    bin: Option<BTreeMap<String, String>>,
    has_install_script: bool,
}

#[derive(serde::Deserialize)]
struct SidecarEntry {
    url: String,
    #[serde(default)]
    bin: BTreeMap<String, String>,
    #[serde(default)]
    has_install_script: bool,
}

#[derive(serde::Deserialize)]
struct Sidecar {
    #[allow(dead_code)]
    version: u32,
    #[serde(flatten)]
    entries: BTreeMap<String, SidecarEntry>,
}

const CONFIG: &str = r#"
lockfile_path   = "pnpm-lock.yaml"
third_party_dir = "third-party/js"

[platforms.linux-x64-gnu]
os   = "linux"
cpu  = "x64"
libc = "glibc"

[platforms.darwin-arm64]
os  = "darwin"
cpu = "arm64"
"#;

#[test]
#[ignore = "downloads several hundred tarballs from registry.npmjs.org"]
fn the_vendor_pass_agrees_with_the_registry_on_every_package() {
    let fixture = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/lock/real");
    let oracle: Vec<OracleEntry> = serde_json::from_str(
        &std::fs::read_to_string(fixture.join("oracle/manifests.json")).unwrap(),
    )
    .unwrap();
    let oracle: BTreeMap<String, OracleEntry> =
        oracle.into_iter().map(|e| (e.key.clone(), e)).collect();

    let dir = tempfile::tempdir().unwrap();
    std::fs::copy(
        fixture.join("pnpm-lock.yaml"),
        dir.path().join("pnpm-lock.yaml"),
    )
    .unwrap();
    std::fs::write(dir.path().join("pudu.toml"), CONFIG).unwrap();
    let cache = tempfile::tempdir().unwrap();

    let out = common::pudu(dir.path())
        .env("PUDU_CACHE_DIR", cache.path())
        .arg("vendor")
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "vendor failed:\n{}",
        String::from_utf8_lossy(&out.stderr)
    );

    let text = std::fs::read_to_string(dir.path().join("third-party/js/pudu.lock")).unwrap();
    let sidecar: Sidecar = toml::from_str(&text).unwrap();

    assert!(
        !sidecar.entries.is_empty(),
        "the vendor pass recorded nothing"
    );

    let mut mismatches: Vec<String> = Vec::new();
    for (key, got) in &sidecar.entries {
        let Some(want) = oracle.get(key) else {
            mismatches.push(format!("{key}: vendored but absent from the oracle"));
            continue;
        };
        if got.url != want.url {
            mismatches.push(format!("{key}: url {} != {}", got.url, want.url));
        }
        if let Some(want_bin) = &want.bin
            && &got.bin != want_bin
        {
            mismatches.push(format!("{key}: bin {:?} != {:?}", got.bin, want_bin));
        }
        if got.has_install_script != want.has_install_script && key != "fsevents@2.3.3" {
            // fsevents@2.3.3 is the one known, verified exception: the
            // registry manifest disagrees with its own published tarball.
            // See the module doc comment above.
            mismatches.push(format!(
                "{key}: has_install_script {} != {}",
                got.has_install_script, want.has_install_script
            ));
        }
    }
    assert!(
        mismatches.is_empty(),
        "{} disagreement(s) with the registry:\n{}",
        mismatches.len(),
        mismatches.join("\n")
    );

    // The vendored set must be the *union* across platforms, not one
    // platform's view and not everything in the lockfile. Three named
    // packages pin all three directions:
    let vendored: BTreeSet<&String> = sidecar.entries.keys().collect();
    assert!(
        vendored.contains(&"fsevents@2.3.3".to_string()),
        "fsevents is darwin-only; vendoring it proves the set is a union, not one platform's view"
    );
    assert!(
        vendored.contains(&"@esbuild/linux-x64@0.25.12".to_string()),
        "the linux platform's own optional dep must be in the union too"
    );
    assert!(
        !vendored.contains(&"@esbuild/win32-x64@0.25.12".to_string()),
        "a win32-only package survives no configured platform and must not be downloaded"
    );

    // Not `Some(true)`: fsevents@2.3.3's published tarball (hash-verified
    // against the lockfile's integrity) contains neither an `install` script
    // nor a `binding.gyp`, unlike its registry manifest — see the module doc
    // comment. pudu reads the tarball, which is the ground truth pnpm itself
    // would build from.
    assert_eq!(
        sidecar
            .entries
            .get("fsevents@2.3.3")
            .map(|e| e.has_install_script),
        Some(false),
        "fsevents ships a prebuilt binary in its tarball; no install script or binding.gyp is actually present, despite the registry manifest claiming otherwise"
    );
    assert_eq!(
        sidecar
            .entries
            .get("@babel/parser@7.29.8")
            .map(|e| e.bin.clone()),
        Some(BTreeMap::from([(
            "parser".to_string(),
            "bin/babel-parser.js".to_string()
        )])),
        "a string bin on a scoped package is named after the package minus its scope"
    );
}
