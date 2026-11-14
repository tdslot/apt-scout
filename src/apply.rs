#![cfg_attr(not(test), allow(dead_code))] // the CLI reaches this writer in D11
//! The privileged writer for `/etc/apt`.
//!
//! Nothing here runs unless the process is root, and no path is written until it is bound to the
//! disk entry the plan was built from and held against the run's verified freshness baseline. The
//! order is: validate the plan, check the baseline, capture and bind to the disk, check the
//! ownership a manifest can record, back up, re-validate, sweep this program's own leftovers,
//! write.

use std::fmt;
use std::fs::{self, DirBuilder, File, OpenOptions};
use std::io::{self, Read, Write};
use std::os::unix::fs::{DirBuilderExt, MetadataExt, OpenOptionsExt, PermissionsExt, fchown};
use std::path::{Path, PathBuf};

use crate::digest::sha256;
use crate::fresh;
use crate::manifest::{self, Action, BackupId, ManifestEntry};
use crate::plan::{Authority, Effect, FileState, Plan, PlanError, Target};
use crate::verify::VerifiedDate;

/// The apt configuration root every planned path is relative to.
pub const APT_DIR: &str = "/etc/apt";

/// Where replaced files are backed up before anything is overwritten.
pub const BACKUP_ROOT: &str = "/var/backups/apt-scout";

/// `st_mode` also carries the file-type bits; a recorded mode must never keep them.
const MODE_MASK: u32 = 0o7777;

/// Refuse to read anything larger than this. Apt configuration files are tiny.
const MAX_FILE_BYTES: usize = 1024 * 1024;

/// The mode of the backup root, the run's backup directory and its `sources.list.d`.
const DIR_MODE: u32 = 0o700;

/// Every file this run creates itself: the backup copies start here, the manifest ends here.
const FILE_MODE: u32 = 0o600;

/// The manifest's fixed name inside a backup, and the only thing that makes it a backup.
const MANIFEST: &str = "MANIFEST";

/// A candidate temp name is tried at most this often before the write gives up.
const TEMP_TRIES: u32 = 8;

/// The prefix of the temp file a replacement is written into, in the target's own directory.
const TEMP_PREFIX: &str = ".apt-scout-tmp-";

unsafe extern "C" {
    fn geteuid() -> u32;
}

/// Fail unless the process is already root.
///
/// The write path never re-executes itself and never asks for privileges it was not
/// started with, so the only answer for a non-root run is a refusal.
pub fn require_root() -> Result<(), ApplyError> {
    // SAFETY: geteuid takes no arguments, cannot fail, and dereferences nothing.
    let euid = unsafe { geteuid() };
    if euid == 0 {
        Ok(())
    } else {
        Err(ApplyError::NotRoot)
    }
}

/// Why the writer refused to continue.
#[derive(Debug)]
pub enum ApplyError {
    /// The run is not root, so nothing may be written under `/etc/apt`.
    NotRoot,
    /// The plan this run built does not validate, so no disk operation is reached.
    Plan(PlanError),
    /// The run holds no verified freshness baseline for the mirror it selected.
    NoBaseline,
    /// The selected mirror is older than the verified baseline by more than `MAX_LAG_SECS`.
    Stale { uri: String, lag_secs: i64 },
    /// A planned path could not be inspected, opened, or read.
    Read { path: PathBuf, reason: String },
    /// A planned path is larger than the size apt-scout reads: a cap, never a permission problem.
    TooLarge { path: PathBuf, limit: usize },
    /// A planned path is not a plain regular file apt-scout is willing to own.
    Target { path: PathBuf, reason: String },
    /// A planned path carries ownership a restorable manifest cannot record.
    Owner { path: PathBuf, reason: String },
    /// A planned path no longer matches the state the plan was built from.
    Changed { path: PathBuf, reason: String },
    /// A path that must not exist yet does exist; nothing is ever overwritten on `EEXIST`.
    ///
    /// Three sites, three reasons: a file that appeared where the run creates one, a backup
    /// directory for the same UTC second, and eight taken temp names in one directory.
    Exists { path: PathBuf, reason: String },
    /// The backup of a file could not be written, so nothing under `/etc/apt` may be.
    Backup { path: PathBuf, reason: String },
    /// The new content of a planned file could not be written.
    Write { path: PathBuf, reason: String },
    /// The run's timestamp cannot name a backup directory.
    Time { secs: u64 },
    /// The newest backup cannot be trusted as it is, so the restore reads none of it.
    ///
    /// One variant for the whole reader: the backup root, the directory, the `MANIFEST`, the copies
    /// and the records all fail the same way, and the message names the path and the fact.
    Restore { path: PathBuf, reason: String },
}

impl ApplyError {
    /// The process exit code that reports this refusal.
    pub fn exit_code(&self) -> u8 {
        match self {
            Self::NotRoot => 3,
            Self::Plan(_)
            | Self::NoBaseline
            | Self::Stale { .. }
            | Self::Read { .. }
            | Self::TooLarge { .. }
            | Self::Target { .. }
            | Self::Owner { .. }
            | Self::Changed { .. }
            | Self::Exists { .. }
            | Self::Backup { .. }
            | Self::Write { .. }
            | Self::Restore { .. }
            | Self::Time { .. } => 1,
        }
    }

    /// Add what this run had already changed, for a failure that comes after the first change.
    ///
    /// The write phase can reach three groups of variants. `Write` and `Exists` are its own, and
    /// they never claim a clean system: the creations run first, so a failed replacement can
    /// follow a landed creation. The re-validation before the rename can also raise `Changed`,
    /// `Target`, `Read` or `TooLarge`, whose own messages say nothing was written *to `/etc/apt`*;
    /// that is true while no target has landed, and false afterwards, so past that point the
    /// finding moves into a `Write` refusal. Everything else is unreachable here.
    fn after_writing(self, touched: &[Touched]) -> Self {
        let landed = if touched.is_empty() {
            "No file under /etc/apt had been changed before this failure".to_string()
        } else {
            let names: Vec<String> = touched
                .iter()
                .map(|t| t.target.relative().display().to_string())
                .collect();
            format!(
                "This run had already changed {} before this failure, and those changes stand",
                names.join(", ")
            )
        };
        match self {
            Self::Write { path, reason } => Self::Write {
                path,
                reason: format!("{reason}. {landed}"),
            },
            Self::Exists { path, reason } => Self::Exists {
                path,
                reason: format!("{reason}. {landed}"),
            },
            _ if touched.is_empty() => self,
            Self::Changed { path, reason } => Self::Write {
                path,
                reason: format!("it changed while this run was writing: {reason}. {landed}"),
            },
            Self::Target { path, reason } => Self::Write {
                path,
                reason: format!(
                    "it is no longer a plain regular file apt-scout can replace: {reason}. \
                     {landed}"
                ),
            },
            Self::Read { path, reason } => Self::Write {
                path,
                reason: format!("it can no longer be read: {reason}. {landed}"),
            },
            Self::TooLarge { path, limit } => Self::Write {
                path,
                reason: format!(
                    "it is now larger than the {limit} byte limit apt-scout reads for an apt \
                     source file. {landed}"
                ),
            },
            other => other,
        }
    }

    /// Fold a failed cleanup of a half-written creation into the reason that reports it.
    ///
    /// The caller that removes a partial file at the target path cannot stay silent about a
    /// removal that failed: the error would then describe a clean `/etc/apt` with a partial file
    /// in it. Only a `Write` refusal can reach here, and it already names the target path.
    fn cleanup_failed(self, e: io::Error) -> Self {
        match self {
            Self::Write { path, reason } => Self::Write {
                path,
                reason: format!(
                    "{reason}. Removing the partial file this run created then failed too: {e}, \
                     so a half-written file is still in place at that path"
                ),
            },
            other => other,
        }
    }
}

impl fmt::Display for ApplyError {
    /// What failed, which path, why (the OS error verbatim), and what to do next.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NotRoot => write!(
                f,
                "writing /etc/apt needs root and this run is not root. Re-run it as: sudo apt-scout"
            ),
            Self::Plan(e) => write!(
                f,
                "the write plan does not validate: {e}. Nothing was written; this is a fault in the \
                 plan this run built, not in the files on disk, so re-run apt-scout and report it \
                 if the same refusal comes back"
            ),
            Self::NoBaseline => write!(
                f,
                "this run has no verified freshness baseline for the mirror it selected, so it \
                 cannot tell how old that mirror is. Nothing was written: a write needs a verified \
                 -updates InRelease to hold the mirror against. Re-run apt-scout once a verified \
                 -updates InRelease is reachable; read-only ranking still works meanwhile"
            ),
            Self::Stale { uri, lag_secs } => write!(
                f,
                "the mirror this run selected, {uri}, is {lag_secs} seconds behind the verified \
                 freshness baseline, past the {} second limit at which a mirror counts as stale. \
                 Nothing was written: writing it would replace newer data with older. Re-run \
                 apt-scout so ranking can pick a fresher mirror, or use {uri} once it catches up",
                fresh::MAX_LAG_SECS
            ),
            Self::Read { path, reason } => write!(
                f,
                "cannot read {}: {reason}. Nothing was written to /etc/apt. Check that root can \
                 read it and re-run",
                path.display()
            ),
            Self::TooLarge { path, limit } => write!(
                f,
                "cannot read {}: it is larger than the {limit} byte limit apt-scout reads for an \
                 apt source file. Nothing was written to /etc/apt. The limit is about size and not \
                 about access, so no chmod or chown makes it readable: find what wrote a file this \
                 large to that path and re-run",
                path.display()
            ),
            Self::Target { path, reason } => write!(
                f,
                "refusing {}: {reason}. Nothing was written to /etc/apt. apt-scout only replaces a \
                 plain regular file it can reach without following a link, so move the link or \
                 the special file aside and re-run",
                path.display()
            ),
            Self::Owner { path, reason } => write!(
                f,
                "refusing {}: {reason}. apt-scout will not rewrite a file it cannot describe in a \
                 restorable manifest, because --restore must be able to put the original back. \
                 Nothing was written to /etc/apt. Make that file owned by the user this run writes \
                 as, and re-run",
                path.display()
            ),
            Self::Changed { path, reason } => write!(
                f,
                "{} changed since the plan was built: {reason}. Nothing was written to /etc/apt. \
                 Re-run apt-scout so its plan is built from what is on disk now",
                path.display()
            ),
            // The reason carries this site's own case and its own fix: a file that appeared where
            // the run creates one, taken backup names, and taken temp names are three findings.
            Self::Exists { path, reason } => write!(f, "{}: {reason}", path.display()),
            Self::Backup { path, reason } => write!(
                f,
                "cannot write the backup of {}: {reason}. Nothing was written to /etc/apt, \
                 because apt-scout refuses to change a file it could not back up first. Make sure \
                 {} is a root-owned directory in mode 0700 that root can write to, then re-run",
                path.display(),
                BACKUP_ROOT
            ),
            Self::Write { path, reason } => write!(
                f,
                "cannot write {}: {reason}. Check that the directory holding it is writable by \
                 the user this run writes as, and re-run",
                path.display()
            ),
            Self::Time { secs } => write!(
                f,
                "cannot name a backup from {secs} seconds after the epoch: that is past year 9999. \
                 Nothing was written. Check the system clock and re-run"
            ),
            Self::Restore { path, reason } => write!(
                f,
                "refusing to restore {}: {reason}. Nothing was written to /etc/apt. apt-scout \
                 restores nothing rather than guessing, because the manifest is the only source of \
                 truth for a restore: check that backup directory and re-run, or put the file back \
                 by hand from another copy",
                path.display()
            ),
        }
    }
}

/// What one successful run did: the backup it made and the files it changed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Outcome {
    /// The backup every file this run replaced or removed was copied into, before the first
    /// write. A created file has nothing to copy, so it is not in there.
    pub backup_id: BackupId,
    /// Every target this run changed, in the order it changed them. A restore undoes a creation by
    /// removing the file, so the name says what the run did and not just what it wrote.
    pub touched: Vec<Touched>,
}

/// One target a run changed, and which of the three effects it applied to it.
///
/// The effect travels beside the target so a caller can say `wrote` for a creation or a replacement
/// and `removed` for a removal, instead of describing an unlink as a write.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Touched {
    pub target: Target,
    pub effect: Effect,
}

/// Write the plan, as root, to the real `/etc/apt` and `/var/backups/apt-scout`.
///
/// Refuses before its first byte on a non-root run, on a plan that does not validate, on a mirror
/// plan whose verified baseline is missing or does not cover the selected mirror, on a target it
/// cannot describe in a restorable manifest, and on any difference between the plan and the disk.
/// One entry point for both forms: what authorizes the write is the plan's own `Authority`, so a
/// restore plan needs no baseline and gets no second code path.
pub fn apply(
    plan: &Plan,
    baseline: Option<&VerifiedDate>,
    now_secs: u64,
) -> Result<Outcome, ApplyError> {
    require_root()?;
    run(
        Path::new(APT_DIR),
        Path::new(BACKUP_ROOT),
        plan,
        baseline,
        now_secs,
        0,
        0,
    )
}

/// The one write path, with the two roots, the baseline and the run's owner as parameters.
///
/// Private: it is the only place where the run's owner is not `0`, so the "every file and backup
/// belongs to this run's owner" rule cannot be reached from production code with any other value.
/// `0, 0` is a claim about a root run that `create_one` proves against the file it created;
/// `getegid` is deliberately absent, so it cannot be checked at the door.
fn run(
    apt_root: &Path,
    backup_root: &Path,
    plan: &Plan,
    baseline: Option<&VerifiedDate>,
    now_secs: u64,
    owner_uid: u32,
    owner_gid: u32,
) -> Result<Outcome, ApplyError> {
    plan.validate().map_err(ApplyError::Plan)?;
    check_freshness(plan, baseline)?;
    let captured = capture(apt_root, plan)?;
    check_ownership(&captured, owner_uid, owner_gid)?;
    let id = BackupId::from_secs(now_secs).map_err(|_| ApplyError::Time { secs: now_secs })?;
    backup(backup_root, owner_uid, owner_gid, &id, &captured)?;
    validate_targets(&captured)?;
    // S6's pause: present only in a harness build, at the last point before the sweep, where every
    // refusal except a TOCTOU one is behind us. It reads no environment and no path of its own.
    #[cfg(apt_scout_write_harness)]
    crate::harness::pause_after_validation();
    sweep_targets(&captured, owner_uid)?;
    let touched = write_targets(&captured, owner_uid, owner_gid)?;
    Ok(Outcome {
        backup_id: id,
        touched,
    })
}

/// Hold the selected mirror's verified proof against the baseline this run verified earlier.
///
/// The write performs no network request, so it cannot go looking for a baseline: a run without
/// one is refused, and a mirror behind the baseline by more than `fresh::MAX_LAG_SECS` is refused
/// through the same predicate the pipeline uses to call a mirror stale. A mirror *ahead* of the
/// baseline is accepted: clock skew on a mirror cannot be told apart from a fresher mirror.
fn check_freshness(plan: &Plan, baseline: Option<&VerifiedDate>) -> Result<(), ApplyError> {
    // Only a mirror plan is held against a baseline. A restore plan has no mirror, so it has no
    // freshness dimension: what holds its bytes is the manifest re-derivation in `restore::build`,
    // under the backup the plan names.
    let (uri, proof) = match plan.authority() {
        Authority::Mirror { uri, proof } => (uri, *proof),
        Authority::Restore { .. } => return Ok(()),
    };
    let Some(baseline) = baseline else {
        return Err(ApplyError::NoBaseline);
    };
    let lag_secs = fresh::lag(baseline.seconds(), proof.seconds());
    if fresh::is_stale(lag_secs) {
        return Err(ApplyError::Stale {
            uri: uri.clone(),
            lag_secs,
        });
    }
    Ok(())
}

/// Refuse any target whose ownership a restorable manifest cannot record.
///
/// `manifest::parse` accepts only uid 0 and gid 0, so a file owned by anyone else would leave a
/// backup that `--restore` refuses outright. Refused before the backup and before the write: a
/// backup whose record the parser rejects is not a backup.
fn check_ownership(
    captured: &[Captured],
    owner_uid: u32,
    owner_gid: u32,
) -> Result<(), ApplyError> {
    for c in captured {
        let Some(original) = c.original.as_ref() else {
            continue;
        };
        if original.uid != owner_uid || original.gid != owner_gid {
            return Err(ApplyError::Owner {
                path: c.path.clone(),
                reason: format!(
                    "it is owned by uid {} gid {}, and the only ownership apt-scout can record in \
                     a manifest is uid {owner_uid} gid {owner_gid}",
                    original.uid, original.gid
                ),
            });
        }
    }
    Ok(())
}

/// A planned file paired with the state the plan expected to find on disk.
///
/// Self-contained: every byte the write needs is here, so no step indexes back into the plan.
struct Captured {
    target: Target,
    path: PathBuf,
    /// `None` means the plan creates this file.
    original: Option<FileState>,
    new_bytes: Vec<u8>,
    effect: Effect,
}

/// What the disk currently holds at one path.
pub(crate) enum Entry {
    Missing,
    Regular(FileState),
    NotRegular(String),
}

/// Resolve one planned target against the apt root it is relative to.
pub(crate) fn resolve(apt_root: &Path, target: &Target) -> PathBuf {
    target.from_under(apt_root)
}

/// Read every planned path and bind it to the state the plan was built from.
///
/// A plan built from a stale read, or from a read of a different machine, is refused
/// here rather than written.
fn capture(apt_root: &Path, plan: &Plan) -> Result<Vec<Captured>, ApplyError> {
    let mut captured = Vec::new();
    for w in plan.writes() {
        let path = resolve(apt_root, &w.target);
        check_parent(&path)?;
        match read_state(&path)? {
            Entry::Regular(state) => {
                if w.original.as_ref() != Some(&state) {
                    return Err(ApplyError::Changed {
                        path,
                        reason: "the file on disk is not the file the plan was built from".into(),
                    });
                }
            }
            Entry::Missing => {
                if w.original.is_some() {
                    return Err(ApplyError::Changed {
                        path,
                        reason: "the file is gone".into(),
                    });
                }
            }
            Entry::NotRegular(reason) => return Err(ApplyError::Target { path, reason }),
        }
        captured.push(Captured {
            target: w.target.clone(),
            path,
            original: w.original.clone(),
            new_bytes: w.new_bytes.clone(),
            effect: w.effect,
        });
    }
    Ok(captured)
}

/// Refuse a target whose directory is a symlink or not a directory.
///
/// `lstat` of a path does not follow its last component, so a symlinked `sources.list.d` would
/// otherwise be resolved by the kernel when the target is opened or renamed into.
pub(crate) fn check_parent(path: &Path) -> Result<(), ApplyError> {
    let parent = parent_dir(path)?;
    match fs::symlink_metadata(parent) {
        Ok(meta) if meta.file_type().is_symlink() => Err(ApplyError::Target {
            path: parent.to_path_buf(),
            reason: "it is a symlink, and apt-scout never follows one".into(),
        }),
        Ok(meta) if !meta.is_dir() => Err(ApplyError::Target {
            path: parent.to_path_buf(),
            reason: "it is not a directory".into(),
        }),
        Ok(_) => Ok(()),
        // A missing directory is reported by the write itself, with the OS error naming it.
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(read_failed(parent, e)),
    }
}

/// The directory a path is written into.
fn parent_dir(path: &Path) -> Result<&Path, ApplyError> {
    path.parent().ok_or_else(|| ApplyError::Target {
        path: path.to_path_buf(),
        reason: "it has no directory to be written into".into(),
    })
}

/// Re-check every captured path, in the order the plan will write them.
fn validate_targets(captured: &[Captured]) -> Result<(), ApplyError> {
    captured.iter().try_for_each(check_one)
}

/// Re-read one captured path and confirm it still matches the plan.
fn check_one(c: &Captured) -> Result<(), ApplyError> {
    check_parent(&c.path)?;
    let entry = read_state(&c.path)?;
    match (&c.original, entry) {
        (None, Entry::Missing) => Ok(()),
        (Some(expected), Entry::Regular(state)) => {
            if state == *expected {
                Ok(())
            } else {
                Err(ApplyError::Changed {
                    path: c.path.clone(),
                    reason: difference(expected, &state),
                })
            }
        }
        // The plan expected nothing here, so anything on disk is a change.
        (None, Entry::Regular(_)) => Err(ApplyError::Changed {
            path: c.path.clone(),
            reason: "the file appeared after the plan was built".into(),
        }),
        (Some(_), Entry::Missing) => Err(ApplyError::Changed {
            path: c.path.clone(),
            reason: "the file is gone".into(),
        }),
        (None, Entry::NotRegular(reason)) | (Some(_), Entry::NotRegular(reason)) => {
            Err(ApplyError::Target {
                path: c.path.clone(),
                reason,
            })
        }
    }
}

/// Name the first field that differs, in the order the writer cares about it.
fn difference(expected: &FileState, found: &FileState) -> String {
    if expected.dev != found.dev || expected.ino != found.ino {
        "the device or inode changed".into()
    } else if expected.bytes != found.bytes {
        "the bytes changed".into()
    } else {
        "the mode or owner changed".into()
    }
}

/// Copy every file the plan replaces into a fresh backup directory, then write its manifest.
///
/// A failure here aborts the run before the first write, so a backup is never partial in a way the
/// caller cannot see: the manifest is written last, and a directory without one is not a backup.
fn backup(
    backup_root: &Path,
    owner_uid: u32,
    owner_gid: u32,
    id: &BackupId,
    captured: &[Captured],
) -> Result<(), ApplyError> {
    verify_backup_root(backup_root, owner_uid)?;
    let dir = backup_root.join(id.as_str());
    DirBuilder::new().mode(DIR_MODE).create(&dir).map_err(|e| {
        if e.kind() == io::ErrorKind::AlreadyExists {
            // Two runs inside one UTC second: refusing beats overwriting a backup. The directory
            // is left exactly as the earlier run made it, so this message never suggests acting
            // on it, and only the seconds of the run's own clock can clear the collision.
            ApplyError::Exists {
                path: dir.clone(),
                reason: "a backup directory for this UTC second already exists, and apt-scout \
                         neither reuses nor overwrites a backup an earlier run made. Re-run \
                         apt-scout in a later second; nothing under /etc/apt had been written \
                         before this failure"
                    .into(),
            }
        } else {
            backup_failed(&dir, e)
        }
    })?;
    // The umask can only narrow what `DirBuilder` asked for, so 0700 is re-applied here exactly as
    // the backup root does it; otherwise a narrowed directory would later be reported as an apt
    // permission problem when the cause is the umask.
    fs::set_permissions(&dir, fs::Permissions::from_mode(DIR_MODE))
        .map_err(|e| backup_failed(&dir, e))?;
    for c in captured.iter().filter(|c| c.original.is_some()) {
        copy_original(&dir, c)?;
    }
    // The record carries the run's own identity, never a directory's gid: the parent of a backup
    // copy and the parent of a created target can inherit different gids.
    let entries = manifest_entries(captured, owner_uid, owner_gid);
    let text = manifest::render(id, &entries);
    write_manifest(&dir, owner_uid, owner_gid, &text)?;
    sync_dir(&dir, backup_failed)?;
    sync_dir(backup_root, backup_failed)
}

/// Create the backup root if it is missing, and refuse anything that is not a root-owned 0700
/// directory.
///
/// This is the one self-healing step the writer performs, and it verifies its own result: a
/// pre-created path that is a symlink, a file, owned by someone else or world-writable is a hard
/// abort with nothing written.
fn verify_backup_root(root: &Path, owner_uid: u32) -> Result<(), ApplyError> {
    let created = match fs::symlink_metadata(root) {
        Ok(_) => false,
        Err(e) if e.kind() == io::ErrorKind::NotFound => {
            DirBuilder::new()
                .mode(DIR_MODE)
                .create(root)
                .map_err(|e| backup_failed(root, e))?;
            true
        }
        Err(e) => return Err(backup_failed(root, e)),
    };
    // The umask can only narrow the mode DirBuilder asked for, and a directory this run created
    // must end up exactly as the restart and restore rules expect to find it.
    if created {
        fs::set_permissions(root, fs::Permissions::from_mode(DIR_MODE))
            .map_err(|e| backup_failed(root, e))?;
    }
    let meta = fs::symlink_metadata(root).map_err(|e| backup_failed(root, e))?;
    if meta.file_type().is_symlink() {
        return Err(ApplyError::Backup {
            path: root.to_path_buf(),
            reason: "it is a symlink, and apt-scout never follows one".into(),
        });
    }
    if !meta.is_dir() {
        return Err(ApplyError::Backup {
            path: root.to_path_buf(),
            reason: "it is not a directory".into(),
        });
    }
    if meta.uid() != owner_uid {
        return Err(ApplyError::Backup {
            path: root.to_path_buf(),
            reason: format!("it is owned by uid {}, not uid {owner_uid}", meta.uid()),
        });
    }
    let mode = meta.mode() & MODE_MASK;
    if mode != DIR_MODE {
        return Err(ApplyError::Backup {
            path: root.to_path_buf(),
            reason: format!("its mode is {mode:#o}, not {DIR_MODE:#o}"),
        });
    }
    Ok(())
}

/// Copy one replaced file into the backup at its own relative path, with its mode and owner.
fn copy_original(dir: &Path, c: &Captured) -> Result<(), ApplyError> {
    let Some(original) = c.original.as_ref() else {
        return Ok(());
    };
    let dest = dir.join(c.target.relative());
    let parent = parent_dir(&dest)?;
    if parent != dir {
        match DirBuilder::new().mode(DIR_MODE).create(parent) {
            Ok(()) => {
                // The umask can narrow this directory too, and a narrowed one would be reported
                // later as an apt permission problem.
                fs::set_permissions(parent, fs::Permissions::from_mode(DIR_MODE))
                    .map_err(|e| backup_failed(parent, e))?;
            }
            // The directory is inside a directory this run just created, so it is only ever the
            // `sources.list.d` of a second copy.
            Err(e) if e.kind() == io::ErrorKind::AlreadyExists => {}
            Err(e) => return Err(backup_failed(parent, e)),
        }
    }
    let file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(FILE_MODE)
        .open(&dest)
        .map_err(|e| backup_failed(&dest, e))?;
    let mut handle = &file;
    handle
        .write_all(&original.bytes)
        .map_err(|e| backup_failed(&dest, e))?;
    // Owner before mode: chown clears the set-user-id and set-group-id bits.
    fchown(&file, Some(original.uid), Some(original.gid)).map_err(|e| backup_failed(&dest, e))?;
    file.set_permissions(fs::Permissions::from_mode(original.mode))
        .map_err(|e| backup_failed(&dest, e))?;
    file.sync_all().map_err(|e| backup_failed(&dest, e))?;
    sync_dir(parent, backup_failed)
}

/// The manifest records: a replaced file is described by its copy, a created one by what is created.
fn manifest_entries(captured: &[Captured], owner_uid: u32, owner_gid: u32) -> Vec<ManifestEntry> {
    captured
        .iter()
        .map(|c| match &c.original {
            Some(original) => ManifestEntry {
                target: c.target.clone(),
                action: Action::Replace,
                digest: sha256(&original.bytes),
                mode: original.mode,
                uid: original.uid,
                gid: original.gid,
            },
            // A created file has no copy, so its record carries what this run creates it with.
            None => ManifestEntry {
                target: c.target.clone(),
                action: Action::Create,
                digest: sha256(&c.new_bytes),
                mode: FILE_MODE,
                uid: owner_uid,
                gid: owner_gid,
            },
        })
        .collect()
}

/// Write the manifest with an exclusive create, and fsync it: an unfsynced manifest is no backup.
fn write_manifest(
    dir: &Path,
    owner_uid: u32,
    owner_gid: u32,
    text: &str,
) -> Result<(), ApplyError> {
    let path = dir.join(MANIFEST);
    let file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(FILE_MODE)
        .open(&path)
        .map_err(|e| backup_failed(&path, e))?;
    let mut handle = &file;
    handle
        .write_all(text.as_bytes())
        .map_err(|e| backup_failed(&path, e))?;
    fchown(&file, Some(owner_uid), Some(owner_gid)).map_err(|e| backup_failed(&path, e))?;
    file.set_permissions(fs::Permissions::from_mode(FILE_MODE))
        .map_err(|e| backup_failed(&path, e))?;
    file.sync_all().map_err(|e| backup_failed(&path, e))
}

/// Sweep every directory the run is about to write into, once each.
///
/// Only the plan's own target directories: both derive from the closed target enumeration, never
/// from input.
fn sweep_targets(captured: &[Captured], owner_uid: u32) -> Result<(), ApplyError> {
    let mut dirs: Vec<PathBuf> = Vec::new();
    for c in captured {
        let dir = parent_dir(&c.path)?.to_path_buf();
        if !dirs.contains(&dir) {
            dirs.push(dir);
        }
    }
    dirs.iter().try_for_each(|dir| sweep_temp(dir, owner_uid))
}

/// Remove this program's own leftover temp files from one directory.
///
/// `SPEC.md` promises that an interrupted write's temp files are cleaned up on the next run, and a
/// name of ours left in `/etc/apt` would falsify this module's "no half-written files". Four
/// conditions bound what a privileged `unlink` may be steered to by a name: this program's prefix,
/// a regular file, the run's own uid, and at most `TEMP_TRIES` names per directory. An entry that
/// fails one of them is left alone and never reused.
///
/// A directory that cannot be listed is reported as a read refusal that names that directory as
/// itself: this step is a sweep of the run's own leftovers, not a write into the directory, and the
/// fix for a listing that fails is not "make the directory holding it writable".
fn sweep_temp(dir: &Path, owner_uid: u32) -> Result<(), ApplyError> {
    let entries = match fs::read_dir(dir) {
        Ok(entries) => entries,
        // A missing directory holds no leftovers of ours; the write reports it if it cannot write
        // there either.
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(e) => return Err(read_failed(dir, e)),
    };
    let mut considered = 0;
    for entry in entries {
        if considered == TEMP_TRIES {
            break;
        }
        let entry = entry.map_err(|e| read_failed(dir, e))?;
        let name = entry.file_name();
        // A name that is not UTF-8 cannot be one this program wrote.
        let Some(name) = name.to_str() else {
            continue;
        };
        if !name.starts_with(TEMP_PREFIX) {
            continue;
        }
        considered += 1;
        let path = entry.path();
        let meta = fs::symlink_metadata(&path).map_err(|e| write_failed(&path, e))?;
        // Never a symlink, never a directory, never a device: the name alone may not point the
        // removal anywhere, and a file this run did not create is not this program's leftover.
        if !meta.file_type().is_file() || meta.uid() != owner_uid {
            continue;
        }
        fs::remove_file(&path).map_err(|e| write_failed(&path, e))?;
    }
    Ok(())
}

/// Write every captured target: the creations first, the replacements next, the removals last.
///
/// The order is the security property. A creation that finds the path taken aborts the whole run
/// while every other target is still untouched, so creations run first. Removals run last because a
/// removal is the only operation that destroys a path: every replaceable byte has landed by then,
/// so a later failure can never leave a file destroyed while the content that was to replace it did
/// not arrive. Every failure here is reported through `after_writing`, because from the first
/// landed creation on, "nothing was written" is false.
///
/// Each target's directory is flushed here and not inside `create_one`/`replace_one`/`remove_one`,
/// because the flush comes *after* the change has landed: the push onto `touched` is between the
/// two, so a flush that fails is reported as a failure after a landed change, and never as a clean
/// system.
fn write_targets(
    captured: &[Captured],
    owner_uid: u32,
    owner_gid: u32,
) -> Result<Vec<Touched>, ApplyError> {
    let mut touched = Vec::with_capacity(captured.len());
    for c in captured.iter().filter(|c| c.effect == Effect::Create) {
        create_one(c, owner_uid, owner_gid).map_err(|e| e.after_writing(&touched))?;
        touched.push(landed(c));
        sync_touched_dir(c, &touched)?;
    }
    for c in captured.iter().filter(|c| c.effect == Effect::Replace) {
        replace_one(c).map_err(|e| e.after_writing(&touched))?;
        touched.push(landed(c));
        sync_touched_dir(c, &touched)?;
    }
    for c in captured.iter().filter(|c| c.effect == Effect::Remove) {
        remove_one(c).map_err(|e| e.after_writing(&touched))?;
        touched.push(landed(c));
        sync_touched_dir(c, &touched)?;
    }
    Ok(touched)
}

/// One landed target, as the record of what this run changed.
fn landed(c: &Captured) -> Touched {
    Touched {
        target: c.target.clone(),
        effect: c.effect,
    }
}

/// Remove one file, after proving it is still the file the plan was built from.
///
/// The re-check is the same last check `replace_one` runs before its rename, and it is not
/// decoration: `unlink` by path cannot compare dev and ino itself, so the only way to refuse a swap
/// between validation and the unlink is to look immediately before it. The gap that leaves is the
/// same one `rename` already accepts, and it needs write access to a root-owned directory.
fn remove_one(c: &Captured) -> Result<(), ApplyError> {
    check_one(c)?;
    fs::remove_file(&c.path).map_err(|e| write_failed(&c.path, e))
}

/// Flush the directory a landed target was written into, so its new name survives a crash.
///
/// Called only after the target is on `touched`: the bytes are on disk by the time this runs, so
/// every failure here is a failure *after* a change and carries what the run had already touched.
fn sync_touched_dir(c: &Captured, touched: &[Touched]) -> Result<(), ApplyError> {
    let dir = parent_dir(&c.path).map_err(|e| e.after_writing(touched))?;
    sync_dir(dir, write_failed).map_err(|e| e.after_writing(touched))
}

/// Create a planned file with an exclusive create, and never overwrite what is there.
fn create_one(c: &Captured, owner_uid: u32, owner_gid: u32) -> Result<(), ApplyError> {
    let file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(FILE_MODE)
        .open(&c.path)
        .map_err(|e| match e.kind() {
            io::ErrorKind::AlreadyExists => {
                // The file this run planned to create appeared in the meantime; its own path is
                // the only thing the reader can act on.
                ApplyError::Exists {
                    path: c.path.clone(),
                    reason: "it was free when this run's plan was built from the disk and is \
                             taken now, and apt-scout never overwrites a path it did not plan to \
                             replace. Remove or rename that path and re-run"
                        .into(),
                }
            }
            _ => write_failed(&c.path, e),
        })?;
    if let Err(e) = fill_creation(&file, c, owner_uid, owner_gid) {
        // A failed creation must not leave a partial file at the target path, and a cleanup that
        // itself fails must not be swallowed: the file would still be there while the error reads
        // as if nothing had been created.
        return Err(match fs::remove_file(&c.path) {
            Ok(()) => e,
            Err(cleanup) => e.cleanup_failed(cleanup),
        });
    }
    Ok(())
}

/// Fill a created file and prove it is exactly what the manifest already recorded for it.
///
/// The record is written before the file exists, so the claim is checked here, at the point of
/// use: a file the run cannot describe is removed and the run fails with `Write`.
fn fill_creation(
    file: &File,
    c: &Captured,
    owner_uid: u32,
    owner_gid: u32,
) -> Result<(), ApplyError> {
    let mut handle = file;
    handle
        .write_all(&c.new_bytes)
        .map_err(|e| write_failed(&c.path, e))?;
    // The manifest records mode 0600, so the file must carry it whatever the umask is.
    file.set_permissions(fs::Permissions::from_mode(FILE_MODE))
        .map_err(|e| write_failed(&c.path, e))?;
    file.sync_all().map_err(|e| write_failed(&c.path, e))?;
    let meta = file.metadata().map_err(|e| write_failed(&c.path, e))?;
    let (mode, uid, gid) = (meta.mode() & MODE_MASK, meta.uid(), meta.gid());
    if mode != FILE_MODE || uid != owner_uid || gid != owner_gid {
        return Err(ApplyError::Write {
            path: c.path.clone(),
            reason: format!(
                "the manifest already records mode {FILE_MODE:#o} uid {owner_uid} gid {owner_gid} \
                 for this file, and the file this run created is mode {mode:#o} uid {uid} gid \
                 {gid}, so the record does not describe what was created"
            ),
        });
    }
    Ok(())
}

/// Replace one file through a temp file in its own directory, the mode and owner it already had.
fn replace_one(c: &Captured) -> Result<(), ApplyError> {
    let original = c.original.as_ref().ok_or_else(|| ApplyError::Changed {
        path: c.path.clone(),
        reason: "the plan replaces a file that is not on disk".into(),
    })?;
    let dir = parent_dir(&c.path)?;
    let (file, tmp) = create_temp(dir)?;
    if let Err(e) = fill_replacement(&file, original, c) {
        let _ = fs::remove_file(&tmp);
        return Err(e);
    }
    // The last check the rename can still be refused by: the disk must still be the plan's disk.
    if let Err(e) = check_one(c) {
        let _ = fs::remove_file(&tmp);
        return Err(e);
    }
    if let Err(e) = fs::rename(&tmp, &c.path) {
        let _ = fs::remove_file(&tmp);
        return Err(write_failed(&c.path, e));
    }
    Ok(())
}

/// Open a candidate temp file exclusively; a leftover from an earlier run is never reused.
fn create_temp(dir: &Path) -> Result<(File, PathBuf), ApplyError> {
    let mut leftover = PathBuf::new();
    for i in 0..TEMP_TRIES {
        let path = dir.join(format!("{TEMP_PREFIX}{}-{i}", std::process::id()));
        match OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(FILE_MODE)
            .open(&path)
        {
            Ok(file) => return Ok((file, path)),
            Err(e) if e.kind() == io::ErrorKind::AlreadyExists => leftover = path,
            Err(e) => return Err(write_failed(&path, e)),
        }
    }
    // Every candidate is taken by a name this program chose, so the fix is about those names and
    // about nothing else in the directory.
    Err(ApplyError::Exists {
        path: leftover,
        reason: format!(
            "all {TEMP_TRIES} candidate temp names in this directory are taken, and apt-scout \
             never reuses a leftover temp file as content. Remove the {TEMP_PREFIX}* files in that \
             directory and re-run"
        ),
    })
}

/// Fill the temp file: the new bytes, then the original's owner, then its mode.
fn fill_replacement(file: &File, original: &FileState, c: &Captured) -> Result<(), ApplyError> {
    // Writing first: a write and a chown both clear the set-user-id and set-group-id bits, so
    // both precede the chmod that restores them.
    let mut handle = file;
    handle
        .write_all(&c.new_bytes)
        .map_err(|e| write_failed(&c.path, e))?;
    fchown(file, Some(original.uid), Some(original.gid)).map_err(|e| write_failed(&c.path, e))?;
    file.set_permissions(fs::Permissions::from_mode(original.mode))
        .map_err(|e| write_failed(&c.path, e))?;
    file.sync_all().map_err(|e| write_failed(&c.path, e))
}

/// Run the one write path with fixture roots and the calling user's identity. Test-only.
///
/// Production has exactly one caller of `run`, `apply`, which proves the process is root first;
/// a unit test is not root and never will be, so the restore round trip reaches the same writer
/// through here rather than through a second entry point in shipped code.
#[cfg(test)]
pub(crate) fn run_for_test(
    apt_root: &Path,
    backup_root: &Path,
    plan: &Plan,
    baseline: Option<&VerifiedDate>,
    now_secs: u64,
    owner_uid: u32,
    owner_gid: u32,
) -> Result<Outcome, ApplyError> {
    run(
        apt_root,
        backup_root,
        plan,
        baseline,
        now_secs,
        owner_uid,
        owner_gid,
    )
}

/// Inspect one path without following a symlink and without reading it twice.
///
/// The state comes from the open handle, not from the `lstat`, so a swap between the
/// two cannot smuggle a different file into the write.
pub(crate) fn read_state(path: &Path) -> Result<Entry, ApplyError> {
    let link = match fs::symlink_metadata(path) {
        Ok(meta) => meta,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(Entry::Missing),
        Err(e) => return Err(read_failed(path, e)),
    };

    if link.file_type().is_symlink() {
        return Ok(Entry::NotRegular(
            "it is a symlink, and apt-scout never follows one".into(),
        ));
    }
    if !link.file_type().is_file() {
        return Ok(Entry::NotRegular("it is not a regular file".into()));
    }

    let file = File::open(path).map_err(|e| read_failed(path, e))?;
    let meta = file.metadata().map_err(|e| read_failed(path, e))?;
    if meta.dev() != link.dev() || meta.ino() != link.ino() {
        return Err(ApplyError::Changed {
            path: path.to_path_buf(),
            reason: "the device or inode changed between the check and the open".into(),
        });
    }

    let mut bytes = Vec::new();
    let mut limited = file.take(MAX_FILE_BYTES as u64 + 1);
    limited
        .read_to_end(&mut bytes)
        .map_err(|e| read_failed(path, e))?;
    if bytes.len() > MAX_FILE_BYTES {
        return Err(ApplyError::TooLarge {
            path: path.to_path_buf(),
            limit: MAX_FILE_BYTES,
        });
    }

    Ok(Entry::Regular(FileState {
        dev: meta.dev(),
        ino: meta.ino(),
        mode: meta.mode() & MODE_MASK,
        uid: meta.uid(),
        gid: meta.gid(),
        bytes,
    }))
}

#[cfg(test)]
thread_local! {
    /// Armed by a test to fail the very next `sync_dir` call, once.
    ///
    /// The failure this simulates is the last one a landed write can still hit, and no non-root
    /// test can build it: a directory the kernel refuses to flush is a device error. Present only
    /// in a test build, so a release binary carries no switch here.
    static SYNC_DIR_FAULT: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

/// Arm the one-shot directory-sync fault. Test-only.
#[cfg(test)]
fn fail_next_sync_dir() {
    SYNC_DIR_FAULT.with(|armed| armed.set(true));
}

/// The injected flush failure, named so a test can tell which path it came from. Test-only.
#[cfg(test)]
fn injected_sync_failure() -> io::Error {
    io::Error::other("injected directory sync failure")
}

/// Flush one directory, so a name it gained or lost survives a crash.
fn sync_dir(dir: &Path, fail: fn(&Path, io::Error) -> ApplyError) -> Result<(), ApplyError> {
    // Test-only, and consumed here so it can fire at most once per arming.
    #[cfg(test)]
    if SYNC_DIR_FAULT.with(|armed| armed.replace(false)) {
        return Err(fail(dir, injected_sync_failure()));
    }
    File::open(dir)
        .and_then(|handle| handle.sync_all())
        .map_err(|e| fail(dir, e))
}

/// Wrap an IO failure as a read refusal that names the path.
fn read_failed(path: &Path, e: io::Error) -> ApplyError {
    ApplyError::Read {
        path: path.to_path_buf(),
        reason: e.to_string(),
    }
}

/// Wrap an IO failure as a write refusal that names the path.
fn write_failed(path: &Path, e: io::Error) -> ApplyError {
    ApplyError::Write {
        path: path.to_path_buf(),
        reason: e.to_string(),
    }
}

/// Wrap an IO failure as a backup refusal that names the path.
fn backup_failed(path: &Path, e: io::Error) -> ApplyError {
    ApplyError::Backup {
        path: path.to_path_buf(),
        reason: e.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::digest::hex;
    use crate::fresh::{self, Probed, SignedDate};
    use crate::manifest::ManifestError;
    use crate::plan::{PlannedWrite, Platform};
    use crate::verify::SignatureStatus;
    use std::os::unix::fs::symlink;

    const LIST: &str = "sources.list";
    const SECURITY: &str = "sources.list.d/system-security.sources";
    const RFC: &str = "Thu, 12 Nov 2026  4:23:38 UTC";
    /// One second past `RFC` plus `MAX_LAG_SECS`: the first instant a mirror counts as stale.
    const LATE: &str = "Fri, 13 Nov 2026  4:23:39 UTC";
    /// An hour before `RFC`: the mirror is ahead of this baseline, which is not staleness.
    const EARLY: &str = "Thu, 12 Nov 2026  3:23:38 UTC";
    const ARM: &str = "-----BEGIN PGP SIGNED MESSAGE-----";
    /// 1794515445 is `20261112T203045Z`, the id every test run below is dated with.
    const NOW: u64 = 1_794_515_445;
    const ID: &str = "20261112T203045Z";
    const OLD: &[u8] = b"deb http://archive.ubuntu.com/ubuntu noble main\n";

    /// A fixture root under the temp directory, removed however the test ends.
    struct TempRoot(PathBuf);

    impl TempRoot {
        fn new(tag: &str) -> TempRoot {
            let path =
                std::env::temp_dir().join(format!("apt-scout-apply-{tag}-{}", std::process::id()));
            let _ = fs::remove_dir_all(&path);
            fs::create_dir_all(&path).expect("temp root");
            TempRoot(path)
        }

        fn path(&self) -> &Path {
            &self.0
        }

        /// The uid and gid of the caller: every fixture is its own file, so the writer is handed
        /// this identity as the run's owner and the tests never need root.
        fn owner(&self) -> (u32, u32) {
            let meta = fs::metadata(&self.0).expect("temp root metadata");
            (meta.uid(), meta.gid())
        }

        /// A directory inside this root, so a fixture tree is never the temp root itself.
        fn apt(&self) -> PathBuf {
            let apt = self.0.join("apt");
            fs::create_dir(&apt).expect("apt root");
            apt
        }
    }

    impl Drop for TempRoot {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn signed(rfc: &str) -> SignedDate {
        fresh::release_date(&format!("{ARM}\nHash: SHA512\n\nDate: {rfc}\n")).expect("signed date")
    }

    /// A verified date for `rfc`: minted the way the writer gets one, by the verifier's in-region read.
    fn verified(rfc: &str) -> VerifiedDate {
        crate::verify::verified_date_for_test(&format!("{ARM}\nHash: SHA512\n\nDate: {rfc}\n"))
            .expect("a verified date")
    }

    /// The run's baseline: the same instant the probe's proof carries.
    fn baseline() -> VerifiedDate {
        verified(RFC)
    }

    fn probe() -> Probed {
        let date = signed(RFC);
        Probed {
            url: "https://m.example/ubuntu".to_string(),
            updates_date: date.seconds(),
            verified_updates: Some(verified(RFC)),
            lag_secs: 0,
            throughput_bps: 0,
            latency_ms: 0,
            same_as: false,
            signature: SignatureStatus::verified_for_test(),
        }
    }

    fn plan(writes: Vec<PlannedWrite>) -> Plan {
        Plan::for_apply(&probe(), Platform::PopOs, writes).expect("plan for apply")
    }

    /// The state of a fixture file, read without going through the module under test.
    fn state(path: &Path) -> FileState {
        let meta = fs::symlink_metadata(path).expect("fixture metadata");
        FileState {
            dev: meta.dev(),
            ino: meta.ino(),
            mode: meta.mode() & MODE_MASK,
            uid: meta.uid(),
            gid: meta.gid(),
            bytes: fs::read(path).expect("fixture bytes"),
        }
    }

    fn replace(target: &str, original: Option<FileState>) -> PlannedWrite {
        let new_bytes = b"deb https://m.example/ubuntu noble main\n".to_vec();
        let digest = sha256(&new_bytes);
        PlannedWrite {
            target: Target::parse(target).expect("target"),
            new_bytes,
            digest,
            original,
            effect: Effect::Replace,
        }
    }

    fn create(target: &str) -> PlannedWrite {
        let new_bytes =
            b"Types: deb\nURIs: https://m.example/ubuntu\nSuites: noble-security\n".to_vec();
        let digest = sha256(&new_bytes);
        PlannedWrite {
            target: Target::parse(target).expect("target"),
            new_bytes,
            digest,
            original: None,
            effect: Effect::Create,
        }
    }

    /// A removal: the file a `create` record describes, on disk and due to go.
    fn remove(target: &str, original: Option<FileState>) -> PlannedWrite {
        PlannedWrite {
            target: Target::parse(target).expect("target"),
            new_bytes: Vec::new(),
            digest: sha256(&[]),
            original,
            effect: Effect::Remove,
        }
    }

    /// A plan re-derived from a backup: authorized by that backup, not by a mirror.
    fn restore_plan(writes: Vec<PlannedWrite>) -> Plan {
        let id = BackupId::from_secs(NOW).expect("backup id");
        let backup = crate::restore::validated_backup_for_test(id);
        Plan::for_restore(&backup, writes).expect("a restore plan")
    }

    fn fixture(root: &Path, name: &str, bytes: &[u8], mode: u32) -> PathBuf {
        let path = root.join(name);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).expect("fixture directory");
        }
        fs::write(&path, bytes).expect("fixture bytes");
        fs::set_permissions(&path, fs::Permissions::from_mode(mode)).expect("fixture mode");
        path
    }

    fn entries(path: &Path) -> Vec<String> {
        fs::read_dir(path)
            .expect("directory listing")
            .map(|e| e.expect("entry").file_name().to_string_lossy().into_owned())
            .collect()
    }

    /// The leftover temp name this program writes, for candidate `i`.
    fn temp_name(i: u32) -> String {
        format!("{TEMP_PREFIX}{}-{i}", std::process::id())
    }

    fn exists_reason(err: &ApplyError) -> &str {
        let ApplyError::Exists { reason, .. } = err else {
            panic!("not an Exists: {err}");
        };
        reason
    }

    /// Criterion 1: the root check, its message and its exit code.
    #[test]
    fn a_non_root_run_is_refused_with_the_sudo_command() {
        let err = ApplyError::NotRoot;
        let line = err.to_string();
        assert!(line.contains("sudo apt-scout"), "{line}");
        assert!(!line.contains('\n'), "{line}");
        assert_eq!(err.exit_code(), 3);
        assert_eq!(APT_DIR, "/etc/apt");
        assert_eq!(BACKUP_ROOT, "/var/backups/apt-scout");

        let root = TempRoot::new("root");
        let (uid, _) = root.owner();
        assert_eq!(require_root().is_ok(), uid == 0);
        if uid != 0 {
            // `apply` is the root-only entry point: it must refuse before it reads anything, so no
            // part of /etc/apt is touched here, whatever baseline it is handed.
            let planned = plan(vec![create(SECURITY)]);
            assert!(matches!(
                apply(&planned, Some(&baseline()), NOW),
                Err(ApplyError::NotRoot)
            ));
        }
    }

    /// The write harness spells `/etc/apt` and `/var/backups/apt-scout` out itself, so the two
    /// compiled defaults are the contract its cases read back. Weak on its own: only the container
    /// cases prove the paths a run actually touches.
    #[test]
    fn apt_dir_and_backup_root_are_the_compiled_defaults() {
        assert_eq!(APT_DIR, "/etc/apt");
        assert_eq!(BACKUP_ROOT, "/var/backups/apt-scout");
    }

    /// Criterion 2: a recorded mode loses the file-type bits, and a set-gid mode survives the
    /// replace, because `fchown` precedes `set_permissions` (chown clears set-id bits).
    #[test]
    fn a_mode_is_recorded_without_the_file_type_bits_and_a_set_gid_mode_survives() {
        let root = TempRoot::new("mode");
        let apt = root.apt();
        let (uid, gid) = root.owner();
        let backup = root.path().join("backup");
        let path = fixture(&apt, LIST, OLD, 0o2755);
        // `st_mode` carries the file type as well, so an unmasked record of this fixture would be
        // 0o102755 on both Linux and macOS.
        assert_eq!(fs::symlink_metadata(&path).expect("meta").mode(), 0o102755);
        let before = state(&path);
        assert_eq!(before.mode, 0o2755);

        let planned = plan(vec![replace(LIST, Some(before.clone()))]);
        let new_bytes = planned.writes()[0].new_bytes.clone();
        let captured = capture(&apt, &planned).expect("capture binds the plan to the disk");
        assert_eq!(
            captured[0].original.as_ref().expect("original").mode,
            0o2755
        );

        run(&apt, &backup, &planned, Some(&baseline()), NOW, uid, gid)
            .expect("the set-gid replacement succeeds");
        let after = state(&path);
        assert_eq!(after.mode, 0o2755, "the set-gid bit survives the replace");
        assert_eq!(after.mode & !MODE_MASK, 0);
        assert_eq!(after.bytes, new_bytes);
        // The owner is preserved. A root run replacing a file owned by uid 1000 is the case that
        // matters most, and only D9c's container matrix can build it: a non-root test cannot own
        // another user's file.
        assert_eq!((after.uid, after.gid), (before.uid, before.gid));
    }

    /// Criterion 3: the plan is bound to the disk before anything else happens to it.
    #[test]
    fn capture_refuses_a_plan_that_disagrees_with_the_disk() {
        let root = TempRoot::new("bind");
        let apt = root.apt();
        let backup = root.path().join("backup");
        let (uid, gid) = root.owner();
        let target = fixture(&apt, LIST, OLD, 0o644);
        let on_disk = state(&target);

        // (a) the plan was built from different bytes, so the disk is not the plan's disk.
        let stale = FileState {
            bytes: b"deb http://other.example/ubuntu noble main\n".to_vec(),
            ..on_disk.clone()
        };
        let err = run(
            &apt,
            &backup,
            &plan(vec![replace(LIST, Some(stale))]),
            Some(&baseline()),
            NOW,
            uid,
            gid,
        )
        .expect_err("a stale plan is refused");
        assert!(matches!(err, ApplyError::Changed { .. }), "{err}");
        assert_eq!(fs::read(&target).expect("bytes"), on_disk.bytes);
        // The refusal came from the capture, so no backup directory was created either.
        assert!(!backup.exists(), "{backup:?} must not exist");

        // (b) the plan replaces a file that is gone.
        fs::remove_file(&target).expect("remove the fixture");
        let err = run(
            &apt,
            &backup,
            &plan(vec![replace(LIST, Some(on_disk))]),
            Some(&baseline()),
            NOW,
            uid,
            gid,
        )
        .expect_err("a missing original is refused");
        assert!(matches!(err, ApplyError::Changed { .. }), "{err}");
        assert!(!target.exists());
        assert!(!backup.exists(), "{backup:?} must not exist");
    }

    /// Criterion 4: no symlink is ever followed, neither the target nor its directory.
    #[test]
    fn a_hostile_symlink_is_refused_and_the_canary_stays() {
        let root = TempRoot::new("link");
        let apt = root.apt();
        let outside = root.path().join("outside");
        fs::create_dir_all(outside.join("sources.list.d")).expect("outside tree");
        let backup = root.path().join("backup");
        let (uid, gid) = root.owner();
        // The canaries hold exactly the bytes the plan was built from, so only the refusal keeps
        // the write away from them.
        let canary = fixture(&outside, "canary.list", b"canary\n", 0o644);
        let escaped = fixture(
            &outside.join("sources.list.d"),
            "escape.list",
            b"escaped\n",
            0o644,
        );

        // (a) the target itself is a symlink out of the tree.
        symlink(&canary, apt.join(LIST)).expect("symlink");
        let err = run(
            &apt,
            &backup,
            &plan(vec![replace(LIST, Some(state(&canary)))]),
            Some(&baseline()),
            NOW,
            uid,
            gid,
        )
        .expect_err("a symlinked target is refused");
        assert!(matches!(err, ApplyError::Target { .. }), "{err}");
        assert_eq!(fs::read(&canary).expect("canary"), b"canary\n");
        assert!(!backup.exists(), "{backup:?} must not exist");

        // (b) `sources.list.d` is a symlink, so the target resolves outside the tree. `lstat` of
        // the target does not follow only its parent, so this is `check_parent`'s proof.
        fs::remove_file(apt.join(LIST)).expect("remove the link");
        symlink(outside.join("sources.list.d"), apt.join("sources.list.d")).expect("symlink");
        let err = run(
            &apt,
            &backup,
            &plan(vec![replace(
                "sources.list.d/escape.list",
                Some(state(&escaped)),
            )]),
            Some(&baseline()),
            NOW,
            uid,
            gid,
        )
        .expect_err("a symlinked directory is refused");
        assert!(matches!(err, ApplyError::Target { .. }), "{err}");
        assert_eq!(fs::read(&escaped).expect("canary"), b"escaped\n");
        assert!(!backup.exists(), "{backup:?} must not exist");
    }

    /// Criterion 5: the backup root is verified, never trusted.
    #[test]
    fn a_hostile_backup_root_is_refused_and_stays_empty() {
        let root = TempRoot::new("backuproot");
        let apt = root.apt();
        let (uid, gid) = root.owner();
        let target = fixture(&apt, LIST, OLD, 0o644);
        let before = state(&target);
        let writes = || plan(vec![replace(LIST, Some(before.clone()))]);
        let run_with = |backup_root: &Path, planned: &Plan, owner_uid: u32| {
            run(
                &apt,
                backup_root,
                planned,
                Some(&baseline()),
                NOW,
                owner_uid,
                gid,
            )
        };

        // (a) the backup root is a symlink to a directory outside the tree.
        let abroad = root.path().join("abroad");
        fs::create_dir(&abroad).expect("abroad");
        let link = root.path().join("backup-link");
        symlink(&abroad, &link).expect("symlink");
        let err = run_with(&link, &writes(), uid).expect_err("a symlink is refused");
        assert!(matches!(err, ApplyError::Backup { .. }), "{err}");
        assert_eq!(entries(&abroad), Vec::<String>::new());

        // (b) the backup root belongs to another uid: root runs as uid 0, so this is what a
        // directory planted by a local user looks like. The plan only creates, so the ownership
        // gate finds no file to refuse and the backup root is reached.
        fs::create_dir(apt.join("sources.list.d")).expect("sources.list.d");
        let foreign = root.path().join("backup-foreign");
        fs::create_dir(&foreign).expect("foreign root");
        fs::set_permissions(&foreign, fs::Permissions::from_mode(DIR_MODE)).expect("mode");
        let err = run_with(&foreign, &plan(vec![create(SECURITY)]), uid.wrapping_add(1))
            .expect_err("another owner is refused");
        assert!(matches!(err, ApplyError::Backup { .. }), "{err}");
        assert_eq!(entries(&foreign), Vec::<String>::new());

        // (c) the backup root is writable by everyone.
        let loose = root.path().join("backup-loose");
        fs::create_dir(&loose).expect("loose root");
        fs::set_permissions(&loose, fs::Permissions::from_mode(0o777)).expect("mode");
        let err = run_with(&loose, &writes(), uid).expect_err("mode 0777 is refused");
        assert!(matches!(err, ApplyError::Backup { .. }), "{err}");
        assert!(err.to_string().contains("0o777"), "{err}");
        assert_eq!(entries(&loose), Vec::<String>::new());

        assert_eq!(state(&target), before);
    }

    /// Criterion 6, narrowed: this proves the backup root's *mode control* and nothing more. The
    /// real write-failure case needs a directory that passes the backup root and still refuses the
    /// write, which only D9c's S8 container case can build.
    #[test]
    fn an_unwritable_backup_root_is_refused_before_anything_is_written() {
        let root = TempRoot::new("backupfail");
        let apt = root.apt();
        let (uid, gid) = root.owner();
        let target = fixture(&apt, LIST, OLD, 0o644);
        let before = state(&target);
        let backup = root.path().join("backup");
        fs::create_dir(&backup).expect("backup root");
        fs::set_permissions(&backup, fs::Permissions::from_mode(0o500)).expect("mode");
        if uid == 0 {
            // Root writes into a 0500 directory, so this case can only be built by a non-root run.
            return;
        }
        let err = run(
            &apt,
            &backup,
            &plan(vec![replace(LIST, Some(before.clone()))]),
            Some(&baseline()),
            NOW,
            uid,
            gid,
        )
        .expect_err("an unwritable backup root fails the run");
        assert!(matches!(err, ApplyError::Backup { .. }), "{err}");
        // The write phase runs after the backup, so every target is still byte-identical.
        assert_eq!(state(&target), before);
        assert_eq!(entries(&backup), Vec::<String>::new());
    }

    /// Criterion 7: a substituted file (same path, same bytes, other inode) is refused.
    #[test]
    fn a_file_swapped_between_capture_and_write_is_refused() {
        let root = TempRoot::new("swap");
        let apt = root.apt();
        let target = fixture(&apt, LIST, OLD, 0o644);
        let captured = capture(&apt, &plan(vec![replace(LIST, Some(state(&target)))]))
            .expect("capture binds the plan to the disk");

        // Same path and the same bytes, but a different inode: a substitution.
        let substitute = apt.join("substitute");
        fixture(&apt, "substitute", OLD, 0o644);
        fs::rename(&substitute, &target).expect("rename over the target");

        let err = validate_targets(&captured).expect_err("the substitution is refused");
        assert!(matches!(err, ApplyError::Changed { .. }), "{err}");
        assert!(err.to_string().contains("inode"), "{err}");
        assert_eq!(fs::read(&target).expect("bytes"), OLD);
    }

    /// Criterion 8: an edit in place, same inode, is refused too.
    #[test]
    fn bytes_edited_in_place_between_capture_and_write_are_refused() {
        let root = TempRoot::new("edit");
        let apt = root.apt();
        let target = fixture(&apt, LIST, OLD, 0o644);
        let captured = capture(&apt, &plan(vec![replace(LIST, Some(state(&target)))]))
            .expect("capture binds the plan to the disk");

        let ino = fs::symlink_metadata(&target).expect("meta").ino();
        let edited: &[u8] = b"deb http://evil.example/ubuntu noble main\n";
        fs::write(&target, edited).expect("edit in place");
        assert_eq!(fs::symlink_metadata(&target).expect("meta").ino(), ino);

        let err = validate_targets(&captured).expect_err("the edit is refused");
        assert!(matches!(err, ApplyError::Changed { .. }), "{err}");
        assert!(err.to_string().contains("bytes changed"), "{err}");
        assert_eq!(fs::read(&target).expect("bytes"), edited);
    }

    /// Criterion 9: `EEXIST` aborts the whole run, and the three `Exists` sites say three
    /// different things with three different fixes.
    #[test]
    fn a_file_that_appears_where_the_run_creates_aborts_the_whole_run() {
        let root = TempRoot::new("exists");
        let apt = root.apt();
        let (uid, gid) = root.owner();
        let backup = root.path().join("backup");
        let list = fixture(&apt, LIST, OLD, 0o644);
        let before = state(&list);
        let planned = plan(vec![replace(LIST, Some(before.clone())), create(SECURITY)]);
        let captured = capture(&apt, &planned).expect("capture binds the plan to the disk");

        // (a) the window between the capture and the write: someone else creates the file first.
        let created = fixture(&apt, SECURITY, b"planted\n", 0o644);
        let err = write_targets(&captured, uid, gid).expect_err("the creation finds it taken");
        assert!(matches!(err, ApplyError::Exists { .. }), "{err}");
        // The creations run before the replacements, so the other target is untouched too.
        assert_eq!(fs::read(&created).expect("planted"), b"planted\n");
        assert_eq!(state(&list), before);
        let create_taken = err;

        // (b) the `<id>` collision: a backup for this UTC second already exists. The backup root
        // itself is verified first, so it is 0700 before the collision is built.
        fs::create_dir(&backup).expect("backup root");
        fs::set_permissions(&backup, fs::Permissions::from_mode(DIR_MODE)).expect("mode");
        let held = backup.join(ID);
        fs::create_dir(&held).expect("an earlier backup");
        let err = run(
            &apt,
            &backup,
            &plan(vec![replace(LIST, Some(before.clone()))]),
            Some(&baseline()),
            NOW,
            uid,
            gid,
        )
        .expect_err("a taken backup name is refused");
        assert!(matches!(err, ApplyError::Exists { .. }), "{err}");
        let id_taken = err;

        // (c) the temp-name site: every candidate name in the directory is taken.
        for i in 0..TEMP_TRIES {
            fixture(&apt, &temp_name(i), b"leftover\n", FILE_MODE);
        }
        let err = create_temp(&apt).expect_err("every candidate name is taken");
        assert!(matches!(err, ApplyError::Exists { .. }), "{err}");
        let temp_taken = err;

        let texts = [
            create_taken.to_string(),
            id_taken.to_string(),
            temp_taken.to_string(),
        ];
        let reasons = [
            exists_reason(&create_taken).to_string(),
            exists_reason(&id_taken).to_string(),
            exists_reason(&temp_taken).to_string(),
        ];
        for (a, b) in [(0, 1), (0, 2), (1, 2)] {
            assert_ne!(
                reasons[a], reasons[b],
                "site {a} and site {b} share a reason"
            );
            assert_ne!(texts[a], texts[b], "site {a} and site {b} share a message");
        }
        for text in &texts {
            assert!(text.to_lowercase().contains("re-run"), "{text}");
            // A write-phase failure may not claim the system is clean.
            assert!(!text.contains("Nothing was written"), "{text}");
        }
        // The create-collision message is about the target path the user has to act on.
        assert!(texts[0].contains(SECURITY), "{}", texts[0]);
        // The `<id>` collision leaves the earlier backup alone: the fix is a later second, and the
        // message must never suggest removing a backup.
        assert!(!texts[1].to_lowercase().contains("remove"), "{}", texts[1]);
        assert!(texts[1].contains("later second"), "{}", texts[1]);
        // The temp-name message names the files it is about.
        assert!(texts[2].contains(TEMP_PREFIX), "{}", texts[2]);
    }

    /// Criterion 10: a replace keeps content, mode and owner, through a rename.
    #[test]
    fn a_replace_keeps_content_mode_and_owner_and_leaves_no_temp_file() {
        let root = TempRoot::new("replace");
        let apt = root.apt();
        let (uid, gid) = root.owner();
        let backup = root.path().join("backup");
        let target = fixture(&apt, LIST, OLD, 0o640);
        let before = state(&target);
        let planned = plan(vec![replace(LIST, Some(before.clone()))]);
        let new_bytes = planned.writes()[0].new_bytes.clone();

        let outcome = run(&apt, &backup, &planned, Some(&baseline()), NOW, uid, gid)
            .expect("the replacement succeeds");
        assert_eq!(
            outcome.touched,
            vec![Touched {
                target: Target::parse(LIST).expect("target"),
                effect: Effect::Replace,
            }]
        );
        assert_eq!(outcome.backup_id.as_str(), ID);

        let after = state(&target);
        assert_eq!(after.bytes, new_bytes);
        assert_eq!(after.mode, 0o640);
        assert_eq!((after.uid, after.gid), (before.uid, before.gid));
        // A rename, not a write in place: the file the plan was built from is gone.
        assert_ne!(after.ino, before.ino);
        assert_eq!(entries(&apt), vec![LIST.to_string()]);
    }

    /// Criterion 11: what the run creates is 0600 with the exact bytes, and the `create` record
    /// matches the created file's own `fstat`, not a directory's gid.
    #[test]
    fn a_created_file_is_0600_and_its_record_matches_its_own_fstat() {
        let root = TempRoot::new("create");
        let apt = root.apt();
        // A stock install has `sources.list.d`; the run creates the file in it, never the directory.
        fs::create_dir(apt.join("sources.list.d")).expect("sources.list.d");
        let (uid, gid) = root.owner();
        let backup = root.path().join("backup");
        let planned = plan(vec![create(SECURITY)]);
        let new_bytes = planned.writes()[0].new_bytes.clone();

        let outcome = run(&apt, &backup, &planned, Some(&baseline()), NOW, uid, gid)
            .expect("the creation succeeds");
        let created = apt.join(SECURITY);
        assert_eq!(fs::read(&created).expect("bytes"), new_bytes);
        let meta = fs::symlink_metadata(&created).expect("meta");
        assert_eq!(meta.mode() & MODE_MASK, FILE_MODE);
        assert_eq!(
            outcome.touched,
            vec![Touched {
                target: Target::parse(SECURITY).expect("target"),
                effect: Effect::Create,
            }]
        );

        // The record describes what this run created. A `<id>` directory can carry a gid the file
        // does not (a set-group-id `sources.list.d` is a real configuration), which is why the
        // record names the run's own identity and `fill_creation` proves the file carries it; a
        // non-root test cannot make the two differ, so that case is D9c's.
        let text = fs::read_to_string(backup.join(ID).join(MANIFEST)).expect("manifest");
        let line = format!(
            "file {SECURITY} create {} {} {} {}",
            hex(&sha256(&new_bytes)),
            meta.mode() & MODE_MASK,
            meta.uid(),
            meta.gid()
        );
        assert!(text.contains(&line), "{text}");
        assert_eq!((meta.uid(), meta.gid()), (uid, gid));
    }

    /// Criterion 12: the backup holds every replaced file, its manifest and nothing else. The
    /// parse round trip is criterion 15's; here the text is exactly what `render` produces.
    #[test]
    fn the_backup_holds_every_replaced_file_and_its_manifest() {
        let root = TempRoot::new("backup");
        let apt = root.apt();
        let (uid, gid) = root.owner();
        let backup = root.path().join("backup");
        let list = fixture(&apt, LIST, OLD, 0o644);
        let extra = fixture(
            &apt,
            "sources.list.d/extra.list",
            b"deb http://archive.ubuntu.com/ubuntu noble-updates main\n",
            0o640,
        );
        let (list_state, extra_state) = (state(&list), state(&extra));
        let planned = plan(vec![
            replace(LIST, Some(list_state.clone())),
            replace("sources.list.d/extra.list", Some(extra_state.clone())),
        ]);

        let outcome = run(&apt, &backup, &planned, Some(&baseline()), NOW, uid, gid)
            .expect("the write succeeds");
        let dir = backup.join(outcome.backup_id.as_str());
        // Every copy is byte-identical to the original, with the original's mode and owner.
        for (name, s) in [
            (LIST, &list_state),
            ("sources.list.d/extra.list", &extra_state),
        ] {
            let copy = dir.join(name);
            assert_eq!(&fs::read(&copy).expect("copy"), &s.bytes, "{name}");
            let meta = fs::symlink_metadata(&copy).expect("copy metadata");
            assert_eq!(meta.mode() & MODE_MASK, s.mode, "{name}");
            assert_eq!((meta.uid(), meta.gid()), (s.uid, s.gid), "{name}");
        }
        // The backup root and every directory inside it are 0700 whatever the umask is, and the
        // manifest is 0600.
        for held in [&backup, &dir, &dir.join("sources.list.d")] {
            let meta = fs::symlink_metadata(held).expect("backup metadata");
            assert_eq!(meta.mode() & MODE_MASK, DIR_MODE, "{}", held.display());
        }
        let manifest = dir.join(MANIFEST);
        assert_eq!(
            fs::symlink_metadata(&manifest)
                .expect("manifest metadata")
                .mode()
                & MODE_MASK,
            FILE_MODE
        );
        // The text is exactly what the format renders, from the copies and their owners.
        let line = |name: &str, s: &FileState| {
            format!(
                "file {name} replace {} {} {} {}",
                hex(&sha256(&s.bytes)),
                s.mode,
                s.uid,
                s.gid
            )
        };
        let text = fs::read_to_string(&manifest).expect("manifest bytes");
        assert_eq!(
            text,
            format!(
                "apt-scout-backup 1\nid {ID}\nfiles 2\n{}\n{}\nend 2",
                line(LIST, &list_state),
                line("sources.list.d/extra.list", &extra_state)
            )
        );
    }

    /// Criterion 13: no verified baseline, or a mirror behind it, refuses before the first disk
    /// operation; a baseline the mirror matches or trails is accepted.
    #[test]
    fn a_missing_or_later_baseline_refuses_before_the_first_disk_operation() {
        let root = TempRoot::new("fresh");
        let apt = root.apt();
        let (uid, gid) = root.owner();
        let backup = root.path().join("backup");
        let target = fixture(&apt, LIST, OLD, 0o644);
        let before = state(&target);
        // The plan is built from the disk as it is now, so a run that lands does not leave the
        // next case with a stale plan.
        let planned = || plan(vec![replace(LIST, Some(state(&target)))]);
        let run_with = |baseline: Option<&VerifiedDate>, backup_root: &Path| {
            run(&apt, backup_root, &planned(), baseline, NOW, uid, gid)
        };

        // (a) no baseline at all: the writer cannot establish one, because it may not fetch.
        let err = run_with(None, &backup).expect_err("a run without a baseline is refused");
        assert!(matches!(err, ApplyError::NoBaseline), "{err}");
        assert_eq!(err.exit_code(), 1);
        assert!(err.to_string().contains("Nothing was written"), "{err}");
        assert_eq!(state(&target), before);
        assert!(!backup.exists(), "{backup:?} must not exist");

        // (b) the mirror is `MAX_LAG_SECS + 1` behind the baseline: the same stale rule the
        // pipeline uses, with the URI and the lag in the message.
        let late = verified(LATE);
        let err = run_with(Some(&late), &backup).expect_err("a stale mirror is refused");
        let ApplyError::Stale { uri, lag_secs } = &err else {
            panic!("not a Stale: {err}");
        };
        assert_eq!(uri, "https://m.example/ubuntu");
        assert_eq!(*lag_secs, fresh::MAX_LAG_SECS + 1);
        let text = err.to_string();
        assert!(text.contains("https://m.example/ubuntu"), "{text}");
        assert!(
            text.contains(&(fresh::MAX_LAG_SECS + 1).to_string()),
            "{text}"
        );
        assert_eq!(state(&target), before);
        assert!(!backup.exists(), "{backup:?} must not exist");

        // (c) the baseline is behind the mirror (the mirror is fresher): accepted.
        let early = verified(EARLY);
        let outcome = run_with(Some(&early), &root.path().join("backup-early"))
            .expect("a mirror ahead of the baseline is not stale");
        assert_eq!(outcome.backup_id.as_str(), ID);

        // (d) the baseline is exactly the plan's proof: accepted.
        let equal = verified(RFC);
        run_with(Some(&equal), &root.path().join("backup-equal"))
            .expect("a baseline equal to the proof is accepted");
    }

    /// Criterion 14: a target the run does not own is refused before the backup, because a record
    /// the manifest parser rejects is not a backup.
    #[test]
    fn a_target_the_run_does_not_own_is_refused_before_the_backup() {
        let root = TempRoot::new("owner");
        let apt = root.apt();
        let (uid, gid) = root.owner();
        let backup = root.path().join("backup");
        let target = fixture(&apt, LIST, OLD, 0o644);
        let before = state(&target);
        let planned = plan(vec![replace(LIST, Some(before.clone()))]);

        // The run's owner is not the file's owner, which is what a root run meeting a user's file
        // looks like; a non-root test can build the same refusal by handing the writer another
        // uid, because the gate compares the file to the run, not the file to the process.
        let err = run(
            &apt,
            &backup,
            &planned,
            Some(&baseline()),
            NOW,
            uid.wrapping_add(1),
            gid,
        )
        .expect_err("a foreign target is refused");
        assert!(matches!(err, ApplyError::Owner { .. }), "{err}");
        let text = err.to_string();
        assert!(
            text.contains("will not rewrite a file it cannot describe in a restorable manifest"),
            "{text}"
        );
        assert!(text.contains(&before.uid.to_string()), "{text}");
        assert!(text.contains(&uid.wrapping_add(1).to_string()), "{text}");
        assert!(text.contains("Nothing was written"), "{text}");
        assert_eq!(state(&target), before);
        assert!(!backup.exists(), "{backup:?} must not exist");

        // The gate's own rule, on the same captured state: the run's owner passes.
        let captured = capture(&apt, &planned).expect("capture binds the plan to the disk");
        assert!(check_ownership(&captured, uid, gid).is_ok());
    }

    /// Criterion 15: the gate's rule *is* the manifest parser's rule, provable without root.
    #[test]
    fn the_writers_records_round_trip_through_the_manifest_parser() {
        let root_owned = FileState {
            dev: 1,
            ino: 2,
            mode: 0o644,
            uid: 0,
            gid: 0,
            bytes: OLD.to_vec(),
        };
        let new_bytes = b"Types: deb\nURIs: https://m.example/ubuntu\n".to_vec();
        let captured = vec![
            Captured {
                target: Target::parse(LIST).expect("target"),
                path: PathBuf::from(APT_DIR).join(LIST),
                original: Some(root_owned),
                new_bytes: new_bytes.clone(),
                effect: Effect::Replace,
            },
            Captured {
                target: Target::parse(SECURITY).expect("target"),
                path: PathBuf::from(APT_DIR).join(SECURITY),
                original: None,
                new_bytes: new_bytes.clone(),
                effect: Effect::Create,
            },
        ];
        let id = BackupId::from_secs(NOW).expect("backup id");

        // A root-owned state is exactly what the parser accepts, so the writer's own records for
        // such a run round-trip.
        let entries = manifest_entries(&captured, 0, 0);
        assert_eq!(entries[0].action, Action::Replace);
        assert_eq!(entries[1].action, Action::Create);
        assert_eq!(entries[1].mode, FILE_MODE);
        let text = manifest::render(&id, &entries);
        let parsed = manifest::parse(&text).expect("a root-owned state round-trips");
        assert_eq!(parsed.id(), &id);
        assert_eq!(parsed.entries(), &entries[..]);
        // And the ownership gate accepts the same state under the same owner.
        assert!(check_ownership(&captured, 0, 0).is_ok());

        // The same entries with gid 100 are refused by the parser *and* by the gate, which is the
        // proof that the gate's rule is the parser's rule.
        let foreign: Vec<ManifestEntry> = entries
            .iter()
            .map(|e| ManifestEntry {
                gid: 100,
                ..e.clone()
            })
            .collect();
        assert!(matches!(
            manifest::parse(&manifest::render(&id, &foreign)),
            Err(ManifestError::Ownership(_))
        ));
        let foreign_captured: Vec<Captured> = captured
            .iter()
            .map(|c| Captured {
                target: c.target.clone(),
                path: c.path.clone(),
                original: c.original.as_ref().map(|s| FileState {
                    gid: 100,
                    ..s.clone()
                }),
                new_bytes: c.new_bytes.clone(),
                effect: c.effect,
            })
            .collect();
        assert!(matches!(
            check_ownership(&foreign_captured, 0, 0),
            Err(ApplyError::Owner { .. })
        ));
    }

    /// Criterion 16: the sweep removes only this program's own regular files, and only up to
    /// `TEMP_TRIES` per directory.
    #[test]
    fn the_sweep_removes_only_this_programs_own_regular_leftovers() {
        let root = TempRoot::new("sweep");
        let apt = root.apt();
        let (uid, gid) = root.owner();
        let backup = root.path().join("backup");
        let target = fixture(&apt, LIST, OLD, 0o644);
        let before = state(&target);

        // One leftover of ours, one symlink of that name, one directory of that name.
        let leftover = fixture(&apt, &temp_name(0), b"leftover\n", FILE_MODE);
        let linked = root.path().join("linked");
        fs::write(&linked, b"elsewhere\n").expect("link destination");
        let symlink_name = apt.join(temp_name(1));
        symlink(&linked, &symlink_name).expect("symlink");
        let dir_name = apt.join(temp_name(2));
        fs::create_dir(&dir_name).expect("directory");

        let outcome = run(
            &apt,
            &backup,
            &plan(vec![replace(LIST, Some(before.clone()))]),
            Some(&baseline()),
            NOW,
            uid,
            gid,
        )
        .expect("the sweep never blocks a write");
        assert_eq!(
            outcome.touched,
            vec![Touched {
                target: Target::parse(LIST).expect("target"),
                effect: Effect::Replace,
            }]
        );
        assert!(!leftover.exists(), "the leftover is swept");
        assert!(symlink_name.exists(), "a symlink is never swept");
        assert!(dir_name.exists(), "a directory is never swept");
        assert_eq!(fs::read(&linked).expect("link destination"), b"elsewhere\n");

        // A file this run did not create is not its leftover: with another owner, nothing goes.
        let foreign = fixture(&apt, &temp_name(3), b"someone else's\n", FILE_MODE);
        sweep_temp(&apt, uid.wrapping_add(1)).expect("the sweep runs");
        assert!(foreign.exists(), "another owner's file is never swept");
        sweep_temp(&apt, uid).expect("the sweep runs");
        assert!(!foreign.exists(), "the run's own leftover is swept");

        // The bound: more than `TEMP_TRIES` leftovers still let the run succeed, and only
        // `TEMP_TRIES` of them go.
        let bound = TempRoot::new("sweep-bound");
        let apt = bound.apt();
        let (uid, gid) = bound.owner();
        let backup = bound.path().join("backup");
        let list = fixture(&apt, LIST, OLD, 0o644);
        let before = state(&list);
        // Names the replacement never tries, so the leftover count alone decides the outcome.
        let extra: Vec<PathBuf> = (TEMP_TRIES..TEMP_TRIES + TEMP_TRIES + 4)
            .map(|i| fixture(&apt, &temp_name(i), b"leftover\n", FILE_MODE))
            .collect();
        run(
            &apt,
            &backup,
            &plan(vec![replace(LIST, Some(before))]),
            Some(&baseline()),
            NOW,
            uid,
            gid,
        )
        .expect("many leftovers do not block the write");
        let left = extra.iter().filter(|p| p.exists()).count();
        assert_eq!(
            left,
            extra.len() - TEMP_TRIES as usize,
            "the sweep is bounded to {TEMP_TRIES} entries per directory"
        );
    }

    /// Criterion 17: a write-phase failure says what the run had already changed, never that the
    /// system is untouched, and a failed creation leaves no file behind.
    #[test]
    fn a_failed_write_names_what_the_run_had_already_changed() {
        // The variants whose failure provably precedes every mutation may say it; the two the
        // write phase can produce may not.
        let path = PathBuf::from(APT_DIR).join(LIST);
        let before_any_write = [
            ApplyError::Plan(PlanError::NoProof),
            ApplyError::NoBaseline,
            ApplyError::Stale {
                uri: "https://m.example/ubuntu".into(),
                lag_secs: fresh::MAX_LAG_SECS + 1,
            },
            ApplyError::Read {
                path: path.clone(),
                reason: "Permission denied (os error 13)".into(),
            },
            ApplyError::TooLarge {
                path: path.clone(),
                limit: MAX_FILE_BYTES,
            },
            ApplyError::Target {
                path: path.clone(),
                reason: "it is a symlink".into(),
            },
            ApplyError::Owner {
                path: path.clone(),
                reason: "it is owned by uid 1000 gid 1000".into(),
            },
            ApplyError::Changed {
                path: path.clone(),
                reason: "the bytes changed".into(),
            },
            ApplyError::Backup {
                path: path.clone(),
                reason: "its mode is 0o777, not 0o700".into(),
            },
            ApplyError::Time {
                secs: 253_402_300_800,
            },
        ];
        for err in &before_any_write {
            assert_eq!(err.exit_code(), 1, "{err}");
            let text = err.to_string();
            assert!(text.contains("Nothing was written"), "{text}");
            assert!(text.to_lowercase().contains("re-run"), "{text}");
            assert!(!text.contains('\n'), "{text}");
            if !matches!(
                err,
                ApplyError::Plan(_)
                    | ApplyError::NoBaseline
                    | ApplyError::Stale { .. }
                    | ApplyError::Time { .. }
            ) {
                assert!(text.contains("/etc/apt/sources.list"), "{text}");
            }
        }
        // The OS error travels verbatim.
        assert!(
            before_any_write[3]
                .to_string()
                .contains("Permission denied (os error 13)")
        );

        // (a) a creation lands, then the replacement cannot write: the message names the created
        // target and does not claim the system is clean.
        let root = TempRoot::new("truthful");
        let apt = root.apt();
        let (uid, gid) = root.owner();
        if uid != 0 {
            let backup = root.path().join("backup");
            fs::create_dir(apt.join("sources.list.d")).expect("sources.list.d");
            let list = fixture(&apt, LIST, OLD, 0o644);
            let before = state(&list);
            let planned = plan(vec![replace(LIST, Some(before.clone())), create(SECURITY)]);
            // `sources.list.d` stays writable, so the creation lands; the directory holding the
            // replacement refuses the temp file. Root ignores the mode, so only a non-root run
            // can build this.
            fs::set_permissions(&apt, fs::Permissions::from_mode(0o500)).expect("mode");
            let err = run(&apt, &backup, &planned, Some(&baseline()), NOW, uid, gid)
                .expect_err("the replacement cannot be written");
            fs::set_permissions(&apt, fs::Permissions::from_mode(0o755)).expect("mode");
            assert!(matches!(err, ApplyError::Write { .. }), "{err}");
            let text = err.to_string();
            assert!(text.contains(SECURITY), "{text}");
            assert!(
                text.contains("already changed sources.list.d/system-security.sources"),
                "{text}"
            );
            assert!(!text.contains("Nothing was written"), "{text}");
            // The creation really did land, which is why the message may not claim otherwise.
            assert_eq!(
                fs::read(apt.join(SECURITY)).expect("the creation landed"),
                planned.writes()[1].new_bytes
            );
            assert_eq!(state(&list), before);
        }

        // (b) a creation that fails after the file exists leaves nothing at the target path. The
        // only non-root-reachable failure there is the record mismatch: the manifest's `create`
        // entry names an owner the created file does not carry.
        let bare = TempRoot::new("truthful-create");
        let apt = bare.apt();
        let (uid, gid) = bare.owner();
        fs::create_dir(apt.join("sources.list.d")).expect("sources.list.d");
        let planned = plan(vec![create(SECURITY)]);
        let captured = capture(&apt, &planned).expect("capture binds the plan to the disk");
        let err = write_targets(&captured, uid.wrapping_add(1), gid)
            .expect_err("the record does not describe the created file");
        assert!(matches!(err, ApplyError::Write { .. }), "{err}");
        let text = err.to_string();
        assert!(text.contains("the record does not describe"), "{text}");
        assert!(!text.contains("Nothing was written"), "{text}");
        assert!(
            !apt.join(SECURITY).exists(),
            "a failed creation leaves no file at the target path"
        );

        // (c) the re-validation before the rename refuses after a creation landed: the finding is
        // the disk's, but the message may still not claim a clean system.
        let swapped = TempRoot::new("truthful-changed");
        let apt = swapped.apt();
        let (uid, gid) = swapped.owner();
        fs::create_dir(apt.join("sources.list.d")).expect("sources.list.d");
        let list = fixture(&apt, LIST, OLD, 0o644);
        let before = state(&list);
        let planned = plan(vec![replace(LIST, Some(before.clone())), create(SECURITY)]);
        let captured = capture(&apt, &planned).expect("capture binds the plan to the disk");
        // The window between the re-validation and the rename: the file is edited in place, so
        // only the check just before the rename can catch it.
        fs::write(&list, b"deb http://evil.example/ubuntu noble main\n").expect("edit");
        let err = write_targets(&captured, uid, gid)
            .expect_err("the re-validation refuses the changed file");
        assert!(matches!(err, ApplyError::Write { .. }), "{err}");
        let text = err.to_string();
        assert!(text.contains(SECURITY), "{text}");
        assert!(
            text.contains("changed while this run was writing"),
            "{text}"
        );
        assert!(!text.contains("Nothing was written"), "{text}");
        // The creation landed and the edited file is untouched: no temp file survives either.
        assert!(apt.join(SECURITY).exists());
        assert_eq!(
            fs::read(&list).expect("bytes"),
            b"deb http://evil.example/ubuntu noble main\n"
        );
        assert!(!entries(&apt).iter().any(|n| n.starts_with(TEMP_PREFIX)));
    }

    /// Criterion 18: an oversize file is a size refusal, never a permission hint.
    #[test]
    fn a_file_over_the_size_limit_is_too_large_and_not_unreadable() {
        let root = TempRoot::new("oversize");
        let apt = root.apt();
        let target = fixture(&apt, LIST, OLD, 0o644);
        let captured = capture(&apt, &plan(vec![replace(LIST, Some(state(&target)))]))
            .expect("capture binds the plan to the disk");
        assert_eq!(captured.len(), 1);

        // The file grows between the plan and the capture, in place: same inode, more bytes.
        let ino = fs::symlink_metadata(&target).expect("meta").ino();
        fs::write(&target, vec![b'x'; MAX_FILE_BYTES + 1]).expect("grow in place");
        assert_eq!(fs::symlink_metadata(&target).expect("meta").ino(), ino);

        let err = match read_state(&target) {
            Ok(_) => panic!("an oversize file is accepted"),
            Err(e) => e,
        };
        let ApplyError::TooLarge { path, limit } = &err else {
            panic!("not a TooLarge: {err}");
        };
        assert_eq!(path, &target);
        assert_eq!(*limit, MAX_FILE_BYTES);
        let text = err.to_string();
        assert!(text.contains("Nothing was written"), "{text}");
        assert!(!text.contains("permission"), "{text}");
        assert!(!text.contains("Check that root can read"), "{text}");
        assert!(text.to_lowercase().contains("re-run"), "{text}");
        assert_eq!(
            fs::symlink_metadata(&target).expect("meta").len(),
            MAX_FILE_BYTES as u64 + 1
        );
    }

    /// Criterion 19: the directory flush that follows a landed replacement fails, and the run may
    /// not claim `/etc/apt` is clean. The fault is armed immediately before the write phase,
    /// because the backup flushes its own directories first and no other flush runs in between.
    #[test]
    fn a_failed_flush_after_a_landed_replace_names_the_file_the_run_wrote() {
        let root = TempRoot::new("flush");
        let apt = root.apt();
        let (uid, gid) = root.owner();
        let backup_root = root.path().join("backup");
        let target = fixture(&apt, LIST, OLD, 0o644);
        let before = state(&target);
        let planned = plan(vec![replace(LIST, Some(before.clone()))]);
        let captured = capture(&apt, &planned).expect("capture binds the plan to the disk");
        let new_bytes = captured[0].new_bytes.clone();
        let id = BackupId::from_secs(NOW).expect("backup id");
        backup(&backup_root, uid, gid, &id, &captured).expect("the backup succeeds");

        fail_next_sync_dir();
        let err = write_targets(&captured, uid, gid).expect_err("the flush fails");
        println!("flush failure: {err}");
        assert!(matches!(err, ApplyError::Write { .. }), "{err}");
        let text = err.to_string();

        // The replacement landed before the flush: its new bytes are on disk, through a rename,
        // and no temp file survives.
        let after = state(&target);
        assert_eq!(after.bytes, new_bytes);
        assert_ne!(after.ino, before.ino, "the rename happened");
        let left = entries(&apt);
        assert!(
            !left.iter().any(|n| n.starts_with(TEMP_PREFIX)),
            "a temp file survived: {left:?}"
        );
        // The error is the flush of the target's own directory, and it names what already landed.
        assert!(text.contains("cannot write"), "{text}");
        assert!(text.contains(&apt.display().to_string()), "{text}");
        assert!(text.contains("injected directory sync failure"), "{text}");
        assert!(text.contains("already changed sources.list"), "{text}");
        // The clean-system sentence this defect was about may not appear at all.
        assert!(
            !text.contains("had been changed before this failure"),
            "{text}"
        );
        assert!(!text.contains("Nothing was written"), "{text}");
    }

    /// Criterion 20: a cleanup that refuses to remove a half-written creation is folded into the
    /// refusal, so the message never reports a clean path with a partial file at it.
    #[test]
    fn a_failed_cleanup_of_a_creation_is_named_in_the_refusal() {
        let path = PathBuf::from(APT_DIR).join(SECURITY);
        let reason = "the record does not describe what was created";
        let text = ApplyError::Write {
            path: path.clone(),
            reason: reason.into(),
        }
        .cleanup_failed(io::Error::other("Read-only file system (os error 30)"))
        .to_string();
        println!("cleanup failure: {text}");
        assert!(text.contains(reason), "{text}");
        assert!(
            text.contains("Removing the partial file this run created then failed too"),
            "{text}"
        );
        assert!(
            text.contains("Read-only file system (os error 30)"),
            "{text}"
        );
        assert!(text.contains(&path.display().to_string()), "{text}");

        // A refusal that never created anything has no partial file to name and passes through.
        let unchanged = ApplyError::Changed {
            path: path.clone(),
            reason: "the bytes changed".into(),
        };
        let expected = unchanged.to_string();
        assert_eq!(
            unchanged
                .cleanup_failed(io::Error::other("Read-only file system (os error 30)"))
                .to_string(),
            expected
        );
    }

    /// Criterion 21: a directory the sweep cannot list is a read refusal that names that directory
    /// as itself, never a write refusal about the directory holding it.
    #[test]
    fn an_unlistable_directory_is_a_read_refusal_not_a_write_refusal() {
        let root = TempRoot::new("sweep-read");
        let apt = root.apt();
        let (uid, _) = root.owner();
        if uid == 0 {
            // Root lists a 0300 directory, so only a non-root run can build this case.
            return;
        }
        fs::set_permissions(&apt, fs::Permissions::from_mode(0o300)).expect("mode");
        let err = sweep_temp(&apt, uid).expect_err("the directory cannot be listed");
        fs::set_permissions(&apt, fs::Permissions::from_mode(0o755)).expect("mode");
        println!("sweep read failure: {err}");
        assert!(matches!(err, ApplyError::Read { .. }), "{err}");
        let text = err.to_string();
        assert!(
            text.starts_with(&format!("cannot read {}", apt.display())),
            "{text}"
        );
        assert!(!text.contains("cannot write"), "{text}");
        assert!(!text.contains("directory holding it"), "{text}");
        // The sweep runs before the write phase, so its scope is `/etc/apt` and nothing broader.
        assert!(text.contains("Nothing was written to /etc/apt"), "{text}");

        // A missing directory is still no finding at all.
        sweep_temp(&root.path().join("absent"), uid).expect("a missing directory holds nothing");
    }

    /// Criterion 22: every refusal a run can raise after the backup exists is scoped to `/etc/apt`,
    /// because the manifest of that backup is already on disk when it is printed.
    #[test]
    fn a_refusal_that_can_follow_the_backup_is_scoped_to_etc_apt() {
        let path = PathBuf::from(APT_DIR).join(LIST);
        let after_backup = [
            ApplyError::Read {
                path: path.clone(),
                reason: "Permission denied (os error 13)".into(),
            },
            ApplyError::TooLarge {
                path: path.clone(),
                limit: MAX_FILE_BYTES,
            },
            ApplyError::Target {
                path: path.clone(),
                reason: "it is a symlink".into(),
            },
            ApplyError::Owner {
                path: path.clone(),
                reason: "it is owned by uid 1000 gid 1000".into(),
            },
            ApplyError::Changed {
                path: path.clone(),
                reason: "the bytes changed".into(),
            },
        ];
        for err in &after_backup {
            let text = err.to_string();
            println!("scoped refusal: {text}");
            assert!(text.contains("Nothing was written to /etc/apt"), "{text}");
            assert!(!text.contains("Nothing was written."), "{text}");
        }

        // The backup refusal already had that scope, and keeps its own wording.
        let backup = ApplyError::Backup {
            path,
            reason: "its mode is 0o777, not 0o700".into(),
        };
        assert!(
            backup
                .to_string()
                .contains("Nothing was written to /etc/apt"),
            "{backup}"
        );
    }

    /// The security source a mirror run creates, as `create()` plans it.
    const CREATED: &[u8] = b"Types: deb\nURIs: https://m.example/ubuntu\nSuites: noble-security\n";

    /// Criterion 23: a restore replaces before it removes, whatever order its plan lists.
    #[test]
    fn a_restore_replaces_before_it_removes() {
        let root = TempRoot::new("restore-order");
        let apt = root.apt();
        let backup = root.path().join("backup");
        let (uid, gid) = root.owner();
        let list = fixture(&apt, LIST, OLD, 0o644);
        let security = fixture(&apt, SECURITY, CREATED, FILE_MODE);
        // The plan lists the removal first, so only the writer's phase order can produce
        // replace-then-remove.
        let planned = restore_plan(vec![
            remove(SECURITY, Some(state(&security))),
            replace(LIST, Some(state(&list))),
        ]);
        assert_eq!(planned.writes()[0].effect, Effect::Remove);

        let outcome =
            run(&apt, &backup, &planned, None, NOW, uid, gid).expect("the restore writes");
        let touched: Vec<String> = outcome
            .touched
            .iter()
            .map(|t| {
                let path = t.target.relative().display().to_string();
                match t.effect {
                    Effect::Remove => format!("removed {path}"),
                    Effect::Create | Effect::Replace => format!("wrote {path}"),
                }
            })
            .collect();
        assert_eq!(
            touched,
            [format!("wrote {LIST}"), format!("removed {SECURITY}")],
            "every replaceable byte lands before the only destroying operation"
        );
        assert_eq!(
            fs::read(&list).expect("bytes"),
            planned.writes()[1].new_bytes
        );
        assert!(!security.exists(), "the removal landed");

        // A restore is itself a write: it backed up what it replaced under its own id, and its
        // manifest describes that content, so the restore is undoable by the same rules. The
        // manifest is not parsed here because a non-root fixture cannot hold the root ownership
        // `manifest::parse` requires; the digests it carries are what this asserts.
        let path = backup.join(BackupId::from_secs(NOW).expect("id").as_str());
        let text = fs::read_to_string(path.join(MANIFEST)).expect("the restore's own manifest");
        assert!(text.contains(&hex(&sha256(CREATED))), "{text}");
        assert!(text.contains(&hex(&sha256(OLD))), "{text}");
        assert_eq!(
            fs::read(path.join(SECURITY)).expect("the copy of the removed file"),
            CREATED
        );
    }

    /// Criterion 24: a removal is refused when the file changed between validation and the unlink,
    /// which is the S6 rule this phase must not weaken by unlink's hindsight.
    #[test]
    fn a_removal_is_refused_when_the_disk_changed_after_validation() {
        let root = TempRoot::new("restore-swap");
        let apt = root.apt();
        let (uid, gid) = root.owner();
        let security = fixture(&apt, SECURITY, CREATED, FILE_MODE);
        let planned = restore_plan(vec![remove(SECURITY, Some(state(&security)))]);
        let captured = capture(&apt, &planned).expect("capture binds the plan to the disk");
        validate_targets(&captured).expect("the disk is the plan's disk");

        // The content is rewritten in place after validation: same inode, other bytes.
        let swapped = b"Types: deb\nURIs: http://elsewhere.example/ubuntu\n";
        fs::write(&security, swapped).expect("swap the content in place");

        let err = write_targets(&captured, uid, gid).expect_err("the removal is refused");
        println!("removal refused: {err}");
        assert!(matches!(err, ApplyError::Changed { .. }), "{err}");
        let text = err.to_string();
        assert!(text.contains("the bytes changed"), "{text}");
        // Refused, not unlinked: the file is still there, with what was written to it.
        assert_eq!(fs::read(&security).expect("bytes"), swapped);
    }

    /// Criterion 25: a removal leaves no temp file behind, and its directory is flushed after it
    /// lands rather than before it.
    #[test]
    fn a_removal_leaves_no_temp_file_and_flushes_the_directory() {
        // (a) the happy path: the unlink leaves nothing of ours in either directory.
        let root = TempRoot::new("restore-remove");
        let apt = root.apt();
        let (uid, gid) = root.owner();
        let security = fixture(&apt, SECURITY, CREATED, FILE_MODE);
        let planned = restore_plan(vec![remove(SECURITY, Some(state(&security)))]);
        let captured = capture(&apt, &planned).expect("capture binds the plan to the disk");
        let touched = write_targets(&captured, uid, gid).expect("the removal lands");
        assert_eq!(
            touched,
            [Touched {
                target: Target::parse(SECURITY).expect("target"),
                effect: Effect::Remove,
            }]
        );
        assert!(!security.exists());
        for dir in [apt.clone(), apt.join("sources.list.d")] {
            let left = entries(&dir);
            assert!(
                !left.iter().any(|n| n.starts_with(TEMP_PREFIX)),
                "a temp file survived: {left:?}"
            );
        }

        // (b) the flush that follows: armed immediately before the removal, it is reported as a
        // failure *after* a landed write and never as a clean system.
        let root = TempRoot::new("restore-flush");
        let apt = root.apt();
        let (uid, gid) = root.owner();
        let security = fixture(&apt, SECURITY, CREATED, FILE_MODE);
        let planned = restore_plan(vec![remove(SECURITY, Some(state(&security)))]);
        let captured = capture(&apt, &planned).expect("capture binds the plan to the disk");
        fail_next_sync_dir();
        let err = write_targets(&captured, uid, gid).expect_err("the flush fails");
        println!("removal flush failure: {err}");
        assert!(matches!(err, ApplyError::Write { .. }), "{err}");
        let text = err.to_string();
        assert!(text.contains("injected directory sync failure"), "{text}");
        assert!(
            text.contains("already changed sources.list.d/system-security.sources"),
            "{text}"
        );
        assert!(!text.contains("Nothing was written"), "{text}");
        assert!(!security.exists(), "the removal landed before the flush");
    }

    /// The authority split, without weakening D25: a mirror plan still carries its proof and is
    /// still held against a verified baseline, while a restore plan is authorized by its backup.
    #[test]
    fn a_mirror_plan_still_needs_its_baseline_and_a_restore_plan_does_not() {
        let root = TempRoot::new("authority");
        let apt = root.apt();
        let backup = root.path().join("backup");
        let (uid, gid) = root.owner();
        let list = fixture(&apt, LIST, OLD, 0o644);

        // A mirror plan asked to write without a baseline is refused before anything is read.
        let mirror = plan(vec![replace(LIST, Some(state(&list)))]);
        let err = run(&apt, &backup, &mirror, None, NOW, uid, gid).expect_err("no baseline");
        assert!(matches!(err, ApplyError::NoBaseline), "{err}");
        // A mirror staler than the baseline is still refused, through the same predicate.
        let late = verified(LATE);
        let err = run(&apt, &backup, &mirror, Some(&late), NOW, uid, gid).expect_err("stale");
        assert!(matches!(err, ApplyError::Stale { .. }), "{err}");
        assert!(
            !backup.exists(),
            "nothing was backed up and nothing was written"
        );

        // The same bytes through a restore plan need no baseline at all.
        let restore = restore_plan(vec![replace(LIST, Some(state(&list)))]);
        let outcome = run(&apt, &backup, &restore, None, NOW, uid, gid)
            .expect("a restore is authorized by its backup, not by a mirror");
        assert_eq!(outcome.touched.len(), 1);
        assert_eq!(
            fs::read(&list).expect("bytes"),
            restore.writes()[0].new_bytes
        );
    }
}
