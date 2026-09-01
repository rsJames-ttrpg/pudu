//! Tarball verification and inspection.
//!
//! One pass over the bytes answers everything `pudu.lock` records that
//! `pnpm-lock.yaml` cannot: the sha256 Buck can verify, the compressed size,
//! the bin map, and whether the package runs an install script.
//!
//! Reading `package.json` is not sufficient for either of the last two. The
//! install-script rule fires on a root `binding.gyp` or a `.hooks/` entry as
//! well as on scripts, and `directories.bin` names a directory whose contents
//! decide the bin map. Both need the archive's file list — which is free,
//! since the bytes are already being walked to hash them. See the tarball
//! survey, §2 and §3.

use std::collections::BTreeMap;
use std::io::Read;

use base64::Engine as _;
use serde::Deserialize;
use sha2::{Digest, Sha256, Sha512};

use crate::error::{VendorError, VendorWarning};

/// What inspecting the archive yields.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Inspection {
    /// The archive's single root directory, with no trailing slash — what a
    /// build rule passes to `http_archive(strip_prefix = …)`.
    ///
    /// Usually `package`, but not always: every `@types/*` package nests
    /// under its own display name instead (`estree`, `node v22.20`). It is
    /// recorded rather than recomputed because the sidecar is the only
    /// offline input a later build-rule pass has, and re-deriving it would
    /// mean re-downloading every tarball.
    pub root: String,
    pub bin: BTreeMap<String, String>,
    pub has_install_script: bool,
}

/// A tarball whose sha512 matched the lockfile.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Verified {
    /// Lowercase hex — the form `http_archive` expects.
    pub sha256: String,
    /// Compressed byte count: what Buck will download.
    pub size: u64,
    pub inspection: Inspection,
}

/// The subset of `package.json` that matters here.
///
/// Every field is a raw `Value` rather than a typed shape. A published
/// package can carry anything at all in these keys — `"directories": []`,
/// a non-string script — and one malformed manifest six levels down the
/// dependency tree must not fail the whole vendor pass. `Value` defaults to
/// `Null`, so an absent key and a nonsense key both navigate to `None`.
#[derive(Debug, Default, Deserialize)]
pub(crate) struct Manifest {
    #[serde(default)]
    pub(crate) bin: serde_json::Value,
    #[serde(default)]
    pub(crate) directories: serde_json::Value,
    #[serde(default)]
    pub(crate) scripts: serde_json::Value,
}

/// The archive, reduced to what inspection needs.
pub(crate) struct Archive {
    pub(crate) manifest: Manifest,
    /// The single directory every entry nests under, no trailing slash.
    pub(crate) root: String,
    /// File entries only, relative to the package root, `/`-separated.
    pub(crate) entries: Vec<String>,
}

pub fn hex(bytes: &[u8]) -> String {
    use std::fmt::Write as _;
    bytes.iter().fold(String::new(), |mut s, b| {
        let _ = write!(s, "{b:02x}");
        s
    })
}

pub fn sha512_digest(bytes: &[u8]) -> Vec<u8> {
    let mut h = Sha512::new();
    h.update(bytes);
    h.finalize().to_vec()
}

fn sha256_digest(bytes: &[u8]) -> Vec<u8> {
    let mut h = Sha256::new();
    h.update(bytes);
    h.finalize().to_vec()
}

/// Decode a `sha512-<base64>` integrity string to its raw digest.
pub fn decode_integrity(key: &str, integrity: &str) -> Result<Vec<u8>, VendorError> {
    let malformed = || VendorError::MalformedIntegrity {
        key: key.to_string(),
        integrity: integrity.to_string(),
    };
    let b64 = integrity.strip_prefix("sha512-").ok_or_else(malformed)?;
    let raw = base64::engine::general_purpose::STANDARD
        .decode(b64)
        .map_err(|_| malformed())?;
    if raw.len() != 64 {
        return Err(malformed());
    }
    Ok(raw)
}

fn encode_integrity(digest: &[u8]) -> String {
    format!(
        "sha512-{}",
        base64::engine::general_purpose::STANDARD.encode(digest)
    )
}

/// Verify `bytes` against `integrity`, then inspect the archive.
///
/// The order is the trust chain: sha256 is computed only for bytes whose
/// sha512 already matched what pnpm recorded.
pub fn verify_and_inspect(
    key: &str,
    name: &str,
    url: &str,
    bytes: &[u8],
    integrity: &str,
) -> Result<(Verified, Vec<VendorWarning>), VendorError> {
    let expected = decode_integrity(key, integrity)?;
    let actual = sha512_digest(bytes);
    if actual != expected {
        return Err(VendorError::IntegrityMismatch {
            key: key.to_string(),
            url: url.to_string(),
            expected: integrity.to_string(),
            actual: encode_integrity(&actual),
        });
    }

    let archive = read_archive(key, bytes)?;
    let mut warnings = Vec::new();
    let inspection = Inspection {
        bin: resolve_bins(key, name, &archive, &mut warnings),
        has_install_script: has_install_script(&archive),
        root: archive.root.clone(),
    };

    Ok((
        Verified {
            sha256: hex(&sha256_digest(bytes)),
            size: bytes.len() as u64,
            inspection,
        },
        warnings,
    ))
}

/// Walk the archive once, collecting `package.json` and the file list.
fn read_archive(key: &str, bytes: &[u8]) -> Result<Archive, VendorError> {
    let malformed = |reason: String| VendorError::MalformedTarball {
        key: key.to_string(),
        reason,
    };

    let dec = flate2::read::GzDecoder::new(std::io::Cursor::new(bytes));
    let mut ar = tar::Archive::new(dec);
    let mut entries = Vec::new();
    let mut manifest_text: Option<String> = None;
    // Most npm tarballs nest under `package/`, but not all: packages
    // published by DefinitelyTyped's types-publisher (every `@types/*`
    // package) nest under the package's own display name instead — e.g.
    // `@types/estree` unpacks to `estree/`, `@types/node` to `node v22.20/`.
    // What matters is that every entry in one archive shares a single root;
    // that root is whatever a future build-rule pass will pass to
    // `strip_prefix`, not necessarily the literal string `package`.
    let mut root: Option<String> = None;

    for entry in ar
        .entries()
        .map_err(|e| malformed(format!("cannot read tar entries: {e}")))?
    {
        let mut entry = entry.map_err(|e| malformed(format!("cannot read tar entry: {e}")))?;
        let entry_type = entry.header().entry_type();
        // Metadata entries carry a synthetic name that is not part of the
        // archive's directory tree — `pax_global_header` is the common one,
        // emitted by GNU tar's default pax format and by `git archive`. Skip
        // them before the root check, or a perfectly valid archive is
        // rejected for having "inconsistent root directories".
        if !entry_type.is_file() && !entry_type.is_dir() {
            continue;
        }
        let is_file = entry_type.is_file();
        let path = entry
            .path()
            .map_err(|e| malformed(format!("cannot read entry path: {e}")))?
            .to_string_lossy()
            .replace('\\', "/");

        let mut parts = path.splitn(2, '/');
        let this_root = parts.next().unwrap_or_default();
        match &root {
            None => root = Some(this_root.to_string()),
            Some(r) if r != this_root => {
                return Err(malformed(format!(
                    "archive has inconsistent root directories: `{r}` and `{this_root}`"
                )));
            }
            Some(_) => {}
        }
        let Some(rel) = parts.next().filter(|r| !r.is_empty()) else {
            continue; // the root directory entry itself
        };

        if !is_file {
            continue;
        }
        if rel == "package.json" {
            let mut s = String::new();
            entry
                .read_to_string(&mut s)
                .map_err(|e| malformed(format!("cannot read package.json: {e}")))?;
            manifest_text = Some(s);
        }
        entries.push(rel.to_string());
    }

    let text = manifest_text.ok_or_else(|| VendorError::MissingPackageJson {
        key: key.to_string(),
    })?;
    let manifest: Manifest = serde_json::from_str(&text)
        .map_err(|e| malformed(format!("package.json is not valid JSON: {e}")))?;

    entries.sort();
    // Unreachable with `manifest_text` already unwrapped above: a
    // `package.json` was found, so at least one entry set the root.
    let root = root.unwrap_or_default();
    Ok(Archive {
        manifest,
        root,
        entries,
    })
}

/// pnpm's rule, from `@pnpm/building.pkg-requires-build`: install scripts, a
/// root `binding.gyp`, or anything under `.hooks/`. Survey §2.
fn has_install_script(archive: &Archive) -> bool {
    let script = |k: &str| {
        archive
            .manifest
            .scripts
            .get(k)
            .and_then(serde_json::Value::as_str)
            .is_some_and(|s| !s.is_empty())
    };
    // `Value::get` on `Null` is `None`, so a manifest with no `scripts` key
    // at all takes the same path as one whose `scripts` is not an object.
    script("preinstall")
        || script("install")
        || script("postinstall")
        || archive
            .entries
            .iter()
            .any(|e| e == "binding.gyp" || e.starts_with(".hooks/"))
}

/// `@pnpm/package-bins`, reproduced. Survey §3.
///
/// A `bin` field takes precedence over `directories.bin` even when it yields
/// nothing — pnpm's `if (manifest.bin)` branch is already taken by then, so
/// there is no fallback. The branch is a JavaScript truthiness test, so
/// `null`, `false`, and `""` fall through to `directories.bin` while `42` and
/// `[]` do not.
fn resolve_bins(
    key: &str,
    name: &str,
    archive: &Archive,
    warnings: &mut Vec<VendorWarning>,
) -> BTreeMap<String, String> {
    let mut out = BTreeMap::new();

    match &archive.manifest.bin {
        serde_json::Value::String(path) if !path.is_empty() => {
            insert_bin(key, command_name(name), path, &mut out, warnings);
        }
        serde_json::Value::Object(map) => {
            for (raw, value) in map {
                match value.as_str() {
                    Some(path) => insert_bin(key, command_name(raw), path, &mut out, warnings),
                    None => warnings.push(VendorWarning::NonStringBinValue {
                        key: key.to_string(),
                        name: raw.clone(),
                    }),
                }
            }
        }
        // Present but unusable. pnpm's `if (manifest.bin)` branch is already
        // taken, so there is no fall back to `directories.bin`.
        //
        // A number is truthy unless it's zero: `0` (and `-0`) is JavaScript's
        // only falsy number, and JSON cannot express `NaN`, so `!= Some(0.0)`
        // covers the whole falsy-number set here.
        serde_json::Value::Bool(true) | serde_json::Value::Array(_) => {}
        serde_json::Value::Number(n) if n.as_f64() != Some(0.0) => {}
        // Absent, or falsy in JavaScript's sense (`null`, `false`, `""`),
        // which is what `if (manifest.bin)` actually tests.
        _ => {
            if let Some(dir) = archive
                .manifest
                .directories
                .get("bin")
                .and_then(serde_json::Value::as_str)
                && let Some(normalized) = contained_path(dir)
            {
                let prefix = format!("{normalized}/");
                // `archive.entries` is sorted, so "last wins" on a collision
                // is deterministic rather than dependent on archive order.
                for entry in &archive.entries {
                    if let Some(rest) = entry.strip_prefix(&prefix) {
                        let base = rest.rsplit('/').next().unwrap_or(rest);
                        insert_named(key, base, entry, &mut out, warnings);
                    }
                }
            }
        }
    }

    out
}

/// `@scope/tool` → `tool`; `tool` → `tool`.
///
/// pnpm slices from `indexOf('/') + 1`, which for a name with no slash is
/// `slice(0)` — the whole string.
fn command_name(raw: &str) -> &str {
    if raw.starts_with('@') {
        raw.split_once('/').map_or(raw, |(_, rest)| rest)
    } else {
        raw
    }
}

/// Whether `s` survives pnpm's `binName !== encodeURIComponent(binName)`
/// check: the unreserved set `encodeURIComponent` leaves alone.
///
/// An empty name passes, exactly as it does in JavaScript.
fn is_url_safe(s: &str) -> bool {
    s.chars().all(|c| {
        c.is_ascii_alphanumeric()
            || matches!(c, '-' | '_' | '.' | '!' | '~' | '*' | '\'' | '(' | ')')
    })
}

/// Normalize a package-relative path, returning `None` when it escapes the
/// package root.
///
/// A leading `/` is *not* an escape: pnpm joins onto the package path, so
/// `/etc/passwd` lands inside the package and `is-subdir` accepts it.
fn contained_path(rel: &str) -> Option<String> {
    let mut out: Vec<&str> = Vec::new();
    for part in rel.split('/') {
        match part {
            "" | "." => {}
            ".." => {
                out.pop()?;
            }
            p => out.push(p),
        }
    }
    (!out.is_empty()).then(|| out.join("/"))
}

fn insert_bin(
    key: &str,
    name: &str,
    path: &str,
    out: &mut BTreeMap<String, String>,
    warnings: &mut Vec<VendorWarning>,
) {
    if !is_url_safe(name) && name != "$" {
        warnings.push(VendorWarning::BinNameRejected {
            key: key.to_string(),
            name: name.to_string(),
        });
        return;
    }
    let Some(normalized) = contained_path(path) else {
        warnings.push(VendorWarning::BinPathEscapes {
            key: key.to_string(),
            name: name.to_string(),
            path: path.to_string(),
        });
        return;
    };
    if out.insert(name.to_string(), normalized).is_some() {
        warnings.push(VendorWarning::BinNameCollision {
            key: key.to_string(),
            name: name.to_string(),
        });
    }
}

/// The `directories.bin` path: the name comes from the filesystem, so the
/// URL-safe and containment checks that guard a manifest-declared name do
/// not apply — the entry is already inside the archive.
fn insert_named(
    key: &str,
    name: &str,
    path: &str,
    out: &mut BTreeMap<String, String>,
    warnings: &mut Vec<VendorWarning>,
) {
    if out.insert(name.to_string(), path.to_string()).is_some() {
        warnings.push(VendorWarning::BinNameCollision {
            key: key.to_string(),
            name: name.to_string(),
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    const URL: &str = "https://registry.example/p/-/p-1.0.0.tgz";

    /// Build a gzipped tar in memory. Paths are given relative to the
    /// package root and nested under `package/` here.
    pub(crate) fn tarball(files: &[(&str, &str)]) -> Vec<u8> {
        rooted_tarball("package", files)
    }

    pub(crate) fn rooted_tarball(root: &str, files: &[(&str, &str)]) -> Vec<u8> {
        let mut ar = tar::Builder::new(Vec::new());
        for (path, body) in files {
            let mut h = tar::Header::new_gnu();
            h.set_size(body.len() as u64);
            h.set_mode(0o644);
            h.set_cksum();
            ar.append_data(&mut h, format!("{root}/{path}"), body.as_bytes())
                .unwrap();
        }
        let tar_bytes = ar.into_inner().unwrap();
        let mut gz = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
        gz.write_all(&tar_bytes).unwrap();
        gz.finish().unwrap()
    }

    pub(crate) fn integrity_of(bytes: &[u8]) -> String {
        use base64::Engine as _;
        format!(
            "sha512-{}",
            base64::engine::general_purpose::STANDARD.encode(sha512_digest(bytes))
        )
    }

    fn inspect(files: &[(&str, &str)]) -> Verified {
        let bytes = tarball(files);
        let i = integrity_of(&bytes);
        verify_and_inspect("p@1.0.0", "p", URL, &bytes, &i)
            .unwrap()
            .0
    }

    fn inspect_rooted(root: &str, files: &[(&str, &str)]) -> Verified {
        let bytes = rooted_tarball(root, files);
        let i = integrity_of(&bytes);
        verify_and_inspect("p@1.0.0", "p", URL, &bytes, &i)
            .unwrap()
            .0
    }

    #[test]
    fn a_matching_integrity_verifies() {
        let v = inspect(&[("package.json", r#"{"name":"p"}"#)]);
        assert_eq!(v.inspection.root, "package", "the usual npm shape");
        assert_eq!(v.sha256.len(), 64, "sha256 is 32 bytes of lowercase hex");
        assert!(
            v.sha256
                .chars()
                .all(|c| c.is_ascii_hexdigit() && !c.is_uppercase())
        );
    }

    #[test]
    fn size_is_the_compressed_byte_count() {
        let bytes = tarball(&[("package.json", r#"{"name":"p"}"#)]);
        let i = integrity_of(&bytes);
        let (v, _) = verify_and_inspect("p@1.0.0", "p", URL, &bytes, &i).unwrap();
        assert_eq!(v.size, bytes.len() as u64);
    }

    #[test]
    fn a_mismatched_integrity_names_both_hashes() {
        let bytes = tarball(&[("package.json", r#"{"name":"p"}"#)]);
        let wrong = integrity_of(b"different bytes entirely");
        let err = verify_and_inspect("p@1.0.0", "p", URL, &bytes, &wrong).unwrap_err();
        let VendorError::IntegrityMismatch {
            expected,
            actual,
            key,
            url,
        } = &err
        else {
            panic!("wrong variant: {err:?}");
        };
        assert_eq!(key, "p@1.0.0");
        assert_eq!(url, URL, "the error must name the URL it fetched");
        assert_eq!(*expected, wrong);
        assert_eq!(*actual, integrity_of(&bytes));
        assert_ne!(expected, actual);
    }

    #[test]
    fn a_consistent_non_package_root_is_accepted() {
        // `@types/*` packages, published by DefinitelyTyped's
        // types-publisher, nest under the package's display name rather than
        // `package/` — e.g. `@types/estree` unpacks to `estree/`. That is a
        // real, current shape on the registry (see the vendor oracle), not a
        // malformed archive, so it must verify like any other.
        let v = inspect_rooted("estree", &[("package.json", r#"{"name":"@types/estree"}"#)]);
        assert_eq!(v.inspection.bin, BTreeMap::new());
        assert_eq!(
            v.inspection.root, "estree",
            "the archive's actual root must be recorded, not assumed to be `package`"
        );
    }

    #[test]
    fn inconsistent_root_directories_are_rejected() {
        // A genuinely malformed archive: two entries disagree about what
        // directory they are nested under.
        let mut ar = tar::Builder::new(Vec::new());
        for (root, path, body) in [
            ("package", "package.json", "{}"),
            ("other", "index.js", "module.exports = 1;\n"),
        ] {
            let mut h = tar::Header::new_gnu();
            h.set_size(body.len() as u64);
            h.set_mode(0o644);
            h.set_cksum();
            ar.append_data(&mut h, format!("{root}/{path}"), body.as_bytes())
                .unwrap();
        }
        let tar_bytes = ar.into_inner().unwrap();
        let mut gz = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
        gz.write_all(&tar_bytes).unwrap();
        let bytes = gz.finish().unwrap();
        let i = integrity_of(&bytes);
        let err = verify_and_inspect("p@1.0.0", "p", URL, &bytes, &i).unwrap_err();
        let VendorError::MalformedTarball { reason, .. } = &err else {
            panic!("wrong variant: {err:?}");
        };
        assert!(
            reason.contains("package") && reason.contains("other"),
            "the reason must name both roots it found: {reason}"
        );
    }

    #[test]
    fn a_pax_global_header_does_not_poison_the_root_check() {
        // GNU tar's default pax format and `git archive` both prepend a
        // `pax_global_header` entry. It is metadata, not a member of the
        // directory tree, so it must not be read as a second root — a
        // private-registry tarball built that way is perfectly valid.
        let mut ar = tar::Builder::new(Vec::new());
        let mut gh = tar::Header::new_ustar();
        let global = "52 comment=0000000000000000000000000000000000000000\n";
        gh.set_size(global.len() as u64);
        gh.set_entry_type(tar::EntryType::XGlobalHeader);
        gh.set_mode(0o644);
        gh.set_cksum();
        ar.append_data(&mut gh, "pax_global_header", global.as_bytes())
            .unwrap();
        for (path, body) in [("package.json", r#"{"name":"p"}"#), ("index.js", "")] {
            let mut h = tar::Header::new_gnu();
            h.set_size(body.len() as u64);
            h.set_mode(0o644);
            h.set_cksum();
            ar.append_data(&mut h, format!("package/{path}"), body.as_bytes())
                .unwrap();
        }
        let tar_bytes = ar.into_inner().unwrap();
        let mut gz = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
        gz.write_all(&tar_bytes).unwrap();
        let bytes = gz.finish().unwrap();
        let i = integrity_of(&bytes);

        let (v, _) = verify_and_inspect("p@1.0.0", "p", URL, &bytes, &i)
            .unwrap_or_else(|e| panic!("a pax global header must not be a second root: {e}"));
        assert_eq!(v.inspection.bin, BTreeMap::new());
        assert_eq!(v.inspection.root, "package");
    }

    #[test]
    fn a_tarball_without_a_manifest_is_rejected() {
        let bytes = tarball(&[("index.js", "module.exports = 1;\n")]);
        let i = integrity_of(&bytes);
        let err = verify_and_inspect("p@1.0.0", "p", URL, &bytes, &i).unwrap_err();
        assert!(
            matches!(err, VendorError::MissingPackageJson { .. }),
            "{err:?}"
        );
    }

    #[test]
    fn unparseable_json_is_a_malformed_tarball_not_a_panic() {
        let bytes = tarball(&[("package.json", "{not json")]);
        let i = integrity_of(&bytes);
        let err = verify_and_inspect("p@1.0.0", "p", URL, &bytes, &i).unwrap_err();
        assert!(
            matches!(err, VendorError::MalformedTarball { .. }),
            "{err:?}"
        );
    }

    #[test]
    fn an_integrity_that_is_not_sha512_is_rejected() {
        let bytes = tarball(&[("package.json", "{}")]);
        for bad in ["sha1-abc", "not-an-integrity", "sha512", "sha512-!!!!"] {
            let err = verify_and_inspect("p@1.0.0", "p", URL, &bytes, bad).unwrap_err();
            assert!(
                matches!(err, VendorError::MalformedIntegrity { .. }),
                "{bad} should be rejected, got {err:?}"
            );
        }
    }

    // --- has_install_script: each trigger in isolation --------------------

    #[test]
    fn no_scripts_and_no_marker_files_means_no_install_script() {
        let v = inspect(&[
            (
                "package.json",
                r#"{"name":"p","scripts":{"test":"mocha","build":"tsc"}}"#,
            ),
            ("index.js", ""),
        ]);
        assert!(!v.inspection.has_install_script);
    }

    #[test]
    fn each_install_script_key_triggers_on_its_own() {
        for key in ["preinstall", "install", "postinstall"] {
            let json = format!(r#"{{"name":"p","scripts":{{"{key}":"node x.js"}}}}"#);
            let v = inspect(&[("package.json", &json)]);
            assert!(v.inspection.has_install_script, "{key} must trigger");
        }
    }

    #[test]
    fn a_lifecycle_script_that_is_not_an_install_hook_does_not_trigger() {
        let v = inspect(&[(
            "package.json",
            r#"{"name":"p","scripts":{"prepare":"husky","prepublishOnly":"x"}}"#,
        )]);
        assert!(!v.inspection.has_install_script);
    }

    #[test]
    fn an_empty_install_script_does_not_trigger() {
        let v = inspect(&[("package.json", r#"{"name":"p","scripts":{"install":""}}"#)]);
        assert!(!v.inspection.has_install_script);
    }

    #[test]
    fn binding_gyp_triggers_with_no_scripts_at_all() {
        // fsevents@2.3.3 is the live instance of this in our own fixture.
        let v = inspect(&[("package.json", r#"{"name":"p"}"#), ("binding.gyp", "{}")]);
        assert!(v.inspection.has_install_script);
    }

    #[test]
    fn binding_gyp_below_the_root_does_not_trigger() {
        let v = inspect(&[
            ("package.json", r#"{"name":"p"}"#),
            ("src/binding.gyp", "{}"),
        ]);
        assert!(!v.inspection.has_install_script);
    }

    #[test]
    fn a_hooks_file_triggers() {
        let v = inspect(&[
            ("package.json", r#"{"name":"p"}"#),
            (".hooks/postinstall", "#!/bin/sh\n"),
        ]);
        assert!(v.inspection.has_install_script);
    }

    #[test]
    fn a_file_merely_starting_with_dot_hooks_does_not_trigger() {
        // pnpm's regex is `^\.hooks[\\/]`, so the separator is required.
        let v = inspect(&[("package.json", r#"{"name":"p"}"#), (".hooksrc", "{}")]);
        assert!(!v.inspection.has_install_script);
    }

    // --- resolve_bins -------------------------------------------------

    fn bins(files: &[(&str, &str)], name: &str) -> BTreeMap<String, String> {
        let bytes = tarball(files);
        let i = integrity_of(&bytes);
        verify_and_inspect("p@1.0.0", name, URL, &bytes, &i)
            .unwrap()
            .0
            .inspection
            .bin
    }

    fn bins_with_warnings(
        files: &[(&str, &str)],
        name: &str,
    ) -> (BTreeMap<String, String>, Vec<VendorWarning>) {
        let bytes = tarball(files);
        let i = integrity_of(&bytes);
        let (v, w) = verify_and_inspect("p@1.0.0", name, URL, &bytes, &i).unwrap();
        (v.inspection.bin, w)
    }

    #[test]
    fn no_bin_field_yields_no_commands() {
        assert!(bins(&[("package.json", r#"{"name":"p"}"#)], "p").is_empty());
    }

    #[test]
    fn a_string_bin_is_named_after_the_package() {
        let b = bins(&[("package.json", r#"{"name":"p","bin":"cli.js"}"#)], "p");
        assert_eq!(b, BTreeMap::from([("p".to_string(), "cli.js".to_string())]));
    }

    #[test]
    fn a_string_bin_on_a_scoped_package_drops_the_scope() {
        // @babel/parser is the live instance: its command is `parser`.
        let b = bins(
            &[(
                "package.json",
                r#"{"name":"@babel/parser","bin":"./bin/babel-parser.js"}"#,
            )],
            "@babel/parser",
        );
        assert_eq!(
            b,
            BTreeMap::from([("parser".to_string(), "bin/babel-parser.js".to_string())]),
            "the scope is stripped from the command and `./` from the path"
        );
    }

    #[test]
    fn an_object_bin_keeps_every_key() {
        let b = bins(
            &[(
                "package.json",
                r#"{"name":"p","bin":{"one":"a.js","two":"b/c.js"}}"#,
            )],
            "p",
        );
        assert_eq!(
            b,
            BTreeMap::from([
                ("one".to_string(), "a.js".to_string()),
                ("two".to_string(), "b/c.js".to_string()),
            ])
        );
    }

    #[test]
    fn an_object_bin_key_is_scope_stripped_too() {
        let b = bins(
            &[(
                "package.json",
                r#"{"name":"p","bin":{"@scope/tool":"t.js"}}"#,
            )],
            "p",
        );
        assert_eq!(
            b,
            BTreeMap::from([("tool".to_string(), "t.js".to_string())])
        );
    }

    #[test]
    fn a_name_that_is_not_url_safe_is_dropped_with_a_warning() {
        let (b, w) = bins_with_warnings(
            &[(
                "package.json",
                r#"{"name":"p","bin":{"a b":"x.js","ok":"y.js"}}"#,
            )],
            "p",
        );
        assert_eq!(b, BTreeMap::from([("ok".to_string(), "y.js".to_string())]));
        assert!(
            w.iter()
                .any(|x| matches!(x, VendorWarning::BinNameRejected { name, .. } if name == "a b")),
            "{w:?}"
        );
    }

    #[test]
    fn the_dollar_name_is_exempt_from_the_url_safe_rule() {
        let b = bins(
            &[("package.json", r#"{"name":"p","bin":{"$":"x.js"}}"#)],
            "p",
        );
        assert_eq!(b, BTreeMap::from([("$".to_string(), "x.js".to_string())]));
    }

    #[test]
    fn a_path_escaping_the_package_is_dropped_with_a_warning() {
        let (b, w) = bins_with_warnings(
            &[(
                "package.json",
                r#"{"name":"p","bin":{"evil":"../../../etc/passwd","ok":"y.js"}}"#,
            )],
            "p",
        );
        assert_eq!(b, BTreeMap::from([("ok".to_string(), "y.js".to_string())]));
        assert!(
            w.iter()
                .any(|x| matches!(x, VendorWarning::BinPathEscapes { name, .. } if name == "evil")),
            "{w:?}"
        );
    }

    #[test]
    fn a_path_that_climbs_then_returns_stays_inside() {
        let b = bins(
            &[(
                "package.json",
                r#"{"name":"p","bin":{"ok":"lib/../cli.js"}}"#,
            )],
            "p",
        );
        assert_eq!(
            b,
            BTreeMap::from([("ok".to_string(), "cli.js".to_string())])
        );
    }

    #[test]
    fn a_non_string_bin_value_is_dropped_with_a_warning() {
        let (b, w) = bins_with_warnings(
            &[(
                "package.json",
                r#"{"name":"p","bin":{"bad":42,"ok":"y.js"}}"#,
            )],
            "p",
        );
        assert_eq!(b, BTreeMap::from([("ok".to_string(), "y.js".to_string())]));
        assert!(
            w.iter().any(
                |x| matches!(x, VendorWarning::NonStringBinValue { name, .. } if name == "bad")
            ),
            "{w:?}"
        );
    }

    #[test]
    fn a_bin_field_that_is_neither_string_nor_object_yields_nothing() {
        // pnpm's `Object.entries(42)` is `[]`, and it does not fall back to
        // directories.bin — the `if (manifest.bin)` branch is already taken.
        let b = bins(
            &[
                (
                    "package.json",
                    r#"{"name":"p","bin":42,"directories":{"bin":"tools"}}"#,
                ),
                ("tools/t.js", ""),
            ],
            "p",
        );
        assert!(b.is_empty(), "{b:?}");
    }

    #[test]
    fn a_zero_bin_field_falls_through_to_directories_bin() {
        // Unlike `42` above, `0` is JavaScript-falsy, so `if (manifest.bin)`
        // is not taken and `directories.bin` is consulted instead.
        let b = bins(
            &[
                (
                    "package.json",
                    r#"{"name":"p","bin":0,"directories":{"bin":"tools"}}"#,
                ),
                ("tools/t.js", ""),
            ],
            "p",
        );
        assert_eq!(
            b,
            BTreeMap::from([("t.js".to_string(), "tools/t.js".to_string())])
        );
    }

    #[test]
    fn directories_bin_is_walked_recursively_and_keyed_on_basename() {
        let b = bins(
            &[
                (
                    "package.json",
                    r#"{"name":"p","directories":{"bin":"tools"}}"#,
                ),
                ("tools/one.js", ""),
                ("tools/nested/two.js", ""),
            ],
            "p",
        );
        assert_eq!(
            b,
            BTreeMap::from([
                ("one.js".to_string(), "tools/one.js".to_string()),
                ("two.js".to_string(), "tools/nested/two.js".to_string()),
            ]),
            "nested files collapse to bare basenames"
        );
    }

    #[test]
    fn directories_bin_is_ignored_when_bin_is_present() {
        let b = bins(
            &[
                (
                    "package.json",
                    r#"{"name":"p","bin":"cli.js","directories":{"bin":"tools"}}"#,
                ),
                ("tools/t.js", ""),
            ],
            "p",
        );
        assert_eq!(b, BTreeMap::from([("p".to_string(), "cli.js".to_string())]));
    }

    #[test]
    fn a_bin_name_collision_keeps_the_last_and_warns() {
        let (b, w) = bins_with_warnings(
            &[
                (
                    "package.json",
                    r#"{"name":"p","directories":{"bin":"tools"}}"#,
                ),
                ("tools/a/dup.js", "first"),
                ("tools/b/dup.js", "second"),
            ],
            "p",
        );
        assert_eq!(
            b,
            BTreeMap::from([("dup.js".to_string(), "tools/b/dup.js".to_string())]),
            "entries are visited in sorted order, so the last one wins deterministically"
        );
        assert!(
            w.iter().any(
                |x| matches!(x, VendorWarning::BinNameCollision { name, .. } if name == "dup.js")
            ),
            "{w:?}"
        );
    }
}
