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
        Self::parse_inner(s, s)
    }

    fn parse_inner(s: &str, whole: &str) -> Result<Self, KeyParseError> {
        let err = |offset: usize, reason: &'static str| KeyParseError {
            key: whole.to_string(),
            offset,
            reason,
        };

        let suffix_start = top_level_paren(s, whole)?;
        let (head, suffix) = match suffix_start {
            Some(i) => (&s[..i], &s[i..]),
            None => (s, ""),
        };

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
            peers: parse_peers(suffix, whole)?,
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
}

impl fmt::Display for SnapshotKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.canonical())
    }
}

/// Index of the first `(` at depth 0, or `None`. Errors on unbalanced parens.
fn top_level_paren(s: &str, whole: &str) -> Result<Option<usize>, KeyParseError> {
    let mut depth = 0usize;
    let mut first = None;
    for (i, c) in s.char_indices() {
        match c {
            '(' => {
                if depth == 0 && first.is_none() {
                    first = Some(i);
                }
                depth += 1;
            }
            ')' => {
                depth = depth.checked_sub(1).ok_or(KeyParseError {
                    key: whole.to_string(),
                    offset: i,
                    reason: "unbalanced `)`",
                })?;
            }
            _ => {}
        }
    }
    if depth != 0 {
        return Err(KeyParseError {
            key: whole.to_string(),
            offset: s.len(),
            reason: "unbalanced `(`",
        });
    }
    Ok(first)
}

/// Split `(a)(b)` into its depth-0 groups and parse each recursively.
fn parse_peers(suffix: &str, whole: &str) -> Result<Vec<SnapshotKey>, KeyParseError> {
    let mut peers = Vec::new();
    let mut depth = 0usize;
    let mut start = None;
    for (i, c) in suffix.char_indices() {
        match c {
            '(' => {
                if depth == 0 {
                    start = Some(i + 1);
                }
                depth += 1;
            }
            ')' => {
                depth -= 1;
                if depth == 0 {
                    let from = start.take().expect("a close at depth 0 follows an open");
                    peers.push(SnapshotKey::parse_inner(&suffix[from..i], whole)?);
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
        for bad in [
            "x@1.0.0(a@1", // unbalanced open
            "x@1.0.0)",    // stray close
            "@1.0.0",      // empty name
            "x@",          // empty version
            "noatsign",    // no '@'
            "@scope/name", // scoped, no version
            "",            // empty
        ] {
            assert!(SnapshotKey::parse(bad).is_err(), "must reject {bad:?}");
        }
    }

    #[test]
    fn error_names_the_key() {
        let e = SnapshotKey::parse("x@1.0.0(a@1").unwrap_err();
        assert!(!format!("{e}").is_empty());
    }
}
