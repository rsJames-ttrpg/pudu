//! Rendering the generated Buck2 files.
//!
//! Every renderer here returns a `String` and touches no filesystem, so
//! `buckify --check` compares in memory against what is on disk while sharing
//! all rendering with the write path. The two cannot drift.

pub mod format;
