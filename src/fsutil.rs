//! Shared helpers for the temp-file-and-rename write path.
//!
//! `tempfile::NamedTempFile` fixes its own permissions to `0600` regardless
//! of the process umask, and `persist` carries that mode onto the final
//! path. A plain `std::fs::write` would instead go through `File::create`,
//! which applies the umask and so produces `0666 & !umask` — `0644` under
//! the common `umask 022`. Both `pudu vendor` and `pudu buckify` write their
//! (committed, build-read) output through a temp file for atomicity, and
//! both want that same umask-derived mode rather than the tempfile default.
//!
//! [`probe_umask_mode`] and [`apply_probed_mode`] are split so a caller
//! writing several files in one pass (`buck::Generated::write`) can probe
//! once and apply the result to every temp file, rather than re-deriving
//! the umask per file.

use std::path::Path;

/// The mode a plain `std::fs::write` would give a new file in `dir`, or
/// `None` if it could not be determined.
///
/// Derived by actually creating a throwaway file in `dir` via
/// `File::create` (which applies the umask, unlike
/// `NamedTempFile::new_in`) and reading back the mode the OS gave it. Using
/// `dir` itself, rather than assuming a global umask, is what makes this
/// correct under ACLs or a mount option that overrides the process umask
/// for that directory.
///
/// Returns `None` on any failure (unwritable directory, non-Unix target,
/// unreadable metadata) so callers can fall back to leaving a temp file's
/// mode alone: a wrong mode must never turn a working write into a failing
/// one.
#[cfg(unix)]
pub(crate) fn probe_umask_mode(dir: &Path) -> Option<u32> {
    use std::os::unix::fs::PermissionsExt;

    let probe = tempfile::Builder::new()
        .prefix(".pudu-mode-probe-")
        .make_in(dir, |p| std::fs::File::create(p))
        .ok()?;
    let metadata = probe.as_file().metadata().ok()?;
    Some(metadata.permissions().mode())
}

#[cfg(not(unix))]
pub(crate) fn probe_umask_mode(_dir: &Path) -> Option<u32> {
    None
}

/// Apply a mode previously returned by [`probe_umask_mode`] to `tmp`,
/// before it is persisted. A no-op if `mode` is `None`, or if setting the
/// permissions fails — either way the temp file keeps whatever mode it
/// already had rather than failing the write.
#[cfg(unix)]
pub(crate) fn apply_probed_mode(tmp: &tempfile::NamedTempFile, mode: Option<u32>) {
    use std::os::unix::fs::PermissionsExt;

    if let Some(mode) = mode {
        let _ = tmp
            .as_file()
            .set_permissions(std::fs::Permissions::from_mode(mode));
    }
}

#[cfg(not(unix))]
pub(crate) fn apply_probed_mode(_tmp: &tempfile::NamedTempFile, _mode: Option<u32>) {}
