//! Read-only apt source inventory and parsers. This module never writes source files.

use std::path::{Path, PathBuf};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    pub file: PathBuf,
    pub line: usize,
    pub format: Format,
    pub enabled: bool,
    pub suites: Vec<String>,
    pub uris: Vec<String>,
    pub rewritable: Vec<bool>,
    pub issue: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Format {
    Legacy,
    Deb822,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileReport {
    pub path: PathBuf,
    pub entries: Vec<Entry>,
    pub issue: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Inventory {
    pub files: Vec<FileReport>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlannedFile {
    pub path: PathBuf,
    pub content: String,
    /// `true` means a later apply step may create this path; this module never does so.
    pub create_new: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RewritePlan {
    /// Pure output only. No path in this plan is read or written by the rewrite function.
    pub files: Vec<PlannedFile>,
    pub skipped: Vec<String>,
}

impl Inventory {
    pub fn has_rewritable_uri(&self) -> bool {
        self.files
            .iter()
            .flat_map(|f| &f.entries)
            .any(|e| e.rewritable.iter().any(|v| *v))
    }

    /// Human-readable read-only report; one line per file, source entry, and URI token.
    pub fn report(&self) -> String {
        let mut lines = Vec::new();
        for file in &self.files {
            lines.push(format!("source file: {}", file.path.display()));
            if let Some(issue) = &file.issue {
                lines.push(format!("  skipped: {issue}"));
                continue;
            }
            if file.entries.is_empty() {
                lines.push("  no active source entries".into());
            }
            for entry in &file.entries {
                if entry.uris.is_empty() {
                    lines.push(format!(
                        "  line {}: skipped: {}",
                        entry.line,
                        entry.issue.as_deref().unwrap_or("no URI")
                    ));
                    continue;
                }
                for (i, uri) in entry.uris.iter().enumerate() {
                    let state = if entry.rewritable.get(i).copied().unwrap_or(false) {
                        "candidate for rewrite"
                    } else {
                        entry.issue.as_deref().unwrap_or("not eligible for rewrite")
                    };
                    lines.push(format!("  line {}: {uri} — {state}", entry.line));
                }
                if entry.rewritable.iter().any(|v| *v)
                    && let Some(note) = &entry.issue
                {
                    lines.push(format!("  line {}: note: {note}", entry.line));
                }
            }
        }
        if lines.is_empty() {
            lines.push("no apt source files found".into());
        }
        lines.join("\n")
    }
}

/// Parse legacy apt source lines. Comments/blank lines are ignored. Every active deb/deb-src line
/// becomes one entry. `roots` is every host and path prefix this run can recognize as an Ubuntu
/// archive mirror, from any country, plus the canonical archive.
pub fn parse_legacy(
    text: &str,
    file: &Path,
    roots: &[MirrorRoot],
    pop_os_system_sources: bool,
) -> Vec<Entry> {
    let mut entries = Vec::new();
    for (i, line) in text.lines().enumerate() {
        let trimmed = line.trim_start();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }
        let mut words = trimmed.split_whitespace();
        let Some(kind) = words.next() else { continue };
        if !matches!(kind, "deb" | "deb-src") {
            entries.push(invalid(
                file,
                i + 1,
                Format::Legacy,
                format!("unsupported source type '{kind}'"),
            ));
            continue;
        }
        let mut next = words.next();
        if next.is_some_and(|w| w.starts_with('[')) {
            let mut options = next.unwrap_or_default().to_string();
            while !options.ends_with(']') {
                let Some(w) = words.next() else { break };
                options.push(' ');
                options.push_str(w);
            }
            if !options.ends_with(']') {
                entries.push(invalid(
                    file,
                    i + 1,
                    Format::Legacy,
                    "unterminated options".into(),
                ));
                continue;
            }
            next = words.next();
        }
        let Some(uri) = next else {
            entries.push(invalid(file, i + 1, Format::Legacy, "missing URI".into()));
            continue;
        };
        let Some(suite) = words.next().filter(|w| !w.starts_with('#')) else {
            entries.push(invalid(file, i + 1, Format::Legacy, "missing suite".into()));
            continue;
        };
        let rewritable =
            is_rewritable(uri, roots, pop_os_system_sources) && !suite.ends_with("-security");
        let issue = if suite.ends_with("-security") {
            Some("security suite is never rewritten".into())
        } else if uri.starts_with("mirror://") || uri.starts_with("mirror+file:") {
            Some(format!(
                "mirror URI scheme in '{uri}' is unsupported and will be left unchanged"
            ))
        } else if let Some(reason) = dot_segment_reason(uri) {
            Some(reason)
        } else if !is_rewritable(uri, roots, pop_os_system_sources) {
            Some(format!(
                "URI '{uri}' is not an official Ubuntu archive mirror"
            ))
        } else {
            None
        };
        entries.push(Entry {
            file: file.to_path_buf(),
            line: i + 1,
            format: Format::Legacy,
            enabled: true,
            suites: vec![suite.to_string()],
            uris: vec![uri.to_string()],
            rewritable: vec![rewritable],
            issue,
        });
    }
    entries
}

/// Parse deb822 sources into one record per stanza. Continuation values are whitespace-tokenized;
/// source bytes are not modified. Duplicate singleton fields or malformed fields fail closed.
pub fn parse_deb822(
    text: &str,
    file: &Path,
    roots: &[MirrorRoot],
    pop_os_system_sources: bool,
) -> Vec<Entry> {
    let mut out = Vec::new();
    let mut stanza = Vec::<(usize, String, String)>::new();
    let mut issue = None::<String>;
    let flush = |out: &mut Vec<Entry>,
                 stanza: &mut Vec<(usize, String, String)>,
                 issue: &mut Option<String>| {
        if stanza.is_empty() && issue.is_none() {
            return;
        }
        let line = stanza.first().map_or(1, |v| v.0);
        let mut uris = Vec::new();
        let mut suites = Vec::new();
        let mut enabled = true;
        let mut seen = std::collections::HashSet::new();
        for (_, key, value) in stanza.iter() {
            if !seen.insert(key.to_ascii_lowercase())
                && matches!(
                    key.to_ascii_lowercase().as_str(),
                    "types" | "uris" | "suites" | "enabled"
                )
            {
                *issue = Some(format!("duplicate singleton field '{key}'"));
            }
            match key.to_ascii_lowercase().as_str() {
                "uris" => uris.extend(value.split_whitespace().map(str::to_string)),
                "suites" => suites.extend(value.split_whitespace().map(str::to_string)),
                "enabled" => match value.trim().to_ascii_lowercase().as_str() {
                    "yes" => enabled = true,
                    "no" => enabled = false,
                    _ => *issue = Some(format!("invalid Enabled value '{value}'")),
                },
                _ => {}
            }
        }
        if uris.is_empty() {
            *issue = Some("missing or empty URIs field".into());
        }
        if suites.is_empty() {
            *issue = Some("missing or empty Suites field".into());
        }
        if !seen.contains("types") {
            *issue = Some("missing Types field".into());
        }
        let has_security = suites.iter().any(|s| s.ends_with("-security"));
        let has_non_security = suites.iter().any(|s| !s.ends_with("-security"));
        let pop_security_split = pop_os_system_sources && has_security && has_non_security;
        let rewritable = uris
            .iter()
            .map(|uri| {
                issue.is_none()
                    && enabled
                    && (!has_security || pop_security_split)
                    && is_rewritable(uri, roots, pop_os_system_sources)
            })
            .collect::<Vec<_>>();
        if issue.is_none() && !enabled {
            *issue = Some("stanza disabled (Enabled: no)".into());
        }
        if issue.is_none()
            && let Some(reason) = uris.iter().find_map(|uri| dot_segment_reason(uri))
        {
            *issue = Some(reason);
        }
        if issue.is_none()
            && uris
                .iter()
                .any(|uri| !is_rewritable(uri, roots, pop_os_system_sources))
        {
            *issue = Some("one or more URIs are not official Ubuntu archive mirror hosts".into());
        }
        if issue.is_none() && has_security {
            *issue = Some(if pop_security_split {
                "Pop!_OS security suite remains on the original mirror; non-security suites are eligible for rewrite".into()
            } else {
                "stanza includes a security suite and is not eligible for rewrite".into()
            });
        }
        out.push(Entry {
            file: file.to_path_buf(),
            line,
            format: Format::Deb822,
            enabled,
            suites,
            uris,
            rewritable,
            issue: issue.take(),
        });
        stanza.clear();
    };
    for (i, line) in text.lines().enumerate() {
        if line.trim().is_empty() {
            flush(&mut out, &mut stanza, &mut issue);
            continue;
        }
        if line.trim_start().starts_with('#') {
            continue;
        }
        if line.starts_with([' ', '\t']) {
            if let Some((_, _, value)) = stanza.last_mut() {
                value.push(' ');
                value.push_str(line.trim());
            } else {
                issue = Some(format!("orphan continuation at line {}", i + 1));
            }
            continue;
        }
        if let Some((key, value)) = line.split_once(':') {
            let key = key.trim();
            if key.is_empty() || !key.chars().all(|c| c.is_ascii_alphanumeric() || c == '-') {
                issue = Some(format!("invalid field name at line {}", i + 1));
            } else {
                stanza.push((i + 1, key.to_string(), value.trim().to_string()));
            }
        } else {
            issue = Some(format!("malformed field at line {}", i + 1));
        }
    }
    flush(&mut out, &mut stanza, &mut issue);
    out
}

fn invalid(file: &Path, line: usize, format: Format, issue: String) -> Entry {
    Entry {
        file: file.to_path_buf(),
        line,
        format,
        enabled: true,
        suites: vec![],
        uris: vec![],
        rewritable: vec![],
        issue: Some(issue),
    }
}

/// Where a mirror list says an Ubuntu archive lives: a host and the path prefix of the tree it
/// serves. Membership is recognizability, not trust (SPEC: Apply).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MirrorRoot {
    /// Lowercase, no trailing dot.
    pub host: String,
    /// Starts with `/` and is never empty; `/` admits every path on the host.
    pub path: String,
}

/// The `(host, path)` pair a listed URL names, or `None` when its authority or path cannot be read.
/// The authority is parsed the way `is_rewritable` parses one — bracketed IPv6, optional port, host
/// lowercased with the trailing dot trimmed — and the port is dropped, because eligibility never
/// compares it. A caller validates the URL first; a host this cannot read contributes nothing rather
/// than being guessed at, and the run reports it as left out.
pub fn mirror_root(url: &str) -> Option<MirrorRoot> {
    let (scheme, rest) = url.split_once("://")?;
    if !matches!(scheme, "http" | "https") {
        return None;
    }
    let authority = rest.split(['/', '?', '#']).next().unwrap_or_default();
    let tail = &rest[authority.len()..];
    if authority.is_empty() || authority.contains('@') {
        return None;
    }
    let (host, port) = if let Some(v6) = authority.strip_prefix('[') {
        let (host, after) = v6.split_once(']')?;
        let port = match after {
            "" => None,
            suffix if suffix.starts_with(':') => Some(&suffix[1..]),
            _ => return None,
        };
        (host, port)
    } else {
        match authority.rsplit_once(':') {
            Some((host, port)) => (host, Some(port)),
            None => (authority, None),
        }
    };
    if let Some(port) = port
        && (port.is_empty()
            || !port.bytes().all(|b| b.is_ascii_digit())
            || !matches!(port.parse::<u16>(), Ok(p) if p > 0))
    {
        return None;
    }
    if host.is_empty()
        || !host
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-'))
    {
        return None;
    }
    let host = host.trim_end_matches('.');
    if host.is_empty() {
        return None;
    }
    let path = tail.split(['?', '#']).next().unwrap_or_default();
    // A listing that names a dot segment is left out of the set rather than resolved into something
    // else, and the caller reports it with the other entries that cannot enter it.
    if dot_segment(path).is_some() {
        return None;
    }
    Some(MirrorRoot {
        host: host.to_ascii_lowercase(),
        path: if path.is_empty() {
            "/".to_string()
        } else {
            path.to_string()
        },
    })
}

/// The first segment of `path` that names a directory hop rather than a directory, spelled as it was
/// written: `.` or `..`, plainly or with either dot percent-encoded (`%2e%2e`, `%2E%2E`, `.%2e`,
/// `%2e.`). Nothing here decodes escapes in general and nothing resolves a segment: a dot segment is
/// only ever found so that the path can be refused. A listed `/ubuntu` admits an on-disk
/// `/ubuntu/../debian` under the whole-segment rule while that path names the `/debian` tree, and
/// refusing is both the smaller answer and the one a spelling cannot walk around.
fn dot_segment(path: &str) -> Option<&str> {
    path.split('/').find(|segment| {
        let plain = segment.replace("%2e", ".").replace("%2E", ".");
        plain == "." || plain == ".."
    })
}

/// Collapse repeated slashes and drop a trailing one, so that no spelling of a path can change what
/// it admits: `/ubuntu`, `/ubuntu/` and `//ubuntu//` are one tree, and `/` alone stays `/`. A list
/// spells a tree with the trailing slash and a run writes the URI without it, so the two sides of the
/// comparison below must not disagree about a slash that names no segment.
fn normalize_path(path: &str) -> String {
    let mut normalized = String::with_capacity(path.len() + 1);
    for segment in path.split('/').filter(|s| !s.is_empty()) {
        normalized.push('/');
        normalized.push_str(segment);
    }
    if normalized.is_empty() {
        normalized.push('/');
    }
    normalized
}

/// Segment-wise prefix test: a listed `/ubuntu` admits `/ubuntu`, `/ubuntu/` and
/// `/ubuntu/dists/...`, never `/ubuntufoo`; a listed `/` admits the whole host. Both sides are
/// normalised first, so a trailing or repeated slash is the same tree on either side. Nothing else is
/// ignored: the comparison is byte-exact, because only the host is case-insensitive.
fn path_admits(root: &str, path: &str) -> bool {
    let root = normalize_path(root);
    let path = normalize_path(path);
    root == "/"
        || path == root
        || (path.starts_with(&root) && path.as_bytes().get(root.len()) == Some(&b'/'))
}

/// Hosts that are never replaced, whatever a mirror list names: a `-security` or `ports` archive is
/// not an install mirror, and a PPA is a third-party repository (SPEC: Apply).
const NEVER_REWRITE: [&str; 4] = [
    "security.ubuntu.com",
    "ports.ubuntu.com",
    "ppa.launchpad.net",
    "ppa.launchpadcontent.net",
];

/// Whether this URI token may be replaced by the selected mirror. Membership in `roots` answers
/// recognizability only, and it is asked last: the refusal list and the Pop!_OS path and suite
/// conditions are decided first, whatever the list names, and a `-security` suite is refused by the
/// caller.
fn is_rewritable(uri: &str, roots: &[MirrorRoot], pop_os_system_sources: bool) -> bool {
    if uri.chars().any(|c| c.is_whitespace() || c.is_control()) || uri.contains(['\\', '@']) {
        return false;
    }
    let Some((scheme, rest)) = uri.split_once("://") else {
        return false;
    };
    if !matches!(scheme, "http" | "https") {
        return false;
    }
    let authority = rest.split(['/', '?', '#']).next().unwrap_or_default();
    let tail = &rest[authority.len()..];
    if authority.is_empty() || authority.contains('@') {
        return false;
    }
    let (host, port) = if let Some(v6) = authority.strip_prefix('[') {
        let Some((host, tail)) = v6.split_once(']') else {
            return false;
        };
        let port = match tail {
            "" => None,
            suffix if suffix.starts_with(':') => Some(&suffix[1..]),
            _ => return false,
        };
        (host, port)
    } else {
        match authority.rsplit_once(':') {
            Some((host, port)) => (host, Some(port)),
            None => (authority, None),
        }
    };
    if let Some(port) = port
        && (port.is_empty()
            || !port.bytes().all(|b| b.is_ascii_digit())
            || !matches!(port.parse::<u16>(), Ok(p) if p > 0))
    {
        return false;
    }
    if host.is_empty()
        || !host
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-'))
    {
        return false;
    }
    let host = host.trim_end_matches('.').to_ascii_lowercase();
    // The refusal list is explicit and runs before any membership test, whatever a list names:
    // a PPA was safe before only because no list happened to name one, which is not a control.
    if NEVER_REWRITE.contains(&host.as_str())
        || ["launchpad.net", "launchpadcontent.net"]
            .iter()
            .any(|domain| host == *domain || host.ends_with(&format!(".{domain}")))
    {
        return false;
    }
    let path = tail.split(['?', '#']).next().unwrap_or_default();
    // Refused before any host or membership test, so neither a listed path nor the canonical archive's
    // any-path rule can admit a tree the token does not name.
    if dot_segment(path).is_some() {
        return false;
    }
    let policy_match = roots
        .iter()
        .any(|r| r.host.eq_ignore_ascii_case(&host) && path_admits(&r.path, path));
    if host == "apt.pop-os.org" {
        if !policy_match || !pop_os_system_sources {
            return false;
        }
        return normalize_path(path) == "/ubuntu";
    }
    host == "archive.ubuntu.com" || host.ends_with(".archive.ubuntu.com") || policy_match
}

/// Why a URI token with a dot segment is not eligible, naming the segment it found so that the
/// skipped entry is actionable on its own. `None` when the token carries no dot segment, because the
/// refusal then has another reason to state.
fn dot_segment_reason(uri: &str) -> Option<String> {
    let (_, rest) = uri.split_once("://")?;
    let authority = rest.split(['/', '?', '#']).next().unwrap_or_default();
    let path = rest[authority.len()..]
        .split(['?', '#'])
        .next()
        .unwrap_or_default();
    let segment = dot_segment(path)?;
    Some(format!(
        "URI '{uri}' carries a '{segment}' path segment, so it is left unchanged"
    ))
}

/// The reason a URI token was left alone: a dot segment is named, and every other refusal keeps the
/// line a reader already knows.
fn refusal_reason(uri: &str) -> String {
    dot_segment_reason(uri).unwrap_or_else(|| format!("{uri} was not changed"))
}

/// Discover apt source files without following symlinks. Non-UTF8 files are reported, not guessed.
/// `roots` is every host and path prefix this run can recognize as an Ubuntu archive mirror, from
/// any country, plus the canonical archive: membership makes a URI eligible for replacement and
/// nothing else, and `is_rewritable` still refuses the hosts that are never replaced.
pub fn inventory(root: &Path, roots: &[MirrorRoot], pop_os: bool) -> Inventory {
    match std::fs::symlink_metadata(root) {
        Ok(meta) if meta.file_type().is_dir() => {}
        Ok(_) => {
            return Inventory {
                files: vec![FileReport {
                    path: root.to_path_buf(),
                    entries: vec![],
                    issue: Some(
                        "not a directory or is a symlink; symlinks are not followed".into(),
                    ),
                }],
            };
        }
        Err(e) => {
            return Inventory {
                files: vec![FileReport {
                    path: root.to_path_buf(),
                    entries: vec![],
                    issue: Some(format!("cannot inspect {}: {e}", root.display())),
                }],
            };
        }
    }
    let mut paths = Vec::new();
    let mut files = Vec::new();
    let main = root.join("sources.list");
    match std::fs::symlink_metadata(&main) {
        Ok(meta) if meta.file_type().is_file() => paths.push(main),
        Ok(_) => files.push(FileReport {
            path: main,
            entries: vec![],
            issue: Some("not a regular file; symlinks are not followed".into()),
        }),
        Err(e) if e.kind() != std::io::ErrorKind::NotFound => files.push(FileReport {
            path: main.clone(),
            entries: vec![],
            issue: Some(format!("cannot inspect {}: {e}", main.display())),
        }),
        Err(_) => {}
    }
    let dir = root.join("sources.list.d");
    match std::fs::symlink_metadata(&dir) {
        Ok(meta) if meta.file_type().is_dir() => match std::fs::read_dir(&dir) {
            Ok(items) => {
                let mut children = Vec::new();
                for item in items {
                    match item {
                        Ok(entry) => children.push(entry),
                        Err(e) => files.push(FileReport {
                            path: dir.clone(),
                            entries: vec![],
                            issue: Some(format!(
                                "cannot inspect an entry in {}: {e}",
                                dir.display()
                            )),
                        }),
                    }
                }
                children.sort_by_key(|e| e.file_name());
                for entry in children {
                    let path = entry.path();
                    if !matches!(
                        path.extension().and_then(|x| x.to_str()),
                        Some("list" | "sources")
                    ) {
                        files.push(FileReport {
                            path,
                            entries: vec![],
                            issue: Some(
                                "unsupported source filename; only .list and .sources are read"
                                    .into(),
                            ),
                        });
                        continue;
                    }
                    match entry.file_type() {
                        Ok(t) if t.is_file() => paths.push(path),
                        Ok(_) => files.push(FileReport {
                            path,
                            entries: vec![],
                            issue: Some("not a regular file; symlinks are not followed".into()),
                        }),
                        Err(e) => files.push(FileReport {
                            path: path.clone(),
                            entries: vec![],
                            issue: Some(format!("cannot inspect {}: {e}", path.display())),
                        }),
                    }
                }
            }
            Err(e) => files.push(FileReport {
                path: dir.clone(),
                entries: vec![],
                issue: Some(format!("cannot list {}: {e}", dir.display())),
            }),
        },
        Ok(_) => files.push(FileReport {
            path: dir.clone(),
            entries: vec![],
            issue: Some("not a directory; symlinks are not followed".into()),
        }),
        Err(e) if e.kind() != std::io::ErrorKind::NotFound => files.push(FileReport {
            path: dir.clone(),
            entries: vec![],
            issue: Some(format!("cannot inspect {}: {e}", dir.display())),
        }),
        Err(_) => {}
    }
    paths.sort();
    files.extend(paths.into_iter().map(|path| match std::fs::read(&path) {
        Err(e) => FileReport {
            path: path.clone(),
            entries: vec![],
            issue: Some(format!("cannot read {}: {e}", path.display())),
        },
        Ok(bytes) => match String::from_utf8(bytes) {
            Err(_) => FileReport {
                path: path.clone(),
                entries: vec![],
                issue: Some("file is not valid UTF-8; skipped without changes".into()),
            },
            Ok(text) => {
                let entries = if path.extension().and_then(|x| x.to_str()) == Some("sources") {
                    parse_deb822(
                        &text,
                        &path,
                        roots,
                        pop_os && path.file_name().is_some_and(|n| n == "system.sources"),
                    )
                } else {
                    parse_legacy(&text, &path, roots, false)
                };
                FileReport {
                    path,
                    entries,
                    issue: None,
                }
            }
        },
    }));
    Inventory { files }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct PhysicalLine {
    start: usize,
    body_end: usize,
    end: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ValueToken {
    range: std::ops::Range<usize>,
    line: std::ops::Range<usize>,
    continuation: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct RawField {
    key: String,
    range: std::ops::Range<usize>,
    values: Vec<ValueToken>,
}

fn physical_lines(text: &str) -> Vec<PhysicalLine> {
    let mut result = Vec::new();
    let mut start = 0;
    for raw in text.split_inclusive('\n') {
        let end = start + raw.len();
        let mut body_end = end;
        if raw.ends_with('\n') {
            body_end -= 1;
        }
        if body_end > start && text.as_bytes()[body_end - 1] == b'\r' {
            body_end -= 1;
        }
        result.push(PhysicalLine {
            start,
            body_end,
            end,
        });
        start = end;
    }
    result
}

fn value_tokens(
    value: &str,
    base: usize,
    line: std::ops::Range<usize>,
    continuation: bool,
) -> Vec<ValueToken> {
    let mut result = Vec::new();
    let mut token_start = None;
    for (i, ch) in value.char_indices() {
        if ch.is_whitespace() {
            if let Some(start) = token_start.take() {
                result.push(ValueToken {
                    range: base + start..base + i,
                    line: line.clone(),
                    continuation,
                });
            }
        } else if token_start.is_none() {
            token_start = Some(i);
        }
    }
    if let Some(start) = token_start {
        result.push(ValueToken {
            range: base + start..base + value.len(),
            line,
            continuation,
        });
    }
    result
}

fn token_ranges(value: &str) -> Vec<std::ops::Range<usize>> {
    value_tokens(value, 0, 0..value.len(), false)
        .into_iter()
        .map(|t| t.range)
        .collect()
}

fn field_spans(text: &str) -> Vec<RawField> {
    let mut result = Vec::new();
    let mut current: Option<RawField> = None;
    for line in physical_lines(text) {
        let body = &text[line.start..line.body_end];
        if body.trim().is_empty() {
            if let Some(mut field) = current.take() {
                field.range.end = line.start;
                result.push(field);
            }
            continue;
        }
        if body.trim_start().starts_with('#') {
            continue;
        }
        if body.starts_with([' ', '\t']) {
            if let Some(field) = current.as_mut() {
                let leading = body.len() - body.trim_start().len();
                field.values.extend(value_tokens(
                    &body[leading..],
                    line.start + leading,
                    line.start..line.end,
                    true,
                ));
            }
            continue;
        }
        if let Some((key, value)) = body.split_once(':') {
            if let Some(mut field) = current.take() {
                field.range.end = line.start;
                result.push(field);
            }
            let value_start = key.len() + 1;
            let mut field = RawField {
                key: key.trim().to_ascii_lowercase(),
                range: line.start..text.len(),
                values: Vec::new(),
            };
            field.values.extend(value_tokens(
                value,
                line.start + value_start,
                line.start..line.end,
                false,
            ));
            current = Some(field);
        } else if let Some(mut field) = current.take() {
            field.range.end = line.start;
            result.push(field);
        }
    }
    if let Some(mut field) = current {
        field.range.end = text.len();
        result.push(field);
    }
    result
}

fn stanza_ranges(text: &str) -> Vec<std::ops::Range<usize>> {
    let mut result = Vec::new();
    let mut start = 0;
    for line in physical_lines(text) {
        if text[line.start..line.body_end].trim().is_empty() {
            if start < line.start && !text[start..line.start].trim().is_empty() {
                result.push(start..line.start);
            }
            start = line.end;
        }
    }
    if start < text.len() && !text[start..].trim().is_empty() {
        result.push(start..text.len());
    }
    result
}

fn locate_legacy_uri(line: &str) -> Option<std::ops::Range<usize>> {
    let tokens = token_ranges(line);
    if tokens.is_empty() || !matches!(&line[tokens[0].clone()], "deb" | "deb-src") {
        return None;
    }
    let mut i = 1;
    if tokens
        .get(i)
        .is_some_and(|r| line[r.clone()].starts_with('['))
    {
        while i < tokens.len() && !line[tokens[i].clone()].ends_with(']') {
            i += 1;
        }
        if i == tokens.len() {
            return None;
        }
        i += 1;
    }
    let uri = tokens.get(i)?.clone();
    let suite = tokens.get(i + 1)?;
    if line[suite.clone()].starts_with('#') {
        return None;
    }
    Some(uri)
}

fn apply_replacements(
    text: &str,
    mut replacements: Vec<(std::ops::Range<usize>, String)>,
) -> String {
    replacements.sort_by_key(|(range, _)| range.start);
    let mut output = text.to_string();
    for (range, value) in replacements.into_iter().rev() {
        output.replace_range(range, &value);
    }
    output
}

fn security_suite_removals(
    text: &str,
    tokens: &[ValueToken],
    suites: &[String],
) -> Vec<(std::ops::Range<usize>, String)> {
    let mut replacements = Vec::new();
    let mut start = 0;
    while start < tokens.len() {
        let line = tokens[start].line.clone();
        let mut end = start + 1;
        while end < tokens.len() && tokens[end].line == line {
            end += 1;
        }
        let indices: Vec<_> = (start..end)
            .filter(|&i| suites.get(i).is_some_and(|s| s.ends_with("-security")))
            .collect();
        if !indices.is_empty() {
            let all_on_line_are_security = indices.len() == end - start;
            if tokens[start].continuation && all_on_line_are_security {
                replacements.push((line.clone(), String::new()));
            } else {
                let mut run_start = indices[0];
                let mut previous = indices[0];
                for &index in indices.iter().skip(1).chain(std::iter::once(&usize::MAX)) {
                    if index != previous + 1 {
                        let first = &tokens[run_start];
                        let last = &tokens[previous];
                        let range = if run_start > start {
                            tokens[run_start - 1].range.end..last.range.end
                        } else if previous + 1 < end {
                            first.range.start..tokens[previous + 1].range.start
                        } else {
                            first.range.clone()
                        };
                        replacements.push((range, String::new()));
                        if index != usize::MAX {
                            run_start = index;
                        }
                    }
                    previous = index;
                }
            }
        }
        start = end;
    }
    let _ = text;
    replacements
}

fn pop_security_content(
    stanza: &str,
    fields: &[RawField],
    uris: &[String],
    security_suites: &[String],
) -> String {
    let newline = if stanza.contains("\r\n") {
        "\r\n"
    } else {
        "\n"
    };
    let mut output = String::new();
    let append_raw = |output: &mut String, field: Option<&RawField>| {
        if let Some(field) = field {
            let raw = &stanza[field.range.clone()];
            output.push_str(raw);
            if !raw.ends_with('\n') {
                output.push_str(newline);
            }
        }
    };
    append_raw(&mut output, fields.iter().find(|f| f.key == "types"));
    output.push_str(&format!("URIs: {}{newline}", uris.join(" ")));
    output.push_str(&format!("Suites: {}{newline}", security_suites.join(" ")));
    append_raw(&mut output, fields.iter().find(|f| f.key == "components"));
    append_raw(&mut output, fields.iter().find(|f| f.key == "enabled"));
    append_raw(&mut output, fields.iter().find(|f| f.key == "signed-by"));
    output.push_str(&format!("X-Repolib-Name: Pop_OS Security{newline}"));
    output
}

/// Produce a byte-preserving apt source rewrite plan. This function performs no filesystem IO.
/// `roots` is every host and path prefix this run can recognize as an Ubuntu archive mirror, from
/// any country, plus the canonical archive; `is_rewritable` decides membership and keeps refusing
/// the hosts that are never replaced, whatever a list names.
pub fn rewrite_source(
    text: &str,
    file: &Path,
    selected_uri: &str,
    roots: &[MirrorRoot],
    pop_os: bool,
) -> Result<RewritePlan, String> {
    crate::mirrors::validate_url(selected_uri)
        .map_err(|e| format!("selected mirror URI is invalid: {e}"))?;
    let is_deb822 = file.extension().and_then(|e| e.to_str()) == Some("sources");
    let is_pop_system = pop_os && file.file_name().is_some_and(|n| n == "system.sources");
    let mut replacements = Vec::new();
    let mut skipped = Vec::new();
    let mut created_security = String::new();

    if !is_deb822 {
        let entries = parse_legacy(text, file, roots, false);
        let lines = physical_lines(text);
        for entry in entries {
            let Some(uri) = entry.uris.first() else {
                if let Some(issue) = entry.issue {
                    skipped.push(format!("{}:{}: {issue}", file.display(), entry.line));
                }
                continue;
            };
            if !entry.rewritable.first().copied().unwrap_or(false) {
                skipped.push(format!(
                    "{}:{}: {}",
                    file.display(),
                    entry.line,
                    refusal_reason(uri)
                ));
                continue;
            }
            let Some(line) = lines.get(entry.line.saturating_sub(1)) else {
                continue;
            };
            let body = &text[line.start..line.body_end];
            if let Some(range) = locate_legacy_uri(body) {
                replacements.push((
                    line.start + range.start..line.start + range.end,
                    selected_uri.to_string(),
                ));
            }
        }
    } else {
        for stanza_range in stanza_ranges(text) {
            let stanza = &text[stanza_range.clone()];
            let entries = parse_deb822(stanza, file, roots, is_pop_system);
            if entries.len() != 1 {
                continue;
            }
            let entry = &entries[0];
            if entry.uris.is_empty() {
                skipped.push(format!(
                    "{}:{}: {}",
                    file.display(),
                    entry.line,
                    entry.issue.as_deref().unwrap_or("no URI field")
                ));
                continue;
            }
            let fields = field_spans(stanza);
            let uri_tokens: Vec<_> = fields
                .iter()
                .filter(|f| f.key == "uris")
                .flat_map(|f| f.values.iter())
                .collect();
            if uri_tokens.len() != entry.uris.len() {
                skipped.push(format!(
                    "{}:{}: malformed URIs field; left unchanged",
                    file.display(),
                    entry.line
                ));
                continue;
            }
            for (i, token) in uri_tokens.iter().enumerate() {
                if entry.rewritable.get(i).copied().unwrap_or(false) {
                    replacements.push((
                        stanza_range.start + token.range.start
                            ..stanza_range.start + token.range.end,
                        selected_uri.to_string(),
                    ));
                } else if let Some(uri) = entry.uris.get(i) {
                    skipped.push(format!(
                        "{}:{}: {}",
                        file.display(),
                        entry.line,
                        refusal_reason(uri)
                    ));
                }
            }
            let has_security = entry.suites.iter().any(|s| s.ends_with("-security"));
            let has_non_security = entry.suites.iter().any(|s| !s.ends_with("-security"));
            let pop_split = is_pop_system
                && entry.enabled
                && has_security
                && has_non_security
                && entry.uris.len() == entry.rewritable.len()
                && entry.rewritable.iter().all(|v| *v);
            if pop_split {
                let suite_tokens: Vec<_> = fields
                    .iter()
                    .filter(|f| f.key == "suites")
                    .flat_map(|f| f.values.iter())
                    .cloned()
                    .collect();
                if suite_tokens.len() != entry.suites.len() {
                    return Err(format!(
                        "{}:{}: cannot safely split Pop!_OS security suites",
                        file.display(),
                        entry.line
                    ));
                }
                let local_removals = security_suite_removals(stanza, &suite_tokens, &entry.suites);
                replacements.extend(local_removals.into_iter().map(|(range, value)| {
                    (
                        stanza_range.start + range.start..stanza_range.start + range.end,
                        value,
                    )
                }));
                if !created_security.is_empty() {
                    created_security.push_str(if stanza.contains("\r\n") {
                        "\r\n"
                    } else {
                        "\n"
                    });
                }
                created_security.push_str(&pop_security_content(
                    stanza,
                    &fields,
                    &entry.uris,
                    &entry
                        .suites
                        .iter()
                        .filter(|s| s.ends_with("-security"))
                        .cloned()
                        .collect::<Vec<_>>(),
                ));
            }
        }
    }

    let content = apply_replacements(text, replacements);
    let mut files = vec![PlannedFile {
        path: file.to_path_buf(),
        content,
        create_new: false,
    }];
    if !created_security.is_empty() {
        let security_path = file
            .parent()
            .unwrap_or_else(|| Path::new("."))
            .join("system-security.sources");
        files.push(PlannedFile {
            path: security_path,
            content: created_security,
            create_new: true,
        });
    }
    Ok(RewritePlan { files, skipped })
}

#[cfg(test)]
mod tests {
    use super::*;
    const HOSTS: &[MirrorRoot] = &[];

    /// Listed entries at `/` — the whole-host case — for the tests whose subject is not the path.
    fn listed(hosts: &[&str]) -> Vec<MirrorRoot> {
        hosts
            .iter()
            .map(|host| MirrorRoot {
                host: (*host).to_string(),
                path: "/".to_string(),
            })
            .collect()
    }

    /// Listed entries as `(host, path)` pairs, for the tests that turn on the path.
    fn roots(pairs: &[(&str, &str)]) -> Vec<MirrorRoot> {
        pairs
            .iter()
            .map(|(host, path)| MirrorRoot {
                host: (*host).to_string(),
                path: (*path).to_string(),
            })
            .collect()
    }

    #[test]
    fn rewrite_legacy_changes_only_eligible_uri_and_round_trips() {
        let input = "# Keep header\r\ndeb [arch=amd64 signed-by=/key] http://archive.ubuntu.com/ubuntu noble main # keep comment\r\ndeb-src https://security.ubuntu.com/ubuntu noble-security main\r\ndeb https://ppa.example/ubuntu noble main\r\n";
        let expected = "# Keep header\r\ndeb [arch=amd64 signed-by=/key] https://mirror.example/ubuntu/ noble main # keep comment\r\ndeb-src https://security.ubuntu.com/ubuntu noble-security main\r\ndeb https://ppa.example/ubuntu noble main\r\n";
        let path = Path::new("/etc/apt/sources.list");
        let hosts = listed(&["archive.ubuntu.com", "mirror.example"]);
        let result =
            rewrite_source(input, path, "https://mirror.example/ubuntu/", &hosts, false).unwrap();
        assert_eq!(result.files.len(), 1);
        assert_eq!(result.files[0].content, expected);
        assert!(!result.files[0].create_new);
        let round_trip = rewrite_source(
            expected,
            path,
            "http://archive.ubuntu.com/ubuntu",
            &hosts,
            false,
        )
        .unwrap();
        assert_eq!(round_trip.files[0].content, input);
    }

    /// A host another country's Launchpad entry names is eligible for replacement, so a mirror this
    /// tool wrote stays rewritable after a `--country` change. The exceptions still come first: a
    /// PPA, a third-party host, a `-security` stanza and a `mirror://` URI are reported and left
    /// byte for byte, and `security.ubuntu.com` is refused even while it is in the host set.
    #[test]
    fn rewrite_recognizes_a_foreign_mirror_host() {
        let input = "# keep\ndeb http://ubuntu.uib.no/archive/ noble main\ndeb https://ppa.example.net/ubuntu noble main\ndeb http://security.ubuntu.com/ubuntu noble-security main\ndeb mirror://mirror.example/ubuntu noble main\n";
        let expected = "# keep\ndeb https://mirror.example/ubuntu/ noble main\ndeb https://ppa.example.net/ubuntu noble main\ndeb http://security.ubuntu.com/ubuntu noble-security main\ndeb mirror://mirror.example/ubuntu noble main\n";
        // `security.ubuntu.com` is in the set on purpose: the exception is checked before the set,
        // so this test cannot pass because the host happened to be missing from it.
        let hosts = roots(&[
            ("archive.ubuntu.com", "/"),
            ("ubuntu.uib.no", "/archive/"),
            ("security.ubuntu.com", "/"),
        ]);
        let path = Path::new("/etc/apt/sources.list");
        let plan = rewrite_source(input, path, "https://mirror.example/ubuntu/", &hosts, false)
            .expect("the selected mirror is valid");
        assert_eq!(plan.files.len(), 1);
        assert_eq!(plan.files[0].content, expected);
        // Each refusal names the file, the line and the host, so the printed report is actionable
        // on its own and a `skip` can never be silent.
        assert_eq!(
            plan.skipped,
            [
                "/etc/apt/sources.list:3: https://ppa.example.net/ubuntu was not changed",
                "/etc/apt/sources.list:4: http://security.ubuntu.com/ubuntu was not changed",
                "/etc/apt/sources.list:5: mirror://mirror.example/ubuntu was not changed",
            ]
        );
        assert!(is_rewritable(
            "http://ubuntu.uib.no/archive/",
            &hosts,
            false
        ));
        assert!(!is_rewritable(
            "http://security.ubuntu.com/ubuntu",
            &hosts,
            false
        ));
        assert!(!is_rewritable(
            "http://ports.ubuntu.com/ubuntu-ports",
            &roots(&[("archive.ubuntu.com", "/"), ("ports.ubuntu.com", "/")]),
            false
        ));
        assert!(!is_rewritable(
            "https://ppa.example.net/ubuntu",
            &hosts,
            false
        ));
    }

    /// Eligibility is the listed host **and** the listed path, compared by whole segments: a mirror
    /// host carries several trees and only the Ubuntu tree may be replaced, so the SPEC's Debian line
    /// is left alone even though its host is a listed Ubuntu mirror (SPEC: Apply).
    #[test]
    fn rewrite_requires_the_listed_path() {
        let path = Path::new("/etc/apt/sources.list");
        let hosts = roots(&[
            ("ftp.uni-stuttgart.de", "/pub/linux/debian/ubuntu/"),
            ("archive.ubuntu.com", "/"),
        ]);
        let input = "deb http://ftp.uni-stuttgart.de/debian trixie main\ndeb http://archive.ubuntu.com/ubuntu noble main\n";
        let expected = "deb http://ftp.uni-stuttgart.de/debian trixie main\ndeb https://mirror.example/ubuntu/ noble main\n";
        let plan = rewrite_source(input, path, "https://mirror.example/ubuntu/", &hosts, false)
            .expect("the selected mirror is valid");
        assert_eq!(
            plan.files[0].content, expected,
            "the Debian line is untouched"
        );
        assert_eq!(
            plan.skipped,
            ["/etc/apt/sources.list:1: http://ftp.uni-stuttgart.de/debian was not changed"],
            "a skipped entry names the file, the line and the host"
        );
        // The same host inside the listed tree is eligible, with the listed trailing slash or
        // below it.
        for admitted in [
            "http://ftp.uni-stuttgart.de/pub/linux/debian/ubuntu/",
            "http://ftp.uni-stuttgart.de/pub/linux/debian/ubuntu/dists/noble/InRelease",
        ] {
            assert!(is_rewritable(admitted, &hosts, false), "{admitted}");
        }
        // The slash-less spelling of that same tree is that same tree: a list spells `.../ubuntu/`
        // and a run writes `.../ubuntu`, so the two spellings cannot disagree.
        assert!(is_rewritable(
            "http://ftp.uni-stuttgart.de/pub/linux/debian/ubuntu",
            &hosts,
            false
        ));

        // A listed entry without a trailing slash admits its own tree by whole segments only.
        let ubuntu = roots(&[("mirror.example", "/ubuntu")]);
        for admitted in [
            "http://mirror.example/ubuntu",
            "http://mirror.example/ubuntu/",
            "http://mirror.example/ubuntu/dists/noble/InRelease",
        ] {
            assert!(is_rewritable(admitted, &ubuntu, false), "{admitted}");
        }
        assert!(!is_rewritable(
            "http://mirror.example/ubuntufoo",
            &ubuntu,
            false
        ));
        // The port is not part of the comparison.
        assert!(is_rewritable(
            "http://mirror.example:8080/ubuntu/",
            &ubuntu,
            false
        ));

        // A listed `/` names no narrower tree, so every path on that host is admitted: the
        // whole-host case the SPEC states rather than hides.
        assert!(is_rewritable(
            "http://archive.ubuntu.com/anything/at/all",
            &hosts,
            false
        ));
    }

    /// A dot segment names a different tree than the path it is written inside: `..` walks out of the
    /// listed `/ubuntu`, so `http://host/ubuntu/../debian` still starts with the listed segment while the
    /// tree it really names is `/debian`, and rewriting it would substitute a tree the list never named.
    /// Every spelling is refused instead of decoded or resolved, because resolving is the step that has to
    /// agree with an intermediary, and refusing needs no agreement (SPEC: Apply).
    #[test]
    fn a_dot_segment_is_never_eligible_in_any_spelling() {
        let ubuntu = roots(&[("mirror.example", "/ubuntu")]);
        for uri in [
            "http://mirror.example/ubuntu/../debian",
            "http://mirror.example/ubuntu/../debian/dists/trixie/InRelease",
            "http://mirror.example/ubuntu/%2e%2e/debian",
            "http://mirror.example/ubuntu/%2E%2E/debian",
            "http://mirror.example/ubuntu/.%2e/debian",
            "http://mirror.example/ubuntu/%2e./debian",
            "http://mirror.example/../ubuntu",
            "http://mirror.example/ubuntu/./",
        ] {
            assert!(!is_rewritable(uri, &ubuntu, false), "{uri} must be refused");
        }
        // A listed `/` and the canonical archive's any-path rule admit every path, so only the dot-segment
        // refusal can keep `..` out there; the traversal would otherwise reach a tree no list named.
        for uri in [
            "http://archive.ubuntu.com/ubuntu/../debian",
            "http://de.archive.ubuntu.com/ubuntu/%2e%2e/debian",
        ] {
            assert!(!is_rewritable(uri, HOSTS, false), "{uri} must be refused");
        }
        // The rule bans segments, not dots: a dot inside a name is a name.
        for admitted in [
            "http://mirror.example/ubuntu/v1.2/",
            "http://mirror.example/ubuntu/v1.2/dists/noble/InRelease",
            "http://mirror.example/ubuntu/..foo",
            "http://mirror.example/ubuntu/v1.2..3/",
        ] {
            assert!(is_rewritable(admitted, &ubuntu, false), "{admitted}");
        }
    }

    /// The skipped token says which segment it found, and the line keeps its bytes, so a refusal is never
    /// silent and never half-applied (SPEC: Apply).
    #[test]
    fn a_dot_segment_is_reported_and_the_line_is_left_alone() {
        let hosts = roots(&[("mirror.example", "/ubuntu")]);
        let input = "deb http://mirror.example/ubuntu/../debian trixie main\ndeb http://mirror.example/ubuntu/%2e%2e/debian trixie main\ndeb http://mirror.example/ubuntu/v1.2/ noble main\n";
        let plan = rewrite_source(
            input,
            Path::new("/etc/apt/sources.list"),
            "https://mirror.example/ubuntu/",
            &hosts,
            false,
        )
        .expect("the selected mirror is valid");
        assert_eq!(
            plan.skipped,
            [
                "/etc/apt/sources.list:1: URI 'http://mirror.example/ubuntu/../debian' carries a '..' path segment, so it is left unchanged",
                "/etc/apt/sources.list:2: URI 'http://mirror.example/ubuntu/%2e%2e/debian' carries a '%2e%2e' path segment, so it is left unchanged",
            ]
        );
        let rewritten = &plan.files[0].content;
        assert!(
            rewritten.contains("deb http://mirror.example/ubuntu/../debian trixie main"),
            "the refused line keeps its bytes: {rewritten}"
        );
        assert!(
            rewritten.contains("deb https://mirror.example/ubuntu/ noble main"),
            "its harmless sibling is still rewritten: {rewritten}"
        );

        // The same refusal reaches the deb822 path and the inventory report, not only the rewrite plan.
        let stanza = "Types: deb\nURIs: http://mirror.example/ubuntu/../debian\nSuites: trixie\nComponents: main\n";
        let parsed = parse_deb822(
            stanza,
            Path::new("/etc/apt/sources.list.d/x.sources"),
            &hosts,
            false,
        );
        assert_eq!(
            parsed[0].issue.as_deref(),
            Some(
                "URI 'http://mirror.example/ubuntu/../debian' carries a '..' path segment, so it is left unchanged"
            )
        );
        assert!(parsed[0].rewritable.iter().all(|v| !*v));
    }

    /// A list entry whose path carries a dot segment names a tree this cannot read without resolving it,
    /// and resolving is how a listed `/ubuntu` would end up admitting `/debian`. It is left out of the
    /// set instead, so the run reports it and nothing is admitted through it.
    #[test]
    fn a_listed_entry_with_a_dot_segment_never_enters_the_set() {
        for url in [
            "http://mirror.example/ubuntu/../debian",
            "http://mirror.example/ubuntu/%2e%2e/debian",
            "http://mirror.example/ubuntu/%2E%2E/debian",
            "http://mirror.example/ubuntu/.%2e/debian",
            "http://mirror.example/ubuntu/%2e./debian",
            "http://mirror.example/../ubuntu",
        ] {
            assert_eq!(mirror_root(url), None, "{url} must not enter the set");
        }
        // A dot inside a name is still an ordinary entry.
        assert_eq!(
            mirror_root("http://mirror.example/ubuntu/v1.2/"),
            Some(MirrorRoot {
                host: "mirror.example".to_string(),
                path: "/ubuntu/v1.2/".to_string(),
            })
        );
    }

    /// A trailing slash names the same tree on both sides, and a repeated slash is another spelling
    /// of the same path. The list spells `.../ubuntu/` while a run writes `.../ubuntu`, so a rule that
    /// read the two as different trees refused every mirror this tool had already written.
    #[test]
    fn a_trailing_slash_never_changes_what_a_path_admits() {
        for (root, path) in [
            ("/ubuntu", "/ubuntu/"),
            ("/ubuntu/", "/ubuntu"),
            ("/ubuntu", "/ubuntu"),
            ("/ubuntu", "/ubuntu/dists/x"),
            ("/ubuntu/", "/ubuntu/dists/x"),
            ("//ubuntu//", "/ubuntu"),
            ("/ubuntu-mirror/archive/", "/ubuntu-mirror/archive"),
            ("/", "/anything/at/all"),
            ("/", "/"),
            ("/", ""),
        ] {
            assert!(path_admits(root, path), "{root} must admit {path}");
        }
        for (root, path) in [
            ("/ubuntu", "/ubuntufoo"),
            ("/ubuntu/", "/ubuntufoo"),
            ("/ubuntu", "/ubuntu-x"),
            ("/ubuntu/x", "/ubuntu"),
            ("/ubuntu", "/"),
        ] {
            assert!(!path_admits(root, path), "{root} must not admit {path}");
        }
    }

    /// The pair the live runs actually produce: the Launchpad page lists a mirror at `.../ubuntu/`
    /// and `probe::probe` writes the selected URI without that slash. Every entry of the real page
    /// that is eligible as listed must stay eligible as written, and one level below it, or a second
    /// `--country` run cannot recognize the mirror the first one wrote (W23, W24).
    #[test]
    fn a_launchpad_entry_admits_the_uri_a_run_writes() {
        let html = include_str!("../tests/fixtures/launchpad-full-82-countries.html");
        let mirrors = crate::mirrors::parse_launchpad(html).expect("the page fixture parses");
        let roots: Vec<MirrorRoot> = mirrors
            .iter()
            .filter(|m| crate::mirrors::validate_url(&m.url).is_ok())
            .filter_map(|m| mirror_root(&m.url))
            .collect();
        assert!(
            roots.len() > 500,
            "the page fixture must yield a broad set, not {}: a handful of entries cannot tell the \
             page's spellings apart",
            roots.len()
        );
        let mut checked = 0;
        for mirror in &mirrors {
            if crate::mirrors::validate_url(&mirror.url).is_err()
                || !is_rewritable(&mirror.url, &roots, false)
            {
                continue;
            }
            checked += 1;
            let written = mirror.url.trim_end_matches('/');
            assert!(
                is_rewritable(written, &roots, false),
                "{} is listed as eligible, so the URI a run writes for it must be too",
                mirror.url
            );
            assert!(
                is_rewritable(&format!("{written}/dists/noble/InRelease"), &roots, false),
                "{} names a tree, so everything below it is eligible too",
                mirror.url
            );
        }
        assert!(
            checked > 300,
            "only {checked} entries were eligible: the page fixture went stale"
        );
    }

    /// The same normalisation on the two hosts that carry a rule of their own: `apt.pop-os.org` still
    /// needs the explicit policy host and its own `/ubuntu` tree, and the canonical archive is
    /// recognized at any path, so a trailing slash changes neither answer.
    #[test]
    fn the_special_hosts_ignore_a_trailing_slash_too() {
        let pop = roots(&[("apt.pop-os.org", "/ubuntu/")]);
        for uri in [
            "http://apt.pop-os.org/ubuntu",
            "http://apt.pop-os.org/ubuntu/",
        ] {
            assert!(is_rewritable(uri, &pop, true), "{uri}");
            assert!(
                !is_rewritable(uri, &pop, false),
                "{uri} still needs the Pop!_OS policy host"
            );
        }
        assert!(!is_rewritable("http://apt.pop-os.org/ubuntu-x", &pop, true));
        assert!(!is_rewritable("http://apt.pop-os.org/", &pop, true));
        // The canonical archive is recognized at any path with no list entry naming it at all; a
        // host that merely resembles it is not.
        for uri in [
            "http://archive.ubuntu.com/ubuntu",
            "http://archive.ubuntu.com/ubuntu/",
            "http://de.archive.ubuntu.com/ubuntu/",
        ] {
            assert!(is_rewritable(uri, HOSTS, false), "{uri}");
        }
        assert!(!is_rewritable(
            "http://notarchive.ubuntu.com/ubuntu",
            HOSTS,
            false
        ));
    }

    /// The refusal list is explicit and is checked before any membership test: every host below is in
    /// the set at `/`, so only the refusal can skip it (SPEC: Apply).
    #[test]
    fn ppa_and_security_hosts_are_refused_before_membership() {
        let hosts = roots(&[
            ("security.ubuntu.com", "/"),
            ("ports.ubuntu.com", "/"),
            ("ppa.launchpad.net", "/"),
            ("ppa.launchpadcontent.net", "/"),
            ("launchpad.net", "/"),
            ("sub.launchpad.net", "/"),
            ("ubuntu.uib.no", "/"),
        ]);
        let refused = [
            "http://security.ubuntu.com/ubuntu",
            "http://ports.ubuntu.com/ubuntu-ports",
            "https://ppa.launchpad.net/x/y/ubuntu",
            "https://ppa.launchpadcontent.net/x/y/ubuntu",
            "https://launchpad.net/whatever",
            "https://sub.launchpad.net/whatever",
        ];
        for uri in refused {
            assert!(
                !is_rewritable(uri, &hosts, false),
                "{uri} is listed at '/' in this set, so only the refusal list can skip it"
            );
        }
        // The positive control: another host in the same set, at the same `/`, is eligible.
        assert!(is_rewritable(
            "https://ubuntu.uib.no/any/path",
            &hosts,
            false
        ));
        // A host no list names is skipped for the membership reason, not for a refusal.
        assert!(!is_rewritable("https://example.net/ubuntu", &hosts, false));

        // The same lines, through the rewrite plan: each refusal is reported, never silent.
        let input = "deb http://security.ubuntu.com/ubuntu noble main\ndeb http://ports.ubuntu.com/ubuntu-ports noble main\ndeb https://ppa.launchpad.net/x/y/ubuntu noble main\ndeb https://ppa.launchpadcontent.net/x/y/ubuntu noble main\ndeb https://launchpad.net/whatever noble main\ndeb https://sub.launchpad.net/whatever noble main\ndeb https://ubuntu.uib.no/any/path noble main\n";
        let plan = rewrite_source(
            input,
            Path::new("/etc/apt/sources.list"),
            "https://mirror.example/ubuntu/",
            &hosts,
            false,
        )
        .expect("the selected mirror is valid");
        assert_eq!(plan.skipped.len(), refused.len(), "{:?}", plan.skipped);
        let rewritten = &plan.files[0].content;
        assert!(
            rewritten.contains("deb https://mirror.example/ubuntu/ noble main"),
            "{rewritten}"
        );
        for kept in refused {
            assert!(
                rewritten.contains(kept),
                "{kept} must stay byte for byte: {rewritten}"
            );
        }
    }

    /// `mirror_root` builds the set and `is_rewritable` reads an on-disk URI, so the two must agree on
    /// what a host and a path are. A port, a bracketed address and a trailing dot are the shapes where
    /// a second reading of an authority drifts.
    #[test]
    fn mirror_root_agrees_with_is_rewritable_on_the_authority() {
        assert_eq!(
            mirror_root("https://Mirror.Example:8443/ubuntu/?release=noble"),
            Some(MirrorRoot {
                host: "mirror.example".to_string(),
                path: "/ubuntu/".to_string(),
            })
        );
        let root = mirror_root("https://mirror.example:8443/ubuntu/").expect("a host and a path");
        assert!(is_rewritable(
            "http://mirror.example:8080/ubuntu/dists/noble/InRelease",
            std::slice::from_ref(&root),
            false
        ));
        // A URL with no path names the whole host, on both sides.
        assert_eq!(
            mirror_root("http://mirror.example").map(|r| r.path),
            Some("/".to_string())
        );
        // A trailing dot is a spelling of the same host on both sides.
        assert_eq!(
            mirror_root("http://mirror.example./ubuntu/").map(|r| r.host),
            Some("mirror.example".to_string())
        );
        assert!(is_rewritable(
            "http://MIRROR.example./ubuntu/",
            &roots(&[("mirror.example", "/ubuntu/")]),
            false
        ));
        // Bracketed IPv6: neither side treats an address with ':' in it as a mirror host, so a listed
        // address can never make a URI eligible by accident.
        assert_eq!(mirror_root("http://[2001:db8::1]/ubuntu/"), None);
        assert!(!is_rewritable(
            "http://[2001:db8::1]/ubuntu/",
            &roots(&[("2001:db8::1", "/")]),
            false
        ));
        // An authority or a port neither side can read contributes nothing rather than being guessed.
        assert_eq!(mirror_root("http://mirror.example:0/ubuntu/"), None);
        assert_eq!(mirror_root("ftp://mirror.example/ubuntu/"), None);
        assert_eq!(mirror_root("http://user:pw@mirror.example/ubuntu/"), None);
    }

    #[test]
    fn rewrite_deb822_changes_only_uri_tokens_and_preserves_bytes() {
        let input = "# top\r\nTypes: deb\r\nURIs: http://archive.ubuntu.com/ubuntu\r\n https://ppa.example/ubuntu\r\nSuites: noble noble-updates\r\nComponents: main\r\nSigned-By: /usr/share/keyrings/ubuntu.gpg\r\n\r\nTypes: deb\r\nURIs: http://archive.ubuntu.com/ubuntu\r\nSuites: noble\r\nEnabled: no\r\n\r\nTypes: deb\r\nURIs: http://archive.ubuntu.com/ubuntu\r\nSuites: noble-security\r\n";
        let expected = "# top\r\nTypes: deb\r\nURIs: https://mirror.example/ubuntu/\r\n https://ppa.example/ubuntu\r\nSuites: noble noble-updates\r\nComponents: main\r\nSigned-By: /usr/share/keyrings/ubuntu.gpg\r\n\r\nTypes: deb\r\nURIs: http://archive.ubuntu.com/ubuntu\r\nSuites: noble\r\nEnabled: no\r\n\r\nTypes: deb\r\nURIs: http://archive.ubuntu.com/ubuntu\r\nSuites: noble-security\r\n";
        let hosts = listed(&["archive.ubuntu.com", "mirror.example"]);
        let path = Path::new("ubuntu.sources");
        let result =
            rewrite_source(input, path, "https://mirror.example/ubuntu/", &hosts, false).unwrap();
        assert_eq!(result.files[0].content, expected);
        let round_trip = rewrite_source(
            expected,
            path,
            "http://archive.ubuntu.com/ubuntu",
            &hosts,
            false,
        )
        .unwrap();
        assert_eq!(round_trip.files[0].content, input);
    }

    #[test]
    fn rewrite_pop_os_purely_splits_security_file_and_preserves_default_mirror() {
        let input = "# Pop source\r\nTypes: deb\r\nURIs: http://apt.pop-os.org/ubuntu\r\nSuites: jammy jammy-security jammy-updates jammy-backports\r\nComponents: main\r\nX-Repolib-Default-Mirror: http://apt.pop-os.org/ubuntu\r\nEnabled: yes\r\nSigned-By: /usr/share/keyrings/pop-archive-keyring.gpg\r\nX-Repolib-Name: Pop_OS\r\n";
        let expected_main = "# Pop source\r\nTypes: deb\r\nURIs: https://mirror.example/ubuntu/\r\nSuites: jammy jammy-updates jammy-backports\r\nComponents: main\r\nX-Repolib-Default-Mirror: http://apt.pop-os.org/ubuntu\r\nEnabled: yes\r\nSigned-By: /usr/share/keyrings/pop-archive-keyring.gpg\r\nX-Repolib-Name: Pop_OS\r\n";
        let expected_security = "Types: deb\r\nURIs: http://apt.pop-os.org/ubuntu\r\nSuites: jammy-security\r\nComponents: main\r\nEnabled: yes\r\nSigned-By: /usr/share/keyrings/pop-archive-keyring.gpg\r\nX-Repolib-Name: Pop_OS Security\r\n";
        let path = Path::new("/etc/apt/sources.list.d/system.sources");
        let hosts = listed(&["apt.pop-os.org", "mirror.example"]);
        let result =
            rewrite_source(input, path, "https://mirror.example/ubuntu/", &hosts, true).unwrap();
        assert_eq!(result.files.len(), 2);
        assert_eq!(result.files[0].path, path);
        assert_eq!(result.files[0].content, expected_main);
        assert!(!result.files[0].create_new);
        assert_eq!(
            result.files[1].path,
            Path::new("/etc/apt/sources.list.d/system-security.sources")
        );
        assert_eq!(result.files[1].content, expected_security);
        assert!(result.files[1].create_new);

        let security_only = "Types: deb\nURIs: http://apt.pop-os.org/ubuntu\nSuites: jammy-security\nComponents: main\n";
        let unchanged = rewrite_source(
            security_only,
            path,
            "https://mirror.example/ubuntu/",
            &hosts,
            true,
        )
        .unwrap();
        assert_eq!(unchanged.files.len(), 1);
        assert_eq!(unchanged.files[0].content, security_only);
    }

    #[test]
    fn rewrite_plan_does_not_create_source_files() {
        let root =
            std::env::temp_dir().join(format!("apt-scout-rewrite-plan-{}", std::process::id()));
        let path = root.join("sources.list");
        let input = "deb http://archive.ubuntu.com/ubuntu noble main\n";
        let result =
            rewrite_source(input, &path, "https://mirror.example/ubuntu/", &[], false).unwrap();
        assert_eq!(
            result.files[0].content,
            "deb https://mirror.example/ubuntu/ noble main\n"
        );
        assert!(!path.exists());
    }

    #[test]
    fn rewrite_rejects_an_invalid_selected_uri_without_modifying_input() {
        let input = "deb http://archive.ubuntu.com/ubuntu noble main\n";
        assert!(
            rewrite_source(
                input,
                Path::new("sources.list"),
                "http://user@evil.example/",
                HOSTS,
                false
            )
            .is_err()
        );
    }

    #[test]
    fn inventory_report_lists_files_and_rewrite_decisions() {
        let e = parse_legacy(
            "# comment\ndeb http://archive.ubuntu.com/ubuntu noble main\ndeb http://security.ubuntu.com/ubuntu noble-security main\n",
            Path::new("sources.list"),
            HOSTS,
            false,
        );
        let inv = Inventory {
            files: vec![FileReport {
                path: PathBuf::from("/etc/apt/sources.list"),
                entries: e,
                issue: None,
            }],
        };
        let report = inv.report();
        assert!(report.contains("source file: /etc/apt/sources.list"));
        assert!(report.contains("\n  line 2:"));
        assert!(report.contains("candidate for rewrite"));
        assert!(report.contains("security suite is never rewritten"));
        assert!(inv.has_rewritable_uri());
        let empty = Inventory { files: vec![] };
        assert!(!empty.has_rewritable_uri());
        assert_eq!(empty.report(), "no apt source files found");

        let third_party = Inventory {
            files: vec![FileReport {
                path: PathBuf::from("/etc/apt/sources.list.d/ppa.list"),
                entries: parse_legacy(
                    "deb https://packages.example.org/ubuntu noble main\n",
                    Path::new("ppa.list"),
                    HOSTS,
                    false,
                ),
                issue: None,
            }],
        };
        assert!(!third_party.has_rewritable_uri());
        assert!(
            third_party
                .report()
                .contains("not an official Ubuntu archive")
        );
    }

    #[test]
    fn legacy_ignores_comments_parses_options_and_security() {
        let p = Path::new("sources.list");
        let e = parse_legacy(
            "# comment\n\ndeb [arch=amd64 signed-by=/key] http://archive.ubuntu.com/ubuntu noble main\ndeb-src https://us.archive.ubuntu.com/ubuntu noble-updates main\ndeb http://security.ubuntu.com/ubuntu noble-security main\n",
            p,
            HOSTS,
            false,
        );
        assert_eq!(e.len(), 3);
        assert_eq!(e[0].uris, ["http://archive.ubuntu.com/ubuntu"]);
        assert!(e[0].rewritable[0]);
        assert_eq!(e[1].format, Format::Legacy);
        assert!(e[1].rewritable[0]);
        assert!(!e[2].rewritable[0]);
        assert!(e[2].issue.as_deref().unwrap().contains("security"));
    }

    #[test]
    fn legacy_reports_malformed_and_third_party_sources() {
        let e = parse_legacy(
            "deb\ndeb http://archive.ubuntu.com/ubuntu # missing suite\ndeb http://example.org/ubuntu noble main\ndeb http://archive.ubuntu.com:bad/ubuntu noble main\ndeb https://archive.ubuntu.com:99999/ubuntu noble main\ndeb https://user@archive.ubuntu.com/ubuntu noble main\n",
            Path::new("x.list"),
            HOSTS,
            false,
        );
        assert!(e[0].issue.as_deref().unwrap().contains("missing URI"));
        assert!(e[1].issue.as_deref().unwrap().contains("missing suite"));
        assert!(e[1].uris.is_empty());
        assert!(!e[2].rewritable[0]);
        assert!(e[2].issue.as_deref().unwrap().contains("not an official"));
        assert!(!e[3].rewritable[0]);
        assert!(!e[4].rewritable[0]);
        assert!(!e[5].rewritable[0]);
        let old = parse_legacy(
            "deb mirror://mirrors.ubuntu.com/mirrors.txt noble main\n",
            Path::new("old.list"),
            HOSTS,
            false,
        );
        assert!(
            old[0]
                .issue
                .as_deref()
                .unwrap()
                .contains("mirror URI scheme")
        );
    }

    #[test]
    fn deb822_continuations_and_security_are_parsed() {
        let e = parse_deb822(
            "# comment\nTypes: deb\nURIs: http://archive.ubuntu.com/ubuntu\n https://mirror.example/ubuntu\nSuites: noble noble-updates\nComponents: main\n\nTypes: deb\nURIs: http://archive.ubuntu.com/ubuntu\nSuites: noble-security\n",
            Path::new("x.sources"),
            HOSTS,
            false,
        );
        assert_eq!(e.len(), 2);
        assert_eq!(e[0].uris.len(), 2);
        assert!(e[0].rewritable[0]);
        assert!(!e[0].rewritable[1]);
        assert_eq!(e[1].rewritable, [false]);
        assert!(e[1].issue.as_deref().unwrap().contains("security"));
    }

    #[test]
    fn deb822_duplicate_types_is_reported_and_never_rewritable() {
        let e = parse_deb822(
            "Types: deb\nTypes: deb-src\nURIs: http://archive.ubuntu.com/ubuntu\nSuites: noble\n",
            Path::new("duplicate.sources"),
            HOSTS,
            false,
        );
        assert!(
            e[0].issue
                .as_deref()
                .unwrap()
                .contains("duplicate singleton field 'Types'")
        );
        assert_eq!(e[0].rewritable, [false]);
    }

    #[test]
    fn deb822_disabled_duplicate_and_missing_fields_fail_closed() {
        let e = parse_deb822(
            "Types: deb\nURIs: http://archive.ubuntu.com/ubuntu\nSuites: noble\nEnabled: no\n\nTypes: deb\nURIs: http://archive.ubuntu.com/ubuntu\nURIs: https://archive.ubuntu.com/ubuntu\nSuites: noble\n\nTypes: deb\nSuites: noble\n",
            Path::new("x.sources"),
            HOSTS,
            false,
        );
        assert!(!e[0].rewritable[0]);
        assert!(e[0].issue.as_deref().unwrap().contains("disabled"));
        assert!(e[1].issue.as_deref().unwrap().contains("duplicate"));
        assert!(e[2].issue.as_deref().unwrap().contains("missing"));
    }

    #[test]
    fn inventory_refuses_symlinked_directory_and_reports_non_utf8() {
        use std::os::unix::fs::symlink;
        let root =
            std::env::temp_dir().join(format!("apt-scout-sources-root-{}", std::process::id()));
        let target = root.with_extension("target");
        let _ = std::fs::remove_dir_all(&root);
        let _ = std::fs::remove_dir_all(&target);
        std::fs::create_dir_all(&root).unwrap();
        std::fs::create_dir_all(&target).unwrap();
        std::fs::write(target.join("bad.sources"), [0xff, 0xfe]).unwrap();
        symlink(&target, root.join("sources.list.d")).unwrap();
        let result = inventory(&root, HOSTS, false);
        assert!(result.files.iter().any(|f| {
            f.path.ends_with("sources.list.d")
                && f.issue
                    .as_deref()
                    .unwrap()
                    .contains("symlinks are not followed")
        }));
        assert!(!result.files.iter().any(|f| f.path.ends_with("bad.sources")));
        std::fs::remove_file(root.join("sources.list.d")).unwrap();
        std::fs::remove_dir_all(root).unwrap();
        std::fs::remove_dir_all(target).unwrap();
    }

    #[test]
    fn inventory_refuses_symlinked_root() {
        use std::os::unix::fs::symlink;
        let base =
            std::env::temp_dir().join(format!("apt-scout-sources-link-{}", std::process::id()));
        let target = base.with_extension("target");
        let _ = std::fs::remove_file(&base);
        let _ = std::fs::remove_dir_all(&target);
        std::fs::create_dir_all(&target).unwrap();
        symlink(&target, &base).unwrap();
        let result = inventory(&base, HOSTS, false);
        assert_eq!(result.files.len(), 1);
        assert!(
            result.files[0]
                .issue
                .as_deref()
                .unwrap()
                .contains("symlinks are not followed")
        );
        std::fs::remove_file(base).unwrap();
        std::fs::remove_dir_all(target).unwrap();
    }

    #[test]
    fn inventory_reports_symlinks_and_scans_only_regular_source_files() {
        use std::os::unix::fs::symlink;
        let root = std::env::temp_dir().join(format!("apt-scout-sources-{}", std::process::id()));
        let dir = root.join("sources.list.d");
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            root.join("sources.list"),
            "deb http://archive.ubuntu.com/ubuntu noble main\\n",
        )
        .unwrap();
        std::fs::write(
            dir.join("ok.list"),
            "deb http://archive.ubuntu.com/ubuntu noble main\\n",
        )
        .unwrap();
        std::fs::write(dir.join("ignore.conf"), "not an apt source\\n").unwrap();
        std::fs::write(dir.join("binary.sources"), [0xff, 0xfe]).unwrap();
        symlink(dir.join("ok.list"), dir.join("linked.list")).unwrap();
        let result = inventory(&root, HOSTS, false);
        assert_eq!(
            result
                .files
                .iter()
                .filter(|f| f.path.ends_with("sources.list"))
                .count(),
            1
        );
        assert_eq!(
            result
                .files
                .iter()
                .filter(|f| f.path.ends_with("ok.list"))
                .count(),
            1
        );
        let ignored = result
            .files
            .iter()
            .find(|f| f.path.ends_with("ignore.conf"))
            .unwrap();
        assert!(
            ignored
                .issue
                .as_deref()
                .unwrap()
                .contains("unsupported source filename")
        );
        let binary = result
            .files
            .iter()
            .find(|f| f.path.ends_with("binary.sources"))
            .unwrap();
        assert!(binary.issue.as_deref().unwrap().contains("not valid UTF-8"));
        let link = result
            .files
            .iter()
            .find(|f| f.path.ends_with("linked.list"))
            .unwrap();
        assert!(
            link.issue
                .as_deref()
                .unwrap()
                .contains("symlinks are not followed")
        );
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn pop_os_official_ubuntu_uri_requires_explicit_policy_host() {
        let hosts = listed(&["apt.pop-os.org"]);
        let security_only = parse_deb822(
            "Types: deb\nURIs: http://apt.pop-os.org/ubuntu\nSuites: jammy-security\nComponents: main\n",
            Path::new("system.sources"),
            &hosts,
            true,
        );
        assert!(!security_only[0].rewritable[0]);
        let non_pop_mixed = parse_deb822(
            "Types: deb\nURIs: http://apt.pop-os.org/ubuntu\nSuites: jammy jammy-security\nComponents: main\n",
            Path::new("system.sources"),
            &hosts,
            false,
        );
        assert!(!non_pop_mixed[0].rewritable[0]);
        let e = parse_deb822(
            "Types: deb\nURIs: http://apt.pop-os.org/ubuntu\nSuites: jammy jammy-updates\nComponents: main\n",
            Path::new("system.sources"),
            &hosts,
            true,
        );
        assert!(e[0].rewritable[0]);
        let mixed = parse_deb822(
            "Types: deb\nURIs: http://apt.pop-os.org/ubuntu\nSuites: jammy jammy-updates jammy-security\nComponents: main\n",
            Path::new("system.sources"),
            &hosts,
            true,
        );
        assert!(mixed[0].rewritable[0]);
        assert!(
            mixed[0]
                .issue
                .as_deref()
                .unwrap()
                .contains("security suite remains on the original mirror")
        );
        assert!(
            Inventory {
                files: vec![FileReport {
                    path: PathBuf::from("system.sources"),
                    entries: mixed.clone(),
                    issue: None
                }]
            }
            .report()
            .contains("security suite remains on the original mirror")
        );
        let vendor = parse_deb822(
            "Types: deb\nURIs: http://apt.pop-os.org/release\nSuites: jammy\nComponents: main\n",
            Path::new("other.sources"),
            &hosts,
            true,
        );
        assert!(!vendor[0].rewritable[0]);
        let non_pop = parse_deb822(
            "Types: deb\nURIs: http://apt.pop-os.org/ubuntu\nSuites: jammy\nComponents: main\n",
            Path::new("system.sources"),
            &hosts,
            false,
        );
        assert!(!non_pop[0].rewritable[0]);

        let root =
            std::env::temp_dir().join(format!("apt-scout-pop-policy-{}", std::process::id()));
        let dir = root.join("sources.list.d");
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&dir).unwrap();
        let content =
            "Types: deb\nURIs: http://apt.pop-os.org/ubuntu\nSuites: jammy\nComponents: main\n";
        std::fs::write(dir.join("system.sources"), content).unwrap();
        std::fs::write(dir.join("vendor.sources"), content).unwrap();
        let inventory = inventory(&root, &hosts, true);
        assert!(
            inventory
                .files
                .iter()
                .find(|f| f.path.ends_with("system.sources"))
                .unwrap()
                .entries[0]
                .rewritable[0]
        );
        assert!(
            !inventory
                .files
                .iter()
                .find(|f| f.path.ends_with("vendor.sources"))
                .unwrap()
                .entries[0]
                .rewritable[0]
        );
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn security_and_ports_hosts_are_never_rewritable_even_if_listed() {
        let hosts = listed(&["security.ubuntu.com", "ports.ubuntu.com"]);
        let entries = parse_legacy(
            "deb http://security.ubuntu.com/ubuntu noble main\ndeb http://ports.ubuntu.com/ubuntu noble main\n",
            Path::new("x.list"),
            &hosts,
            false,
        );
        assert!(entries.iter().all(|e| !e.rewritable[0]));
    }

    #[test]
    fn current_mirror_host_is_recognized_but_host_matching_is_exact() {
        let hosts = listed(&["mirror.example"]);
        assert!(
            parse_legacy(
                "deb https://mirror.example/ubuntu noble main\n",
                Path::new("x"),
                &hosts,
                false,
            )[0]
            .rewritable[0]
        );
        assert!(
            !parse_legacy(
                "deb https://notmirror.example/ubuntu noble main\n",
                Path::new("x"),
                &hosts,
                false,
            )[0]
            .rewritable[0]
        );
    }
}
