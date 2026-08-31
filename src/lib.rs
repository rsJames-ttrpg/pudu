//! pudu — translate `pnpm-lock.yaml` into Buck2 build rules.
//!
//! No public API is committed in v1.

pub mod cli;
pub mod config;
pub mod error;
pub mod lock;
pub mod platform;
pub mod registry;
pub mod sidecar;
pub mod tarball;
