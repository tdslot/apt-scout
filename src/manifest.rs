#![cfg_attr(not(test), allow(dead_code))] // the backup writer consumes this module (D9)
//! The backup manifest: what one backup run covered, in the single text form it is read back in.
//!
//! `parse` *is* the format — it fails closed on anything `render` would not have written — and
//! nothing here touches a disk: the backup writer owns every path and every copied byte.
//!
//! Every line is capped at [`MAX_LINE_BYTES`], which is defence in depth rather than the real
//! guard: `Target::parse` bounds the target and the other fields are fixed-shape, and the cap is
//! wide enough that a manifest `render` produced always parses.

use std::fmt;

use crate::digest::hex;
use crate::plan::Target;

const HEADER: &str = "apt-scout-backup 1";
const MAX_LINE_BYTES: usize = 192;
const MAX_ENTRIES: usize = 32;
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
pub fn parse(text: &str) -> Result<Manifest, ManifestError> {
    let mut lines = text.split('\n');
    if lines.next() != Some(HEADER) {
        return Err(ManifestError::Header);
    }
    let (mut id, mut files, mut end) = (None, None, None);
    let mut entries: Vec<ManifestEntry> = Vec::new();
    for line in lines {
        if end.is_some() {
            // A second `end` is the duplicate case; anything else is content past the manifest.
            return Err(match line.split(' ').next() {
                Some("end") => ManifestError::DuplicateTag("end"),
                _ => ManifestError::Trailing,
            });
        }
        if line.len() > MAX_LINE_BYTES {
            return Err(ManifestError::LineTooLong(line.len()));
        }
        let f: Vec<&str> = line.split(' ').collect();
        match f[0] {
            "" => return Err(ManifestError::FieldCount(f.len())),
            "id" if id.is_some() => return Err(ManifestError::DuplicateTag("id")),
            "files" if files.is_some() => return Err(ManifestError::DuplicateTag("files")),
            "id" => id = Some(BackupId::parse(value(&f)?)?),
            "files" => files = Some(number(value(&f)?)? as usize),
            "file" => {
                let e = entry(&f)?;
                let dup = entries.iter().any(|prev| prev.target == e.target);
                if dup {
                    return Err(ManifestError::DuplicateTarget(target_str(&e.target)));
                }
                entries.push(e);
            }
            "end" => end = Some(number(value(&f)?)? as usize),
            tag => return Err(ManifestError::UnknownTag(tag.to_string())),
        }
    }
    let id = id.ok_or(ManifestError::MissingTag("id"))?;
    let files = files.ok_or(ManifestError::MissingTag("files"))?;
    let end = end.ok_or(ManifestError::MissingTag("end"))?;
    if entries.is_empty() || entries.len() > MAX_ENTRIES {
        return Err(ManifestError::EntryCount(entries.len()));
    }
    if files != entries.len() || end != entries.len() {
        return Err(ManifestError::CountMismatch);
    }
    Ok(Manifest { id, entries })
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
    Ok(ManifestEntry {
        target: parsed,
        action,
        digest: digest_bytes(digest)?,
        mode: number(mode)?,
        uid: number(uid)?,
        gid: number(gid)?,
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
    DuplicateTag(&'static str),
    DuplicateTarget(String),
    CountMismatch,
    EntryCount(usize),
    LineTooLong(usize),
    BadDigest,
    BadNumber,
    EmptyTarget,
    BadTarget(String),
    BadAction(String),
    CreateNotSecurity(String),
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
            Self::DuplicateTag(t) => write!(f, "duplicate rule: {t} appears twice"),
            Self::DuplicateTarget(t) => write!(f, "duplicate rule: {t} appears twice"),
            Self::CountMismatch => write!(f, "count rule: files, the file lines and end disagree"),
            Self::EntryCount(n) => write!(f, "count rule: 1..={MAX_ENTRIES} entries, not {n}"),
            Self::LineTooLong(n) => write!(f, "size rule: {n} bytes on one line"),
            Self::BadDigest => write!(f, "digest rule: 64 lowercase hex characters"),
            Self::BadNumber => write!(f, "number rule: plain decimal u32"),
            Self::EmptyTarget => write!(f, "target rule: must not be empty"),
            Self::BadTarget(t) => write!(f, "target rule: {t:?} is not a target"),
            Self::BadAction(a) => write!(f, "action rule: {a:?} is neither replace nor create"),
            Self::CreateNotSecurity(t) => write!(f, "create rule: {t} may not be created"),
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
                uid: 1000,
                gid: 1000,
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
        assert_eq!((second.mode, second.uid, second.gid), (0o600, 1000, 1000));
        assert_eq!(render(parsed.id(), parsed.entries()), text);
        // The widest entry the rules allow must still fit on one line, otherwise `render` could
        // produce a manifest that `parse` refuses; a widened name cap fails here instead.
        let widest = ManifestEntry {
            target: Target::parse(&format!("sources.list.d/{}.list", "b".repeat(59))).unwrap(),
            action: Action::Replace,
            digest: [0xff; 32],
            mode: u32::MAX,
            uid: u32::MAX,
            gid: u32::MAX,
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
                ManifestError::UnknownTag("junk".to_string()),
            ),
            (
                format!(
                    "{HEADER}\nid {ID}\nid {ID}\nfiles 1\n{}end 1",
                    replace_line("sources.list")
                ),
                ManifestError::DuplicateTag("id"),
            ),
            (
                format!(
                    "{HEADER}\nfiles 1\nfiles 1\n{}end 1",
                    replace_line("sources.list")
                ),
                ManifestError::DuplicateTag("files"),
            ),
            (format!("{ok}\nend 1"), ManifestError::DuplicateTag("end")),
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
                ManifestError::CountMismatch,
            ),
            (
                manifest(ID, "2", &replace_line("sources.list"), "2"),
                ManifestError::CountMismatch,
            ),
            (
                manifest(ID, "1", &replace_line("sources.list"), "2"),
                ManifestError::CountMismatch,
            ),
            (manifest(ID, "0", "", "0"), ManifestError::EntryCount(0)),
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
                format!("{HEADER}\nfiles 1\n{}end 1", replace_line("sources.list")),
                ManifestError::MissingTag("id"),
            ),
            (
                format!("{HEADER}\nid {ID}\n{}end 1", replace_line("sources.list")),
                ManifestError::MissingTag("files"),
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
}
