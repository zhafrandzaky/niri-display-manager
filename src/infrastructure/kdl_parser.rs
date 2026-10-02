//! Managed-section handling for the niri display configuration file.
//!
//! The manager never parses or rewrites user KDL. It appends or replaces a
//! fenced section delimited by markers, leaving every byte outside the fence
//! untouched. `niri validate` remains the authority on whether the result is
//! valid before it is loaded.
//!
//! This module also provides full-path include detection so the manager can
//! tell whether a dedicated display file is wired into the main configuration
//! or whether the portable inline mode must be used.

use std::path::{Component, Path, PathBuf};

use crate::domain::profile::{LayoutPlan, ProfileKind};

/// First line of the managed section.
pub const BEGIN_MARKER: &str = "// >>> niri-display-manager: managed section";
/// Last line of the managed section.
pub const END_MARKER: &str = "// <<< niri-display-manager: end of managed section";

/// Render the managed KDL section for a layout plan.
pub fn render_section(plan: &LayoutPlan) -> String {
    let mut section = String::new();
    section.push_str(BEGIN_MARKER);
    section.push('\n');
    section.push_str("// profile: ");
    section.push_str(plan.profile.id());
    section.push('\n');
    for output in &plan.outputs {
        section.push_str("output \"");
        section.push_str(output.output.as_str());
        section.push_str("\" {\n");
        if !output.enabled {
            section.push_str("    off\n");
        }
        if let Some((x, y)) = output.position {
            section.push_str(&format!("    position x={x} y={y}\n"));
        }
        if let Some(scale) = output.scale {
            section.push_str("    scale ");
            section.push_str(&crate::domain::display::format_scale(scale));
            section.push('\n');
        }
        section.push_str("}\n");
    }
    section.push_str(END_MARKER);
    section.push('\n');
    section
}

/// Replace the existing managed section or append a new one.
pub fn splice(original: &str, section: &str) -> String {
    match find_section_range(original) {
        Some((start, end)) => {
            let mut result = String::with_capacity(original.len() + section.len());
            result.push_str(&original[..start]);
            result.push_str(section);
            result.push_str(&original[end..]);
            normalize_trailing_newlines(result)
        }
        None => {
            let mut result = original.to_owned();
            if !result.is_empty() && !result.ends_with('\n') {
                result.push('\n');
            }
            if !result.is_empty() {
                result.push('\n');
            }
            result.push_str(section);
            normalize_trailing_newlines(result)
        }
    }
}

/// Remove the managed section; returns the new contents and whether anything
/// was removed.
pub fn remove_section(original: &str) -> (String, bool) {
    match find_section_range(original) {
        Some((start, end)) => {
            let mut result = String::with_capacity(original.len());
            result.push_str(&original[..start]);
            result.push_str(&original[end..]);
            (normalize_trailing_newlines(result), true)
        }
        None => (original.to_owned(), false),
    }
}

/// Whether the contents contain a managed section.
pub fn has_section(contents: &str) -> bool {
    find_section_range(contents).is_some()
}

/// Profile recorded in the managed section, if any.
pub fn section_profile(contents: &str) -> Option<ProfileKind> {
    let (start, end) = find_section_range(contents)?;
    let section = &contents[start..end];
    section
        .lines()
        .find_map(|line| line.strip_prefix("// profile: "))
        .and_then(|id| ProfileKind::from_id(id.trim()))
}

/// Extract file paths referenced by top-level `include` directives.
///
/// Handles `include "path"` and `include optional=true "path"`. Only
/// directives starting at the beginning of a line are considered, matching
/// niri's requirement that includes are top-level; comments and malformed
/// lines are ignored.
pub fn collect_include_paths(contents: &str) -> Vec<String> {
    contents.lines().filter_map(parse_include_line).collect()
}

fn parse_include_line(raw_line: &str) -> Option<String> {
    let rest = raw_line.strip_prefix("include")?;
    if !rest.starts_with(|character: char| character.is_whitespace() || character == '"') {
        return None;
    }
    // Options such as `optional=true` may precede the quoted path.
    let mut quoted = rest.split('"');
    quoted.next()?;
    let path = quoted.next()?;
    if path.is_empty() {
        None
    } else {
        Some(path.to_owned())
    }
}

/// Lexically normalize a path, resolving `.` and `..` without filesystem
/// access. Used for comparison only; it never touches the disk.
pub fn normalize_path(path: &Path) -> PathBuf {
    let mut normalized = PathBuf::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                if !normalized.pop() {
                    normalized.push("..");
                }
            }
            other => normalized.push(other.as_os_str()),
        }
    }
    normalized
}

/// Resolve an include path the way niri does: relative to the including file,
/// absolute, or `~/`-relative when a home directory is known.
pub fn resolve_include(include: &str, base_dir: &Path, home: Option<&Path>) -> PathBuf {
    if let Some(rest) = include.strip_prefix("~/")
        && let Some(home) = home
    {
        return normalize_path(&home.join(rest));
    }
    let path = Path::new(include);
    if path.is_absolute() {
        normalize_path(path)
    } else {
        normalize_path(&base_dir.join(path))
    }
}

/// Whether the include directives in `contents` resolve to `target`.
///
/// `target` must already be absolute and normalized. The comparison is a full
/// path comparison, so an unrelated file that merely shares the basename (for
/// example `old/display.kdl` when the target is `cfg/display.kdl`) is not
/// treated as a match.
pub fn includes_target(
    contents: &str,
    base_dir: &Path,
    target: &Path,
    home: Option<&Path>,
) -> bool {
    collect_include_paths(contents)
        .iter()
        .any(|include| resolve_include(include, base_dir, home) == target)
}

fn find_section_range(contents: &str) -> Option<(usize, usize)> {
    let begin = contents.find(BEGIN_MARKER)?;
    let after_begin = begin + BEGIN_MARKER.len();
    let end_marker_offset = contents[after_begin..].find(END_MARKER)?;
    let end_start = after_begin + end_marker_offset;
    let end = end_start + END_MARKER.len();
    let end = if contents[end..].starts_with('\n') {
        end + 1
    } else {
        end
    };
    Some((begin, end))
}

fn normalize_trailing_newlines(mut text: String) -> String {
    while text.ends_with("\n\n") {
        text.pop();
    }
    if !text.is_empty() && !text.ends_with('\n') {
        text.push('\n');
    }
    text
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::display::OutputId;
    use crate::domain::profile::OutputPlan;

    fn sample_plan() -> LayoutPlan {
        LayoutPlan {
            profile: ProfileKind::ExtendRight,
            outputs: vec![
                OutputPlan {
                    output: OutputId::new("eDP-1"),
                    enabled: true,
                    position: Some((0, 0)),
                    scale: None,
                    expect_present: true,
                },
                OutputPlan {
                    output: OutputId::new("HDMI-A-1"),
                    enabled: true,
                    position: Some((1920, 0)),
                    scale: Some(1.0),
                    expect_present: true,
                },
            ],
            focus_output: None,
        }
    }

    #[test]
    fn renders_expected_kdl_section() {
        let section = render_section(&sample_plan());
        assert!(section.starts_with(BEGIN_MARKER));
        assert!(section.ends_with(&format!("{END_MARKER}\n")));
        assert!(section.contains("// profile: extend-right"));
        assert!(section.contains("output \"eDP-1\" {"));
        assert!(section.contains("    position x=0 y=0\n"));
        assert!(section.contains("output \"HDMI-A-1\" {"));
        assert!(section.contains("    position x=1920 y=0\n"));
        assert!(section.contains("    scale 1\n"));
    }

    #[test]
    fn renders_disabled_outputs_with_off_flag() {
        let mut plan = sample_plan();
        plan.outputs[0].enabled = false;
        plan.outputs[0].position = None;
        plan.outputs[0].scale = None;
        let section = render_section(&plan);
        assert!(section.contains("output \"eDP-1\" {\n    off\n}"));
    }

    #[test]
    fn splices_into_empty_and_preserves_user_content() {
        let section = render_section(&sample_plan());
        let empty = splice("", &section);
        assert_eq!(empty, section);

        let user_config = "// my own notes\noutput \"DP-1\" {\n    scale 2\n}\n";
        let spliced = splice(user_config, &section);
        assert!(spliced.starts_with(user_config));
        assert!(spliced.contains(BEGIN_MARKER));
        assert!(has_section(&spliced));
    }

    #[test]
    fn splicing_twice_replaces_instead_of_duplicating() {
        let section = render_section(&sample_plan());
        let first = splice("// user line\n", &section);
        let mut changed = sample_plan();
        changed.profile = ProfileKind::ExtendLeft;
        let second = splice(&first, &render_section(&changed));

        assert_eq!(second.matches(BEGIN_MARKER).count(), 1);
        assert_eq!(second.matches(END_MARKER).count(), 1);
        assert!(second.contains("// profile: extend-left"));
        assert!(second.starts_with("// user line\n"));
    }

    #[test]
    fn removal_restores_user_content() {
        let section = render_section(&sample_plan());
        let user_config = "// keep me\n";
        let spliced = splice(user_config, &section);
        let (restored, removed) = remove_section(&spliced);
        assert!(removed);
        assert_eq!(restored, user_config);
        assert!(!has_section(&restored));

        let (unchanged, removed_again) = remove_section(&restored);
        assert!(!removed_again);
        assert_eq!(unchanged, user_config);
    }

    #[test]
    fn exposes_profile_of_existing_section() {
        let section = render_section(&sample_plan());
        let spliced = splice("", &section);
        assert_eq!(section_profile(&spliced), Some(ProfileKind::ExtendRight));
        assert_eq!(section_profile("no section here"), None);
    }

    #[test]
    fn collects_include_paths_from_top_level_directives() {
        let contents = "// include \"commented.kdl\"\ninclude \"a.kdl\"\ninclude optional=true \"./cfg/display.kdl\"\n\nlayout {\n    include \"nested.kdl\"\n}\ninclude \"b.kdl\"\n";
        assert_eq!(
            collect_include_paths(contents),
            vec!["a.kdl", "./cfg/display.kdl", "b.kdl"]
        );
        assert!(collect_include_paths("").is_empty());
        assert!(collect_include_paths("included-something \"x.kdl\"\n").is_empty());
        assert!(collect_include_paths("include \"\"\n").is_empty());
    }

    #[test]
    fn resolves_and_normalizes_include_paths() {
        let base = Path::new("/home/user/.config/niri");
        assert_eq!(
            resolve_include("./cfg/display.kdl", base, None),
            PathBuf::from("/home/user/.config/niri/cfg/display.kdl")
        );
        assert_eq!(
            resolve_include("/abs/display.kdl", base, None),
            PathBuf::from("/abs/display.kdl")
        );
        assert_eq!(
            resolve_include("~/niri/display.kdl", base, Some(Path::new("/home/user"))),
            PathBuf::from("/home/user/niri/display.kdl")
        );
        assert_eq!(
            normalize_path(Path::new("/a/b/../c/./d")),
            PathBuf::from("/a/c/d")
        );
    }

    #[test]
    fn matches_only_the_full_target_path() {
        let base = Path::new("/home/user/.config/niri");
        let target = PathBuf::from("/home/user/.config/niri/cfg/display.kdl");

        let direct = "include \"./cfg/display.kdl\"\n";
        assert!(includes_target(direct, base, &target, None));

        let absolute = "include \"/home/user/.config/niri/cfg/display.kdl\"\n";
        assert!(includes_target(absolute, base, &target, None));

        // A different file with the same basename must not match.
        let false_positive = "include \"old/display.kdl\"\n";
        assert!(!includes_target(false_positive, base, &target, None));

        let unrelated = "include \"./cfg/input.kdl\"\n";
        assert!(!includes_target(unrelated, base, &target, None));
    }
}
