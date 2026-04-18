//! Line-level PKGBUILD diff vs a baseline string (last load / successful save),
//! used for git-style insert/remove highlighting in the full-text editor.

use similar::{ChangeTag, TextDiff};

/// What: Line diff between saved baseline text and the current buffer.
///
/// Output:
/// - `inserted_lines`: 0-based line numbers in `current` that are additions.
/// - `removed_lines`: deleted baseline lines in diff order (no trailing newline).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PkgbuildLineDiff {
    pub inserted_lines: Vec<usize>,
    pub removed_lines: Vec<String>,
}

/// What: Computes insert line indices and removed line contents for PKGBUILD text.
///
/// Inputs:
/// - `baseline`: reference text (typically last disk load or post-save snapshot).
/// - `current`: editor buffer text.
///
/// Details:
/// - Uses `similar` line diff (`TextDiff::diff_lines`).
pub fn diff_pkgbuild_lines(baseline: &str, current: &str) -> PkgbuildLineDiff {
    let diff = TextDiff::configure().diff_lines(baseline, current);
    let mut inserted_lines = Vec::new();
    let mut removed_lines = Vec::new();
    for ch in diff.iter_all_changes() {
        match ch.tag() {
            ChangeTag::Equal => {}
            ChangeTag::Delete => {
                removed_lines.push(trim_diff_line_ending(ch.value_ref()));
            }
            ChangeTag::Insert => {
                if let Some(i) = ch.new_index() {
                    inserted_lines.push(i);
                }
            }
        }
    }
    PkgbuildLineDiff {
        inserted_lines,
        removed_lines,
    }
}

fn trim_diff_line_ending(s: &str) -> String {
    s.trim_end_matches('\n').trim_end_matches('\r').to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn single_line_replacement_tracks_insert_and_remove() {
        let d = diff_pkgbuild_lines("a\nb\nc", "a\nb\nC");
        assert_eq!(d.removed_lines, vec!["c".to_string()]);
        assert_eq!(d.inserted_lines, vec![2]);
    }

    #[test]
    fn pure_insertion_appends_line_index() {
        let d = diff_pkgbuild_lines("a\n", "a\nb\n");
        assert!(d.removed_lines.is_empty());
        assert_eq!(d.inserted_lines, vec![1]);
    }

    #[test]
    fn identical_buffers_empty_diff() {
        let s = "pkgbase=foo\npkgver=1\n";
        let d = diff_pkgbuild_lines(s, s);
        assert!(d.inserted_lines.is_empty());
        assert!(d.removed_lines.is_empty());
    }
}
