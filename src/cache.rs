//! The tarball cache: `~/.cache/pudu/tarballs/sha512/<ab>/<hex>.tgz`.
//!
//! Addressed by the integrity `pnpm-lock.yaml` already records, so the path
//! of a wanted tarball is computable with no network at all. `--no-network`
//! against a warm cache depends on exactly that.
//!
//! Cached bytes are re-verified on read. The hash is cheap next to the I/O,
//! and it is what makes a cache hit as trustworthy as a fresh download.

use std::path::{Path, PathBuf};

use crate::error::VendorError;
use crate::tarball::{decode_integrity, hex, sha512_digest};

pub struct Cache {
    root: PathBuf,
}

impl Cache {
    /// `PUDU_CACHE_DIR`, else the OS cache directory plus `pudu`.
    ///
    /// `PUDU_CACHE_DIR` exists so the integration tests are hermetic; it is
    /// deliberately not advertised in `--help`.
    ///
    /// A set-but-empty value is treated as unset. `PUDU_CACHE_DIR=""` is what
    /// a CI system writes for a variable it defines but never fills in, and
    /// an empty `PathBuf` is the *relative* root — it would silently grow a
    /// `tarballs/` tree inside the user's repository.
    pub fn open() -> Result<Self, VendorError> {
        let root = match std::env::var_os("PUDU_CACHE_DIR").filter(|v| !v.is_empty()) {
            Some(v) => PathBuf::from(v),
            None => dirs::cache_dir()
                .ok_or(VendorError::CacheUnavailable)?
                .join("pudu"),
        };
        Ok(Self { root })
    }

    pub fn with_root(root: PathBuf) -> Self {
        Self { root }
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn path_for(&self, key: &str, integrity: &str) -> Result<PathBuf, VendorError> {
        let digest = hex(&decode_integrity(key, integrity)?);
        Ok(self
            .root
            .join("tarballs")
            .join("sha512")
            .join(&digest[..2])
            .join(format!("{digest}.tgz")))
    }

    /// The cached bytes, if they are present *and* still hash correctly.
    ///
    /// A corrupt entry reads as a miss rather than an error: the right
    /// response is to fetch it again, and under `--no-network` the caller
    /// already reports the miss precisely.
    pub fn get(&self, key: &str, integrity: &str) -> Option<Vec<u8>> {
        let path = self.path_for(key, integrity).ok()?;
        let bytes = std::fs::read(path).ok()?;
        let expected = decode_integrity(key, integrity).ok()?;
        (sha512_digest(&bytes) == expected).then_some(bytes)
    }

    /// Write via a temporary file in the same directory, then rename, so a
    /// killed run never leaves a truncated entry a later run would trust.
    pub fn put(&self, key: &str, integrity: &str, bytes: &[u8]) -> Result<(), VendorError> {
        let path = self.path_for(key, integrity)?;
        let dir = path.parent().unwrap_or(&self.root);
        let failed = |source: std::io::Error| VendorError::CacheWriteFailed {
            path: path.clone(),
            source,
        };
        std::fs::create_dir_all(dir).map_err(failed)?;
        let mut tmp = tempfile::NamedTempFile::new_in(dir).map_err(failed)?;
        std::io::Write::write_all(&mut tmp, bytes).map_err(failed)?;
        tmp.persist(&path)
            .map_err(|e| failed(std::io::Error::from(e)))?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tarball::{hex, sha512_digest};

    fn integrity_of(bytes: &[u8]) -> String {
        use base64::Engine as _;
        format!(
            "sha512-{}",
            base64::engine::general_purpose::STANDARD.encode(sha512_digest(bytes))
        )
    }

    #[test]
    fn the_path_is_derived_from_the_digest_not_the_url() {
        let dir = tempfile::tempdir().unwrap();
        let c = Cache::with_root(dir.path().to_path_buf());
        let bytes = b"hello";
        let h = hex(&sha512_digest(bytes));
        let p = c.path_for("p@1.0.0", &integrity_of(bytes)).unwrap();
        assert_eq!(
            p,
            dir.path()
                .join("tarballs")
                .join("sha512")
                .join(&h[..2])
                .join(format!("{h}.tgz"))
        );
    }

    #[test]
    fn a_malformed_integrity_has_no_path() {
        let dir = tempfile::tempdir().unwrap();
        let c = Cache::with_root(dir.path().to_path_buf());
        assert!(c.path_for("p@1.0.0", "sha1-abc").is_err());
    }

    #[test]
    fn put_then_get_round_trips() {
        let dir = tempfile::tempdir().unwrap();
        let c = Cache::with_root(dir.path().to_path_buf());
        let bytes = b"some tarball bytes".to_vec();
        let i = integrity_of(&bytes);
        c.put("p@1.0.0", &i, &bytes).unwrap();
        assert_eq!(c.get("p@1.0.0", &i), Some(bytes));
    }

    #[test]
    fn a_miss_is_none() {
        let dir = tempfile::tempdir().unwrap();
        let c = Cache::with_root(dir.path().to_path_buf());
        assert_eq!(c.get("p@1.0.0", &integrity_of(b"never stored")), None);
    }

    #[test]
    fn corrupt_cached_bytes_read_as_a_miss() {
        // A truncated or tampered cache entry must not be trusted just
        // because it sits at the right path — `--no-network` depends on this.
        let dir = tempfile::tempdir().unwrap();
        let c = Cache::with_root(dir.path().to_path_buf());
        let bytes = b"good bytes".to_vec();
        let i = integrity_of(&bytes);
        c.put("p@1.0.0", &i, &bytes).unwrap();

        let path = c.path_for("p@1.0.0", &i).unwrap();
        std::fs::write(&path, b"tampered").unwrap();

        assert_eq!(c.get("p@1.0.0", &i), None);
    }

    #[test]
    fn put_overwrites_an_existing_entry() {
        let dir = tempfile::tempdir().unwrap();
        let c = Cache::with_root(dir.path().to_path_buf());
        let bytes = b"bytes".to_vec();
        let i = integrity_of(&bytes);
        c.put("p@1.0.0", &i, &bytes).unwrap();
        c.put("p@1.0.0", &i, &bytes).unwrap();
        assert_eq!(c.get("p@1.0.0", &i), Some(bytes));
    }

    /// Bundled into the one environment-reading test rather than added as a
    /// second: `open()` reads process-global state, so two such tests could
    /// not run in parallel.
    #[test]
    fn pudu_cache_dir_overrides_the_os_cache_directory() {
        // `open()` reads the environment, which is process-global, so this
        // test sets and restores it rather than running in parallel with a
        // second environment-reading test. There is only one.
        let dir = tempfile::tempdir().unwrap();
        let previous = std::env::var_os("PUDU_CACHE_DIR");
        unsafe { std::env::set_var("PUDU_CACHE_DIR", dir.path()) };
        let c = Cache::open().unwrap();
        assert_eq!(c.root(), dir.path());

        // A set-but-blank variable must fall back to the OS cache directory,
        // not root the cache at the (relative) empty path — which in CI means
        // a `tarballs/` tree appearing inside the checked-out repository.
        unsafe { std::env::set_var("PUDU_CACHE_DIR", "") };
        let blank = Cache::open().unwrap();
        assert_ne!(
            blank.root(),
            Path::new(""),
            "an empty PUDU_CACHE_DIR must not root the cache in the working tree"
        );
        assert_eq!(blank.root(), dirs::cache_dir().unwrap().join("pudu"));

        match previous {
            Some(v) => unsafe { std::env::set_var("PUDU_CACHE_DIR", v) },
            None => unsafe { std::env::remove_var("PUDU_CACHE_DIR") },
        }
    }
}
