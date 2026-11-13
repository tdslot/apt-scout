#![cfg_attr(not(test), allow(dead_code))] // the CLI reaches this writer in D11
//! The privileged writer for `/etc/apt`.
//!
//! Nothing here runs unless the process is root, and no path is written until it is
//! bound to the disk entry the plan was built from. The order is: validate the plan,
//! capture and bind to the disk, back up, re-validate, write.

use std::fmt;
use std::fs::{self, DirBuilder, File, OpenOptions};
use std::io::{self, Read, Write};
use std::os::unix::fs::{DirBuilderExt, MetadataExt, OpenOptionsExt, PermissionsExt, fchown};
use std::path::{Path, PathBuf};

use crate::digest::sha256;
use crate::manifest::{self, Action, BackupId, ManifestEntry};
use crate::plan::{FileState, Plan, PlanError, Target};

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
    /// A planned path could not be inspected, opened, or read.
    Read { path: PathBuf, reason: String },
    /// A planned path is not a plain regular file apt-scout is willing to own.
    Target { path: PathBuf, reason: String },
    /// A planned path no longer matches the state the plan was built from.
    Changed { path: PathBuf, reason: String },
    /// A path that must not exist yet does exist; nothing is ever overwritten on `EEXIST`.
    Exists { path: PathBuf },
    /// The backup of a file could not be written, so nothing under `/etc/apt` may be.
    Backup { path: PathBuf, reason: String },
    /// The new content of a planned file could not be written.
    Write { path: PathBuf, reason: String },
    /// The run's timestamp cannot name a backup directory.
    Time { secs: u64 },
}

impl ApplyError {
    /// The process exit code that reports this refusal.
    pub fn exit_code(&self) -> u8 {
        match self {
            Self::NotRoot => 3,
            Self::Plan(_)
            | Self::Read { .. }
            | Self::Target { .. }
            | Self::Changed { .. }
            | Self::Exists { .. }
            | Self::Backup { .. }
            | Self::Write { .. }
            | Self::Time { .. } => 1,
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
            Self::Read { path, reason } => write!(
                f,
                "cannot read {}: {reason}. Nothing was written. Check that root can read the file \
                 and re-run",
                path.display()
            ),
            Self::Target { path, reason } => write!(
                f,
                "refusing {}: {reason}. Nothing was written. apt-scout only replaces a plain \
                 regular file it can reach without following a link, so move the link or the \
                 special file aside and re-run",
                path.display()
            ),
            Self::Changed { path, reason } => write!(
                f,
                "{} changed since the plan was built: {reason}. Nothing was written. Re-run \
                 apt-scout so its plan is built from what is on disk now",
                path.display()
            ),
            Self::Exists { path } => write!(
                f,
                "{} already exists and apt-scout never overwrites a path it did not plan to \
                 replace. Nothing was written. Remove or rename it and re-run",
                path.display()
            ),
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
                "cannot write {}: {reason}. Nothing was left half-written: apt-scout writes a \
                 temporary file and renames it. Check the permissions of the directory that holds \
                 it and re-run",
                path.display()
            ),
            Self::Time { secs } => write!(
                f,
                "cannot name a backup from {secs} seconds after the epoch: that is past year 9999. \
                 Nothing was written. Check the system clock and re-run"
            ),
        }
    }
}

/// What one successful run did: the backup it made and the files it wrote.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Outcome {
    /// The backup every replaced file was copied into, before the first write.
    pub backup_id: BackupId,
    /// The targets this run wrote, in the order it wrote them.
    pub written: Vec<Target>,
}

/// Write the plan, as root, to the real `/etc/apt` and `/var/backups/apt-scout`.
///
/// Refuses before its first byte on a non-root run, on a plan that does not validate, and on any
/// difference between the plan and the disk.
pub fn apply(plan: &Plan, now_secs: u64) -> Result<Outcome, ApplyError> {
    require_root()?;
    run(
        Path::new(APT_DIR),
        Path::new(BACKUP_ROOT),
        plan,
        now_secs,
        0,
    )
}

/// The one write path, with the two roots and the backup owner as parameters.
///
/// Private: it is the only place where the backup owner is not `0`, so the "the backup belongs to
/// root" rule cannot be reached from production code with any other value.
fn run(
    apt_root: &Path,
    backup_root: &Path,
    plan: &Plan,
    now_secs: u64,
    owner_uid: u32,
) -> Result<Outcome, ApplyError> {
    plan.validate().map_err(ApplyError::Plan)?;
    let captured = capture(apt_root, plan)?;
    let id = BackupId::from_secs(now_secs).map_err(|_| ApplyError::Time { secs: now_secs })?;
    backup(backup_root, owner_uid, &id, &captured)?;
    validate_targets(&captured)?;
    let written = write_targets(&captured)?;
    Ok(Outcome {
        backup_id: id,
        written,
    })
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
    create_new: bool,
}

/// What the disk currently holds at one path.
enum Entry {
    Missing,
    Regular(FileState),
    NotRegular(String),
}

/// Resolve one planned target against the apt root it is relative to.
fn resolve(apt_root: &Path, target: &Target) -> PathBuf {
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
            create_new: w.create_new,
        });
    }
    Ok(captured)
}

/// Refuse a target whose directory is a symlink or not a directory.
///
/// `lstat` of a path does not follow its last component, so a symlinked `sources.list.d` would
/// otherwise be resolved by the kernel when the target is opened or renamed into.
fn check_parent(path: &Path) -> Result<(), ApplyError> {
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
    id: &BackupId,
    captured: &[Captured],
) -> Result<(), ApplyError> {
    verify_backup_root(backup_root, owner_uid)?;
    let dir = backup_root.join(id.as_str());
    DirBuilder::new().mode(DIR_MODE).create(&dir).map_err(|e| {
        if e.kind() == io::ErrorKind::AlreadyExists {
            // Two runs inside one second: refusing beats overwriting a backup.
            ApplyError::Exists { path: dir.clone() }
        } else {
            backup_failed(&dir, e)
        }
    })?;
    // A new file gets the run's effective gid. std exposes no `getegid`, and `unsafe` is reserved
    // for `geteuid`, so the gid is read from the directory this run just created: a directory
    // without the set-group-id bit carries the creating process's gid.
    let owner_gid = fs::symlink_metadata(&dir)
        .map_err(|e| backup_failed(&dir, e))?
        .gid();
    for c in captured.iter().filter(|c| c.original.is_some()) {
        copy_original(&dir, c)?;
    }
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
            Ok(()) => {}
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

/// Write every captured target: the creations first, then the replacements.
///
/// The order is the security property: a creation that finds the path taken aborts the whole run
/// while every other target is still untouched.
fn write_targets(captured: &[Captured]) -> Result<Vec<Target>, ApplyError> {
    let mut written = Vec::with_capacity(captured.len());
    for c in captured.iter().filter(|c| c.create_new) {
        create_one(c)?;
        written.push(c.target.clone());
    }
    for c in captured.iter().filter(|c| !c.create_new) {
        replace_one(c)?;
        written.push(c.target.clone());
    }
    Ok(written)
}

/// Create a planned file with an exclusive create, and never overwrite what is there.
fn create_one(c: &Captured) -> Result<(), ApplyError> {
    let file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(FILE_MODE)
        .open(&c.path)
        .map_err(|e| match e.kind() {
            io::ErrorKind::AlreadyExists => ApplyError::Exists {
                path: c.path.clone(),
            },
            _ => write_failed(&c.path, e),
        })?;
    let mut handle = &file;
    handle
        .write_all(&c.new_bytes)
        .map_err(|e| write_failed(&c.path, e))?;
    // The manifest records mode 0600, so the file must carry it whatever the umask is.
    file.set_permissions(fs::Permissions::from_mode(FILE_MODE))
        .map_err(|e| write_failed(&c.path, e))?;
    file.sync_all().map_err(|e| write_failed(&c.path, e))?;
    sync_dir(parent_dir(&c.path)?, write_failed)
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
    sync_dir(dir, write_failed)
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
    Err(ApplyError::Exists { path: leftover })
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

/// Inspect one path without following a symlink and without reading it twice.
///
/// The state comes from the open handle, not from the `lstat`, so a swap between the
/// two cannot smuggle a different file into the write.
fn read_state(path: &Path) -> Result<Entry, ApplyError> {
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
        return Err(ApplyError::Read {
            path: path.to_path_buf(),
            reason: format!("larger than the {MAX_FILE_BYTES} byte limit"),
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

/// Flush one directory, so a name it gained or lost survives a crash.
fn sync_dir(dir: &Path, fail: fn(&Path, io::Error) -> ApplyError) -> Result<(), ApplyError> {
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
    use crate::plan::{PlannedWrite, Platform};
    use crate::verify::SignatureStatus;
    use std::os::unix::fs::symlink;

    const LIST: &str = "sources.list";
    const SECURITY: &str = "sources.list.d/system-security.sources";
    const RFC: &str = "Thu, 12 Nov 2026  4:23:38 UTC";
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
        /// this uid as the backup owner and the tests never need root.
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

    fn probe() -> Probed {
        let date = signed(RFC);
        Probed {
            url: "https://m.example/ubuntu".to_string(),
            updates_date: date.seconds(),
            signed_updates_date: Some(date),
            lag_secs: 0,
            throughput_bps: 0,
            latency_ms: 0,
            same_as: false,
            signature: SignatureStatus::Verified,
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
            create_new: false,
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
            create_new: true,
        }
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

    #[test]
    fn a_non_root_run_is_refused_with_the_sudo_command() {
        let err = ApplyError::NotRoot;
        let line = err.to_string();
        assert!(line.contains("sudo apt-scout"), "{line}");
        assert!(!line.contains('\n'), "{line}");
        assert_eq!(err.exit_code(), 3);

        let root = TempRoot::new("root");
        let (uid, _) = root.owner();
        assert_eq!(require_root().is_ok(), uid == 0);
        // `apply` is the root-only entry point: as a non-root caller it must refuse before it
        // reads anything, so no part of /etc/apt is touched here.
        assert_eq!(APT_DIR, "/etc/apt");
        assert_eq!(BACKUP_ROOT, "/var/backups/apt-scout");
        if uid != 0 {
            let plan = plan(vec![create(SECURITY)]);
            assert!(matches!(apply(&plan, NOW), Err(ApplyError::NotRoot)));
        }

        // Every other refusal is a runtime error whose message says nothing was written.
        let path = PathBuf::from("/etc/apt/sources.list");
        let others = [
            ApplyError::Plan(PlanError::NoProof),
            ApplyError::Read {
                path: path.clone(),
                reason: "Permission denied (os error 13)".into(),
            },
            ApplyError::Target {
                path: path.clone(),
                reason: "it is a symlink".into(),
            },
            ApplyError::Changed {
                path: path.clone(),
                reason: "the bytes changed".into(),
            },
            ApplyError::Exists { path: path.clone() },
            ApplyError::Backup {
                path: path.clone(),
                reason: "its mode is 0o777, not 0o700".into(),
            },
            ApplyError::Write {
                path: path.clone(),
                reason: "Permission denied (os error 13)".into(),
            },
            ApplyError::Time {
                secs: 253_402_300_800,
            },
        ];
        for err in &others {
            assert_eq!(err.exit_code(), 1, "{err}");
            let text = err.to_string();
            // Every refusal says what to do next; none of them is a bare OS error.
            assert!(text.to_lowercase().contains("re-run"), "{text}");
            if !matches!(err, ApplyError::Plan(_) | ApplyError::Time { .. }) {
                assert!(text.contains("/etc/apt/sources.list"), "{text}");
            }
        }
        // The OS error is carried verbatim, and a backup failure says what to fix.
        for at in [1, 6] {
            let text = others[at].to_string();
            assert!(text.contains("Permission denied (os error 13)"), "{text}");
        }
        assert!(others[5].to_string().contains("0700"), "{}", others[5]);
    }

    #[test]
    fn capture_records_a_mode_without_the_file_type_bits() {
        let root = TempRoot::new("mode");
        let apt = root.apt();
        let path = fixture(&apt, LIST, OLD, 0o755);
        // `st_mode` carries the file type as well, so an unmasked record of this fixture would be
        // 0o100755 on both Linux and macOS.
        assert_eq!(fs::symlink_metadata(&path).expect("meta").mode(), 0o100755);
        let captured = capture(&apt, &plan(vec![replace(LIST, Some(state(&path)))]))
            .expect("capture binds the plan to the disk");
        let recorded = captured[0].original.as_ref().expect("original").mode;
        assert_eq!(recorded, 0o755);
        assert_eq!(recorded & !MODE_MASK, 0);
    }

    #[test]
    fn capture_refuses_a_plan_that_disagrees_with_the_disk() {
        let root = TempRoot::new("bind");
        let apt = root.apt();
        let backup = root.path().join("backup");
        let (uid, _) = root.owner();
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
            NOW,
            uid,
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
            NOW,
            uid,
        )
        .expect_err("a missing original is refused");
        assert!(matches!(err, ApplyError::Changed { .. }), "{err}");
        assert!(!target.exists());
        assert!(!backup.exists(), "{backup:?} must not exist");
    }

    #[test]
    fn a_hostile_symlink_is_refused_and_the_canary_stays() {
        let root = TempRoot::new("link");
        let apt = root.apt();
        let outside = root.path().join("outside");
        fs::create_dir_all(outside.join("sources.list.d")).expect("outside tree");
        let backup = root.path().join("backup");
        let (uid, _) = root.owner();
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
            NOW,
            uid,
        )
        .expect_err("a symlinked target is refused");
        assert!(matches!(err, ApplyError::Target { .. }), "{err}");
        assert_eq!(fs::read(&canary).expect("canary"), b"canary\n");
        assert!(!backup.exists(), "{backup:?} must not exist");

        // (b) `sources.list.d` is a symlink, so the target resolves outside the tree.
        fs::remove_file(apt.join(LIST)).expect("remove the link");
        symlink(outside.join("sources.list.d"), apt.join("sources.list.d")).expect("symlink");
        let err = run(
            &apt,
            &backup,
            &plan(vec![replace(
                "sources.list.d/escape.list",
                Some(state(&escaped)),
            )]),
            NOW,
            uid,
        )
        .expect_err("a symlinked directory is refused");
        assert!(matches!(err, ApplyError::Target { .. }), "{err}");
        assert_eq!(fs::read(&escaped).expect("canary"), b"escaped\n");
        assert!(!backup.exists(), "{backup:?} must not exist");
    }

    #[test]
    fn a_hostile_backup_root_is_refused_and_stays_empty() {
        let root = TempRoot::new("backuproot");
        let apt = root.apt();
        let (uid, _) = root.owner();
        let target = fixture(&apt, LIST, OLD, 0o644);
        let before = state(&target);
        let writes = || plan(vec![replace(LIST, Some(before.clone()))]);

        // (a) the backup root is a symlink to a directory outside the tree.
        let abroad = root.path().join("abroad");
        fs::create_dir(&abroad).expect("abroad");
        let link = root.path().join("backup-link");
        symlink(&abroad, &link).expect("symlink");
        let err = run(&apt, &link, &writes(), NOW, uid).expect_err("a symlink is refused");
        assert!(matches!(err, ApplyError::Backup { .. }), "{err}");
        assert_eq!(entries(&abroad), Vec::<String>::new());

        // (b) the backup root belongs to another uid: root runs as uid 0, so this is what a
        // directory planted by a local user looks like.
        let foreign = root.path().join("backup-foreign");
        fs::create_dir(&foreign).expect("foreign root");
        fs::set_permissions(&foreign, fs::Permissions::from_mode(DIR_MODE)).expect("mode");
        let err = run(&apt, &foreign, &writes(), NOW, uid.wrapping_add(1))
            .expect_err("another owner is refused");
        assert!(matches!(err, ApplyError::Backup { .. }), "{err}");
        assert_eq!(entries(&foreign), Vec::<String>::new());

        // (c) the backup root is writable by everyone.
        let loose = root.path().join("backup-loose");
        fs::create_dir(&loose).expect("loose root");
        fs::set_permissions(&loose, fs::Permissions::from_mode(0o777)).expect("mode");
        let err = run(&apt, &loose, &writes(), NOW, uid).expect_err("mode 0777 is refused");
        assert!(matches!(err, ApplyError::Backup { .. }), "{err}");
        assert!(err.to_string().contains("0o777"), "{err}");
        assert_eq!(entries(&loose), Vec::<String>::new());

        assert_eq!(state(&target), before);
    }

    #[test]
    fn a_backup_that_cannot_be_written_leaves_every_target_alone() {
        let root = TempRoot::new("backupfail");
        let apt = root.apt();
        let (uid, _) = root.owner();
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
            NOW,
            uid,
        )
        .expect_err("an unwritable backup root fails the run");
        assert!(matches!(err, ApplyError::Backup { .. }), "{err}");
        // The write phase runs after the backup, so every target is still byte-identical.
        assert_eq!(state(&target), before);
        assert_eq!(entries(&backup), Vec::<String>::new());
    }

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

    #[test]
    fn a_file_that_appears_where_the_run_creates_aborts_the_whole_run() {
        let root = TempRoot::new("exists");
        let apt = root.apt();
        let list = fixture(&apt, LIST, OLD, 0o644);
        let before = state(&list);
        let planned = plan(vec![replace(LIST, Some(before.clone())), create(SECURITY)]);
        let captured = capture(&apt, &planned).expect("capture binds the plan to the disk");

        // The window between the capture and the write: someone else creates the file first.
        let created = fixture(&apt, SECURITY, b"planted\n", 0o644);

        let err = write_targets(&captured).expect_err("the creation finds the path taken");
        assert!(matches!(err, ApplyError::Exists { .. }), "{err}");
        // The creations run before the replacements, so the other target is untouched too.
        assert_eq!(fs::read(&created).expect("planted"), b"planted\n");
        assert_eq!(state(&list), before);
    }

    #[test]
    fn a_replace_keeps_content_mode_and_owner_and_leaves_no_temp_file() {
        let root = TempRoot::new("replace");
        let apt = root.apt();
        let (uid, _) = root.owner();
        let backup = root.path().join("backup");
        let target = fixture(&apt, LIST, OLD, 0o640);
        let before = state(&target);
        let planned = plan(vec![replace(LIST, Some(before.clone()))]);
        let new_bytes = planned.writes()[0].new_bytes.clone();

        let outcome = run(&apt, &backup, &planned, NOW, uid).expect("the replacement succeeds");
        assert_eq!(outcome.written, vec![Target::parse(LIST).expect("target")]);
        assert_eq!(outcome.backup_id.as_str(), ID);

        let after = state(&target);
        assert_eq!(after.bytes, new_bytes);
        assert_eq!(after.mode, 0o640);
        assert_eq!((after.uid, after.gid), (before.uid, before.gid));
        // A rename, not a write in place: the file the plan was built from is gone.
        assert_ne!(after.ino, before.ino);
        assert_eq!(entries(&apt), vec![LIST.to_string()]);
    }

    #[test]
    fn a_created_file_has_mode_0600_and_a_create_record() {
        let root = TempRoot::new("create");
        let apt = root.apt();
        // A stock install has `sources.list.d`; the run creates the file in it, never the directory.
        fs::create_dir(apt.join("sources.list.d")).expect("sources.list.d");
        let (uid, _) = root.owner();
        let backup = root.path().join("backup");
        let planned = plan(vec![create(SECURITY)]);
        let new_bytes = planned.writes()[0].new_bytes.clone();

        let outcome = run(&apt, &backup, &planned, NOW, uid).expect("the creation succeeds");
        let created = apt.join(SECURITY);
        assert_eq!(fs::read(&created).expect("bytes"), new_bytes);
        let meta = fs::symlink_metadata(&created).expect("meta");
        assert_eq!(meta.mode() & MODE_MASK, 0o600);
        assert_eq!(
            outcome.written,
            vec![Target::parse(SECURITY).expect("target")]
        );

        // The record describes what this run created: the new bytes' digest, 0600 and the owner
        // the file actually carries.
        let text = fs::read_to_string(backup.join(ID).join(MANIFEST)).expect("manifest");
        let line = format!(
            "file {SECURITY} create {} 384 {} {}",
            hex(&sha256(&new_bytes)),
            meta.uid(),
            meta.gid()
        );
        assert!(text.contains(&line), "{text}");
    }

    #[test]
    fn the_backup_holds_every_replaced_file_and_its_manifest() {
        let root = TempRoot::new("backup");
        let apt = root.apt();
        let (uid, _) = root.owner();
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

        let outcome = run(&apt, &backup, &planned, NOW, uid).expect("the write succeeds");
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
        // The backup root and every directory inside it are 0700, the manifest is 0600.
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
        // `manifest::parse` defines the format but accepts nothing except root-owned records, so
        // only a root run can read this manifest back.
        if uid == 0 {
            let parsed = manifest::parse(&text).expect("parse the manifest back");
            assert_eq!(parsed.id(), &outcome.backup_id);
            assert_eq!(parsed.entries().len(), 2);
            assert_eq!(parsed.entries()[0].digest, sha256(&list_state.bytes));
        }
    }
}
