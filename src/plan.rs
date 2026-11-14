#![cfg_attr(not(test), allow(dead_code))] // production consumers arrive with the apply step (D9)
//! Which apt source files may be written, and only when a signature-verified mirror backs it.

use std::fmt;
use std::path::{Path, PathBuf};

use crate::digest::sha256;
use crate::fresh::Probed;
// `manifest` names `plan::Target`, this module names `manifest::BackupId` and `restore` names
// `Plan`: module cycles, so no type is copied into another module to break one.
use crate::manifest::BackupId;
use crate::restore::ValidatedBackup;
use crate::verify::VerifiedDate;

const MAX_NAME_BYTES: usize = 64;
const MAX_WRITES: usize = 32;
const MAX_FILE_BYTES: usize = 1024 * 1024;
/// The one file a plan may create: a stock install has no source for the security suite.
const SECURITY_SOURCES: &str = "system-security.sources";

/// The distribution family a plan is for. Creating the security source repairs a Pop!_OS install;
/// Ubuntu's archive already ships that source, so a plan that creates it there was built for the
/// wrong system.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Platform {
    Ubuntu,
    PopOs,
}

/// What a write does to its target.
///
/// Three effects that exclude each other: an `Effect::Remove`-by-empty-bytes rule inside the writer
/// would be a magic value, and two booleans would have a fourth combination that means nothing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Effect {
    /// Create a target that is not on disk.
    Create,
    /// Overwrite the target that is on disk.
    Replace,
    /// Remove the target that is on disk: undoing the creation a backup recorded.
    Remove,
}

/// What authorizes this plan to be written.
///
/// Two forms and never both: a mirror plan is authorized by the probe that verified its URI, a
/// restore plan by a backup `restore::build` read and re-derived. Each arm's token exists in one
/// module only — [`VerifiedDate`] in the verifier, [`ValidatedBackup`] in the restore reader — so a
/// caller cannot mint an authority out of a URI, a date or a [`BackupId`], and a restore cannot be
/// authorized by a backup nothing has read.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Authority {
    Mirror { uri: String, proof: VerifiedDate },
    Restore { backup: BackupId },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Target {
    SourcesList,
    SourcesListD(SourceName),
}

/// A file name inside `sources.list.d`, checked to be one harmless path component.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SourceName(String);

impl SourceName {
    /// The only constructor. The charset excludes `/`, `\` and NUL, so a name is always one path
    /// component; with the required suffix, `..` (the only traversal spelling) is not a name either.
    pub fn parse(s: &str) -> Result<SourceName, PlanError> {
        if s.is_empty() {
            return Err(PlanError::EmptyName);
        }
        if s.len() > MAX_NAME_BYTES {
            return Err(PlanError::NameTooLong);
        }
        let allowed = |b: u8| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'-' | b'_');
        if !s.bytes().all(allowed) {
            return Err(PlanError::NameCharset);
        }
        if !(s.ends_with(".list") || s.ends_with(".sources")) {
            return Err(PlanError::NameSuffix);
        }
        if s.starts_with('.') {
            return Err(PlanError::NameLeadingDot);
        }
        Ok(SourceName(s.to_string()))
    }
}

impl Target {
    /// Accept exactly `sources.list` or `sources.list.d/<valid name>`; no other spelling exists.
    pub fn parse(s: &str) -> Result<Target, PlanError> {
        if s == "sources.list" {
            return Ok(Target::SourcesList);
        }
        match s.strip_prefix("sources.list.d/") {
            Some(rest) => Ok(Target::SourcesListD(SourceName::parse(rest)?)),
            None => Err(PlanError::UnknownTarget(s.to_string())),
        }
    }

    /// The only path builder in the module; every path below is derived from it.
    pub fn relative(&self) -> PathBuf {
        match self {
            Target::SourcesList => PathBuf::from("sources.list"),
            Target::SourcesListD(name) => Path::new("sources.list.d").join(&name.0),
        }
    }

    /// Containment is by construction: the relative path cannot leave `base`.
    #[allow(clippy::wrong_self_convention)] // converts this target; builds no target from a value
    pub fn from_under(&self, base: &Path) -> PathBuf {
        base.join(self.relative())
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileState {
    pub dev: u64,
    pub ino: u64,
    pub mode: u32,
    pub uid: u32,
    pub gid: u32,
    pub bytes: Vec<u8>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlannedWrite {
    pub target: Target,
    pub new_bytes: Vec<u8>,
    /// The SHA-256 of `new_bytes`. It travels with the bytes, so `validate` can refuse a write
    /// whose proof was issued for one content and whose bytes are another.
    pub digest: [u8; 32],
    pub original: Option<FileState>,
    /// What this write does to the target. A [`Effect::Remove`] carries `new_bytes: Vec::new()` and
    /// `digest: sha256(&[])`, so the digest rule below is one rule for every effect.
    pub effect: Effect,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Plan {
    authority: Authority,
    /// The family the plan was built for. `None` on a restore plan, which creates nothing.
    platform: Option<Platform>,
    writes: Vec<PlannedWrite>,
}

impl Plan {
    /// Both halves of the proposal come from `probed` (its URL and the `verified_updates` verification
    /// handed it), so a URI and a `VerifiedDate` from two different probes cannot be paired by the
    /// caller, and a probe that verifies nothing yields `NoProof`. The platform comes from the caller
    /// because the create rule is the only rule that needs it.
    pub fn for_apply(
        probed: &Probed,
        platform: Platform,
        writes: Vec<PlannedWrite>,
    ) -> Result<Plan, PlanError> {
        let proof = probed.verified_updates.ok_or(PlanError::NoProof)?;
        let plan = Plan {
            authority: Authority::Mirror {
                uri: probed.url.clone(),
                proof,
            },
            platform: Some(platform),
            writes,
        };
        plan.validate()?;
        Ok(plan)
    }

    /// A plan re-derived from a backup: authorized by that backup, carrying no mirror.
    ///
    /// The parameter is the validated backup, not its id: [`ValidatedBackup`] is constructible in
    /// `restore` alone, so a restore authority cannot be minted by a caller that never read the
    /// manifest, the copies and the disk. It is the same gate `for_apply` puts on its proof.
    ///
    /// `platform: None` is what keeps a restore from creating anything: the create rules below
    /// need `Some(PopOs)`, so a restore writes only what the backup already describes.
    pub fn for_restore(
        backup: &ValidatedBackup,
        writes: Vec<PlannedWrite>,
    ) -> Result<Plan, PlanError> {
        let plan = Plan {
            authority: Authority::Restore {
                backup: backup.id().clone(),
            },
            platform: None,
            writes,
        };
        plan.validate()?;
        Ok(plan)
    }

    pub fn validate(&self) -> Result<(), PlanError> {
        if self.writes.is_empty() || self.writes.len() > MAX_WRITES {
            return Err(PlanError::WriteCount(self.writes.len()));
        }
        for (i, w) in self.writes.iter().enumerate() {
            let target = || w.target.relative().display().to_string();
            if self.writes[..i].iter().any(|prev| prev.target == w.target) {
                return Err(PlanError::DuplicateTarget(target()));
            }
            // The bytes are what reaches the disk, so they must be the bytes the digest recorded.
            if sha256(&w.new_bytes) != w.digest {
                return Err(PlanError::BytesMismatch(target()));
            }
            match (w.effect, &w.target, &w.original) {
                (Effect::Create, Target::SourcesList, _) => {
                    return Err(PlanError::CreateNewOnList);
                }
                (Effect::Create, Target::SourcesListD(_), Some(_)) => {
                    return Err(PlanError::CreateNewWithOriginal(target()));
                }
                (Effect::Create, Target::SourcesListD(_), None)
                    if self.platform != Some(Platform::PopOs) =>
                {
                    return Err(PlanError::CreateNewWrongPlatform(target()));
                }
                (Effect::Create, Target::SourcesListD(n), None) if n.0 != SECURITY_SOURCES => {
                    return Err(PlanError::CreateNewName(target()));
                }
                (Effect::Create, Target::SourcesListD(_), None) => {}
                (Effect::Replace | Effect::Remove, _, None) => {
                    return Err(PlanError::ReplaceMissingOriginal(target()));
                }
                (Effect::Replace | Effect::Remove, _, Some(_)) => {}
            }
            let big = |b: &[u8]| b.len() > MAX_FILE_BYTES;
            if big(&w.new_bytes) || w.original.as_ref().is_some_and(|o| big(&o.bytes)) {
                return Err(PlanError::FileTooLarge(target()));
            }
        }
        // Only a mirror plan names a URI, so only a mirror plan has one to check.
        if let Authority::Mirror { uri, .. } = &self.authority
            && !(uri.starts_with("http://") || uri.starts_with("https://"))
        {
            return Err(PlanError::SelectedUri(uri.clone()));
        }
        Ok(())
    }

    // One-expression accessors stay on one line so `src/plan.rs` keeps its size budget.
    #[rustfmt::skip]
    pub fn authority(&self) -> &Authority { &self.authority }
    #[rustfmt::skip]
    pub fn writes(&self) -> &[PlannedWrite] { &self.writes }
    #[rustfmt::skip]
    pub fn is_empty(&self) -> bool { self.writes.is_empty() }
}

/// Why a name, a target or a whole plan was refused. Each message names the rule it comes from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PlanError {
    EmptyName,
    NameTooLong,
    NameCharset,
    NameSuffix,
    NameLeadingDot,
    UnknownTarget(String),
    NoProof,
    WriteCount(usize),
    DuplicateTarget(String),
    BytesMismatch(String),
    CreateNewOnList,
    CreateNewName(String),
    CreateNewWrongPlatform(String),
    CreateNewWithOriginal(String),
    ReplaceMissingOriginal(String),
    FileTooLarge(String),
    SelectedUri(String),
}

impl fmt::Display for PlanError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            PlanError::EmptyName => write!(f, "source name rule: must not be empty"),
            PlanError::NameTooLong => write!(f, "source name rule: at most {MAX_NAME_BYTES} bytes"),
            PlanError::NameCharset => write!(f, "source name rule: bad character"),
            PlanError::NameSuffix => write!(f, "source name rule: needs .list or .sources"),
            PlanError::NameLeadingDot => write!(f, "source name rule: no leading '.'"),
            PlanError::UnknownTarget(s) => write!(f, "target rule: {s:?} is not a target"),
            PlanError::NoProof => write!(f, "proof rule: the mirror is not verified"),
            PlanError::WriteCount(n) => write!(f, "write count rule: 1..={MAX_WRITES}, not {n}"),
            PlanError::DuplicateTarget(t) => write!(f, "duplicate rule: {t} appears twice"),
            PlanError::BytesMismatch(t) => {
                write!(
                    f,
                    "digest rule: the bytes of {t} do not match the recorded digest"
                )
            }
            PlanError::CreateNewOnList => write!(f, "create_new rule: never sources.list"),
            PlanError::CreateNewName(t) => write!(f, "create_new rule: {t} may not be created"),
            PlanError::CreateNewWrongPlatform(t) => {
                write!(f, "create_new rule: {t} may only be created on Pop!_OS")
            }
            PlanError::CreateNewWithOriginal(t) => write!(f, "create_new rule: {t} exists"),
            PlanError::ReplaceMissingOriginal(t) => write!(f, "replace rule: {t} has no original"),
            PlanError::FileTooLarge(t) => write!(f, "size rule: {t} over {MAX_FILE_BYTES} bytes"),
            PlanError::SelectedUri(u) => write!(f, "uri rule: {u:?} is not http(s)"),
        }
    }
}

// The skip keeps the helpers and one-line assertions below inside the size budget.
#[cfg(test)]
#[rustfmt::skip]
mod tests {
    use super::*;
    use crate::digest::sha256;
    use crate::fresh::{self, SignedDate};
    use crate::verify::SignatureStatus;

    const LIST: &str = "sources.list";
    const SECURITY: &str = "sources.list.d/system-security.sources";
    const RFC_A: &str = "Thu, 12 Nov 2026  4:23:38 UTC";
    const RFC_B: &str = "Thu, 1 Jan 2026 00:00:00 UTC";
    const ARM: &str = "-----BEGIN PGP SIGNED MESSAGE-----";

    fn signed(rfc: &str) -> SignedDate {
        fresh::release_date(&format!("{ARM}\nHash: SHA512\n\nDate: {rfc}\n")).unwrap()
    }

    /// A verified date, minted the only way a test can: the verifier's own in-region read.
    fn proof_of(rfc: &str) -> Option<VerifiedDate> {
        crate::verify::verified_date_for_test(&format!("{ARM}\nHash: SHA512\n\nDate: {rfc}\n"))
    }

    fn proof() -> VerifiedDate { proof_of(RFC_A).unwrap() }

    fn probe(url: &str, rfc: &str) -> Probed {
        let date = signed(rfc);
        Probed { url: url.to_string(), updates_date: date.seconds(), verified_updates: proof_of(rfc),
            lag_secs: 0, throughput_bps: 0, latency_ms: 0, same_as: false,
            signature: SignatureStatus::verified_for_test() }
    }

    fn old() -> Option<FileState> {
        let bytes = b"old\n".to_vec();
        Some(FileState { dev: 1, ino: 2, mode: 0o644, uid: 0, gid: 0, bytes })
    }

    fn w(target: &str, effect: Effect, original: Option<FileState>) -> PlannedWrite {
        let new_bytes = b"new\n".to_vec();
        let digest = sha256(&new_bytes);
        PlannedWrite { target: Target::parse(target).unwrap(), new_bytes, digest, original, effect }
    }

    fn plan(platform: Platform, writes: Vec<PlannedWrite>) -> Plan {
        let uri = "https://m.example/ubuntu".to_string();
        Plan { authority: Authority::Mirror { uri, proof: proof() }, platform: Some(platform), writes }
    }

    /// A removable target: a `create` a mirror run made, on disk now.
    fn removable() -> PlannedWrite {
        let mut write = w(SECURITY, Effect::Remove, old());
        write.new_bytes = Vec::new();
        write.digest = sha256(&write.new_bytes);
        write
    }

    fn restore(writes: Vec<PlannedWrite>) -> Result<Plan, PlanError> {
        let id = crate::manifest::BackupId::parse("20261112T203045Z").unwrap();
        Plan::for_restore(&crate::restore::validated_backup_for_test(id), writes)
    }

    fn bad(writes: Vec<PlannedWrite>) -> Option<PlanError> {
        plan(Platform::PopOs, writes).validate().err()
    }

    #[test]
    fn target_parse_refuses_everything_outside_the_enumeration() {
        let long = format!("{}.list", "a".repeat(60));
        assert_eq!(long.len(), 65);
        let hostile = ["", "/etc/apt/sources.list", "../sources.list",
            "sources.list.d/../sources.list", "sources.list.d/a/b.list", "sources.list.d/",
            "sources.list.d/.hidden.list", "sources.list.d/x.txt", long.as_str(),
            "sources.list.d/a\0.list", "Sources.list", "sources.list.bak"];
        for s in hostile {
            assert!(Target::parse(s).is_err(), "{s:?} must be refused");
        }
        let list = Target::parse(LIST).unwrap();
        assert_eq!(list.relative(), Path::new(LIST));
        assert_eq!(list.from_under(Path::new("/etc/apt")), Path::new("/etc/apt/sources.list"));
        let security = Target::parse(SECURITY).unwrap();
        assert_eq!(security.relative(), Path::new(SECURITY));
    }

    #[test]
    fn validate_refuses_every_incomplete_plan() {
        assert_eq!(bad(vec![]), Some(PlanError::WriteCount(0)));
        let many = (0..33).map(|i| w(&format!("sources.list.d/m{i}.list"), Effect::Replace, old())).collect();
        assert_eq!(bad(many), Some(PlanError::WriteCount(33)));
        let dup = vec![w(LIST, Effect::Replace, old()), w(LIST, Effect::Replace, old())];
        assert_eq!(bad(dup), Some(PlanError::DuplicateTarget(LIST.to_string())));
        assert_eq!(bad(vec![w(LIST, Effect::Create, None)]), Some(PlanError::CreateNewOnList));
        let other = vec![w("sources.list.d/backports.sources", Effect::Create, None)];
        assert_eq!(bad(other), Some(PlanError::CreateNewName("sources.list.d/backports.sources".into())));        assert_eq!(bad(vec![w(SECURITY, Effect::Create, old())]), Some(PlanError::CreateNewWithOriginal(SECURITY.into())));
        assert_eq!(bad(vec![w(LIST, Effect::Replace, None)]), Some(PlanError::ReplaceMissingOriginal(LIST.into())));
        let mut huge = w(LIST, Effect::Replace, old());
        huge.new_bytes = vec![b'x'; MAX_FILE_BYTES + 1];
        huge.digest = sha256(&huge.new_bytes);
        assert_eq!(bad(vec![huge]), Some(PlanError::FileTooLarge(LIST.to_string())));
        for uri in ["", "ftp://m.example/ubuntu"] {
            let one = vec![w(LIST, Effect::Replace, old())];
            let plan = Plan { authority: Authority::Mirror { uri: uri.to_string(), proof: proof() }, platform: Some(Platform::PopOs), writes: one };
            assert_eq!(plan.validate(), Err(PlanError::SelectedUri(uri.to_string())));
        }
        let ok = Plan { authority: Authority::Mirror { uri: "https://m.example/ubuntu".to_string(), proof: proof() }, platform: Some(Platform::PopOs), writes: vec![w(LIST, Effect::Replace, old())] };
        assert!(ok.validate().is_ok());
        assert!(!ok.is_empty());
    }

    #[test]
    fn validate_refuses_bytes_that_do_not_match_their_digest() {
        let at = LIST.to_string();
        // (a) a digest that belongs to some other bytes.
        let mut other = w(LIST, Effect::Replace, old());
        other.digest = sha256(b"some other content\n");
        assert_eq!(bad(vec![other]), Some(PlanError::BytesMismatch(at.clone())));
        // (b) no digest at all.
        let mut zeroed = w(LIST, Effect::Replace, old());
        zeroed.digest = [0; 32];
        assert_eq!(bad(vec![zeroed]), Some(PlanError::BytesMismatch(at.clone())));
        // (c) the abuse path: a write whose proof and digest are good for A's bytes, then the
        // bytes swapped for B's while A's digest is kept.
        let mut swapped = w(LIST, Effect::Replace, old());
        assert_eq!(bad(vec![swapped.clone()]), None);
        swapped.new_bytes = b"tampered\n".to_vec();
        assert_eq!(bad(vec![swapped]), Some(PlanError::BytesMismatch(at)));
    }

    #[test]
    fn a_restore_plan_is_authorized_by_its_backup_not_a_mirror() {
        let id = crate::manifest::BackupId::parse("20261112T203045Z").unwrap();
        let writes = vec![w(LIST, Effect::Replace, old()), removable()];
        let rebuilt = Plan::for_restore(&crate::restore::validated_backup_for_test(id.clone()), writes)
            .unwrap();
        assert_eq!(rebuilt.authority(), &Authority::Restore { backup: id });
        assert!(!matches!(rebuilt.authority(), Authority::Mirror { .. }));
        // A bare `BackupId` is not a restore authority: `for_restore` takes the token only
        // `restore::build` can mint, so no module can skip the manifest and the disk, and the
        // parameter type is what makes that unrepresentable rather than merely discouraged.
        //
        // A mirror plan is authorized the other way round, and it is the only form carrying a URI.
        let proof_needed = Plan::for_apply(&probe("https://a.example/ubuntu", RFC_A), Platform::PopOs, vec![]);
        assert_eq!(proof_needed, Err(PlanError::WriteCount(0)));
        let mirror = plan(Platform::PopOs, vec![w(LIST, Effect::Replace, old())]);
        let Authority::Mirror { uri, proof } = mirror.authority() else {
            panic!("an apply plan must be authorized by its mirror");
        };
        assert_eq!(uri, "https://m.example/ubuntu");
        assert_eq!(proof.seconds(), fresh::parse_date(RFC_A).unwrap());
    }

    #[test]
    fn a_restore_plan_cannot_create() {
        // `for_restore` leaves the platform unset, so the create rules refuse by construction.
        let created = vec![w(SECURITY, Effect::Create, None)];
        assert_eq!(restore(created), Err(PlanError::CreateNewWrongPlatform(SECURITY.to_string())));
        let list = vec![w(LIST, Effect::Create, None)];
        assert_eq!(restore(list), Err(PlanError::CreateNewOnList));
        // The two effects a restore does use are accepted, and a removal carries no bytes.
        let plan = restore(vec![w(LIST, Effect::Replace, old()), removable()]).unwrap();
        assert_eq!(plan.writes().len(), 2);
        assert_eq!(plan.writes()[1].new_bytes, Vec::new());
        assert_eq!(plan.writes()[1].digest, sha256(&[]));
    }

    #[test]
    fn create_is_refused_off_pop_os() {
        let writes = || vec![w(SECURITY, Effect::Create, None)];
        let probe = probe("https://m.example/ubuntu", RFC_A);
        let ubuntu = Plan::for_apply(&probe, Platform::Ubuntu, writes());
        assert_eq!(ubuntu, Err(PlanError::CreateNewWrongPlatform(SECURITY.to_string())));
        let pop = Plan::for_apply(&probe, Platform::PopOs, writes());
        assert_eq!(pop.map(|p| p.writes().len()), Ok(1));
    }

    #[test]
    fn for_apply_binds_the_uri_to_its_own_proof() {
        let first = probe("https://a.example/ubuntu", RFC_A);
        let second = probe("https://b.example/ubuntu", RFC_B);
        let plan = Plan::for_apply(&first, Platform::PopOs, vec![w(LIST, Effect::Replace, old())]).unwrap();
        let Authority::Mirror { uri, proof } = plan.authority() else {
            panic!("an apply plan must be authorized by its mirror");
        };
        assert_eq!(uri, "https://a.example/ubuntu");
        assert_ne!(uri, &second.url);
        assert_eq!(proof.seconds(), fresh::parse_date(RFC_A).unwrap());
        assert_ne!(proof.seconds(), fresh::parse_date(RFC_B).unwrap());
        assert_eq!(plan.writes().len(), 1);
        let unavailable = SignatureStatus::Unavailable("no gpgv".to_string());
        let unverified = Probed { signature: unavailable, verified_updates: None,
            ..probe("https://a.example/ubuntu", RFC_A) };
        assert_eq!(Plan::for_apply(&unverified, Platform::PopOs, vec![]), Err(PlanError::NoProof));
        // A verified status around a date from outside the signed region yields no proof either;
        // that body is `verify::tests::gpgv_accepts_a_prefixed_body`'s, and it carries no
        // `verified_updates` at all.
        let unsigned = Probed { verified_updates: None, ..probe("https://a.example/ubuntu", RFC_A) };
        assert_eq!(Plan::for_apply(&unsigned, Platform::PopOs, vec![]), Err(PlanError::NoProof));    }
}
