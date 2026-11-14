#![cfg_attr(not(test), allow(dead_code))] // the CLI reaches this reader in D11b
//! Re-derive a plan from the newest backup. The manifest is the source of truth, not a list.
//!
//! Nothing here is reachable from a CLI path yet, and nothing here trusts a disk: the backup root,
//! the directory, the `MANIFEST` and every copy it names are checked against `docs/threat-model.md`
//! §8 G1 (a root-owned regular file in mode 0600, never a symlink, in a root-owned 0700 directory)
//! before a byte of the manifest is parsed, and then every record is re-derived against the copy it
//! names and against what `/etc/apt` holds now. The first disagreement refuses the whole restore
//! rather than falling back to an older backup or to a subset of the plan.
//!
//! The plan this module returns is the plan the ordinary writer applies, so the write re-reads the
//! same targets a second time and `capture` is what refuses a swap between the two reads. [`run`] is
//! the one production entry point: it builds that plan and hands it to the writer with no baseline,
//! because what authorizes a restore is the backup the plan was re-derived from.

use std::fs::{self, File, Metadata};
use std::io::{self, Read};
use std::os::unix::fs::MetadataExt;
use std::path::Path;

use crate::apply::{self, ApplyError, Entry, Outcome};
use crate::digest::sha256;
use crate::manifest::{self, Action, BackupId, ManifestEntry};
use crate::plan::{Effect, Plan, PlannedWrite};
use crate::verify::VerifiedDate;

/// A backup this module has read and re-derived, and the only thing that authorizes a restore.
///
/// The witness a restore needs, in the shape a mirror's proof already has: the field is private to
/// this module, so the token cannot be built from a bare [`BackupId`] that nobody read, and
/// [`Plan::for_restore`] cannot be reached from a caller that skipped the manifest, the copies and
/// the disk. `build` is the one production producer, and `build_as` the one place it is built.
pub struct ValidatedBackup {
    id: BackupId,
}

impl ValidatedBackup {
    /// The backup the token was minted for: the id a restore plan carries and a run reports.
    pub fn id(&self) -> &BackupId {
        &self.id
    }
}

/// A token for a test outside this module, built the one way `build_as` builds one.
///
/// Test-only, so a shipped build has exactly one producer: the alternative is a public constructor,
/// which is the hole this type exists to close.
#[cfg(test)]
pub(crate) fn validated_backup_for_test(id: BackupId) -> ValidatedBackup {
    ValidatedBackup { id }
}

/// The manifest's fixed name inside a backup, and the only thing that makes a directory a backup.
const MANIFEST: &str = "MANIFEST";

/// The mode of the backup root and of every backup directory below it.
const DIR_MODE: u32 = 0o700;

/// The mode of the manifest, and of every copy the writer made.
const FILE_MODE: u32 = 0o600;

/// `st_mode` also carries the file-type bits, so a comparison of modes must mask them off.
const MODE_MASK: u32 = 0o7777;

/// Read the newest backup and return the plan that puts it back.
///
/// Root-only, like the read it performs and the write that follows it: the manifest is under
/// `/var/backups/apt-scout`, which only root can read, so a non-root run is refused before the
/// backup is opened rather than after it is refused.
pub fn build(backup_root: &Path, apt_root: &Path) -> Result<Plan, ApplyError> {
    apply::require_root()?;
    build_as(backup_root, apt_root, 0, 0)
}

/// The one restore builder, with the expected owner of the backup as a parameter.
///
/// Private: it is the only place where the expected owner is not `0`, so the G1 ownership rules
/// cannot be reached from production code with any other value — the same shape as `apply::run`.
fn build_as(
    backup_root: &Path,
    apt_root: &Path,
    owner_uid: u32,
    owner_gid: u32,
) -> Result<Plan, ApplyError> {
    check_backup_root(backup_root, owner_uid)?;
    let id = newest_id(backup_root)?;
    let dir = backup_root.join(id.as_str());
    check_directory(&dir, owner_uid)?;
    let manifest_path = dir.join(MANIFEST);
    let text = read_manifest(&manifest_path, owner_uid, owner_gid)?;
    let manifest = manifest::parse(&text)
        .map_err(|e| refusal(&manifest_path, format!("its content is refused: {e}")))?;
    // A manifest carried over from another backup directory describes a different run, and the
    // copies beside it would then be justified by a record written for something else.
    if manifest.id() != &id {
        return Err(refusal(
            &manifest_path,
            format!(
                "it names backup {} and it is in the directory of backup {}",
                manifest.id().as_str(),
                id.as_str()
            ),
        ));
    }
    let writes = derive(&dir, apt_root, manifest.entries(), owner_uid, owner_gid)?;
    Plan::for_restore(&ValidatedBackup { id }, writes).map_err(ApplyError::Plan)
}

/// Put the newest backup back: build its plan, then hand it to the one writer.
///
/// Root-only, like the read and the write it spans, and the read is checked first: the manifest is
/// root's, so a non-root run is refused before the backup is opened. The writer's second argument is
/// a mirror's freshness baseline, and a restore has no mirror: what authorizes its write is the
/// backup `build_as` just re-derived, so this path passes `None` and never derives anything else to
/// pass. `now` names the backup this restore makes before it writes, the same way the mirror path
/// names its own. Both roots bind the read only, because `apply::apply` is the one production writer
/// and it writes to the crate's fixed `/etc/apt` and `/var/backups/apt-scout`: that asymmetry is why
/// a test reaches this path through `run_as` with `apply::run_for_test`.
pub fn run(backup_root: &Path, apt_root: &Path) -> Result<Outcome, ApplyError> {
    apply::require_root()?;
    // The clock is the mirror path's, so the two cannot drift; its refusal is typed for `main`, so
    // it is mapped into this module's error instead of the idiom being spelled a second time.
    let now = crate::now_secs().map_err(|(_, reason)| refusal(backup_root, reason))?;
    run_as(backup_root, apt_root, now, 0, 0, apply::apply)
}

/// The one restore path, with the clock, the run's owner and the writer as parameters.
///
/// Private for the same reason `build_as` is: `0, 0` is the only owner production reaches, and
/// `apply::apply` the only writer it hands the plan to, because the writer keeps one entry point for
/// both forms. A unit test is not root and never will be, so it passes `apply::run_for_test` here
/// and can watch the baseline this function hands over, which for a restore is always `None`.
fn run_as(
    backup_root: &Path,
    apt_root: &Path,
    now_secs: u64,
    owner_uid: u32,
    owner_gid: u32,
    write: impl FnOnce(&Plan, Option<&VerifiedDate>, u64) -> Result<Outcome, ApplyError>,
) -> Result<Outcome, ApplyError> {
    let plan = build_as(backup_root, apt_root, owner_uid, owner_gid)?;
    // A restore is authorized by its backup, not by a mirror, so there is no baseline to hand over.
    write(&plan, None, now_secs)
}

/// Refuse a backup file apt-scout did not write, naming the path and the fact that failed.
fn refusal(path: &Path, reason: impl Into<String>) -> ApplyError {
    ApplyError::Restore {
        path: path.to_path_buf(),
        reason: reason.into(),
    }
}

/// Refuse a backup root that is missing or not the root-owned 0700 directory a run left behind.
///
/// Read-only: unlike the writer's own root check, a restore never creates this directory, because
/// there is nothing to restore from an empty one.
fn check_backup_root(root: &Path, owner_uid: u32) -> Result<(), ApplyError> {
    match fs::symlink_metadata(root) {
        Ok(_) => check_directory(root, owner_uid),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Err(refusal(
            root,
            "no backup has been written on this machine, so there is nothing to restore",
        )),
        Err(e) => Err(refusal(root, e.to_string())),
    }
}

/// Refuse a directory that is not a root-owned 0700 directory apt-scout wrote itself.
///
/// The backup root and every `<id>` below it carry the same shape, so one rule covers both.
fn check_directory(path: &Path, owner_uid: u32) -> Result<(), ApplyError> {
    let meta = fs::symlink_metadata(path).map_err(|e| refusal(path, e.to_string()))?;
    if meta.file_type().is_symlink() {
        return Err(refusal(
            path,
            "it is a symlink, and apt-scout never follows one",
        ));
    }
    if !meta.is_dir() {
        return Err(refusal(path, "it is not a directory"));
    }
    if meta.uid() != owner_uid {
        return Err(refusal(
            path,
            format!("it is owned by uid {}, not uid {owner_uid}", meta.uid()),
        ));
    }
    let mode = meta.mode() & MODE_MASK;
    if mode != DIR_MODE {
        return Err(refusal(
            path,
            format!("its mode is {mode:#o}, not {DIR_MODE:#o}"),
        ));
    }
    Ok(())
}

/// The newest backup: the greatest id by string order, and the only one this run considers.
///
/// The ids are zero-padded UTC timestamps, so string order is time order. A name that is not an id
/// is ignored, because the root may hold other files; a name that is an id must be a real directory
/// and not a symlink, because it is a backup a restore may be asked for. A refusal here is never
/// answered by an older backup: that would restore files the operator did not ask for.
fn newest_id(root: &Path) -> Result<BackupId, ApplyError> {
    let entries = fs::read_dir(root).map_err(|e| refusal(root, e.to_string()))?;
    let mut newest: Option<BackupId> = None;
    for entry in entries {
        let entry = entry.map_err(|e| refusal(root, e.to_string()))?;
        // A name that is not UTF-8 cannot be one this program wrote.
        let Some(name) = entry.file_name().to_str().map(str::to_string) else {
            continue;
        };
        let Ok(id) = BackupId::parse(&name) else {
            continue;
        };
        let path = root.join(id.as_str());
        let meta = fs::symlink_metadata(&path).map_err(|e| refusal(&path, e.to_string()))?;
        if meta.file_type().is_symlink() {
            return Err(refusal(
                &path,
                "it is a symlink, and apt-scout never follows one",
            ));
        }
        if !meta.is_dir() {
            return Err(refusal(&path, "it is not a directory"));
        }
        let newer = match &newest {
            None => true,
            Some(previous) => id.as_str() > previous.as_str(),
        };
        if newer {
            newest = Some(id);
        }
    }
    newest.ok_or_else(|| refusal(root, "there is no backup to restore"))
}

/// Read the manifest, insisting on G1's facts twice: before the open and on the handle.
///
/// The `lstat` check refuses a symlink, which `open` would otherwise follow; the handle check proves
/// the inode that was actually opened carries the same facts; and the identity check proves it is
/// the *same* inode, so two files that both carry the facts cannot be swapped between the two
/// reads. Only then is a byte of the content read.
fn read_manifest(path: &Path, owner_uid: u32, owner_gid: u32) -> Result<String, ApplyError> {
    let link = match fs::symlink_metadata(path) {
        Ok(meta) => meta,
        // Its own sentence rather than the OS error: a directory with no manifest is not a
        // half-written backup to retry, it is not a backup of anything.
        Err(e) if e.kind() == io::ErrorKind::NotFound => {
            return Err(refusal(
                path,
                "the backup holds no manifest, so this restore cannot tell what it holds",
            ));
        }
        Err(e) => return Err(refusal(path, e.to_string())),
    };
    check_manifest_meta(path, &link, owner_uid, owner_gid)?;
    let file = File::open(path).map_err(|e| refusal(path, e.to_string()))?;
    let meta = file.metadata().map_err(|e| refusal(path, e.to_string()))?;
    check_manifest_meta(path, &meta, owner_uid, owner_gid)?;
    check_same_inode(path, &link, &meta)?;

    let mut bytes = Vec::new();
    let mut limited = file.take(manifest::MAX_TOTAL_BYTES as u64 + 1);
    limited
        .read_to_end(&mut bytes)
        .map_err(|e| refusal(path, e.to_string()))?;
    if bytes.len() > manifest::MAX_TOTAL_BYTES {
        return Err(refusal(
            path,
            format!(
                "it is larger than the {} byte manifest format allows",
                manifest::MAX_TOTAL_BYTES
            ),
        ));
    }
    String::from_utf8(bytes).map_err(|e| refusal(path, format!("it is not UTF-8: {e}")))
}

/// The four facts G1 requires of a file apt-scout reads back, named in the order they are checked.
fn check_manifest_meta(
    path: &Path,
    meta: &Metadata,
    owner_uid: u32,
    owner_gid: u32,
) -> Result<(), ApplyError> {
    if meta.file_type().is_symlink() {
        return Err(refusal(
            path,
            "it is a symlink, and apt-scout never follows one",
        ));
    }
    if !meta.file_type().is_file() {
        return Err(refusal(path, "it is not a regular file"));
    }
    if meta.uid() != owner_uid {
        return Err(refusal(
            path,
            format!("it is owned by uid {}, not uid {owner_uid}", meta.uid()),
        ));
    }
    if meta.gid() != owner_gid {
        return Err(refusal(
            path,
            format!("it is owned by gid {}, not gid {owner_gid}", meta.gid()),
        ));
    }
    let mode = meta.mode() & MODE_MASK;
    if mode != FILE_MODE {
        return Err(refusal(
            path,
            format!("its mode is {mode:#o}, not {FILE_MODE:#o}"),
        ));
    }
    Ok(())
}

/// Prove the opened handle is the file the `lstat` named, not a second one of the same shape.
///
/// `check_manifest_meta` states what the opened inode *is*; it cannot state that it is the inode the
/// check before the open described, and two root-owned 0600 files swap cleanly under that rule
/// alone. The writer compares dev/ino for the same reason (`apply::read_state`), and the manifest is
/// the one file whose content decides what root writes.
fn check_same_inode(path: &Path, link: &Metadata, opened: &Metadata) -> Result<(), ApplyError> {
    if link.dev() != opened.dev() || link.ino() != opened.ino() {
        return Err(refusal(
            path,
            "the device or inode changed between the check and the open",
        ));
    }
    Ok(())
}

/// Re-derive every write from the manifest, in the manifest's order.
fn derive(
    dir: &Path,
    apt_root: &Path,
    entries: &[ManifestEntry],
    owner_uid: u32,
    owner_gid: u32,
) -> Result<Vec<PlannedWrite>, ApplyError> {
    let mut writes = Vec::with_capacity(entries.len());
    for entry in entries {
        writes.push(derive_one(dir, apt_root, entry, owner_uid, owner_gid)?);
    }
    Ok(writes)
}

/// Re-derive one write, refusing the first fact that disagrees with the record or the disk.
///
/// `Action::Replace` restores the copy at the target; `Action::Create` undoes the creation the
/// backup recorded, so it becomes a removal with no bytes and the digest of no bytes. Both carry the
/// state the target has now, so the writer's `capture` compares the disk it re-reads against what
/// this run was told, and a change between the two reads refuses the write.
///
/// The recorded uid and gid are always 0, because `manifest::parse` refuses any other ownership, so
/// the run's own owner *is* the recorded owner: ownership is compared against `owner_uid` and
/// `owner_gid` rather than against the record for that reason, and the comparison is the same rule
/// a root run applies.
fn derive_one(
    dir: &Path,
    apt_root: &Path,
    entry: &ManifestEntry,
    owner_uid: u32,
    owner_gid: u32,
) -> Result<PlannedWrite, ApplyError> {
    let path = apply::resolve(apt_root, &entry.target);
    // A symlinked `sources.list.d` would be followed by the kernel when the writer opens the path.
    apply::check_parent(&path)?;
    let current = match apply::read_state(&path)? {
        Entry::Regular(state) => state,
        Entry::Missing => {
            return Err(ApplyError::Changed {
                path,
                reason: "the backup describes this file and it is not on disk, so this restore \
                         cannot be re-derived without guessing what it held"
                    .into(),
            });
        }
        Entry::NotRegular(reason) => return Err(ApplyError::Target { path, reason }),
    };
    let identity_matches =
        current.mode == entry.mode && current.uid == owner_uid && current.gid == owner_gid;
    match entry.action {
        Action::Replace => {
            let copy = dir.join(entry.target.relative());
            apply::check_parent(&copy)?;
            let copy = match apply::read_state(&copy)? {
                Entry::Regular(state) => state,
                Entry::Missing => {
                    return Err(refusal(
                        &copy,
                        "the backup does not hold the file its own manifest names, so it is not a \
                         backup of what it claims",
                    ));
                }
                Entry::NotRegular(reason) => return Err(refusal(&copy, reason)),
            };
            if copy.mode != entry.mode || copy.uid != owner_uid || copy.gid != owner_gid {
                return Err(refusal(
                    &dir.join(entry.target.relative()),
                    format!(
                        "the copy is mode {:#o} uid {} gid {}, and the manifest records mode {:#o} \
                         uid {owner_uid} gid {owner_gid}",
                        copy.mode, copy.uid, copy.gid, entry.mode
                    ),
                ));
            }
            if sha256(&copy.bytes) != entry.digest {
                return Err(refusal(
                    &dir.join(entry.target.relative()),
                    "its digest is not the digest the manifest records, so it is not the bytes that \
                     were backed up",
                ));
            }
            // The writer restores the mode and owner it finds on disk, so the disk must already be
            // the record's: otherwise the restore would land the copy under another identity. The
            // bytes on disk are *not* compared with the digest, because a replace's whole point is
            // that the disk holds what this run wrote and the copy holds what it replaced.
            if !identity_matches {
                return Err(ApplyError::Changed {
                    path,
                    reason: format!(
                        "the manifest records mode {:#o} uid {owner_uid} gid {owner_gid} and the \
                         file on disk is mode {:#o} uid {} gid {}, so this restore would write the \
                         backup over a file it was not built from",
                        entry.mode, current.mode, current.uid, current.gid
                    ),
                });
            }
            Ok(PlannedWrite {
                target: entry.target.clone(),
                new_bytes: copy.bytes,
                digest: entry.digest,
                original: Some(current),
                effect: Effect::Replace,
            })
        }
        // A file this backup recorded as created has no copy: undoing it is removing it, and only
        // while it is still exactly what the record says was created.
        Action::Create => {
            if !identity_matches || sha256(&current.bytes) != entry.digest {
                return Err(ApplyError::Changed {
                    path,
                    reason: format!(
                        "the manifest records this file as one this run created, in mode {:#o} \
                         uid {owner_uid} gid {owner_gid} with the digest it records, and what is on \
                         disk is mode {:#o} uid {} gid {} with other bytes, so removing it would \
                         destroy something a restore cannot put back",
                        entry.mode, current.mode, current.uid, current.gid
                    ),
                });
            }
            Ok(PlannedWrite {
                target: entry.target.clone(),
                new_bytes: Vec::new(),
                digest: sha256(&[]),
                original: Some(current),
                effect: Effect::Remove,
            })
        }
    }
}

#[cfg(test)]
#[rustfmt::skip]
mod tests {
    use super::*;
    use crate::apply::Touched;
    use crate::manifest::{Action, ManifestEntry};
    use crate::plan::{Authority, FileState, Target};
    use std::os::unix::fs::{PermissionsExt, symlink};
    use std::path::PathBuf;

    const ID: &str = "20261112T203045Z";
    const NEWER: &str = "20261113T203045Z";
    const NOW: u64 = 1_794_515_445;
    const LIST: &str = "sources.list";
    const SECURITY: &str = "sources.list.d/system-security.sources";
    const OLD: &[u8] = b"deb http://archive.ubuntu.com/ubuntu noble main\n";
    const MIRROR: &[u8] = b"deb https://m.example/ubuntu noble main\n";
    const CREATED: &[u8] = b"Types: deb\nURIs: https://m.example/ubuntu\nSuites: noble-security\n";

    /// A fixture root under the temp directory, removed however the test ends.
    struct TempRoot(PathBuf);

    impl TempRoot {
        fn new(tag: &str) -> TempRoot {
            let path = std::env::temp_dir()
                .join(format!("apt-scout-restore-{tag}-{}", std::process::id()));
            let _ = fs::remove_dir_all(&path);
            fs::create_dir_all(&path).expect("temp root");
            TempRoot(path)
        }

        fn path(&self) -> &Path { &self.0 }

        /// The uid and gid of the caller: a non-root test cannot own a root-owned backup, so every
        /// fixture is this test's own file and the builder is handed this identity as the owner.
        fn owner(&self) -> (u32, u32) {
            let meta = fs::metadata(&self.0).expect("temp root metadata");
            (meta.uid(), meta.gid())
        }

        /// The backup root, in the mode a run leaves behind.
        fn backups(&self) -> PathBuf { dir(&self.0, "backups") }

        /// The fixture `/etc/apt`.
        fn apt(&self) -> PathBuf { dir(&self.0, "apt") }
    }

    impl Drop for TempRoot {
        fn drop(&mut self) { let _ = fs::remove_dir_all(&self.0); }
    }

    fn dir(root: &Path, name: &str) -> PathBuf {
        let path = root.join(name);
        let _ = fs::create_dir(&path);
        fs::set_permissions(&path, fs::Permissions::from_mode(DIR_MODE)).expect("mode");
        path
    }

    /// One backup directory, in the mode the writer creates it with.
    fn backup_dir(backups: &Path, id: &str) -> PathBuf {
        let path = backups.join(id);
        fs::create_dir(&path).expect("backup directory");
        fs::set_permissions(&path, fs::Permissions::from_mode(DIR_MODE)).expect("mode");
        path
    }

    /// A record as a root run would have written it: `manifest::parse` accepts no other ownership.
    fn entry(target: &str, action: Action, bytes: &[u8], mode: u32) -> ManifestEntry {
        ManifestEntry {
            target: Target::parse(target).expect("target"),
            action,
            digest: sha256(bytes),
            mode,
            uid: 0,
            gid: 0,
        }
    }

    /// The manifest as the writer would leave it: mode 0600, named after its own backup.
    fn write_manifest(dir: &Path, id: &str, entries: &[ManifestEntry]) -> PathBuf {
        let path = dir.join(MANIFEST);
        let text = manifest::render(&BackupId::parse(id).expect("id"), entries);
        fs::write(&path, text).expect("manifest bytes");
        fs::set_permissions(&path, fs::Permissions::from_mode(FILE_MODE)).expect("mode");
        path
    }

    /// The copy the writer would have made, at its own relative path inside the backup.
    fn write_copy(dir: &Path, target: &str, bytes: &[u8], mode: u32) -> PathBuf {
        let path = dir.join(Target::parse(target).expect("target").relative());
        if let Some(parent) = path.parent() && parent != dir {
            fs::create_dir_all(parent).expect("copy directory");
        }
        fs::write(&path, bytes).expect("copy bytes");
        fs::set_permissions(&path, fs::Permissions::from_mode(mode)).expect("mode");
        path
    }

    fn apt_file(apt: &Path, name: &str, bytes: &[u8], mode: u32) -> PathBuf {
        let path = apt.join(name);
        if let Some(parent) = path.parent() && parent != apt {
            fs::create_dir_all(parent).expect("apt directory");
        }
        fs::write(&path, bytes).expect("apt bytes");
        fs::set_permissions(&path, fs::Permissions::from_mode(mode)).expect("mode");
        path
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

    fn expect_refusal(result: Result<Plan, ApplyError>, needle: &str) {
        let err = result.expect_err("the restore is refused");
        assert!(matches!(err, ApplyError::Restore { .. }), "{err}");
        let text = err.to_string();
        assert!(text.contains(needle), "{text}");
        assert!(text.contains("Nothing was written to /etc/apt"), "{text}");
    }

    /// The restore authority is minted where the backup is read and nowhere else.
    ///
    /// `Plan::for_restore` takes the token rather than a `BackupId` so a caller cannot skip the
    /// manifest, the copies and the disk; that is a compile-time claim only while one module can
    /// write `ValidatedBackup { .. }`, so a second producer anywhere in `src/` is the hole reopened.
    #[test]
    fn the_restore_authority_is_minted_only_in_the_restore_reader() {
        let src = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
        let mut checked = 0;
        for entry in fs::read_dir(&src).expect("the crate's source directory") {
            let path = entry.expect("a source file").path();
            if path.extension().is_none_or(|ext| ext != "rs") || path == src.join("restore.rs") {
                continue;
            }
            let text = fs::read_to_string(&path).expect("a source file is readable UTF-8");
            assert!(
                !text.contains("ValidatedBackup {"),
                "{} mints a restore authority; only src/restore.rs may (D10b)",
                path.display()
            );
            checked += 1;
        }
        assert!(
            checked >= 5,
            "expected the crate's modules, checked {checked}"
        );
    }

    /// Criterion 1: nothing is parsed until the backup root and the chosen directory are G1's shape.
    #[test]
    fn build_refuses_a_hostile_backup_root() {
        let root = TempRoot::new("root");
        let (uid, gid) = root.owner();
        let apt = root.apt();

        // (a) no backup root at all: there is nothing to restore, and nothing is created for it.
        let missing = root.path().join("no-backups");
        let err = build_as(&missing, &apt, uid, gid).expect_err("a missing root is refused");
        assert!(matches!(err, ApplyError::Restore { .. }), "{err}");
        assert!(err.to_string().contains("no backup has been written"), "{err}");
        assert!(!missing.exists(), "a restore never creates the backup root");

        // (b) the root is a symlink, which the kernel would follow on `read_dir`.
        let real = root.backups();
        let linked = root.path().join("linked-backups");
        symlink(&real, &linked).expect("symlink");
        expect_refusal(build_as(&linked, &apt, uid, gid), "symlink");

        // (c) the root is world-writable: anything could be planted in it, so it is not read.
        let open = root.path().join("backups-0777");
        fs::create_dir(&open).expect("directory");
        fs::set_permissions(&open, fs::Permissions::from_mode(0o777)).expect("mode");
        expect_refusal(build_as(&open, &apt, uid, gid), "its mode is 0o777, not 0o700");

        // (d) the root belongs to another uid. A non-root test cannot own a directory of another
        // user, so this case is built the only way it can be: by expecting an owner it lacks.
        expect_refusal(build_as(&real, &apt, uid + 1, gid), "owned by uid");

        // (e) an `<id>` that is a symlink, which the kernel would follow on open.
        let elsewhere = root.path().join("elsewhere");
        fs::create_dir(&elsewhere).expect("directory");
        symlink(&elsewhere, real.join(ID)).expect("symlink");
        expect_refusal(build_as(&real, &apt, uid, gid), "symlink");

        // (f) an `<id>` that is a plain file rather than a directory.
        let flat = root.path().join("backups-flat");
        fs::create_dir(&flat).expect("directory");
        fs::set_permissions(&flat, fs::Permissions::from_mode(DIR_MODE)).expect("mode");
        fs::write(flat.join(ID), b"not a directory\n").expect("file");
        expect_refusal(build_as(&flat, &apt, uid, gid), "it is not a directory");

        // (g) a backup root that is a plain file.
        let file = root.path().join("backups-file");
        fs::write(&file, b"not a directory\n").expect("file");
        expect_refusal(build_as(&file, &apt, uid, gid), "it is not a directory");
    }

    /// A backup root of the right shape that holds no backup at all is refused rather than read as
    /// one: a directory a run created and then failed inside is not a backup of nothing.
    #[test]
    fn build_refuses_a_backup_root_that_holds_no_backup() {
        let root = TempRoot::new("empty-root");
        let (uid, gid) = root.owner();
        let backups = root.backups();
        // The root is G1's shape and empty, so only the empty listing can refuse this.
        assert!(
            fs::read_dir(&backups).expect("the root is readable").next().is_none(),
            "the fixture root is empty"
        );
        // The fixture `/etc/apt` is never created: reaching `derive` at all would answer with a
        // missing path instead of the root's own refusal.
        let apt = root.path().join("apt-absent");
        let err = build_as(&backups, &apt, uid, gid).expect_err("an empty root is refused");
        assert!(matches!(err, ApplyError::Restore { .. }), "{err}");
        let text = err.to_string();
        assert!(text.contains("there is no backup to restore"), "{text}");
        assert!(text.contains(&backups.display().to_string()), "{text}");
        assert!(text.contains("Nothing was written to /etc/apt"), "{text}");
        assert!(!apt.exists(), "nothing under /etc/apt was looked at");
    }

    /// A backup directory of the right shape with no `MANIFEST` in it is refused where the reader
    /// looks for the manifest, and says what is missing instead of forwarding an OS error.
    ///
    /// This is the shape a run interrupted between its copies and its manifest leaves behind, and
    /// the sentence is the reader's own, so deleting that branch turns the assertion red.
    #[test]
    fn build_refuses_a_backup_directory_without_a_manifest() {
        let root = TempRoot::new("no-manifest");
        let (uid, gid) = root.owner();
        let backups = root.backups();
        let dir = backup_dir(&backups, ID);
        // The copies are there, so the directory is otherwise a backup; only the manifest is not.
        write_copy(&dir, LIST, OLD, 0o644);
        let manifest_path = dir.join(MANIFEST);
        assert!(!manifest_path.exists(), "the fixture holds no manifest");
        // The fixture `/etc/apt` is never created: the refusal precedes any target it names.
        let apt = root.path().join("apt-absent");
        let err =
            build_as(&backups, &apt, uid, gid).expect_err("a backup with no manifest is refused");
        assert!(matches!(err, ApplyError::Restore { .. }), "{err}");
        let text = err.to_string();
        assert!(text.contains(&manifest_path.display().to_string()), "{text}");
        assert!(text.contains("holds no manifest"), "{text}");
        assert!(text.contains("Nothing was written to /etc/apt"), "{text}");
        assert!(!apt.exists(), "nothing under /etc/apt was opened");
    }

    /// Criterion 2: the manifest is refused on its own facts, before a byte of it is parsed.
    #[test]
    fn build_refuses_an_untrusted_manifest() {
        let root = TempRoot::new("manifest");
        let (uid, gid) = root.owner();
        let apt = root.apt();
        let backups = root.backups();
        let list = apt_file(&apt, LIST, MIRROR, 0o644);
        let record = entry(LIST, Action::Replace, OLD, state(&list).mode);

        // (a) MANIFEST is a symlink to a manifest that would otherwise parse: the refusal must name
        // the symlink rather than the content, so no symlink is ever followed.
        let dir = backup_dir(&backups, ID);
        let real = root.path().join("real-manifest");
        fs::write(&real, manifest::render(&BackupId::parse(ID).expect("id"), std::slice::from_ref(&record)))
            .expect("manifest bytes");
        fs::set_permissions(&real, fs::Permissions::from_mode(FILE_MODE)).expect("mode");
        let link = dir.join(MANIFEST);
        symlink(&real, &link).expect("symlink");
        expect_refusal(build_as(&backups, &apt, uid, gid), "symlink");

        // (b) a world-readable manifest. The symlink goes first, or `fs::write` would follow it.
        fs::remove_file(&link).expect("remove the symlink");
        let path = write_manifest(&dir, ID, std::slice::from_ref(&record));
        fs::set_permissions(&path, fs::Permissions::from_mode(0o644)).expect("mode");
        expect_refusal(build_as(&backups, &apt, uid, gid), "its mode is 0o644, not 0o600");

        // (c) a manifest owned by another uid, then by another gid.
        fs::set_permissions(&path, fs::Permissions::from_mode(FILE_MODE)).expect("mode");
        expect_refusal(build_as(&backups, &apt, uid + 1, gid), "owned by uid");
        expect_refusal(build_as(&backups, &apt, uid, gid + 1), "owned by gid");

        // (d) a directory where the manifest should be.
        fs::remove_file(&path).expect("remove the manifest");
        fs::create_dir(&path).expect("directory");
        expect_refusal(build_as(&backups, &apt, uid, gid), "it is not a regular file");
        fs::remove_dir(&path).expect("remove the directory");

        // (e) a manifest larger than the format allows.
        fs::write(&path, vec![b'a'; manifest::MAX_TOTAL_BYTES + 1]).expect("oversize manifest");
        fs::set_permissions(&path, fs::Permissions::from_mode(FILE_MODE)).expect("mode");
        expect_refusal(build_as(&backups, &apt, uid, gid), "larger than the");

        // (f) content that is not UTF-8.
        fs::write(&path, b"apt-scout-backup 1\nid \xff\xfe\n").expect("non-utf8 manifest");
        expect_refusal(build_as(&backups, &apt, uid, gid), "it is not UTF-8");

        // (g) a manifest copied out of another backup directory.
        let other = backup_dir(&backups, NEWER);
        write_manifest(&other, ID, std::slice::from_ref(&record));
        expect_refusal(build_as(&backups, &apt, uid, gid), "it names backup");
    }

    /// The manifest's identity, not only its five facts: a swap between two files that both carry
    /// mode 0600 and the run's ownership is refused, which is the check the writer's own read
    /// applies to every target.
    ///
    /// The swap itself cannot be raced in a test — it needs a third process between the `lstat` and
    /// the open — so the comparison is driven directly, with the metadata of two real files.
    #[test]
    fn a_manifest_swapped_between_the_check_and_the_open_is_refused() {
        let root = TempRoot::new("manifest-swap");
        let dir = backup_dir(&root.backups(), ID);
        let path = write_manifest(&dir, ID, &[]);
        let other = dir.join("other-manifest");
        fs::write(&other, b"apt-scout-backup 1\n").expect("bytes");
        fs::set_permissions(&other, fs::Permissions::from_mode(FILE_MODE)).expect("mode");

        let link = fs::symlink_metadata(&path).expect("metadata");
        let opened = fs::metadata(&path).expect("metadata");
        check_same_inode(&path, &link, &opened).expect("the same file carries the same identity");

        let swapped = fs::metadata(&other).expect("metadata");
        let err = check_same_inode(&path, &link, &swapped).expect_err("a swap is refused");
        assert!(matches!(err, ApplyError::Restore { .. }), "{err}");
        let text = err.to_string();
        assert!(text.contains("device or inode"), "{text}");
        assert!(text.contains(&path.display().to_string()), "{text}");
    }

    /// Criterion 3: a target outside the closed enumeration is refused before any `/etc/apt` path is
    /// resolved, so no name a hostile manifest carries reaches the filesystem.
    #[test]
    fn build_refuses_a_hostile_target_name() {
        let hostile = ["/etc/shadow", "../sources.list", "sources.list.d/../../etc/passwd",
            "/etc/apt/../etc/shadow", "/etc/aptx/sources.list", "/usr/bin/gpgv"];
        for name in hostile {
            let root = TempRoot::new("target");
            let (uid, gid) = root.owner();
            let backups = root.backups();
            let dir = backup_dir(&backups, ID);
            // The fixture `/etc/apt` is never created: reaching `resolve` or `read_state` at all
            // would answer with a missing file instead of the manifest's own refusal.
            let apt = root.path().join("apt-absent");
            let digest = crate::digest::hex(&sha256(OLD));
            let text = format!(
                "apt-scout-backup 1\nid {ID}\nfiles 1\nfile {name} replace {digest} 644 0 0\nend 1"
            );
            let path = dir.join(MANIFEST);
            fs::write(&path, text).expect("hostile manifest");
            fs::set_permissions(&path, fs::Permissions::from_mode(FILE_MODE)).expect("mode");
            let err = build_as(&backups, &apt, uid, gid).expect_err("refused");
            println!("{name:?}: {err}");
            assert!(matches!(err, ApplyError::Restore { .. }), "{err}");
            let text = err.to_string();
            assert!(text.contains(&path.display().to_string()), "{text}");
            assert!(text.contains("target rule"), "{text}");
            assert!(!apt.exists(), "no apt path was resolved or opened");
        }
    }

    /// Criterion 4: a record that disagrees with the copy it names or with the disk is refused.
    #[test]
    fn build_refuses_a_record_that_disagrees() {
        // (a) the copy the record names is missing.
        let root = TempRoot::new("record-missing-copy");
        let (uid, gid) = root.owner();
        let apt = root.apt();
        let backups = root.backups();
        let dir = backup_dir(&backups, ID);
        let list = apt_file(&apt, LIST, MIRROR, 0o644);
        let mode = state(&list).mode;
        write_manifest(&dir, ID, &[entry(LIST, Action::Replace, OLD, mode)]);
        expect_refusal(build_as(&backups, &apt, uid, gid), "does not hold the file");

        // (b) the copy's digest is not the record's: the tampered-manifest case.
        write_copy(&dir, LIST, b"deb http://tampered.example/ubuntu noble main\n", mode);
        expect_refusal(build_as(&backups, &apt, uid, gid), "digest");

        // (c) the copy's mode is not the record's.
        write_copy(&dir, LIST, OLD, FILE_MODE);
        expect_refusal(build_as(&backups, &apt, uid, gid), "the copy is mode");

        // (d) the target on disk is a symlink, which no plan may follow.
        write_copy(&dir, LIST, OLD, mode);
        let root2 = TempRoot::new("record-link");
        let (uid2, gid2) = root2.owner();
        let apt2 = root2.apt();
        let backups2 = root2.backups();
        let dir2 = backup_dir(&backups2, ID);
        let canary = root2.path().join("canary.list");
        fs::write(&canary, b"canary\n").expect("canary");
        symlink(&canary, apt2.join(LIST)).expect("symlink");
        write_copy(&dir2, LIST, OLD, mode);
        write_manifest(&dir2, ID, &[entry(LIST, Action::Replace, OLD, mode)]);
        let err = build_as(&backups2, &apt2, uid2, gid2).expect_err("a symlinked target is refused");
        assert!(matches!(err, ApplyError::Target { .. }), "{err}");
        assert!(err.to_string().contains("symlink"), "{err}");
        assert_eq!(fs::read(&canary).expect("bytes"), b"canary\n", "the canary is untouched");

        // (e) the target is gone, which is a disagreement and not a no-op.
        let root3 = TempRoot::new("record-gone");
        let (uid3, gid3) = root3.owner();
        let apt3 = root3.apt();
        let backups3 = root3.backups();
        let dir3 = backup_dir(&backups3, ID);
        write_copy(&dir3, LIST, OLD, mode);
        write_manifest(&dir3, ID, &[entry(LIST, Action::Replace, OLD, mode)]);
        let err = build_as(&backups3, &apt3, uid3, gid3).expect_err("a missing target is refused");
        assert!(matches!(err, ApplyError::Changed { .. }), "{err}");
        assert!(err.to_string().contains("not on disk"), "{err}");

        // (f) the target on disk is no longer the file the record describes.
        let target = apt_file(&apt3, LIST, MIRROR, 0o600);
        let err = build_as(&backups3, &apt3, uid3, gid3).expect_err("a changed target is refused");
        assert!(matches!(err, ApplyError::Changed { .. }), "{err}");
        assert!(err.to_string().contains("the file on disk is mode"), "{err}");
        assert_eq!(fs::read(&target).expect("bytes"), MIRROR);

        // (g) a created file whose content is not what the record says was created.
        let root4 = TempRoot::new("record-created");
        let (uid4, gid4) = root4.owner();
        let apt4 = root4.apt();
        let backups4 = root4.backups();
        let dir4 = backup_dir(&backups4, ID);
        apt_file(&apt4, SECURITY, b"Types: deb\nURIs: http://elsewhere.example/ubuntu\n", FILE_MODE);
        write_manifest(&dir4, ID, &[entry(SECURITY, Action::Create, CREATED, FILE_MODE)]);
        let err = build_as(&backups4, &apt4, uid4, gid4).expect_err("a changed creation is refused");
        assert!(matches!(err, ApplyError::Changed { .. }), "{err}");
        assert!(err.to_string().contains("one this run created"), "{err}");

        // (h) a created file that is already gone.
        let root5 = TempRoot::new("record-created-gone");
        let (uid5, gid5) = root5.owner();
        let apt5 = root5.apt();
        let backups5 = root5.backups();
        let dir5 = backup_dir(&backups5, ID);
        write_manifest(&dir5, ID, &[entry(SECURITY, Action::Create, CREATED, FILE_MODE)]);
        let err = build_as(&backups5, &apt5, uid5, gid5).expect_err("a missing creation is refused");
        assert!(matches!(err, ApplyError::Changed { .. }), "{err}");
        assert!(err.to_string().contains("not on disk"), "{err}");
    }

    /// Criterion 5: the newest backup is the greatest id by string order, it is the only one
    /// considered, and a refused newest backup never falls back to an older one.
    #[test]
    fn build_reads_the_newest_backup_and_never_falls_back() {
        let root = TempRoot::new("newest");
        let (uid, gid) = root.owner();
        let apt = root.apt();
        let backups = root.backups();
        let list = apt_file(&apt, LIST, MIRROR, 0o644);
        let mode = state(&list).mode;

        // The older backup is complete and the newer one is not: the refusal stands, because a
        // fallback would restore files the operator did not ask for.
        let older = backup_dir(&backups, ID);
        write_copy(&older, LIST, OLD, mode);
        write_manifest(&older, ID, &[entry(LIST, Action::Replace, OLD, mode)]);
        let newer = backup_dir(&backups, NEWER);
        write_manifest(&newer, NEWER, &[entry(LIST, Action::Replace, OLD, mode)]);
        let err = build_as(&backups, &apt, uid, gid).expect_err("the newest backup is refused");
        assert!(err.to_string().contains(NEWER), "{err}");

        // A file in the root that is not an id is ignored: the root may hold other things.
        fs::write(backups.join("notes.txt"), b"a note\n").expect("note");
        fs::copy(older.join(LIST), newer.join(LIST)).expect("copy");
        let plan = build_as(&backups, &apt, uid, gid).expect("the newest backup is read");
        assert_eq!(plan.authority(), &Authority::Restore {
            backup: BackupId::parse(NEWER).expect("id"),
        });
    }

    /// Criterion 6: the plan is the manifest's, in the manifest's order, and it validates.
    #[test]
    fn build_derives_the_plan_from_the_manifest_alone() {
        let root = TempRoot::new("derive");
        let (uid, gid) = root.owner();
        let apt = root.apt();
        let backups = root.backups();
        let dir = backup_dir(&backups, ID);
        let list = apt_file(&apt, LIST, MIRROR, 0o644);
        let security = apt_file(&apt, SECURITY, CREATED, FILE_MODE);
        let records = [
            entry(SECURITY, Action::Create, CREATED, state(&security).mode),
            entry(LIST, Action::Replace, OLD, state(&list).mode),
        ];
        write_copy(&dir, LIST, OLD, state(&list).mode);
        write_manifest(&dir, ID, &records);

        let plan = build_as(&backups, &apt, uid, gid).expect("the backup is read");
        assert_eq!(plan.authority(), &Authority::Restore {
            backup: BackupId::parse(ID).expect("id"),
        });
        let writes = plan.writes();
        assert_eq!(writes.len(), 2);
        // The manifest's order, not the writer's: the writer reorders the phases itself.
        assert_eq!(writes[0].target.relative(), Path::new(SECURITY));
        assert_eq!(writes[0].effect, Effect::Remove);
        assert_eq!(writes[0].new_bytes, Vec::new());
        assert_eq!(writes[0].digest, sha256(&[]));
        assert_eq!(writes[0].original.as_ref().map(|s| s.bytes.clone()), Some(CREATED.to_vec()));
        assert_eq!(writes[1].target.relative(), Path::new(LIST));
        assert_eq!(writes[1].effect, Effect::Replace);
        assert_eq!(writes[1].new_bytes, OLD, "a replace carries the copy's bytes");
        assert_eq!(writes[1].digest, sha256(OLD));
        assert_eq!(writes[1].original.as_ref().map(|s| s.bytes.clone()), Some(MIRROR.to_vec()),
            "a replace carries the state the disk has now");
        assert!(plan.validate().is_ok());

        // `for_restore` leaves the platform unset, so a restore cannot create: the create rules
        // refuse by construction rather than by a second check.
        let created = vec![PlannedWrite {
            target: Target::parse(LIST).expect("target"),
            new_bytes: b"new\n".to_vec(),
            digest: sha256(b"new\n"),
            original: None,
            effect: Effect::Create,
        }];
        let err = Plan::for_restore(&validated_backup_for_test(BackupId::parse(ID).expect("id")), created)
            .expect_err("a restore plan may not create");
        assert!(matches!(err, crate::plan::PlanError::CreateNewOnList), "{err}");
    }

    /// Criterion 7: the plan a backup yields, applied by the ordinary writer, puts the original
    /// bytes back and undoes the creation the mirror run recorded.
    ///
    /// The backup is written by hand because `manifest::parse` accepts root ownership only, and a
    /// non-root test cannot own a root-owned manifest; the writer that consumes the plan is the
    /// real one, reached through its test seam.
    #[test]
    fn a_restore_from_a_backup_puts_the_original_bytes_back() {
        let root = TempRoot::new("round-trip");
        let (uid, gid) = root.owner();
        let apt = root.apt();
        let backups = root.backups();
        let dir = backup_dir(&backups, ID);
        // The fixture machine after a mirror run: sources.list points at the mirror and the
        // security source exists, while the backup holds what both were before that run.
        let list = apt_file(&apt, LIST, MIRROR, 0o644);
        let security = apt_file(&apt, SECURITY, CREATED, FILE_MODE);
        write_copy(&dir, LIST, OLD, state(&list).mode);
        write_manifest(&dir, ID, &[
            entry(SECURITY, Action::Create, CREATED, state(&security).mode),
            entry(LIST, Action::Replace, OLD, state(&list).mode),
        ]);

        let plan = build_as(&backups, &apt, uid, gid).expect("the backup is read");
        apply::run_for_test(&apt, &backups, &plan, None, NOW + 1, uid, gid)
            .expect("a restore is authorized by its backup, so it needs no baseline");
        assert_eq!(fs::read(&list).expect("bytes"), OLD, "the original bytes are back");
        assert!(!security.exists(), "the created file is removed");
        // The backup root now holds the mirror run's backup and the restore's, newest last.
        assert!(backups.join("20261112T203046Z").is_dir(), "the restore backed up what it replaced");

        // The root entry point refuses a non-root run before the backup is opened.
        if uid != 0 {
            assert!(matches!(build(&backups, &apt), Err(ApplyError::NotRoot)));
        }
    }

    /// Criterion 8: `run` is the one production entry point — it re-derives the plan and hands it
    /// to the one writer with no baseline — and a second run against the same backup is refused by
    /// the record-versus-disk rule rather than guessed.
    ///
    /// The baseline is asserted on the argument and not on the outcome, because `check_freshness`
    /// accepts either value for a restore plan: only the value the run chose shows which it passed.
    #[test]
    fn run_restores_the_backup_and_a_second_run_is_refused() {
        let root = TempRoot::new("run");
        let (uid, gid) = root.owner();
        let apt = root.apt();
        let backups = root.backups();
        let dir = backup_dir(&backups, ID);
        // The machine a mirror run left behind: sources.list points at the mirror and the security
        // source exists, while the backup holds what both were before that run.
        let list = apt_file(&apt, LIST, MIRROR, 0o644);
        let security = apt_file(&apt, SECURITY, CREATED, FILE_MODE);
        let list_mode = state(&list).mode;
        write_copy(&dir, LIST, OLD, list_mode);
        write_manifest(&dir, ID, &[
            entry(SECURITY, Action::Create, CREATED, state(&security).mode),
            entry(LIST, Action::Replace, OLD, list_mode),
        ]);

        let mut handed = Some(0i64);
        let outcome = run_as(&backups, &apt, NOW + 1, uid, gid, |plan, baseline, now| {
            handed = baseline.map(|date| date.seconds());
            apply::run_for_test(&apt, &backups, plan, baseline, now, uid, gid)
        })
        .expect("the newest backup is read and its plan applied");
        assert_eq!(handed, None, "a restore hands the writer no baseline");

        // The bytes and the mode are the record's again, and the creation is gone. Ownership is not
        // asserted here: a non-root test's files are owned by the test's own uid whichever way the
        // writer treats them, so that comparison belongs to D11b's container case.
        let restored = state(&list);
        assert_eq!(restored.bytes, OLD, "the replace put the backup's bytes back");
        assert_eq!(restored.mode, list_mode, "the mode is the record's");
        assert!(!security.exists(), "the file the manifest records as created is removed");
        assert_eq!(outcome.backup_id, BackupId::from_secs(NOW + 1).expect("id"));
        assert_eq!(outcome.touched, vec![
            Touched { target: Target::parse(LIST).expect("target"), effect: Effect::Replace },
            Touched { target: Target::parse(SECURITY).expect("target"), effect: Effect::Remove },
        ], "the replacement lands before the removal");

        // The restore is itself a write, so it backed up what it replaced under its own id, which
        // is now the newest backup a second run reads.
        let restore_dir = backups.join("20261112T203046Z");
        assert_eq!(fs::read(restore_dir.join(LIST)).expect("the restore's copy"), MIRROR,
            "the restore backed up the file it replaced");
        // A root run's backup records root ownership, which is the only ownership `manifest::parse`
        // accepts and which a non-root test cannot create; rewritten that one way, the manifest is
        // the record the restore's own run would have left behind.
        write_manifest(&restore_dir, "20261112T203046Z", &[
            entry(SECURITY, Action::Replace, CREATED, FILE_MODE),
            entry(LIST, Action::Replace, MIRROR, list_mode),
        ]);

        // The removed file is recorded there as a replacement whose copy still exists, and the
        // target does not: the record and the disk disagree, so the second restore writes nothing.
        let err = run_as(&backups, &apt, NOW + 2, uid, gid, |plan, baseline, now| {
            apply::run_for_test(&apt, &backups, plan, baseline, now, uid, gid)
        })
        .expect_err("a second restore is refused");
        assert!(matches!(err, ApplyError::Changed { .. }), "{err}");
        assert!(err.to_string().contains("not on disk"), "{err}");
        assert_eq!(state(&list).bytes, OLD, "the refused run wrote nothing");

        // The root entry point refuses a non-root run before the backup is opened, like `build`.
        if uid != 0 {
            assert!(matches!(run(&backups, &apt), Err(ApplyError::NotRoot)));
        }
    }
}
