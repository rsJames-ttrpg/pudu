//! The pnpm snapshot-key grammar.
//!
//! ```text
//! key     := name "@" version peers?
//! name    := ("@" scope "/")? ident
//! peers   := "(" key ")" peers?
//! ```
//!
//! A peer is itself a full key, so the grammar is recursive and the parens
//! balance to arbitrary depth — real lockfiles nest three levels and reach
//! 422 characters. Two shortcuts are wrong and both are guarded by tests:
//! splitting on the first `(` ignores nesting, and splitting the head on the
//! first `@` breaks scoped names.

use std::fmt;

use sha2::{Digest, Sha256};

/// Maximum peer-nesting depth. Real lockfiles nest three levels; 64 is
/// generous headroom. Without a cap, parsing is superlinear in nesting depth
/// (measured: ~4s at 4000 levels, no return within two minutes at 100,000) —
/// an untrusted or malformed lockfile could hang the process rather than
/// erroring, so depth is capped explicitly.
pub const MAX_PEER_DEPTH: usize = 64;

/// Maximum total key length in bytes. Checked once, at [`SnapshotKey::parse`]
/// — the public entry point — not in the recursive `parse_inner`, so a key's
/// nested peers are not each re-checked against it. The real-world corpus's
/// longest key is 422 bytes; 8192 leaves roughly nineteen times headroom.
///
/// [`MAX_PEER_DEPTH`] alone does not bound worst-case cost: a key can stay
/// under the depth cap while still being enormous (many peers at shallow
/// depth, or long identifiers), and parsing time was observed to grow
/// quadratically with key length even with the depth cap in place — a
/// 40,000-level, ~549KB adversarial key took over 20s despite the cap
/// correctly firing at depth 65 every time. The two caps guard different
/// shapes (depth vs. raw length) and both are cheap to check.
pub const MAX_KEY_LEN: usize = 8192;

/// A parsed snapshot key.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SnapshotKey {
    pub name: String,
    pub version: String,
    /// In lockfile order. **Never sorted** — pnpm's directory naming hashes
    /// this order, so re-sorting would diverge from the real virtual store.
    pub peers: Vec<SnapshotKey>,
}

/// Why a key failed to parse.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KeyParseError {
    pub key: String,
    pub offset: usize,
    pub reason: &'static str,
}

impl fmt::Display for KeyParseError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "invalid snapshot key `{}` at byte {}: {}",
            self.key, self.offset, self.reason
        )
    }
}

impl std::error::Error for KeyParseError {}

impl SnapshotKey {
    /// Parse a snapshot key.
    pub fn parse(s: &str) -> Result<Self, KeyParseError> {
        if s.len() > MAX_KEY_LEN {
            return Err(KeyParseError {
                key: s.to_string(),
                offset: MAX_KEY_LEN,
                reason: "key exceeds the maximum length of 8192 bytes",
            });
        }
        Self::parse_inner(s, s, 0, 0)
    }

    /// Parse the substring `s`, which begins at absolute byte offset `base`
    /// within `whole`. `depth` is this key's peer-nesting depth (0 for the
    /// top-level key), checked against [`MAX_PEER_DEPTH`] before recursing
    /// into peers.
    ///
    /// Every locally-computed offset in this function is relative to `s`; it
    /// must be added to `base` before it reaches a `KeyParseError`, since
    /// `KeyParseError::offset` is always documented (and tested) as an
    /// absolute offset into the original, top-level `whole`.
    fn parse_inner(s: &str, whole: &str, base: usize, depth: usize) -> Result<Self, KeyParseError> {
        let err = |local_offset: usize, reason: &'static str| KeyParseError {
            key: whole.to_string(),
            offset: base + local_offset,
            reason,
        };

        let suffix_start = top_level_paren(s, whole, base)?;
        let (head, suffix) = match suffix_start {
            Some(i) => (&s[..i], &s[i..]),
            None => (s, ""),
        };
        let suffix_base = base + head.len();

        // The split is the LAST '@' at index > 0. Index 0 is excluded because
        // a scoped name starts with '@'.
        let at = head
            .char_indices()
            .filter(|(i, c)| *c == '@' && *i > 0)
            .map(|(i, _)| i)
            .next_back()
            .ok_or_else(|| err(0, "expected `name@version`"))?;

        let (name, version) = (&head[..at], &head[at + 1..]);
        if name.is_empty() {
            return Err(err(0, "empty package name"));
        }
        if version.is_empty() {
            return Err(err(at + 1, "empty version"));
        }

        Ok(Self {
            name: name.to_string(),
            version: version.to_string(),
            peers: parse_peers(suffix, whole, suffix_base, depth)?,
        })
    }

    /// The key with its peer suffix removed — the `packages:` table's key.
    pub fn base(&self) -> String {
        format!("{}@{}", self.name, self.version)
    }

    /// Render back to lockfile form, peer order preserved.
    pub fn canonical(&self) -> String {
        let mut out = self.base();
        for p in &self.peers {
            out.push('(');
            out.push_str(&p.canonical());
            out.push(')');
        }
        out
    }

    /// The Buck target name for this key. See [`target_name`].
    pub fn target_name(&self) -> String {
        target_name(&self.canonical())
    }
}

/// pnpm's `virtual-store-dir-max-length` default.
///
/// Hardcoded for v0.1.0. A project that has changed the pnpm setting gets
/// names that do not match its own store — cosmetic only, since the names
/// stay internally consistent.
pub const MAX_LEN_WITHOUT_HASH: usize = 120;

/// Characters pnpm escapes to `+`: the Windows-illegal set plus `#`.
const ILLEGAL: &[char] = &['\\', '/', ':', '*', '?', '"', '<', '>', '|', '#'];

/// The Buck target name for a snapshot key.
///
/// A direct port of `depPathToFilename` from `@pnpm/dependency-path`
/// v1001.1.10, so generated names are byte-identical to the directory names
/// in a real `node_modules/.pnpm/`. Verified against 1363 real directories;
/// see the field survey. **Do not "improve" this** — any deviation breaks
/// greppability against the real store, which is the whole point.
pub fn target_name(dep_path: &str) -> String {
    // Truncation below is by UTF-8 *byte* budget; pnpm's JS truncates by
    // UTF-16 code unit. For a multi-byte name near the boundary this yields a
    // shorter stem than pnpm would — never longer. Unreachable in practice:
    // the npm registry admits only ASCII package names, so no real lockfile
    // key can trigger it. A synthetic or hostile lockfile could, and the
    // result is a name that stays internally consistent but no longer matches
    // that package's directory in a real virtual store.
    //
    // pnpm strips a leading '/' (a legacy v5 dep-path form) before escaping.
    // The reference does this only for non-`file:` paths, but a `file:` path
    // never starts with '/', and the reference's `file:` branch replaces just
    // the first ':' — which the escape below subsumes by replacing all of
    // them. So the unconditional form here is equivalent, not a shortcut;
    // verified by differential fuzzing against the reference implementation.
    let s = dep_path.strip_prefix('/').unwrap_or(dep_path);
    let mut filename: String = s
        .chars()
        .map(|c| if ILLEGAL.contains(&c) { '+' } else { c })
        .collect();

    if filename.contains('(') {
        // Order matters: drop the trailing ')' first, so `)(` -> `_` does not
        // have to account for it.
        if let Some(stripped) = filename.strip_suffix(')') {
            filename = stripped.to_string();
        }
        filename = filename.replace(")(", "_").replace(['(', ')'], "_");
    }

    // Uppercase forces the hash regardless of length: a case-insensitive
    // filesystem would otherwise collapse two distinct packages.
    let needs_hash = filename.len() > MAX_LEN_WITHOUT_HASH
        || (filename != filename.to_lowercase() && !filename.starts_with("file+"));

    if needs_hash {
        let digest = format!("{:x}", Sha256::digest(filename.as_bytes()));
        let keep = MAX_LEN_WITHOUT_HASH - 33;
        // Truncate on a char boundary. Package names are ASCII in practice,
        // but a non-ASCII name must not panic.
        let stem = if filename.len() > keep {
            let mut end = keep;
            while !filename.is_char_boundary(end) {
                end -= 1;
            }
            &filename[..end]
        } else {
            &filename[..]
        };
        return format!("{stem}_{}", &digest[..32]);
    }

    filename
}

impl fmt::Display for SnapshotKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.canonical())
    }
}

/// Index of the first `(` at depth 0 (relative to `s`), or `None`. Errors on
/// unbalanced parens. `base` is `s`'s absolute offset within `whole`, used
/// only to make error offsets absolute — the returned `Some` index stays
/// local to `s`, since the caller uses it to slice `s`.
fn top_level_paren(s: &str, whole: &str, base: usize) -> Result<Option<usize>, KeyParseError> {
    let mut paren_depth = 0usize;
    let mut first = None;
    for (i, c) in s.char_indices() {
        match c {
            '(' => {
                if paren_depth == 0 && first.is_none() {
                    first = Some(i);
                }
                paren_depth += 1;
            }
            ')' => {
                paren_depth = paren_depth.checked_sub(1).ok_or(KeyParseError {
                    key: whole.to_string(),
                    offset: base + i,
                    reason: "unbalanced `)`",
                })?;
            }
            _ => {}
        }
    }
    if paren_depth != 0 {
        return Err(KeyParseError {
            key: whole.to_string(),
            offset: base + s.len(),
            reason: "unbalanced `(`",
        });
    }
    Ok(first)
}

/// Split `(a)(b)` into its depth-0 groups and parse each recursively. `base`
/// is `suffix`'s absolute offset within `whole`, threaded into each peer's
/// recursive parse so its error offsets land on the right byte of `whole`
/// rather than the peer's own local substring. `depth` is the nesting depth
/// of the key `suffix` hangs off of; each peer is one level deeper, checked
/// against [`MAX_PEER_DEPTH`] before recursing so a pathologically nested key
/// errors instead of hanging.
fn parse_peers(
    suffix: &str,
    whole: &str,
    base: usize,
    depth: usize,
) -> Result<Vec<SnapshotKey>, KeyParseError> {
    let mut peers = Vec::new();
    let mut paren_depth = 0usize;
    let mut start = None;
    for (i, c) in suffix.char_indices() {
        match c {
            '(' => {
                if paren_depth == 0 {
                    start = Some(i + 1);
                }
                paren_depth += 1;
            }
            ')' => {
                paren_depth -= 1;
                if paren_depth == 0 {
                    let from = start.take().expect("a close at depth 0 follows an open");
                    let peer_depth = depth + 1;
                    if peer_depth > MAX_PEER_DEPTH {
                        return Err(KeyParseError {
                            key: whole.to_string(),
                            offset: base + from,
                            reason: "peer nesting exceeds the maximum depth of 64",
                        });
                    }
                    peers.push(SnapshotKey::parse_inner(
                        &suffix[from..i],
                        whole,
                        base + from,
                        peer_depth,
                    )?);
                }
            }
            _ => {}
        }
    }
    Ok(peers)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Verbatim from a real lockfile — the corpus's longest key at 272 chars.
    /// Nested three deep. If this parses, the grammar is right.
    const LONG_REAL_KEY: &str = "@sveltejs/kit@2.50.1(@sveltejs/vite-plugin-svelte@6.2.4(svelte@5.49.1)(vite@7.3.1(@types/node@22.19.7)(jiti@2.6.1)(lightningcss@1.30.2)(terser@5.46.0)))(svelte@5.49.1)(vite@7.3.1(@types/node@22.19.7)(jiti@2.6.1)(lightningcss@1.30.2)(terser@5.46.0))";

    #[test]
    fn parses_bare_name() {
        let k = SnapshotKey::parse("svelte@5.49.1").unwrap();
        assert_eq!(k.name, "svelte");
        assert_eq!(k.version, "5.49.1");
        assert!(k.peers.is_empty());
    }

    #[test]
    fn parses_scoped_name_without_splitting_on_the_leading_at() {
        let k = SnapshotKey::parse("@babel/core@7.28.6").unwrap();
        assert_eq!(k.name, "@babel/core");
        assert_eq!(k.version, "7.28.6");
    }

    #[test]
    fn parses_single_peer() {
        let k = SnapshotKey::parse("react-dom@18.3.1(react@18.3.1)").unwrap();
        assert_eq!(k.name, "react-dom");
        assert_eq!(k.peers.len(), 1);
        assert_eq!(k.peers[0].name, "react");
        assert_eq!(k.peers[0].version, "18.3.1");
    }

    #[test]
    fn parses_nested_peers() {
        // The shortcut of splitting on the first '(' produces garbage here.
        let k = SnapshotKey::parse(
            "eslint-plugin-svelte@3.14.0(eslint@9.39.2(jiti@2.6.1))(svelte@5.49.1)",
        )
        .unwrap();
        assert_eq!(k.name, "eslint-plugin-svelte");
        assert_eq!(k.peers.len(), 2, "two top-level peers, not three");
        assert_eq!(k.peers[0].name, "eslint");
        assert_eq!(k.peers[0].peers.len(), 1, "eslint carries its own peer");
        assert_eq!(k.peers[0].peers[0].name, "jiti");
        assert_eq!(k.peers[1].name, "svelte");
    }

    #[test]
    fn round_trips_the_long_real_key() {
        let k = SnapshotKey::parse(LONG_REAL_KEY).unwrap();
        assert_eq!(k.name, "@sveltejs/kit");
        assert_eq!(k.version, "2.50.1");
        assert_eq!(k.peers.len(), 3);
        assert_eq!(
            k.canonical(),
            LONG_REAL_KEY,
            "canonical() must reproduce the key exactly, peer order included"
        );
    }

    #[test]
    fn base_strips_the_peer_suffix() {
        let k = SnapshotKey::parse(LONG_REAL_KEY).unwrap();
        assert_eq!(k.base(), "@sveltejs/kit@2.50.1");
        let plain = SnapshotKey::parse("svelte@5.49.1").unwrap();
        assert_eq!(plain.base(), "svelte@5.49.1");
    }

    #[test]
    fn peer_order_is_preserved_not_sorted() {
        // pnpm's naming hashes the lockfile's own order; sorting would make
        // every hashed target name diverge from the real virtual store.
        let k = SnapshotKey::parse("x@1.0.0(b@2.0.0)(a@1.0.0)").unwrap();
        assert_eq!(k.peers[0].name, "b", "must NOT be sorted");
        assert_eq!(k.peers[1].name, "a");
        assert_eq!(k.canonical(), "x@1.0.0(b@2.0.0)(a@1.0.0)");
    }

    #[test]
    fn parses_prerelease_and_build_metadata_versions() {
        assert_eq!(
            SnapshotKey::parse("x@1.0.0-rc.1").unwrap().version,
            "1.0.0-rc.1"
        );
        assert_eq!(
            SnapshotKey::parse("x@1.0.0+build.5").unwrap().version,
            "1.0.0+build.5"
        );
    }

    #[test]
    fn rejects_malformed_keys() {
        // (input, expected reason). `@1.0.0` and `@scope/name` share the
        // generic "no split point" message: excluding the index-0 '@' (needed
        // for scoped names) means neither ever finds a valid name/version
        // split, so the parser can't tell them apart from "no '@' at all".
        for (bad, reason) in [
            ("x@1.0.0(a@1", "unbalanced `(`"),
            ("x@1.0.0)", "unbalanced `)`"),
            ("@1.0.0", "expected `name@version`"),
            ("x@", "empty version"),
            ("noatsign", "expected `name@version`"),
            ("@scope/name", "expected `name@version`"),
            ("", "expected `name@version`"),
        ] {
            let e = SnapshotKey::parse(bad).expect_err(&format!("must reject {bad:?}"));
            assert_eq!(e.reason, reason, "wrong reason for {bad:?}: {e}");
        }
    }

    #[test]
    fn error_names_the_key_and_offset() {
        let e = SnapshotKey::parse("x@1.0.0(a@1").unwrap_err();
        assert_eq!(e.key, "x@1.0.0(a@1");
        assert_eq!(e.offset, 11, "offset of the unbalanced `(` at EOF");
    }

    #[test]
    fn nested_peer_error_offset_is_absolute_not_relative() {
        // The empty version's '@' sits inside the peer group, at byte 9 of
        // the whole key; the empty-version error points one past it, at
        // byte 10 — not at the local offset within the recursed substring
        // "y@" (which would incorrectly report byte 1).
        let e = SnapshotKey::parse("x@1.0.0(y@)").unwrap_err();
        assert_eq!(e.key, "x@1.0.0(y@)");
        assert_eq!(e.reason, "empty version");
        assert_eq!(e.offset, 10);
    }

    #[test]
    fn two_level_nested_peer_error_offset_is_absolute() {
        // "x@1.0.0(y@1.0.0(z@))" — the innermost peer "z@" starts at byte
        // 16; its '@' is at byte 17, so the empty-version offset is 18.
        let key = "x@1.0.0(y@1.0.0(z@))";
        let e = SnapshotKey::parse(key).unwrap_err();
        assert_eq!(e.key, key);
        assert_eq!(e.reason, "empty version");
        assert_eq!(e.offset, 18);
    }

    #[test]
    fn peer_nesting_at_the_cap_still_parses() {
        let key = nested_key(MAX_PEER_DEPTH);
        assert!(
            SnapshotKey::parse(&key).is_ok(),
            "nesting exactly at MAX_PEER_DEPTH must still parse"
        );
    }

    #[test]
    fn peer_nesting_past_the_cap_errors_instead_of_hanging() {
        let key = nested_key(MAX_PEER_DEPTH + 1);
        let e = SnapshotKey::parse(&key).unwrap_err();
        assert!(e.reason.contains("depth"), "must name the depth limit: {e}");
    }

    /// Build a key nested `n` levels deep: `p{n}@1.0.0(p{n-1}@1.0.0(...))`.
    fn nested_key(n: usize) -> String {
        let mut s = "p0@1.0.0".to_string();
        for i in 1..=n {
            s = format!("p{i}@1.0.0({s})");
        }
        s
    }

    #[test]
    fn key_just_under_the_length_cap_parses_or_fails_for_its_own_reason() {
        // A single bare name@version padded with identifier characters to
        // just under MAX_KEY_LEN. It must not be rejected for length.
        let padding = "a".repeat(MAX_KEY_LEN - "x@1.0.0".len() - 1);
        let key = format!("x{padding}@1.0.0");
        assert!(key.len() < MAX_KEY_LEN);
        let k = SnapshotKey::parse(&key)
            .unwrap_or_else(|e| panic!("a key under the cap must parse, not fail: {e}"));
        assert_eq!(k.name, format!("x{padding}"));
        assert_eq!(k.version, "1.0.0");
    }

    #[test]
    fn key_over_the_length_cap_is_rejected_for_length() {
        let key = format!("x@{}", "1".repeat(MAX_KEY_LEN));
        assert!(key.len() > MAX_KEY_LEN);
        let e = SnapshotKey::parse(&key).unwrap_err();
        assert_eq!(e.reason, "key exceeds the maximum length of 8192 bytes");
    }

    #[test]
    fn deeply_nested_oversized_key_is_rejected_quickly_not_hung() {
        // The pathological case from review: 40,000 levels deep, ~549KB.
        // The length cap must reject it well before any recursion happens.
        let key = nested_key(40_000);
        let start = std::time::Instant::now();
        let e = SnapshotKey::parse(&key).unwrap_err();
        let elapsed = start.elapsed();
        assert_eq!(e.reason, "key exceeds the maximum length of 8192 bytes");
        assert!(
            elapsed < std::time::Duration::from_millis(50),
            "must reject in well under 50ms, took {elapsed:?}"
        );
    }
}

#[cfg(test)]
mod target_name_tests {
    use super::*;

    #[test]
    fn plain_name_is_unchanged() {
        assert_eq!(target_name("svelte@5.49.1"), "svelte@5.49.1");
    }

    #[test]
    fn scope_slash_becomes_plus() {
        assert_eq!(target_name("@babel/core@7.28.6"), "@babel+core@7.28.6");
    }

    #[test]
    fn escapes_every_illegal_path_character() {
        // The full set, not just '/'.
        for (raw, want) in [
            ("a/b@1.0.0", "a+b@1.0.0"),
            ("a:b@1.0.0", "a+b@1.0.0"),
            ("a*b@1.0.0", "a+b@1.0.0"),
            ("a?b@1.0.0", "a+b@1.0.0"),
            ("a<b@1.0.0", "a+b@1.0.0"),
            ("a>b@1.0.0", "a+b@1.0.0"),
            ("a|b@1.0.0", "a+b@1.0.0"),
            ("a#b@1.0.0", "a+b@1.0.0"),
        ] {
            assert_eq!(target_name(raw), want, "escaping {raw}");
        }
    }

    #[test]
    fn short_peer_sets_flatten_readably_without_hashing() {
        assert_eq!(
            target_name("vite@7.3.1(@types/node@22.19.7)(terser@5.46.0)"),
            "vite@7.3.1_@types+node@22.19.7_terser@5.46.0",
            "short peers must stay readable, not be hashed"
        );
        assert_eq!(
            target_name("react-dom@18.3.1(react@18.3.1)"),
            "react-dom@18.3.1_react@18.3.1"
        );
    }

    #[test]
    fn uppercase_forces_the_hash_path_even_when_short() {
        let got = target_name("MyPkg@1.0.0");
        assert!(got.starts_with("MyPkg@1.0.0_"), "keeps the stem: {got}");
        assert_eq!(
            got.len(),
            "MyPkg@1.0.0".len() + 33,
            "stem + '_' + 32 hex: {got}"
        );
        assert_ne!(got, "MyPkg@1.0.0", "must not pass through unhashed");
    }

    #[test]
    fn long_names_are_truncated_to_exactly_120() {
        let long = "@sveltejs/kit@2.50.1(@sveltejs/vite-plugin-svelte@6.2.4(svelte@5.49.1)(vite@7.3.1(@types/node@22.19.7)(jiti@2.6.1)(lightningcss@1.30.2)(terser@5.46.0)))(svelte@5.49.1)(vite@7.3.1(@types/node@22.19.7)(jiti@2.6.1)(lightningcss@1.30.2)(terser@5.46.0))";
        let got = target_name(long);
        assert_eq!(got.len(), 120, "pnpm's max length: {got}");
        assert!(
            got.starts_with("@sveltejs+kit@2.50.1_"),
            "readable stem survives: {got}"
        );
        let hash = &got[got.len() - 32..];
        assert!(
            hash.chars().all(|c| c.is_ascii_hexdigit()),
            "hex tail: {hash}"
        );
        assert_eq!(got.as_bytes()[got.len() - 33], b'_');
    }

    #[test]
    fn hash_is_sha256_of_the_flattened_name_truncated_to_32() {
        use sha2::{Digest, Sha256};
        let flat = "MyPkg@1.0.0";
        let want = format!("{:x}", Sha256::digest(flat.as_bytes()));
        assert_eq!(target_name(flat), format!("{flat}_{}", &want[..32]));
    }

    #[test]
    fn peer_order_changes_the_name() {
        assert_ne!(
            target_name("x@1.0.0(a@1.0.0)(b@2.0.0)"),
            target_name("x@1.0.0(b@2.0.0)(a@1.0.0)"),
            "peers are not sorted, so order is significant"
        );
    }

    #[test]
    fn snapshot_key_method_agrees_with_the_free_function() {
        // `SnapshotKey::target_name` is defined as
        // `target_name(&self.canonical())`, so comparing it against
        // `target_name(raw)` would only restate that definition. Assert a
        // concrete expected string instead, so a change to either the
        // parser's canonicalization or the escaping rules is caught.
        let raw = "eslint-plugin-svelte@3.14.0(eslint@9.39.2(jiti@2.6.1))(svelte@5.49.1)";
        let k = SnapshotKey::parse(raw).unwrap();
        assert_eq!(
            k.target_name(),
            "eslint-plugin-svelte@3.14.0_eslint@9.39.2_jiti@2.6.1__svelte@5.49.1"
        );
    }
}
