//! Differential fuzz: `admits` against pnpm's real `checkPlatform`.
//!
//! `#[ignore]`d by default so the suite needs neither node nor a network.
//! Run explicitly:
//!
//! ```sh
//! cd tests/fixtures/platform && npm install @pnpm/package-is-installable@1000.0.21 && cd -
//! cargo test --test platform_fuzz -- --ignored --nocapture
//! ```
//!
//! The version is pinned exactly, not range-matched: `reference.mjs` reaches
//! into this package's internal `lib/*.js` by filesystem path, bypassing its
//! `exports` map (TD-S2-04). That is low-risk for a developer script nothing
//! else depends on, but this file is now also what CI runs, so an upstream
//! layout change must not silently start failing builds — bump the pin
//! deliberately instead.
//!
//! This is where `libc` and negation coverage comes from. No install against
//! the public npm registry can produce a lockfile carrying a `libc` field —
//! pnpm fetches npm's abbreviated packument, which omits it — so no fixture
//! can exercise those paths. See the platform matching survey §2.

use std::io::Write;
use std::process::{Command, Stdio};

use pudu::platform::admits;

/// A tiny deterministic PRNG: the corpus must be reproducible, and pulling
/// in a dependency for a developer-only test is not worth it.
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        // xorshift64*
        let mut x = self.0;
        x ^= x >> 12;
        x ^= x << 25;
        x ^= x >> 27;
        self.0 = x;
        x.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }

    fn below(&mut self, n: usize) -> usize {
        (self.next() % n as u64) as usize
    }
}

const OS_TOKENS: &[&str] = &[
    "linux",
    "darwin",
    "win32",
    "aix",
    "android",
    "freebsd",
    "sunos",
    "openharmony",
    "any",
];
const CPU_TOKENS: &[&str] = &[
    "x64", "arm64", "arm", "ia32", "ppc64", "s390x", "wasm32", "loong64", "any",
];
const LIBC_TOKENS: &[&str] = &["glibc", "musl", "any", "unknown"];

fn tokens(axis: &str) -> &'static [&'static str] {
    match axis {
        "os" => OS_TOKENS,
        "cpu" => CPU_TOKENS,
        _ => LIBC_TOKENS,
    }
}

/// One generated case: a field list (or absence) and the value to test it
/// against.
struct Case {
    axis: &'static str,
    list: Option<Vec<String>>,
    current: String,
}

fn generate(count: usize) -> Vec<Case> {
    let mut rng = Rng(0x5EED_1234_ABCD_9876);
    let mut cases = Vec::with_capacity(count);

    for i in 0..count {
        let axis = ["os", "cpu", "libc"][i % 3];
        let pool = tokens(axis);
        let current = pool[rng.below(pool.len())].to_string();

        // Shapes, weighted so the interesting ones are well covered:
        // absent, empty, singleton, multi, all-negative, and mixed.
        let list = match rng.below(10) {
            0 => None,
            1 => Some(Vec::new()),
            2..=4 => Some(vec![pool[rng.below(pool.len())].to_string()]),
            5 | 6 => {
                let n = 1 + rng.below(3);
                Some(
                    (0..n)
                        .map(|_| pool[rng.below(pool.len())].to_string())
                        .collect(),
                )
            }
            7 | 8 => {
                let n = 1 + rng.below(3);
                Some(
                    (0..n)
                        .map(|_| format!("!{}", pool[rng.below(pool.len())]))
                        .collect(),
                )
            }
            _ => {
                // Mixed positive and negative — the shape whose rule is
                // least intuitive and most worth fuzzing.
                let n = 2 + rng.below(3);
                Some(
                    (0..n)
                        .map(|j| {
                            let t = pool[rng.below(pool.len())];
                            if j % 2 == 0 {
                                format!("!{t}")
                            } else {
                                t.to_string()
                            }
                        })
                        .collect(),
                )
            }
        };

        cases.push(Case {
            axis,
            list,
            current,
        });
    }
    cases
}

#[test]
#[ignore = "requires node and @pnpm/package-is-installable; run explicitly"]
fn admits_agrees_with_pnpm_check_platform() {
    let cases = generate(3000);

    // Each line carries its own index, echoed back by the harness, so a
    // dropped or reordered line is caught even if the line counts still
    // happen to match — `assert_eq!(expected.len(), cases.len())` alone
    // would not catch a swap of two lines.
    let mut input = String::new();
    for (i, c) in cases.iter().enumerate() {
        let list = match &c.list {
            None => "null".to_string(),
            Some(v) => serde_json::to_string(v).expect("serialize list"),
        };
        input.push_str(&format!(
            r#"{{"i":{i},"list":{list},"current":{},"axis":{}}}"#,
            serde_json::to_string(&c.current).unwrap(),
            serde_json::to_string(c.axis).unwrap()
        ));
        input.push('\n');
    }

    let mut child = Command::new("node")
        .arg("reference.mjs")
        .current_dir("tests/fixtures/platform")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .expect("spawn node — see this file's header for setup");
    child
        .stdin
        .as_mut()
        .unwrap()
        .write_all(input.as_bytes())
        .expect("write cases");
    let out = child.wait_with_output().expect("run reference");
    assert!(out.status.success(), "reference harness failed");

    let expected: Vec<bool> = String::from_utf8_lossy(&out.stdout)
        .lines()
        .filter(|l| !l.trim().is_empty())
        .enumerate()
        .map(|(line_no, l)| {
            let (idx_str, verdict) = l
                .split_once('\t')
                .unwrap_or_else(|| panic!("reference line {line_no} missing index tab: {l:?}"));
            let idx: usize = idx_str.parse().unwrap_or_else(|_| {
                panic!("reference line {line_no} has non-numeric index: {l:?}")
            });
            assert_eq!(
                idx, line_no,
                "reference line {line_no} echoed index {idx} — a line was dropped or reordered"
            );
            verdict == "true"
        })
        .collect();
    assert_eq!(expected.len(), cases.len(), "one verdict per case");

    let mut disagreements = Vec::new();
    for (c, want) in cases.iter().zip(expected) {
        let got = admits(c.list.as_deref(), &c.current);
        if got != want {
            disagreements.push(format!(
                "axis={} list={:?} current={} pudu={} pnpm={}",
                c.axis, c.list, c.current, got, want
            ));
        }
    }

    assert!(
        disagreements.is_empty(),
        "{} of {} cases disagree with pnpm:\n{}",
        disagreements.len(),
        cases.len(),
        disagreements
            .iter()
            .take(20)
            .cloned()
            .collect::<Vec<_>>()
            .join("\n")
    );

    println!("{} cases, zero disagreements with pnpm", cases.len());
}
