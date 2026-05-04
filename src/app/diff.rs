#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct PullRequestDiff {
    pub(super) files: Vec<DiffFile>,
    pub(super) additions: usize,
    pub(super) deletions: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct DiffFile {
    pub(super) old_path: String,
    pub(super) new_path: String,
    pub(super) metadata: Vec<String>,
    pub(super) hunks: Vec<DiffHunk>,
    pub(super) additions: usize,
    pub(super) deletions: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct DiffHunk {
    pub(super) header: String,
    pub(super) lines: Vec<DiffLine>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct DiffLine {
    pub(super) kind: DiffLineKind,
    pub(super) old_line: Option<usize>,
    pub(super) new_line: Option<usize>,
    pub(super) text: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum DiffLineKind {
    Context,
    Added,
    Removed,
    Metadata,
}

pub(super) fn parse_pull_request_diff(raw: &str) -> std::result::Result<PullRequestDiff, String> {
    let mut diff = PullRequestDiff {
        files: Vec::new(),
        additions: 0,
        deletions: 0,
    };
    let mut file: Option<DiffFile> = None;
    let mut hunk: Option<DiffHunk> = None;
    let mut old_line = 0_usize;
    let mut new_line = 0_usize;

    for raw_line in raw.replace('\r', "").lines() {
        if raw_line.starts_with("diff --git ") {
            flush_diff_file(&mut diff, &mut file, &mut hunk);
            let (old_path, new_path) = parse_diff_git_paths(raw_line);
            file = Some(DiffFile {
                old_path,
                new_path,
                metadata: Vec::new(),
                hunks: Vec::new(),
                additions: 0,
                deletions: 0,
            });
            continue;
        }

        let Some(current_file) = file.as_mut() else {
            continue;
        };

        if raw_line.starts_with("@@ ") {
            flush_diff_hunk(current_file, &mut hunk);
            let (next_old, next_new) = parse_hunk_line_starts(raw_line);
            old_line = next_old;
            new_line = next_new;
            hunk = Some(DiffHunk {
                header: raw_line.to_string(),
                lines: Vec::new(),
            });
            continue;
        }

        if let Some(current_hunk) = hunk.as_mut() {
            push_diff_hunk_line(
                current_file,
                current_hunk,
                raw_line,
                &mut old_line,
                &mut new_line,
            );
        } else if let Some(path) = raw_line.strip_prefix("--- ") {
            current_file.old_path = normalize_diff_path(path.trim());
        } else if let Some(path) = raw_line.strip_prefix("+++ ") {
            current_file.new_path = normalize_diff_path(path.trim());
        } else if !raw_line.trim().is_empty() {
            current_file.metadata.push(raw_line.to_string());
        }
    }

    flush_diff_file(&mut diff, &mut file, &mut hunk);
    diff.additions = diff.files.iter().map(|file| file.additions).sum();
    diff.deletions = diff.files.iter().map(|file| file.deletions).sum();
    Ok(diff)
}

pub(super) fn parse_inline_diff_hunk(raw: &str) -> Option<DiffHunk> {
    let mut hunk: Option<DiffHunk> = None;
    let mut file = DiffFile {
        old_path: "-".to_string(),
        new_path: "-".to_string(),
        metadata: Vec::new(),
        hunks: Vec::new(),
        additions: 0,
        deletions: 0,
    };
    let mut old_line = 0_usize;
    let mut new_line = 0_usize;

    for raw_line in raw.replace('\r', "").lines() {
        if raw_line.starts_with("@@ ") {
            let (next_old, next_new) = parse_hunk_line_starts(raw_line);
            old_line = next_old;
            new_line = next_new;
            hunk = Some(DiffHunk {
                header: raw_line.to_string(),
                lines: Vec::new(),
            });
            continue;
        }

        if let Some(current_hunk) = hunk.as_mut() {
            push_diff_hunk_line(
                &mut file,
                current_hunk,
                raw_line,
                &mut old_line,
                &mut new_line,
            );
        }
    }

    hunk.filter(|hunk| !hunk.lines.is_empty())
}

fn flush_diff_file(
    diff: &mut PullRequestDiff,
    file: &mut Option<DiffFile>,
    hunk: &mut Option<DiffHunk>,
) {
    if let Some(current_file) = file.as_mut() {
        flush_diff_hunk(current_file, hunk);
    }
    if let Some(current_file) = file.take() {
        diff.files.push(current_file);
    }
}

fn flush_diff_hunk(file: &mut DiffFile, hunk: &mut Option<DiffHunk>) {
    if let Some(current_hunk) = hunk.take() {
        file.hunks.push(current_hunk);
    }
}

fn parse_diff_git_paths(line: &str) -> (String, String) {
    let mut parts = line
        .strip_prefix("diff --git ")
        .unwrap_or_default()
        .split_whitespace();
    let old_path = parts
        .next()
        .map(normalize_diff_path)
        .unwrap_or_else(|| "-".to_string());
    let new_path = parts
        .next()
        .map(normalize_diff_path)
        .unwrap_or_else(|| old_path.clone());
    (old_path, new_path)
}

fn normalize_diff_path(path: &str) -> String {
    let path = path.trim().trim_matches('"');
    if path == "/dev/null" {
        return path.to_string();
    }
    path.strip_prefix("a/")
        .or_else(|| path.strip_prefix("b/"))
        .unwrap_or(path)
        .to_string()
}

fn parse_hunk_line_starts(header: &str) -> (usize, usize) {
    let mut parts = header.split_whitespace();
    let _marker = parts.next();
    let old = parts.next().and_then(parse_diff_range).unwrap_or(0);
    let new = parts.next().and_then(parse_diff_range).unwrap_or(0);
    (old, new)
}

fn parse_diff_range(range: &str) -> Option<usize> {
    let range = range.trim_start_matches(['-', '+']);
    range
        .split_once(',')
        .map(|(start, _)| start)
        .unwrap_or(range)
        .parse()
        .ok()
}

fn push_diff_hunk_line(
    file: &mut DiffFile,
    hunk: &mut DiffHunk,
    raw_line: &str,
    old_line: &mut usize,
    new_line: &mut usize,
) {
    let line = if raw_line.starts_with('+') && !raw_line.starts_with("+++") {
        file.additions += 1;
        let line = DiffLine {
            kind: DiffLineKind::Added,
            old_line: None,
            new_line: Some(*new_line),
            text: raw_line[1..].to_string(),
        };
        *new_line = new_line.saturating_add(1);
        line
    } else if raw_line.starts_with('-') && !raw_line.starts_with("---") {
        file.deletions += 1;
        let line = DiffLine {
            kind: DiffLineKind::Removed,
            old_line: Some(*old_line),
            new_line: None,
            text: raw_line[1..].to_string(),
        };
        *old_line = old_line.saturating_add(1);
        line
    } else if let Some(text) = raw_line.strip_prefix(' ') {
        let line = DiffLine {
            kind: DiffLineKind::Context,
            old_line: Some(*old_line),
            new_line: Some(*new_line),
            text: text.to_string(),
        };
        *old_line = old_line.saturating_add(1);
        *new_line = new_line.saturating_add(1);
        line
    } else {
        DiffLine {
            kind: DiffLineKind::Metadata,
            old_line: None,
            new_line: None,
            text: raw_line.trim_start_matches('\\').trim_start().to_string(),
        }
    };
    hunk.lines.push(line);
}
