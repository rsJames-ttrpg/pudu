//! The network layer: the only module in S3 that opens a socket.
//!
//! Each worker runs the whole per-package pipeline — fetch, cache, verify,
//! inspect — and drops the bytes before taking the next package, so peak
//! memory is bounded by `--jobs` rather than by the dependency graph.
//!
//! Results land in a `BTreeMap`, so output order is a property of the keys
//! rather than of which thread finished first. Determinism under parallelism
//! is by construction here, not by luck.

use std::collections::BTreeMap;
use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use crate::cache::Cache;
use crate::error::{VendorError, VendorWarning};
use crate::tarball::{Verified, verify_and_inspect};

/// A hostile or misconfigured registry must not be able to exhaust memory.
/// The largest package on the public registry is far below this.
const MAX_TARBALL: u64 = 256 * 1024 * 1024;

const BACKOFF_MS: [u64; 3] = [250, 500, 1000];

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Request {
    pub key: String,
    /// The package name alone. The string-`bin` rule names the command after
    /// the package, and `key` carries a version too.
    pub name: String,
    pub url: String,
    pub integrity: String,
}

pub type Outcome = Result<(Verified, Vec<VendorWarning>), VendorError>;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Stats {
    pub downloaded: usize,
    pub cached: usize,
}

/// Whether a status is worth retrying. A 4xx is the registry's final answer
/// and is never retried; 408 and 429 are the two that are not.
fn retryable(status: u16) -> bool {
    status == 408 || status == 429 || (500..600).contains(&status)
}

pub struct Fetcher {
    agent: ureq::Agent,
    jobs: usize,
    no_network: bool,
    verbose: bool,
    cache: Cache,
}

impl Fetcher {
    pub fn new(jobs: usize, no_network: bool, verbose: bool, cache: Cache) -> Self {
        let agent: ureq::Agent = ureq::Agent::config_builder()
            .timeout_connect(Some(Duration::from_secs(10)))
            .timeout_global(Some(Duration::from_secs(120)))
            .user_agent(concat!("pudu/", env!("CARGO_PKG_VERSION")))
            .build()
            .into();
        Self {
            agent,
            jobs: jobs.clamp(1, 64),
            no_network,
            verbose,
            cache,
        }
    }

    pub fn run(&self, requests: Vec<Request>) -> (BTreeMap<String, Outcome>, Stats) {
        let queue = Mutex::new(requests);
        let out: Mutex<BTreeMap<String, Outcome>> = Mutex::new(BTreeMap::new());
        let downloaded = AtomicUsize::new(0);
        let cached = AtomicUsize::new(0);

        std::thread::scope(|scope| {
            for _ in 0..self.jobs {
                scope.spawn(|| {
                    loop {
                        let Some(req) = queue.lock().unwrap().pop() else {
                            break;
                        };
                        let result = self.one(&req, &downloaded, &cached);
                        out.lock().unwrap().insert(req.key.clone(), result);
                    }
                });
            }
        });

        (
            out.into_inner().unwrap(),
            Stats {
                downloaded: downloaded.load(Ordering::Relaxed),
                cached: cached.load(Ordering::Relaxed),
            },
        )
    }

    fn one(&self, req: &Request, downloaded: &AtomicUsize, cached: &AtomicUsize) -> Outcome {
        let bytes = match self.cache.get(&req.key, &req.integrity) {
            Some(bytes) => {
                cached.fetch_add(1, Ordering::Relaxed);
                bytes
            }
            None => {
                if self.no_network {
                    return Err(VendorError::NetworkDisabled {
                        key: req.key.clone(),
                        url: req.url.clone(),
                    });
                }
                if self.verbose {
                    eprintln!("  downloading {}", req.key);
                }
                let bytes = self.download(req)?;
                downloaded.fetch_add(1, Ordering::Relaxed);
                // Cached before verification would mean a poisoned entry a
                // later run reads back; `Cache::get` re-hashes, so a bad
                // entry would read as a miss anyway, but not writing it at
                // all is clearer and cheaper.
                let verified =
                    verify_and_inspect(&req.key, &req.name, &req.url, &bytes, &req.integrity)?;
                self.cache.put(&req.key, &req.integrity, &bytes)?;
                return Ok(verified);
            }
        };

        verify_and_inspect(&req.key, &req.name, &req.url, &bytes, &req.integrity)
    }

    fn download(&self, req: &Request) -> Result<Vec<u8>, VendorError> {
        let mut attempt = 0usize;
        loop {
            let err = match self.agent.get(&req.url).call() {
                Ok(mut resp) => match resp
                    .body_mut()
                    .with_config()
                    .limit(MAX_TARBALL)
                    .read_to_vec()
                {
                    Ok(bytes) => return Ok(bytes),
                    Err(source) => VendorError::Transport {
                        key: req.key.clone(),
                        url: req.url.clone(),
                        source,
                    },
                },
                Err(ureq::Error::StatusCode(status)) => {
                    let e = VendorError::HttpStatus {
                        key: req.key.clone(),
                        url: req.url.clone(),
                        status,
                    };
                    if !retryable(status) {
                        return Err(e);
                    }
                    e
                }
                Err(source) => VendorError::Transport {
                    key: req.key.clone(),
                    url: req.url.clone(),
                    source,
                },
            };

            if attempt >= BACKOFF_MS.len() {
                return Err(err);
            }
            std::thread::sleep(Duration::from_millis(BACKOFF_MS[attempt]));
            attempt += 1;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write as _;

    use httpmock::prelude::*;

    fn tarball(files: &[(&str, &str)]) -> Vec<u8> {
        let mut ar = tar::Builder::new(Vec::new());
        for (path, body) in files {
            let mut h = tar::Header::new_gnu();
            h.set_size(body.len() as u64);
            h.set_mode(0o644);
            h.set_cksum();
            ar.append_data(&mut h, format!("package/{path}"), body.as_bytes())
                .unwrap();
        }
        let bytes = ar.into_inner().unwrap();
        let mut gz = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
        gz.write_all(&bytes).unwrap();
        gz.finish().unwrap()
    }

    fn integrity_of(bytes: &[u8]) -> String {
        use base64::Engine as _;
        format!(
            "sha512-{}",
            base64::engine::general_purpose::STANDARD.encode(crate::tarball::sha512_digest(bytes))
        )
    }

    fn request(url: String, integrity: String) -> Request {
        Request {
            key: "p@1.0.0".to_string(),
            name: "p".to_string(),
            url,
            integrity,
        }
    }

    fn body() -> Vec<u8> {
        tarball(&[("package.json", r#"{"name":"p","bin":"cli.js"}"#)])
    }

    #[test]
    fn retryable_covers_transient_statuses_only() {
        for s in [408, 429, 500, 502, 503, 504] {
            assert!(retryable(s), "{s} should be retried");
        }
        for s in [400, 401, 403, 404, 200, 301] {
            assert!(!retryable(s), "{s} must not be retried");
        }
    }

    #[test]
    fn a_successful_fetch_verifies_inspects_and_caches() {
        let server = MockServer::start();
        let bytes = body();
        let m = server.mock(|when, then| {
            when.method(GET).path("/p.tgz");
            then.status(200).body(bytes.clone());
        });

        let dir = tempfile::tempdir().unwrap();
        let f = Fetcher::new(2, false, false, Cache::with_root(dir.path().to_path_buf()));
        let i = integrity_of(&bytes);
        let (out, stats) = f.run(vec![request(server.url("/p.tgz"), i.clone())]);

        let (verified, _) = out["p@1.0.0"].as_ref().unwrap();
        assert_eq!(verified.size, bytes.len() as u64);
        assert_eq!(verified.inspection.bin["p"], "cli.js");
        assert_eq!(stats.downloaded, 1);
        assert_eq!(stats.cached, 0);
        m.assert();

        let warm = Cache::with_root(dir.path().to_path_buf());
        assert_eq!(warm.get("p@1.0.0", &i), Some(bytes));
    }

    #[test]
    fn a_warm_cache_is_used_without_a_request() {
        let bytes = body();
        let i = integrity_of(&bytes);
        let dir = tempfile::tempdir().unwrap();
        Cache::with_root(dir.path().to_path_buf())
            .put("p@1.0.0", &i, &bytes)
            .unwrap();

        let server = MockServer::start();
        let m = server.mock(|when, then| {
            when.method(GET).path("/p.tgz");
            then.status(500);
        });

        let f = Fetcher::new(2, false, false, Cache::with_root(dir.path().to_path_buf()));
        let (out, stats) = f.run(vec![request(server.url("/p.tgz"), i)]);

        assert!(out["p@1.0.0"].is_ok(), "{:?}", out["p@1.0.0"]);
        assert_eq!(stats.cached, 1);
        assert_eq!(stats.downloaded, 0);
        m.assert_calls(0);
    }

    #[test]
    fn no_network_with_a_cold_cache_names_the_package_and_url() {
        let dir = tempfile::tempdir().unwrap();
        let f = Fetcher::new(1, true, false, Cache::with_root(dir.path().to_path_buf()));
        let bytes = body();
        let (out, _) = f.run(vec![request(
            "https://registry.example/p.tgz".to_string(),
            integrity_of(&bytes),
        )]);

        let err = out["p@1.0.0"].as_ref().unwrap_err();
        let VendorError::NetworkDisabled { key, url } = err else {
            panic!("wrong variant: {err:?}");
        };
        assert_eq!(key, "p@1.0.0");
        assert_eq!(url, "https://registry.example/p.tgz");
    }

    #[test]
    fn no_network_with_a_warm_cache_succeeds() {
        let bytes = body();
        let i = integrity_of(&bytes);
        let dir = tempfile::tempdir().unwrap();
        Cache::with_root(dir.path().to_path_buf())
            .put("p@1.0.0", &i, &bytes)
            .unwrap();

        let f = Fetcher::new(1, true, false, Cache::with_root(dir.path().to_path_buf()));
        let (out, _) = f.run(vec![request(
            "https://registry.example/p.tgz".to_string(),
            i,
        )]);
        assert!(out["p@1.0.0"].is_ok());
    }

    #[test]
    fn a_503_is_retried_three_times_before_giving_up() {
        let server = MockServer::start();
        let bytes = body();
        let fail = server.mock(|when, then| {
            when.method(GET).path("/p.tgz");
            then.status(503);
        });
        let dir = tempfile::tempdir().unwrap();
        let f = Fetcher::new(1, false, false, Cache::with_root(dir.path().to_path_buf()));
        let (out, _) = f.run(vec![request(server.url("/p.tgz"), integrity_of(&bytes))]);

        assert!(out["p@1.0.0"].is_err());
        assert_eq!(
            fail.calls(),
            4,
            "one attempt plus three retries, so a flaky registry is survivable"
        );
    }

    #[test]
    fn a_404_is_not_retried() {
        let server = MockServer::start();
        let m = server.mock(|when, then| {
            when.method(GET).path("/gone.tgz");
            then.status(404);
        });
        let dir = tempfile::tempdir().unwrap();
        let f = Fetcher::new(1, false, false, Cache::with_root(dir.path().to_path_buf()));
        let (out, _) = f.run(vec![request(
            server.url("/gone.tgz"),
            integrity_of(&body()),
        )]);

        let err = out["p@1.0.0"].as_ref().unwrap_err();
        assert!(
            matches!(err, VendorError::HttpStatus { status: 404, .. }),
            "{err:?}"
        );
        assert_eq!(m.calls(), 1, "a 4xx is the registry's final answer");
    }

    #[test]
    fn served_bytes_that_fail_the_integrity_are_rejected_and_not_cached() {
        let server = MockServer::start();
        server.mock(|when, then| {
            when.method(GET).path("/p.tgz");
            then.status(200).body(body());
        });

        let dir = tempfile::tempdir().unwrap();
        let wrong = integrity_of(b"entirely different bytes");
        let f = Fetcher::new(1, false, false, Cache::with_root(dir.path().to_path_buf()));
        let (out, _) = f.run(vec![request(server.url("/p.tgz"), wrong.clone())]);

        let err = out["p@1.0.0"].as_ref().unwrap_err();
        assert!(
            matches!(err, VendorError::IntegrityMismatch { .. }),
            "{err:?}"
        );
        assert_eq!(
            Cache::with_root(dir.path().to_path_buf()).get("p@1.0.0", &wrong),
            None,
            "bytes that failed verification must not be readable as a cache hit"
        );
    }

    #[test]
    fn results_are_keyed_and_ordered_independently_of_completion() {
        let server = MockServer::start();
        let bytes = body();
        // No path matcher: every GET this test makes should be served the
        // same body, and the assertion is about result ordering, not routing.
        server.mock(|when, then| {
            when.method(GET);
            then.status(200).body(bytes.clone());
        });

        let i = integrity_of(&bytes);
        let requests: Vec<Request> = ["c@1", "a@1", "b@1"]
            .iter()
            .map(|k| Request {
                key: k.to_string(),
                name: "p".to_string(),
                url: server.url(format!("/{k}.tgz")),
                integrity: i.clone(),
            })
            .collect();

        let dir = tempfile::tempdir().unwrap();
        let f = Fetcher::new(3, false, false, Cache::with_root(dir.path().to_path_buf()));
        let (out, _) = f.run(requests);
        assert_eq!(
            out.keys().collect::<Vec<_>>(),
            vec!["a@1", "b@1", "c@1"],
            "a BTreeMap is what makes determinism hold under parallelism"
        );
    }
}
