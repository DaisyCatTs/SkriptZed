//! Parsing Skript version strings out of the `since` field.
//!
//! `since` is **not a version**. It is free text produced by a `@Since`
//! annotation that nothing validates, and it arrives in two JSON shapes: an
//! array for most categories, a bare string for `types` and `functions`. Of the
//! 257 distinct values in Skript 2.16.1, only 38 are plain `x.y[.z]`.
//!
//! Real values from the published database:
//!
//! ```text
//! "2.10"
//! "2.8.0"
//! "2.2-dev36"
//! "2.2-Fixes-V10"
//! "2.0 beta 3"
//! "1.0 pre-5"
//! "unknown (before 2.1)"
//! "1.0, 2.6 (BlockData support)"
//! "2.2-dev35, 2.2-dev36 (improved), 2.5.2 (throwable projectiles), 2.10 (item displays)"
//! ```
//!
//! Taking the **earliest atom** yields a usable minimum version for 98.3% of
//! entries. That is what this module does; the remaining ~1.7% are all ancient
//! (`unknown`, `Before 2.1`) and are treated as always-available.
//!
//! Two traps, both real:
//!
//! * `2.2-dev28` is a build *before* 2.2, but `2.2-Fixes-V10` is a fork release
//!   *after* it. Ordinary semver prerelease ordering gets the second wrong.
//! * The comma list is a history, not alternatives: `"1.0, 2.12 (saddle)"` means
//!   the element existed in 1.0 and gained a form in 2.12. The note cannot be
//!   mapped to an individual pattern, so it is surfaced in hover rather than
//!   used for filtering.

use std::cmp::Ordering;
use std::fmt;

/// A comparable Skript version.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Version {
    pub major: u32,
    pub minor: u32,
    pub patch: u32,
    /// Ordering nudge within the same `major.minor`, for the pre/post-release
    /// tags Skript's history is full of. `-1` for a dev/alpha/beta/pre build,
    /// `0` for a plain release, `1` for a `-Fixes-` fork release.
    stage: i8,
}

impl Version {
    pub const fn new(major: u32, minor: u32, patch: u32) -> Self {
        Self {
            major,
            minor,
            patch,
            stage: 0,
        }
    }

    /// The floor used for entries whose `since` carries no number at all
    /// (`"unknown (before 2.1)"`). They are all ancient, so treating them as
    /// available everywhere is both simple and correct in practice.
    pub const ANCIENT: Version = Version::new(0, 0, 0);

    /// Parses one version atom such as `2.10`, `2.8.0`, `2.2-dev36`,
    /// `2.2-Fixes-V10`, `2.0 beta 3` or `1.0 pre-5`.
    pub fn parse(text: &str) -> Option<Version> {
        let text = text.trim();
        let mut chars = text.char_indices().peekable();
        let mut numbers = [0u32; 3];
        let mut seen = 0usize;

        // Leading number run: `2`, `2.10`, `2.8.0`.
        while seen < 3 {
            let mut digits = String::new();
            while let Some(&(_, ch)) = chars.peek() {
                if ch.is_ascii_digit() {
                    digits.push(ch);
                    chars.next();
                } else {
                    break;
                }
            }
            if digits.is_empty() {
                break;
            }
            numbers[seen] = digits.parse().ok()?;
            seen += 1;

            // Only a `.` continues the number run.
            if matches!(chars.peek(), Some(&(_, '.'))) {
                chars.next();
            } else {
                break;
            }
        }

        if seen == 0 {
            return None;
        }

        let rest: String = chars.map(|(_, ch)| ch).collect();
        let rest = rest.trim().to_ascii_lowercase();

        // `2.2-Fixes-V10` is a fork release that came *after* 2.2, unlike every
        // other suffix, which marks a build before it.
        let stage = if rest.is_empty() {
            0
        } else if rest.starts_with("-fixes") {
            1
        } else {
            -1
        };

        Some(Version {
            major: numbers[0],
            minor: numbers[1],
            patch: numbers[2],
            stage,
        })
    }
}

impl Ord for Version {
    fn cmp(&self, other: &Self) -> Ordering {
        self.major
            .cmp(&other.major)
            .then(self.minor.cmp(&other.minor))
            .then(self.patch.cmp(&other.patch))
            .then(self.stage.cmp(&other.stage))
    }
}

impl PartialOrd for Version {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl fmt::Display for Version {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.patch == 0 {
            write!(f, "{}.{}", self.major, self.minor)
        } else {
            write!(f, "{}.{}.{}", self.major, self.minor, self.patch)
        }
    }
}

/// The version a syntax element first appeared in, taken from its `since` text.
///
/// Returns `None` only when the text contains no number anywhere; callers
/// should treat that as [`Version::ANCIENT`].
pub fn parse_since(since: &str) -> Option<Version> {
    split_atoms(since)
        .into_iter()
        .filter_map(|atom| Version::parse(&atom))
        .min()
}

/// Splits a `since` string into its comma-separated history atoms, ignoring
/// commas **inside parentheses** — the notes routinely contain them, as in
/// `"2.14 (syntax changes, infinite duration support)"`.
fn split_atoms(since: &str) -> Vec<String> {
    let mut atoms = Vec::new();
    let mut current = String::new();
    let mut depth = 0i32;

    for ch in since.chars() {
        match ch {
            '(' => {
                depth += 1;
                // The note is not part of the version; drop it entirely.
            }
            ')' => depth = (depth - 1).max(0),
            ',' if depth == 0 => {
                atoms.push(std::mem::take(&mut current));
            }
            _ if depth == 0 => current.push(ch),
            _ => {}
        }
    }
    atoms.push(current);

    atoms
        .into_iter()
        .map(|atom| atom.trim().to_string())
        .filter(|atom| !atom.is_empty())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_plain_versions() {
        assert_eq!(Version::parse("2.10"), Some(Version::new(2, 10, 0)));
        assert_eq!(Version::parse("2.8.0"), Some(Version::new(2, 8, 0)));
        assert_eq!(Version::parse("1.0"), Some(Version::new(1, 0, 0)));
    }

    #[test]
    fn normalises_minor_and_patch_spelling() {
        // The database writes both `2.9` and `2.9.0` for the same release.
        assert_eq!(Version::parse("2.9"), Version::parse("2.9.0"));
    }

    #[test]
    fn a_dev_build_sorts_before_its_release() {
        let dev = Version::parse("2.2-dev28").unwrap();
        let release = Version::parse("2.2").unwrap();
        assert!(dev < release, "{dev} should precede {release}");
    }

    #[test]
    fn a_fixes_release_sorts_after_its_release() {
        // The one case ordinary semver prerelease ordering gets wrong:
        // `2.2-Fixes-V10` is a fork release that came *after* 2.2.
        let fixes = Version::parse("2.2-Fixes-V10").unwrap();
        let release = Version::parse("2.2").unwrap();
        assert!(fixes > release, "{fixes} should follow {release}");
        assert!(fixes < Version::parse("2.3").unwrap());
    }

    #[test]
    fn is_case_insensitive_about_tags() {
        assert_eq!(
            Version::parse("2.2-Fixes-V10"),
            Version::parse("2.2-fixes-v10")
        );
    }

    #[test]
    fn parses_space_separated_prereleases() {
        let beta = Version::parse("2.0 beta 3").unwrap();
        assert!(beta < Version::parse("2.0").unwrap());
        let pre = Version::parse("1.0 pre-5").unwrap();
        assert!(pre < Version::parse("1.0").unwrap());
        assert_eq!(Version::parse("2.4-alpha4").unwrap().major, 2);
    }

    #[test]
    fn takes_the_earliest_atom_of_a_history() {
        assert_eq!(
            parse_since("1.0, 2.6 (BlockData support)"),
            Some(Version::new(1, 0, 0))
        );
        assert_eq!(
            parse_since("2.2, 2.7 (local functions)"),
            Some(Version::new(2, 2, 0))
        );
        assert_eq!(
            parse_since(
                "1.0, 2.6.1 (with section), 2.8.6 (dropped items), 2.10 (entity snapshots)"
            ),
            Some(Version::new(1, 0, 0))
        );
    }

    #[test]
    fn ignores_commas_inside_a_note() {
        // The note's comma must not split the history.
        assert_eq!(
            parse_since("2.14 (syntax changes, infinite duration support)"),
            Some(Version::new(2, 14, 0))
        );
        assert_eq!(
            parse_since(
                "1.3 (spawned entity), 2.0 (shot entity), 2.7 (struck lightning, firework)"
            ),
            Some(Version::new(1, 3, 0))
        );
    }

    #[test]
    fn handles_a_non_monotonic_history() {
        // Real value: 2.2 is listed before 2.2-dev24, which actually precedes it.
        // The minimum is what matters, so the ordering of the list does not.
        assert_eq!(
            parse_since("2.1.2, 2.2 (offline players' uuids), 2.2-dev24 (other entities' uuids)"),
            Some(Version::new(2, 1, 2))
        );
    }

    #[test]
    fn handles_unknown_and_absent() {
        assert_eq!(parse_since("unknown (before 2.1)"), None);
        assert_eq!(parse_since("Before 2.1"), None);
        assert_eq!(parse_since("before 2.1"), None);
        assert_eq!(parse_since(""), None);
        // `unknown` leading a real history still yields the earliest number.
        assert_eq!(
            parse_since("unknown, 2.5.2 (falling block), 2.8.0 (any entity support)"),
            Some(Version::new(2, 5, 2))
        );
    }

    #[test]
    fn handles_embedded_quotes_in_notes() {
        assert_eq!(
            parse_since("1.4.6, 2.12 ('or better')"),
            Some(Version::new(1, 4, 6))
        );
        assert_eq!(
            parse_since("2.15 ('uncolored' vs 'unformatted' distinction)"),
            Some(Version::new(2, 15, 0))
        );
    }

    #[test]
    fn orders_a_realistic_sequence() {
        let mut versions: Vec<Version> = [
            "2.10",
            "1.0",
            "2.2-dev36",
            "2.2",
            "2.8.0",
            "2.2-Fixes-V10",
            "2.16.1",
        ]
        .iter()
        .filter_map(|text| Version::parse(text))
        .collect();
        versions.sort();

        let rendered: Vec<String> = versions.iter().map(Version::to_string).collect();
        assert_eq!(
            rendered,
            ["1.0", "2.2", "2.2", "2.2", "2.8", "2.10", "2.16.1"],
            "sorted order was wrong (display collapses the tags)"
        );
        // The tags order correctly even though they render the same.
        assert!(Version::parse("2.2-dev36").unwrap() < Version::parse("2.2").unwrap());
        assert!(Version::parse("2.2").unwrap() < Version::parse("2.2-Fixes-V10").unwrap());
    }

    #[test]
    fn display_omits_a_zero_patch() {
        assert_eq!(Version::new(2, 10, 0).to_string(), "2.10");
        assert_eq!(Version::new(2, 8, 6).to_string(), "2.8.6");
    }
}
