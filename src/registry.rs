//! Tarball URL derivation.
//!
//! `<registry>/<name>/-/<basename>-<version>.tgz`, where `basename` is the
//! package name after its scope. Verified exact against 400 live registry
//! manifests — see the tarball survey, §1. Private registries are why the
//! resolved URL is recorded in `packages.toml` rather than re-derived later.

use url::Url;

use crate::config::RegistryConfig;
use crate::error::VendorError;

/// The registry serving `name`: an exact `@scope` override when one is
/// configured, else the default.
///
/// Matching is exact. npm has no nested scopes, so `@myorgtools` must not
/// pick up an `@myorg` override.
pub fn registry_for<'a>(name: &str, cfg: &'a RegistryConfig) -> &'a Url {
    if let Some(scope) = scope_of(name)
        && let Some(url) = cfg.scopes.get(scope)
    {
        return url;
    }
    &cfg.default
}

/// `@babel/core` → `Some("@babel")`; `left-pad` → `None`.
fn scope_of(name: &str) -> Option<&str> {
    let rest = name.strip_prefix('@')?;
    let slash = rest.find('/')?;
    // `rest` is one byte shorter than `name`, so the separator sits at
    // `slash + 1` in `name` and `..slash + 1` is the scope without it.
    Some(&name[..slash + 1])
}

/// The tarball URL for `name@version`.
pub fn tarball_url(name: &str, version: &str, cfg: &RegistryConfig) -> Result<Url, VendorError> {
    let base = registry_for(name, cfg);
    let basename = name.rsplit('/').next().unwrap_or(name);
    // Built by hand rather than with `Url::join`, which resolves relatively
    // and would discard the last path segment of a registry served under a
    // prefix like `/artifactory/api/npm/repo`.
    let raw = format!(
        "{}/{name}/-/{basename}-{version}.tgz",
        base.as_str().trim_end_matches('/')
    );
    Url::parse(&raw).map_err(|_| VendorError::BadDerivedUrl {
        key: format!("{name}@{version}"),
        name: name.to_string(),
        url: raw,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cfg(default: &str, scopes: &[(&str, &str)]) -> RegistryConfig {
        RegistryConfig {
            default: Url::parse(default).unwrap(),
            scopes: scopes
                .iter()
                .map(|(k, v)| (k.to_string(), Url::parse(v).unwrap()))
                .collect(),
        }
    }

    fn url(name: &str, version: &str, c: &RegistryConfig) -> String {
        tarball_url(name, version, c).unwrap().to_string()
    }

    #[test]
    fn an_unscoped_name_derives_the_npm_url() {
        let c = cfg("https://registry.npmjs.org", &[]);
        assert_eq!(
            url("left-pad", "1.3.0", &c),
            "https://registry.npmjs.org/left-pad/-/left-pad-1.3.0.tgz"
        );
    }

    #[test]
    fn a_scoped_name_drops_the_scope_from_the_basename_only() {
        let c = cfg("https://registry.npmjs.org", &[]);
        assert_eq!(
            url("@babel/core", "7.28.6", &c),
            "https://registry.npmjs.org/@babel/core/-/core-7.28.6.tgz"
        );
    }

    #[test]
    fn a_scope_override_changes_the_host() {
        let c = cfg(
            "https://registry.npmjs.org",
            &[("@myorg", "https://npm.mycorp.example")],
        );
        assert_eq!(
            url("@myorg/thing", "1.0.0", &c),
            "https://npm.mycorp.example/@myorg/thing/-/thing-1.0.0.tgz"
        );
    }

    #[test]
    fn a_scope_with_no_override_falls_back_to_the_default() {
        let c = cfg(
            "https://registry.npmjs.org",
            &[("@myorg", "https://npm.mycorp.example")],
        );
        assert_eq!(
            url("@babel/core", "7.28.6", &c),
            "https://registry.npmjs.org/@babel/core/-/core-7.28.6.tgz"
        );
    }

    #[test]
    fn scope_matching_is_exact_not_prefix() {
        // `@myorgtools` must not match the `@myorg` override: npm has no
        // notion of nested or prefixed scopes.
        let c = cfg(
            "https://registry.npmjs.org",
            &[("@myorg", "https://npm.mycorp.example")],
        );
        assert_eq!(
            url("@myorgtools/thing", "1.0.0", &c),
            "https://registry.npmjs.org/@myorgtools/thing/-/thing-1.0.0.tgz"
        );
    }

    #[test]
    fn a_registry_path_prefix_is_preserved() {
        // `Url::join` would discard the `repo` segment here, which is why
        // derivation appends to the path by hand.
        let c = cfg("https://npm.example.com/artifactory/api/npm/repo", &[]);
        assert_eq!(
            url("left-pad", "1.3.0", &c),
            "https://npm.example.com/artifactory/api/npm/repo/left-pad/-/left-pad-1.3.0.tgz"
        );
    }

    #[test]
    fn a_trailing_slash_on_the_registry_changes_nothing() {
        let with = cfg("https://npm.example.com/repo/", &[]);
        let without = cfg("https://npm.example.com/repo", &[]);
        assert_eq!(
            url("left-pad", "1.3.0", &with),
            url("left-pad", "1.3.0", &without)
        );
    }

    #[test]
    fn a_prerelease_version_survives_verbatim() {
        let c = cfg("https://registry.npmjs.org", &[]);
        assert_eq!(
            url("@babel/core", "7.0.0-beta.4", &c),
            "https://registry.npmjs.org/@babel/core/-/core-7.0.0-beta.4.tgz"
        );
    }

    #[test]
    fn registry_for_returns_the_override_itself() {
        let c = cfg(
            "https://registry.npmjs.org",
            &[("@myorg", "https://npm.mycorp.example")],
        );
        assert_eq!(
            registry_for("@myorg/thing", &c).as_str(),
            "https://npm.mycorp.example/"
        );
    }
}
