//! Does a package's npm platform field admit a given platform?

/// Does a package's npm platform field admit `current`?
///
/// A port of pnpm's `checkList` (`@pnpm/package-is-installable`), evaluated
/// for a single `current` value because pudu considers one platform at a
/// time. `field` is the raw list from the lockfile with negation intact;
/// `None` is an absent field.
///
/// The final rule — `matched || negations == list.len()` — is pnpm's, and
/// carries two consequences worth stating because they are not what a
/// reader expects:
///
/// * A list mixing negative and positive entries requires an explicit
///   positive hit. `["!win32", "darwin"]` does not admit linux.
/// * An empty list admits everything, since `0 == 0`.
///
/// pnpm additionally discards non-string list entries before matching.
/// YAML gives pudu a `Vec<String>`, so a non-string entry is rejected by
/// serde long before reaching here; the divergence is unreachable and is
/// noted only so a reader comparing the two implementations is not left
/// wondering.
pub fn admits(field: Option<&[String]>, current: &str) -> bool {
    let Some(list) = field else { return true };

    // `any` is special only as a singleton — `["any", "darwin"]` is an
    // ordinary two-entry positive list.
    if list.len() == 1 && list[0] == "any" {
        return true;
    }

    let mut matched = false;
    let mut negations = 0usize;

    for entry in list {
        if let Some(body) = entry.strip_prefix('!') {
            if body == current {
                return false;
            }
            negations += 1;
        } else if entry == current {
            matched = true;
        }
    }

    matched || negations == list.len()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Convenience: build the `Option<&[String]>` shape `admits` takes.
    fn list(items: &[&str]) -> Vec<String> {
        items.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn absent_field_admits_everything() {
        assert!(admits(None, "linux"));
        assert!(admits(None, "wasm32"));
    }

    #[test]
    fn positive_entry_admits_only_itself() {
        assert!(admits(Some(&list(&["linux"])), "linux"));
        assert!(!admits(Some(&list(&["darwin"])), "linux"));
    }

    #[test]
    fn negation_excludes_its_own_value() {
        assert!(admits(Some(&list(&["!win32"])), "linux"));
        assert!(!admits(Some(&list(&["!win32"])), "win32"));
    }

    #[test]
    fn all_negative_list_admits_anything_it_does_not_name() {
        assert!(admits(Some(&list(&["!win32", "!darwin"])), "linux"));
        assert!(!admits(Some(&list(&["!win32", "!darwin"])), "darwin"));
    }

    /// pnpm's rule is `matched || negations == list.len()`. A list mixing
    /// negative and positive entries therefore requires an explicit
    /// positive hit: negations only ever subtract, they never widen.
    /// `["!win32", "darwin"]` does NOT mean "anything but win32".
    #[test]
    fn mixed_list_requires_an_explicit_positive_hit() {
        assert!(
            !admits(Some(&list(&["!win32", "darwin"])), "linux"),
            "a mixed list must not admit a value no positive entry names"
        );
        assert!(admits(Some(&list(&["!win32", "linux"])), "linux"));
        assert!(!admits(Some(&list(&["!win32", "linux"])), "win32"));
    }

    /// `any` is special ONLY as a singleton list. In any other position it
    /// is an ordinary token that matches nothing.
    #[test]
    fn any_is_special_only_as_a_singleton() {
        assert!(admits(Some(&list(&["any"])), "linux"));
        assert!(
            !admits(Some(&list(&["any", "darwin"])), "linux"),
            "`any` alongside another entry is an ordinary token"
        );
    }

    #[test]
    fn empty_list_admits_everything() {
        // `matched=false, negations=0, len=0` satisfies `negations == len`.
        assert!(admits(Some(&[]), "linux"));
    }

    /// Unknown tokens are ordinary positives that match nothing. They must
    /// never error: the committed fixture alone carries seven `os` and
    /// seven `cpu` values outside pudu's enums.
    #[test]
    fn unknown_tokens_match_nothing_and_never_panic() {
        assert!(!admits(Some(&list(&["wasm32"])), "x64"));
        assert!(!admits(Some(&list(&["openharmony"])), "linux"));
        assert!(admits(Some(&list(&["loong64", "x64"])), "x64"));
    }

    #[test]
    fn negation_of_an_unknown_token_still_admits() {
        assert!(admits(Some(&list(&["!openharmony"])), "linux"));
    }

    /// A bare `!` has an empty body, which equals no platform value.
    #[test]
    fn bare_bang_is_a_negation_of_the_empty_string() {
        assert!(admits(Some(&list(&["!"])), "linux"));
    }
}
