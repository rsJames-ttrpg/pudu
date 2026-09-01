//! Rendering third-party data into generated Starlark.
//!
//! `root`, and the keys and values of `bin`, come from a package's own
//! tarball. S4 is the first stage to write them into generated code, so this
//! module is a trust boundary rather than a formatting convenience. Spec §1.1
//! is the same class of problem one layer down — an unquoted archive root
//! reaching a shell — and it shipped in the buck2 prelude.

use crate::error::BuckError;

/// A complete double-quoted Starlark string literal, quotes included.
///
/// Starlark source is UTF-8, so non-ASCII is emitted verbatim; escaping it
/// would be noise. Only what would end the literal or corrupt the line is
/// escaped.
pub fn starlark_string(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if c.is_control() => out.push_str(&format!("\\x{:02x}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

/// Reject a bin name buck2 could not address as a sub-target.
///
/// A `bin` entry becomes the sub-target `bin/<name>`, and buck2's
/// target-pattern parser accepts only alphanumerics and `, = - / + _` — plus
/// `.`, which it accepts in practice despite omitting it from its own error
/// message (verified against buck2 2026-05-18). npm's rule is wider: pudu's
/// own `tarball::is_url_safe` additionally admits `! ~ * ' ( )`, every one of
/// which buck2 rejects.
///
/// Failing here names the package. Failing in buck2 produces "Invalid
/// provider name" with nothing to say which of several hundred packages
/// caused it.
pub fn validate_bin_name(package: &str, name: &str) -> Result<(), BuckError> {
    let ok = !name.is_empty()
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '+' | '-' | '_'));
    if ok {
        Ok(())
    } else {
        Err(BuckError::UnrepresentableBinName {
            package: package.to_string(),
            name: name.to_string(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_plain_string_is_quoted() {
        assert_eq!(starlark_string("package"), "\"package\"");
    }

    #[test]
    fn a_space_in_an_archive_root_survives_quoting() {
        // The `@types/node` case. This is the value that broke
        // `strip_prefix` (spec §1.1); as a quoted Starlark literal it is
        // ordinary text.
        assert_eq!(starlark_string("node v22.20"), "\"node v22.20\"");
    }

    #[test]
    fn quotes_and_backslashes_are_escaped() {
        assert_eq!(starlark_string(r#"a"b\c"#), r#""a\"b\\c""#);
    }

    #[test]
    fn control_characters_are_escaped_numerically() {
        assert_eq!(starlark_string("a\nb\tc\rd\u{7}"), r#""a\nb\tc\rd\x07""#);
    }

    #[test]
    fn non_ascii_is_passed_through_as_utf8() {
        // Starlark source is UTF-8; escaping these would be noise.
        assert_eq!(starlark_string("café–ü"), "\"café–ü\"");
    }

    #[test]
    fn buck_addressable_bin_names_are_accepted() {
        // `.` and `+` are accepted by buck2 in practice even though its
        // error message does not list `.` — verified against buck2
        // 2026-05-18 with `//third-party/js:probe[bin/dot.js]`.
        for name in ["semver", "tsc", "next.js", "a+b", "a-b", "a_b", "A9"] {
            assert!(
                validate_bin_name("pkg@1.0.0", name).is_ok(),
                "{name} must be accepted"
            );
        }
    }

    #[test]
    fn bin_names_buck_cannot_address_are_rejected() {
        // npm's own rule (`is_url_safe` in tarball.rs) permits all of these,
        // so they can reach the table. buck2's target-pattern parser rejects
        // every one: "Invalid provider name". Failing here names the package;
        // failing in buck2 does not.
        for name in ["bang!", "tilde~", "star*", "quote'", "paren(", "close)"] {
            let err = validate_bin_name("pkg@1.0.0", name).unwrap_err();
            let msg = err.to_string();
            assert!(
                msg.contains("pkg@1.0.0"),
                "{name}: {msg} must name the package"
            );
            assert!(msg.contains(name), "{name}: {msg} must name the value");
        }
    }

    #[test]
    fn an_empty_bin_name_is_rejected() {
        assert!(validate_bin_name("pkg@1.0.0", "").is_err());
    }
}
