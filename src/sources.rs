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
/// becomes one entry. `archive_hosts` includes the canonical archive and currently known mirrors.
pub fn parse_legacy(
    text: &str,
    file: &Path,
    archive_hosts: &[String],
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
        let rewritable = is_rewritable(uri, archive_hosts, pop_os_system_sources)
            && !suite.ends_with("-security");
        let issue = if suite.ends_with("-security") {
            Some("security suite is never rewritten".into())
        } else if uri.starts_with("mirror://") || uri.starts_with("mirror+file:") {
            Some(format!(
                "mirror URI scheme in '{uri}' is unsupported and will be left unchanged"
            ))
        } else if !is_rewritable(uri, archive_hosts, pop_os_system_sources) {
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
    archive_hosts: &[String],
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
                    && is_rewritable(uri, archive_hosts, pop_os_system_sources)
            })
            .collect::<Vec<_>>();
        if issue.is_none() && !enabled {
            *issue = Some("stanza disabled (Enabled: no)".into());
        }
        if issue.is_none()
            && uris
                .iter()
                .any(|uri| !is_rewritable(uri, archive_hosts, pop_os_system_sources))
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

fn is_rewritable(uri: &str, archive_hosts: &[String], pop_os_system_sources: bool) -> bool {
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
    if matches!(host.as_str(), "security.ubuntu.com" | "ports.ubuntu.com") {
        return false;
    }
    let policy_match = archive_hosts
        .iter()
        .any(|h| h.trim_end_matches('.').eq_ignore_ascii_case(&host));
    if host == "apt.pop-os.org" {
        if !policy_match || !pop_os_system_sources {
            return false;
        }
        let path = tail.split(['?', '#']).next().unwrap_or_default();
        return path == "/ubuntu" || path == "/ubuntu/";
    }
    host == "archive.ubuntu.com" || host.ends_with(".archive.ubuntu.com") || policy_match
}

/// Discover apt source files without following symlinks. Non-UTF8 files are reported, not guessed.
pub fn inventory(root: &Path, archive_hosts: &[String], pop_os: bool) -> Inventory {
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
                        archive_hosts,
                        pop_os && path.file_name().is_some_and(|n| n == "system.sources"),
                    )
                } else {
                    parse_legacy(&text, &path, archive_hosts, false)
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

#[cfg(test)]
mod tests {
    use super::*;
    const HOSTS: &[String] = &[];

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
        let hosts = vec!["apt.pop-os.org".to_string()];
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
        let hosts = vec!["security.ubuntu.com".into(), "ports.ubuntu.com".into()];
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
        let hosts = vec!["mirror.example".to_string()];
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
