#![cfg_attr(not(test), allow(dead_code))] // the backup writer consumes this module (D9)
//! The backup manifest: what one backup run covered, in the single text form it is read back in.
//!
//! `parse` *is* the format — it fails closed on anything `render` would not have written — and
//! nothing here touches a disk: the backup writer owns every path and every copied byte.
//!
//! Every line is capped at [`MAX_LINE_BYTES`] and the whole body at [`MAX_TOTAL_BYTES`] with at
//! most `MAX_ENTRIES + 4` lines, and both caps are checked before any entry is parsed. The line cap
//! is defence in depth rather than the real guard: `Target::parse` bounds the target and the other
//! fields are fixed-shape, and the caps are wide enough that a manifest `render` produced always
//! parses.

use std::fmt;

use crate::digest::hex;
use crate::plan::Target;

const HEADER: &str = "apt-scout-backup 1";
const MAX_LINE_BYTES: usize = 192;
/// The whole body, so a hostile manifest is refused before its lines are read one by one.
const MAX_TOTAL_BYTES: usize = 65_536;
const MAX_ENTRIES: usize = 32;
/// The lines that are not entries: the header, `id`, `files` and `end`.
const FRAME_LINES: usize = 4;
const SECS_PER_DAY: u64 = 86_400;
/// The one target a run may create; `plan` enforces the same rule when it plans the write.
const SECURITY_SOURCES: &str = "sources.list.d/system-security.sources";

/// What the run did to a target: overwrite the existing file, or create the missing one.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    Replace,
    Create,
}

impl Action {
    fn as_str(self) -> &'static str {
        match self {
            Self::Replace => "replace",
            Self::Create => "create",
        }
    }
}

/// One backed-up file, described by content and metadata so a restore can refuse to guess.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ManifestEntry {
    pub target: Target,
    pub action: Action,
    pub digest: [u8; 32],
    pub mode: u32,
    pub uid: u32,
    pub gid: u32,
}

/// A run identifier: a UTC timestamp as `YYYYMMDDTHHMMSSZ`. The field is private, so the only
/// values that exist are the ones this module's two constructors produced and validated.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BackupId(String);

impl BackupId {
    /// Days-from-civil conversion, so no clock is involved: the caller supplies the seconds.
    pub fn from_secs(secs: u64) -> Result<BackupId, ManifestError> {
        let (year, month, day) = civil_from_days((secs / SECS_PER_DAY) as i64);
        let year = u32::try_from(year).map_err(|_| ManifestError::TimeOutOfRange)?;
        if year > 9999 {
            return Err(ManifestError::TimeOutOfRange);
        }
        let rest = secs % SECS_PER_DAY;
        let (hour, min, sec) = (rest / 3600, rest % 3600 / 60, rest % 60);
        Ok(BackupId(format!(
            "{year:04}{month:02}{day:02}T{hour:02}{min:02}{sec:02}Z"
        )))
    }

    /// Exactly 16 bytes, `T` and `Z` where they belong, digits elsewhere, plausible ranges.
    pub fn parse(s: &str) -> Result<BackupId, ManifestError> {
        let bad = || ManifestError::BadId(s.to_string());
        let b = s.as_bytes();
        if b.len() != 16 || b[8] != b'T' || b[15] != b'Z' {
            return Err(bad());
        }
        if !(0..8).chain(9..15).all(|i| b[i].is_ascii_digit()) {
            return Err(bad());
        }
        let two = |i: usize| u32::from(b[i] - b'0') * 10 + u32::from(b[i + 1] - b'0');
        let (month, day, hour, min, sec) = (two(4), two(6), two(9), two(11), two(13));
        let plausible = (1..=12).contains(&month)
            && (1..=31).contains(&day)
            && hour <= 23
            && min <= 59
            && sec <= 59;
        if !plausible {
            return Err(bad());
        }
        Ok(BackupId(s.to_string()))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// A parsed manifest: the run it names and the files that run covered.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Manifest {
    id: BackupId,
    entries: Vec<ManifestEntry>,
}

impl Manifest {
    pub fn id(&self) -> &BackupId {
        &self.id
    }

    pub fn entries(&self) -> &[ManifestEntry] {
        &self.entries
    }
}

/// The canonical text. It ends at the `end` line: a trailing newline is content `parse` refuses.
pub fn render(id: &BackupId, entries: &[ManifestEntry]) -> String {
    let mut out = format!("{HEADER}\nid {}\nfiles {}\n", id.as_str(), entries.len());
    for e in entries {
        let (target, action, digest, mode, uid, gid) = (
            target_str(&e.target),
            e.action.as_str(),
            hex(&e.digest),
            e.mode,
            e.uid,
            e.gid,
        );
        out.push_str(&format!(
            "file {target} {action} {digest} {mode} {uid} {gid}\n"
        ));
    }
    out.push_str(&format!("end {}", entries.len()));
    out
}

/// Read the canonical text back; every deviation from it is a refused manifest.
///
/// The grammar is one order with nothing optional: the header, `id`, `files N`, exactly N `file`
/// lines, then `end N`. `files N` bounds the entry loop before a single entry is parsed, and the
/// byte and line caps bound that loop before the text is read line by line at all.
pub fn parse(text: &str) -> Result<Manifest, ManifestError> {
    if text.len() > MAX_TOTAL_BYTES {
        return Err(ManifestError::TooLarge(text.len()));
    }
    // Four lines frame the entries, so each line past them claims one more entry than allowed.
    let framed = text.split('\n').count();
    if framed > MAX_ENTRIES + FRAME_LINES {
        return Err(ManifestError::EntryCount(framed - FRAME_LINES));
    }
    let mut lines = text.split('\n').enumerate();
    if lines.next().map(|(_, l)| l) != Some(HEADER) {
        return Err(ManifestError::Header);
    }
    let (line, f) = next_line(&mut lines, "id")?;
    let id = match f[0] {
        "id" => BackupId::parse(value(&f)?)?,
        _ => return Err(wrong_tag(&f, line, "id")),
    };
    let (line, f) = next_line(&mut lines, "files")?;
    let declared = match f[0] {
        "files" => number(value(&f)?)? as usize,
        _ => return Err(wrong_tag(&f, line, "files")),
    };
    // The declared count bounds the loop, so it is capped before the loop can grow.
    if declared > MAX_ENTRIES {
        return Err(ManifestError::EntryCount(declared));
    }
    let mut entries: Vec<ManifestEntry> = Vec::new();
    for _ in 0..declared {
        let (line, f) = next_line(&mut lines, "end")?;
        let e = match f[0] {
            "file" => entry(&f)?,
            _ => return Err(wrong_tag(&f, line, "file")),
        };
        if entries.iter().any(|prev| prev.target == e.target) {
            return Err(ManifestError::DuplicateTarget(target_str(&e.target)));
        }
        entries.push(e);
    }
    let (line, f) = next_line(&mut lines, "end")?;
    let end = match f[0] {
        "end" => number(value(&f)?)? as usize,
        _ => return Err(wrong_tag(&f, line, "end")),
    };
    if lines.next().is_some() {
        return Err(ManifestError::Trailing);
    }
    if entries.is_empty() {
        return Err(ManifestError::EntryCount(entries.len()));
    }
    if declared != end {
        return Err(ManifestError::CountMismatch);
    }
    Ok(Manifest { id, entries })
}

/// The next line as its 1-based number and its fields, refusing an over-long line and reporting an
/// absent line as the tag the grammar still needs.
fn next_line<'a>(
    lines: &mut impl Iterator<Item = (usize, &'a str)>,
    expected: &'static str,
) -> Result<(usize, Vec<&'a str>), ManifestError> {
    let (i, line) = lines.next().ok_or(ManifestError::MissingTag(expected))?;
    if line.len() > MAX_LINE_BYTES {
        return Err(ManifestError::LineTooLong(line.len()));
    }
    Ok((i + 1, line.split(' ').collect()))
}

/// A known tag in the wrong place is an order fault, naming the line and what belongs there;
/// anything else is not a tag at all.
fn wrong_tag(f: &[&str], line: usize, expected: &'static str) -> ManifestError {
    match f[0] {
        "" => ManifestError::FieldCount(f.len()),
        "id" | "files" | "file" | "end" => ManifestError::TagOrder { line, expected },
        tag => ManifestError::UnknownTag(tag.to_string()),
    }
}

/// One `file` line: seven fields, the target validated by the enumeration in `plan`.
fn entry(f: &[&str]) -> Result<ManifestEntry, ManifestError> {
    let [_, target, action, digest, mode, uid, gid] = f else {
        return Err(ManifestError::FieldCount(f.len()));
    };
    if target.is_empty() {
        return Err(ManifestError::EmptyTarget);
    }
    let parsed =
        Target::parse(target).map_err(|_| ManifestError::BadTarget((*target).to_string()))?;
    let action = match *action {
        "replace" => Action::Replace,
        "create" => Action::Create,
        other => return Err(ManifestError::BadAction(other.to_string())),
    };
    if action == Action::Create && *target != SECURITY_SOURCES {
        return Err(ManifestError::CreateNotSecurity((*target).to_string()));
    }
    let (mode, uid, gid) = (number(mode)?, number(uid)?, number(gid)?);
    // A backup manifest names files under /etc/apt, which are root-owned: recording any other
    // ownership would let a restore chown a root file to someone else.
    if mode & !0o7777 != 0 || uid != 0 || gid != 0 {
        return Err(ManifestError::Ownership((*target).to_string()));
    }
    Ok(ManifestEntry {
        target: parsed,
        action,
        digest: digest_bytes(digest)?,
        mode,
        uid,
        gid,
    })
}

/// The single value of a two-field tag line.
fn value<'a>(f: &[&'a str]) -> Result<&'a str, ManifestError> {
    match f {
        [_, v] => Ok(v),
        _ => Err(ManifestError::FieldCount(f.len())),
    }
}

/// Plain decimal `u32`: no sign, no whitespace, no overflow, and nothing octal-looking.
fn number(s: &str) -> Result<u32, ManifestError> {
    if s.is_empty() || !s.bytes().all(|b| b.is_ascii_digit()) {
        return Err(ManifestError::BadNumber);
    }
    s.parse::<u32>().map_err(|_| ManifestError::BadNumber)
}

/// Exactly 64 lowercase hex characters; uppercase and everything else is refused.
fn digest_bytes(s: &str) -> Result<[u8; 32], ManifestError> {
    let b = s.as_bytes();
    if b.len() != 64 {
        return Err(ManifestError::BadDigest);
    }
    let nibble = |c: u8| match c {
        b'0'..=b'9' => Some(c - b'0'),
        b'a'..=b'f' => Some(c - b'a' + 10),
        _ => None,
    };
    let mut out = [0u8; 32];
    let (pairs, rest) = b.as_chunks::<2>();
    if !rest.is_empty() {
        return Err(ManifestError::BadDigest);
    }
    for (slot, pair) in out.iter_mut().zip(pairs) {
        let hi = nibble(pair[0]).ok_or(ManifestError::BadDigest)?;
        let lo = nibble(pair[1]).ok_or(ManifestError::BadDigest)?;
        *slot = hi << 4 | lo;
    }
    Ok(out)
}

/// The target as the one spelling `Target::parse` accepts, joined with `/` on any platform.
fn target_str(target: &Target) -> String {
    let parts: Vec<String> = target
        .relative()
        .components()
        .map(|c| c.as_os_str().to_string_lossy().into_owned())
        .collect();
    parts.join("/")
}

/// Howard Hinnant's civil-from-days: proleptic Gregorian date of `days` since 1970-01-01.
fn civil_from_days(days: i64) -> (i64, u32, u32) {
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    (year, month as u32, day as u32)
}

/// Why a manifest (or a backup id) was refused. Each message names the rule it comes from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ManifestError {
    TimeOutOfRange,
    BadId(String),
    Header,
    FieldCount(usize),
    UnknownTag(String),
    TagOrder { line: usize, expected: &'static str },
    DuplicateTarget(String),
    CountMismatch,
    EntryCount(usize),
    TooLarge(usize),
    LineTooLong(usize),
    BadDigest,
    BadNumber,
    EmptyTarget,
    BadTarget(String),
    BadAction(String),
    CreateNotSecurity(String),
    Ownership(String),
    MissingTag(&'static str),
    Trailing,
}

impl fmt::Display for ManifestError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::TimeOutOfRange => write!(f, "id rule: the timestamp is past year 9999"),
            Self::BadId(s) => write!(f, "id rule: {s:?} is not YYYYMMDDTHHMMSSZ"),
            Self::Header => write!(f, "header rule: line 1 must be {HEADER:?}"),
            Self::FieldCount(n) => write!(f, "field rule: {n} space-separated fields on one line"),
            Self::UnknownTag(t) => write!(f, "tag rule: {t:?} is not a tag"),
            Self::TagOrder { line, expected } => {
                write!(f, "order rule: line {line} must be {expected:?}")
            }
            Self::DuplicateTarget(t) => write!(f, "duplicate rule: {t} appears twice"),
            Self::CountMismatch => write!(f, "count rule: files, the file lines and end disagree"),
            Self::EntryCount(n) => write!(f, "count rule: 1..={MAX_ENTRIES} entries, not {n}"),
            Self::TooLarge(n) => {
                write!(
                    f,
                    "size rule: {n} bytes over the {MAX_TOTAL_BYTES}-byte cap"
                )
            }
            Self::LineTooLong(n) => write!(f, "size rule: {n} bytes on one line"),
            Self::BadDigest => write!(f, "digest rule: 64 lowercase hex characters"),
            Self::BadNumber => write!(f, "number rule: plain decimal u32"),
            Self::EmptyTarget => write!(f, "target rule: must not be empty"),
            Self::BadTarget(t) => write!(f, "target rule: {t:?} is not a target"),
            Self::BadAction(a) => write!(f, "action rule: {a:?} is neither replace nor create"),
            Self::CreateNotSecurity(t) => write!(f, "create rule: {t} may not be created"),
            Self::Ownership(t) => write!(
                f,
                "ownership rule: {t} must be at most mode 0o7777 and root-owned (uid 0, gid 0), \
                 because files under /etc/apt are root-owned"
            ),
            Self::MissingTag(t) => write!(f, "missing rule: no {t} line"),
            Self::Trailing => write!(f, "end rule: content after the end line"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::digest::sha256;

    const ID: &str = "20261112T203045Z";
    const D: &str = "3d0c3d0c3d0c3d0c3d0c3d0c3d0c3d0c3d0c3d0c3d0c3d0c3d0c3d0c3d0c3d0c";

    fn manifest(id: &str, files: &str, body: &str, end: &str) -> String {
        format!("{HEADER}\nid {id}\nfiles {files}\n{body}end {end}")
    }

    fn replace_line(target: &str) -> String {
        format!("file {target} replace {D} 420 0 0\n")
    }

    #[test]
    fn manifest_round_trip() {
        let id = BackupId::from_secs(1_794_515_445).unwrap();
        assert_eq!(id.as_str(), ID);
        let entries = vec![
            ManifestEntry {
                target: Target::parse("sources.list").unwrap(),
                action: Action::Replace,
                digest: sha256(b"originalsources"),
                mode: 0o644,
                uid: 0,
                gid: 0,
            },
            ManifestEntry {
                target: Target::parse("sources.list.d/system-security.sources").unwrap(),
                action: Action::Create,
                digest: sha256(b"newcontents"),
                mode: 0o600,
                uid: 0,
                gid: 0,
            },
        ];
        let text = render(&id, &entries);
        assert!(text.ends_with("end 2"), "{text}");
        assert_eq!(parse(&text).unwrap().entries(), entries.as_slice());
        let parsed = parse(&text).unwrap();
        assert_eq!(parsed.id(), &id);
        assert_eq!(parsed.entries().len(), 2);
        let (first, second) = (&parsed.entries()[0], &parsed.entries()[1]);
        assert_eq!(first.target, Target::parse("sources.list").unwrap());
        assert_eq!(first.action, Action::Replace);
        assert_eq!(first.digest, sha256(b"originalsources"));
        assert_eq!((first.mode, first.uid, first.gid), (0o644, 0, 0));
        assert_eq!(
            second.target.relative().display().to_string(),
            SECURITY_SOURCES
        );
        assert_eq!(second.action, Action::Create);
        assert_eq!(second.digest, sha256(b"newcontents"));
        assert_eq!((second.mode, second.uid, second.gid), (0o600, 0, 0));
        assert_eq!(render(parsed.id(), parsed.entries()), text);
        // The widest entry the rules allow must still fit on one line, otherwise `render` could
        // produce a manifest that `parse` refuses; a widened name cap fails here instead.
        let widest = ManifestEntry {
            target: Target::parse(&format!("sources.list.d/{}.list", "b".repeat(59))).unwrap(),
            action: Action::Replace,
            digest: [0xff; 32],
            mode: 0o7777,
            uid: 0,
            gid: 0,
        };
        for entry in [&widest, second] {
            let one: &[ManifestEntry] = std::slice::from_ref(entry);
            let long = render(&id, one);
            let longest = long.split('\n').map(str::len).max().unwrap_or(0);
            assert!(
                longest <= MAX_LINE_BYTES,
                "{longest} bytes over a cap of {MAX_LINE_BYTES}"
            );
            let back = parse(&long).unwrap();
            assert_eq!(back.entries(), one);
        }
    }

    #[test]
    fn backup_id_string_order_matches_time_order() {
        let secs = [
            0,
            1,
            951_782_400,
            1_709_164_800,
            1_794_515_445,
            1_794_515_446,
            1_798_761_599,
        ];
        let ids: Vec<String> = secs
            .iter()
            .map(|&s| BackupId::from_secs(s).unwrap().as_str().to_string())
            .collect();
        let mut sorted = ids.clone();
        sorted.sort();
        assert_eq!(ids, sorted);
        // Leap day and the last second of a year, in both directions.
        let leap = BackupId::from_secs(1_709_208_000).unwrap();
        assert_eq!(leap.as_str(), "20240229T120000Z");
        let end = BackupId::from_secs(1_798_761_599).unwrap();
        assert_eq!(end.as_str(), "20261231T235959Z");
        for text in [
            ID,
            "20240229T120000Z",
            "20261231T235959Z",
            "19700101T000000Z",
        ] {
            assert_eq!(BackupId::parse(text).unwrap().as_str(), text);
        }
        assert_eq!(
            parse(&manifest(
                end.as_str(),
                "1",
                &replace_line("sources.list"),
                "1"
            ))
            .unwrap()
            .id(),
            &end
        );
        // The year must fit four digits, which is where the range ends.
        assert_eq!(
            BackupId::from_secs(253_402_300_800),
            Err(ManifestError::TimeOutOfRange)
        );
        assert!(BackupId::from_secs(253_402_300_799).is_ok());
    }

    #[test]
    fn parse_refuses_every_hostile_manifest() {
        let ok = manifest(ID, "1", &replace_line("sources.list"), "1");
        assert_eq!(parse(&ok).unwrap().entries().len(), 1);
        let many: String = (0..33)
            .map(|i| replace_line(&format!("sources.list.d/m{i}.list")))
            .collect();
        let cases: Vec<(String, ManifestError)> = vec![
            (String::new(), ManifestError::Header),
            (
                ok.replacen(HEADER, "apt-scout-backup 2", 1),
                ManifestError::Header,
            ),
            (
                format!("{HEADER}\nid {ID}\nfiles 1\nfile sources.list replace {D} 420 0\nend 1"),
                ManifestError::FieldCount(6),
            ),
            (
                format!(
                    "{HEADER}\nid {ID}\nfiles 1\nfile sources.list replace {D} 420 0 0 0\nend 1"
                ),
                ManifestError::FieldCount(8),
            ),
            (
                format!("{HEADER}\nfiles 1\njunk 1\n{HEADER}"),
                ManifestError::TagOrder {
                    line: 2,
                    expected: "id",
                },
            ),
            (
                manifest(ID, "1", "junk\n", "1"),
                ManifestError::UnknownTag("junk".to_string()),
            ),
            (
                format!(
                    "{HEADER}\n\nid {ID}\nfiles 1\n{}end 1",
                    replace_line("sources.list")
                ),
                ManifestError::FieldCount(1),
            ),
            (
                format!(
                    "{HEADER}\nid {ID}\nid {ID}\nfiles 1\n{}end 1",
                    replace_line("sources.list")
                ),
                ManifestError::TagOrder {
                    line: 3,
                    expected: "files",
                },
            ),
            (
                format!(
                    "{HEADER}\nfiles 1\nfiles 1\n{}end 1",
                    replace_line("sources.list")
                ),
                ManifestError::TagOrder {
                    line: 2,
                    expected: "id",
                },
            ),
            (
                format!(
                    "{HEADER}\nid {ID}\nfiles 1\nfiles 1\n{}end 1",
                    replace_line("sources.list")
                ),
                ManifestError::TagOrder {
                    line: 4,
                    expected: "file",
                },
            ),
            (format!("{ok}\nend 1"), ManifestError::Trailing),
            (
                manifest(
                    ID,
                    "2",
                    &(replace_line("sources.list") + &replace_line("sources.list")),
                    "2",
                ),
                ManifestError::DuplicateTarget("sources.list".to_string()),
            ),
            (
                manifest(
                    ID,
                    "1",
                    &(replace_line("sources.list") + &replace_line("sources.list.d/a.list")),
                    "2",
                ),
                ManifestError::TagOrder {
                    line: 5,
                    expected: "end",
                },
            ),
            (
                manifest(ID, "2", &replace_line("sources.list"), "2"),
                ManifestError::TagOrder {
                    line: 5,
                    expected: "file",
                },
            ),
            (
                manifest(ID, "1", &replace_line("sources.list"), "2"),
                ManifestError::CountMismatch,
            ),
            (manifest(ID, "0", "", "0"), ManifestError::EntryCount(0)),
            (
                manifest(ID, "40", &replace_line("sources.list"), "40"),
                ManifestError::EntryCount(40),
            ),
            (
                manifest(ID, "33", &many, "33"),
                ManifestError::EntryCount(33),
            ),
            (
                manifest(
                    ID,
                    "1",
                    &format!("file sources.list replace {D} 420 0 {}\n", "0".repeat(96)),
                    "1",
                ),
                ManifestError::LineTooLong(193),
            ),
            (
                manifest(
                    ID,
                    "1",
                    &format!("file sources.list replace {} 420 0 0\n", &D[..62]),
                    "1",
                ),
                ManifestError::BadDigest,
            ),
            (
                manifest(
                    ID,
                    "1",
                    &format!("file sources.list replace {D}0 420 0 0\n",),
                    "1",
                ),
                ManifestError::BadDigest,
            ),
            (
                manifest(
                    ID,
                    "1",
                    &format!("file sources.list replace {} 420 0 0\n", D.to_uppercase()),
                    "1",
                ),
                ManifestError::BadDigest,
            ),
            (
                manifest(
                    ID,
                    "1",
                    &format!("file sources.list replace {} 420 0 0\n", "g".repeat(64)),
                    "1",
                ),
                ManifestError::BadDigest,
            ),
            (
                manifest(
                    ID,
                    "1",
                    &format!("file sources.list replace {D} +420 0 0\n"),
                    "1",
                ),
                ManifestError::BadNumber,
            ),
            (
                manifest(
                    ID,
                    "1",
                    &format!("file sources.list replace {D} 420 -1 0\n"),
                    "1",
                ),
                ManifestError::BadNumber,
            ),
            (
                manifest(
                    ID,
                    "1",
                    &format!("file sources.list replace {D} 420 0 4294967296\n"),
                    "1",
                ),
                ManifestError::BadNumber,
            ),
            (
                manifest(
                    ID,
                    "1",
                    &format!("file sources.list replace {D} 420 0 0x0\n"),
                    "1",
                ),
                ManifestError::BadNumber,
            ),
            (
                manifest(ID, "1", &format!("file  replace {D} 420 0 0\n"), "1"),
                ManifestError::EmptyTarget,
            ),
            (
                manifest(
                    ID,
                    "1",
                    &format!("file ../sources.list replace {D} 420 0 0\n"),
                    "1",
                ),
                ManifestError::BadTarget("../sources.list".to_string()),
            ),
            (
                manifest(
                    ID,
                    "1",
                    &format!("file /etc/apt/sources.list replace {D} 420 0 0\n"),
                    "1",
                ),
                ManifestError::BadTarget("/etc/apt/sources.list".to_string()),
            ),
            (
                manifest(
                    ID,
                    "1",
                    &format!("file sources.list.d/../sources.list replace {D} 420 0 0\n"),
                    "1",
                ),
                ManifestError::BadTarget("sources.list.d/../sources.list".to_string()),
            ),
            (
                manifest(
                    ID,
                    "1",
                    &format!("file sources.list.d/x.txt replace {D} 420 0 0\n"),
                    "1",
                ),
                ManifestError::BadTarget("sources.list.d/x.txt".to_string()),
            ),
            (
                manifest(
                    ID,
                    "1",
                    &format!("file sources.list patch {D} 420 0 0\n"),
                    "1",
                ),
                ManifestError::BadAction("patch".to_string()),
            ),
            (
                manifest(
                    ID,
                    "1",
                    &format!("file sources.list create {D} 420 0 0\n"),
                    "1",
                ),
                ManifestError::CreateNotSecurity("sources.list".to_string()),
            ),
            (
                manifest(
                    ID,
                    "1",
                    &format!("file sources.list replace {D} 33188 0 0\n"),
                    "1",
                ),
                ManifestError::Ownership("sources.list".to_string()),
            ),
            (
                manifest(
                    ID,
                    "1",
                    &format!("file sources.list replace {D} 420 0 1000\n"),
                    "1",
                ),
                ManifestError::Ownership("sources.list".to_string()),
            ),
            (
                format!("{HEADER}\nfiles 1\n{}end 1", replace_line("sources.list")),
                ManifestError::TagOrder {
                    line: 2,
                    expected: "id",
                },
            ),
            (HEADER.to_string(), ManifestError::MissingTag("id")),
            (
                format!("{HEADER}\nid {ID}\n{}end 1", replace_line("sources.list")),
                ManifestError::TagOrder {
                    line: 3,
                    expected: "files",
                },
            ),
            (
                format!(
                    "{HEADER}\nid {ID}\nfiles 1\n{}",
                    replace_line("sources.list").trim_end()
                ),
                ManifestError::MissingTag("end"),
            ),
            (format!("{ok}\n"), ManifestError::Trailing),
            (format!("{ok}\njunk"), ManifestError::Trailing),
            (
                manifest(ID, "1", &replace_line("sources.list"), "1x"),
                ManifestError::BadNumber,
            ),
            (
                format!(
                    "{HEADER}\nid 20261312T203045Z\nfiles 1\n{}end 1",
                    replace_line("sources.list")
                ),
                ManifestError::BadId("20261312T203045Z".to_string()),
            ),
            (
                format!(
                    "{HEADER}\nid 20261112T246000Z\nfiles 1\n{}end 1",
                    replace_line("sources.list")
                ),
                ManifestError::BadId("20261112T246000Z".to_string()),
            ),
            (
                format!(
                    "{HEADER}\nid 20261112T203045\nfiles 1\n{}end 1",
                    replace_line("sources.list")
                ),
                ManifestError::BadId("20261112T203045".to_string()),
            ),
        ];
        for (text, expected) in cases {
            assert_eq!(parse(&text).unwrap_err(), expected, "{text:?}");
        }
    }

    #[test]
    fn parse_refuses_a_non_root_owner() {
        // The inverted case: this line used to be accepted and parsed as (0o644, 1000, 1000).
        // Files under /etc/apt are root-owned, so a recorded 1000:1000 owner is a refused record.
        let text = manifest(
            ID,
            "1",
            &format!("file sources.list replace {D} 420 1000 1000\n"),
            "1",
        );
        assert_eq!(
            parse(&text),
            Err(ManifestError::Ownership("sources.list".to_string()))
        );
        // The rule refuses the owner, not the file: the same line with a root owner parses.
        let owned = parse(&manifest(
            ID,
            "1",
            &format!("file sources.list replace {D} 420 0 0\n"),
            "1",
        ));
        assert!(owned.is_ok());
    }

    #[test]
    fn parse_caps_the_body_before_reading_it() {
        // The widest manifest `render` can produce: 32 entries with the longest allowed names and
        // the widest legal metadata. It must stay under both caps, so the caps never refuse the
        // format's own output.
        let widest: Vec<ManifestEntry> = (0..MAX_ENTRIES)
            .map(|i| ManifestEntry {
                target: Target::parse(&format!("sources.list.d/{}{i:02}.list", "b".repeat(55)))
                    .unwrap(),
                action: Action::Replace,
                digest: [0xab; 32],
                mode: 0o7777,
                uid: 0,
                gid: 0,
            })
            .collect();
        let full = render(&BackupId::parse(ID).unwrap(), &widest);
        assert!(full.len() <= MAX_TOTAL_BYTES, "{} bytes", full.len());
        assert_eq!(full.split('\n').count(), MAX_ENTRIES + FRAME_LINES);
        assert_eq!(parse(&full).unwrap().entries().len(), MAX_ENTRIES);

        // The byte cap: a body past it is refused with the size it observed.
        let ok = manifest(ID, "1", &replace_line("sources.list"), "1");
        let over = format!("{ok}{}", "x".repeat(MAX_TOTAL_BYTES));
        assert_eq!(parse(&over), Err(ManifestError::TooLarge(over.len())));

        // The line cap: four frame lines plus one line per allowed entry. One line more is refused
        // before the junk lines are ever read entry by entry.
        let junk = format!(
            "{HEADER}\nid {ID}\nfiles 1\n{}{}end 1",
            replace_line("sources.list"),
            "junk\n".repeat(MAX_ENTRIES)
        );
        assert_eq!(junk.split('\n').count(), MAX_ENTRIES + FRAME_LINES + 1);
        assert_eq!(
            parse(&junk),
            Err(ManifestError::EntryCount(MAX_ENTRIES + 1))
        );
    }
}
