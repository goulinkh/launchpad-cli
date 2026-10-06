use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

#[derive(Copy, Clone, Debug, Deserialize, Eq, PartialEq, JsonSchema, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum DiffSide {
    Original,
    Modified,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct DiffLocation {
    pub path: String,
    pub side: DiffSide,
    pub file_line: u64,
    pub diff_line: usize,
    pub kind: DiffLineKind,
}

impl DiffLocation {
    pub fn is_commentable(&self) -> bool {
        self.side == DiffSide::Modified || self.kind == DiffLineKind::Removed
    }
}

#[derive(Copy, Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum DiffLineKind {
    Context,
    Added,
    Removed,
}

pub fn map_file_line(
    diff: &str,
    requested_path: &str,
    side: DiffSide,
    file_line: u64,
) -> Option<DiffLocation> {
    if file_line == 0 {
        return None;
    }

    let lines: Vec<_> = diff.lines().collect();
    let mut index = 0;
    while index < lines.len() {
        if !lines[index].starts_with("diff --git ") {
            index += 1;
            continue;
        }

        let file_end = lines[index + 1..]
            .iter()
            .position(|line| line.starts_with("diff --git "))
            .map(|offset| index + offset + 1)
            .unwrap_or(lines.len());
        if let Some(location) =
            map_file_segment(&lines, index + 1, file_end, requested_path, side, file_line)
        {
            return Some(location);
        }
        index = file_end;
    }
    None
}

pub fn locate_diff_line(diff: &str, diff_line: usize) -> Option<DiffLocation> {
    let target = diff_line.checked_sub(1)?;
    let lines: Vec<_> = diff.lines().collect();
    let file_start = (0..=target).rev().find(|index| {
        lines
            .get(*index)
            .is_some_and(|line| line.starts_with("diff --git "))
    })?;
    let file_end = lines[file_start + 1..]
        .iter()
        .position(|line| line.starts_with("diff --git "))
        .map(|offset| file_start + offset + 1)
        .unwrap_or(lines.len());
    find_in_file_segment(
        &lines,
        file_start + 1,
        file_end,
        |_, _| true,
        |current_diff_line, kind, original_line, modified_line| {
            if current_diff_line != diff_line {
                return None;
            }
            let side = if kind == DiffLineKind::Removed {
                DiffSide::Original
            } else {
                DiffSide::Modified
            };
            let file_line = match side {
                DiffSide::Original => original_line?,
                DiffSide::Modified => modified_line?,
            };
            Some((side, file_line))
        },
    )
}

fn map_file_segment(
    lines: &[&str],
    start: usize,
    end: usize,
    requested_path: &str,
    side: DiffSide,
    file_line: u64,
) -> Option<DiffLocation> {
    find_in_file_segment(
        lines,
        start,
        end,
        |original_path, modified_path| {
            (requested_path == original_path || requested_path == modified_path)
                && !(side == DiffSide::Original && original_path == "/dev/null")
                && !(side == DiffSide::Modified && modified_path == "/dev/null")
        },
        |_, _, original_line, modified_line| {
            let current_line = match side {
                DiffSide::Original => original_line,
                DiffSide::Modified => modified_line,
            };
            (current_line == Some(file_line)).then_some((side, file_line))
        },
    )
}

fn find_in_file_segment(
    lines: &[&str],
    start: usize,
    end: usize,
    accept_paths: impl FnOnce(&str, &str) -> bool,
    mut select_line: impl FnMut(
        usize,
        DiffLineKind,
        Option<u64>,
        Option<u64>,
    ) -> Option<(DiffSide, u64)>,
) -> Option<DiffLocation> {
    let header = (start..end).find(|index| {
        lines[*index].starts_with("--- ")
            && lines
                .get(*index + 1)
                .is_some_and(|line| line.starts_with("+++ "))
    })?;
    let original_path = normalise_path(&lines[header][4..])?;
    let modified_path = normalise_path(&lines[header + 1][4..])?;
    if !accept_paths(&original_path, &modified_path) {
        return None;
    }
    let display_path = if modified_path == "/dev/null" {
        original_path
    } else {
        modified_path
    };
    let mut index = header + 2;
    while index < end {
        let Some((mut original_line, mut modified_line)) = parse_hunk_header(lines[index]) else {
            index += 1;
            continue;
        };
        index += 1;
        while index < end && !lines[index].starts_with("@@ ") {
            let line = lines[index];
            let (kind, current_original, current_modified) = if line.starts_with('+') {
                let current_modified = Some(modified_line);
                modified_line = modified_line.checked_add(1)?;
                (DiffLineKind::Added, None, current_modified)
            } else if line.starts_with('-') {
                let current_original = Some(original_line);
                original_line = original_line.checked_add(1)?;
                (DiffLineKind::Removed, current_original, None)
            } else if line.starts_with(' ') {
                let current_original = Some(original_line);
                let current_modified = Some(modified_line);
                original_line = original_line.checked_add(1)?;
                modified_line = modified_line.checked_add(1)?;
                (DiffLineKind::Context, current_original, current_modified)
            } else {
                index += 1;
                continue;
            };
            let diff_line = index + 1;
            if let Some((side, file_line)) =
                select_line(diff_line, kind, current_original, current_modified)
            {
                return Some(DiffLocation {
                    path: display_path,
                    side,
                    file_line,
                    diff_line,
                    kind,
                });
            }
            index += 1;
        }
    }
    None
}

fn normalise_path(raw: &str) -> Option<String> {
    let raw = raw.trim();
    let path = if let Some(quoted) = raw.strip_prefix('"').and_then(|raw| raw.strip_suffix('"')) {
        decode_quoted_path(quoted)?
    } else {
        raw.to_owned()
    };
    Some(
        path.strip_prefix("a/")
            .or_else(|| path.strip_prefix("b/"))
            .unwrap_or(&path)
            .to_owned(),
    )
}

fn decode_quoted_path(path: &str) -> Option<String> {
    let path = path.as_bytes();
    let mut decoded = Vec::with_capacity(path.len());
    let mut index = 0;
    while index < path.len() {
        if path[index] != b'\\' {
            decoded.push(path[index]);
            index += 1;
            continue;
        }

        index += 1;
        let escaped = *path.get(index)?;
        let byte = match escaped {
            b'a' => 0x07,
            b'b' => 0x08,
            b't' => b'\t',
            b'n' => b'\n',
            b'v' => 0x0b,
            b'f' => 0x0c,
            b'r' => b'\r',
            b'\\' => b'\\',
            b'"' => b'"',
            b'0'..=b'7' => {
                let mut value = 0_u16;
                let mut digits = 0;
                while digits < 3 && index < path.len() && matches!(path[index], b'0'..=b'7') {
                    value = value * 8 + u16::from(path[index] - b'0');
                    digits += 1;
                    index += 1;
                }
                decoded.push(u8::try_from(value).ok()?);
                continue;
            }
            _ => return None,
        };
        decoded.push(byte);
        index += 1;
    }
    String::from_utf8(decoded).ok()
}

fn parse_hunk_header(line: &str) -> Option<(u64, u64)> {
    let line = line.strip_prefix("@@ -")?;
    let (original, line) = line.split_once(' ')?;
    let line = line.strip_prefix('+')?;
    let (modified, _) = line.split_once(' ')?;
    Some((parse_range_start(original)?, parse_range_start(modified)?))
}

fn parse_range_start(range: &str) -> Option<u64> {
    range.split(',').next()?.parse().ok()
}

/// Read deleted source paths and their added/removed line counts from a Git preview diff.
pub fn deleted_file_stats(diff: &str) -> Vec<(String, [u64; 2])> {
    diff.split("diff --git ")
        .skip(1)
        .filter_map(|segment| {
            let mut lines = segment.lines();
            let source = lines.find_map(|line| line.strip_prefix("--- "))?;
            let destination = lines.next()?.strip_prefix("+++ ")?;
            if normalise_path(destination)?.as_str() != "/dev/null" {
                return None;
            }
            let source = normalise_path(source)?;
            if source == "/dev/null" {
                return None;
            }
            let mut counts = [0_u64; 2];
            for line in lines {
                if let Some([added, removed]) = parse_hunk_counts(line) {
                    counts[0] = counts[0].checked_add(added)?;
                    counts[1] = counts[1].checked_add(removed)?;
                }
            }
            Some((source, counts))
        })
        .collect()
}

fn parse_hunk_counts(line: &str) -> Option<[u64; 2]> {
    let line = line.strip_prefix("@@ -")?;
    let (original, line) = line.split_once(' ')?;
    let line = line.strip_prefix('+')?;
    let (modified, _) = line.split_once(" @@")?;
    let count = |range: &str| -> Option<u64> {
        match range.split_once(',') {
            Some((_, count)) => count.parse().ok(),
            None => Some(1),
        }
    };
    Some([count(modified)?, count(original)?])
}

#[cfg(test)]
mod tests {
    use super::{DiffLineKind, DiffSide, deleted_file_stats, locate_diff_line, map_file_line};

    const DIFF: &str = concat!(
        "diff --git a/src/old.rs b/src/new.rs\n",
        "similarity index 90%\n",
        "rename from src/old.rs\n",
        "rename to src/new.rs\n",
        "--- a/src/old.rs\n",
        "+++ b/src/new.rs\n",
        "@@ -10,3 +10,4 @@\n",
        " context\n",
        "-removed\n",
        "+added\n",
        "+another\n",
        " context 2\n",
    );

    const QUOTED_PATH_DIFF: &str = concat!(
        "diff --git \"a/\\303\\251.rs\" \"b/\\303\\251.rs\"\n",
        "--- \"a/\\303\\251.rs\"\n",
        "+++ \"b/\\303\\251.rs\"\n",
        "@@ -1 +1 @@\n",
        "-old\n",
        "+new\n",
    );

    const MULTI_FILE_DIFF: &str = concat!(
        "diff --git a/first.rs b/first.rs\n",
        "--- a/first.rs\n",
        "+++ b/first.rs\n",
        "@@ -0,0 +1 @@\n",
        "+first\n",
        "diff --git a/second.rs b/second.rs\n",
        "--- a/second.rs\n",
        "+++ b/second.rs\n",
        "@@ -1 +1 @@\n",
        "-old\n",
        "+new\n",
    );

    const OVERFLOW_DIFF: &str = concat!(
        "diff --git a/max.rs b/max.rs\n",
        "--- a/max.rs\n",
        "+++ b/max.rs\n",
        "@@ -18446744073709551615,1 +1,1 @@\n",
        " context\n",
    );

    #[test]
    fn deleted_file_stats_preserve_multiple_source_paths_and_hunk_sides() {
        let diff = concat!(
            "diff --git a/old.rs b/new.rs\n",
            "rename from old.rs\n",
            "rename to new.rs\n",
            "--- a/old.rs\n",
            "+++ b/new.rs\n",
            "@@ -1 +1,2 @@\n",
            "-old\n",
            "+new\n",
            "+again\n",
            "diff --git \"a/old name.rs\" \"b/old name.rs\"\n",
            "deleted file mode 100644\n",
            "--- \"a/old name.rs\"\n",
            "+++ /dev/null\n",
            "@@ -2,3 +0,0 @@\n",
            "-one\n",
            "-two\n",
            "-three\n",
            "@@ -12,2 +0,0 @@\n",
            "-four\n",
            "-five\n",
            "diff --git \"a/\\303\\251.rs\" \"b/\\303\\251.rs\"\n",
            "deleted file mode 100644\n",
            "--- \"a/\\303\\251.rs\"\n",
            "+++ /dev/null\n",
            "@@ -1 +0,0 @@\n",
            "-last\n",
        );
        assert_eq!(
            deleted_file_stats(diff),
            vec![
                ("old name.rs".to_owned(), [0, 5]),
                ("é.rs".to_owned(), [0, 1]),
            ]
        );
    }

    #[test]
    fn maps_modified_file_line_to_global_diff_line() {
        let location = map_file_line(DIFF, "src/new.rs", DiffSide::Modified, 11).unwrap();
        assert_eq!(location.diff_line, 10);
        assert_eq!(location.kind, DiffLineKind::Added);
        assert!(location.is_commentable());
    }

    #[test]
    fn maps_original_renamed_path() {
        let location = map_file_line(DIFF, "src/old.rs", DiffSide::Original, 11).unwrap();
        assert_eq!(location.path, "src/new.rs");
        assert_eq!(location.diff_line, 9);
        assert_eq!(location.kind, DiffLineKind::Removed);
    }

    #[test]
    fn rejects_lines_outside_diff_hunks() {
        assert!(map_file_line(DIFF, "src/new.rs", DiffSide::Modified, 1).is_none());
    }

    #[test]
    fn decodes_quoted_git_paths() {
        let location = map_file_line(QUOTED_PATH_DIFF, "é.rs", DiffSide::Modified, 1).unwrap();
        assert_eq!(location.diff_line, 6);
        assert_eq!(location.kind, DiffLineKind::Added);
    }

    #[test]
    fn locates_global_diff_lines() {
        let added = locate_diff_line(DIFF, 10).unwrap();
        assert_eq!(added.path, "src/new.rs");
        assert_eq!(added.side, DiffSide::Modified);
        assert_eq!(added.file_line, 11);
        assert_eq!(added.kind, DiffLineKind::Added);

        let removed = locate_diff_line(DIFF, 9).unwrap();
        assert_eq!(removed.side, DiffSide::Original);
        assert_eq!(removed.file_line, 11);
        assert_eq!(removed.kind, DiffLineKind::Removed);
    }

    #[test]
    fn locates_lines_in_later_files() {
        let location = locate_diff_line(MULTI_FILE_DIFF, 11).unwrap();
        assert_eq!(location.path, "second.rs");
        assert_eq!(location.file_line, 1);
        assert_eq!(location.kind, DiffLineKind::Added);
    }

    #[test]
    fn rejects_hunk_counter_overflow() {
        assert!(map_file_line(OVERFLOW_DIFF, "max.rs", DiffSide::Original, u64::MAX).is_none());
    }
}
