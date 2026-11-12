#![allow(dead_code)] // the CLI reaches this writer in D11
//! The privileged writer for `/etc/apt`.
//!
//! Nothing here runs unless the process is root, and no path is written until it is
//! bound to the disk entry the plan was built from. The order is: validate the plan,
//! capture and bind to the disk, back up, re-validate, write.

use std::fmt;
use std::fs::{self, File};
use std::io::{self, Read};
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};

use crate::plan::{FileState, Plan, Target};

/// The apt configuration root every planned path is relative to.
pub const APT_DIR: &str = "/etc/apt";

/// Where replaced files are backed up before anything is overwritten.
pub const BACKUP_ROOT: &str = "/var/backups/apt-scout";

/// `st_mode` also carries the file-type bits; a recorded mode must never keep them.
const MODE_MASK: u32 = 0o7777;

/// Refuse to read anything larger than this. Apt configuration files are tiny.
const MAX_FILE_BYTES: usize = 1024 * 1024;

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
pub enum ApplyError {
    /// The run is not root, so nothing may be written under `/etc/apt`.
    NotRoot,
    /// A planned path could not be inspected, opened, or read.
    Read { path: PathBuf, reason: String },
    /// A planned path is not a plain regular file apt-scout is willing to own.
    Target { path: PathBuf, reason: String },
    /// A planned path no longer matches the state the plan was built from.
    Changed { path: PathBuf, reason: String },
}

impl ApplyError {
    /// The process exit code that reports this refusal.
    pub fn exit_code(&self) -> u8 {
        match self {
            Self::NotRoot => 3,
            Self::Read { .. } | Self::Target { .. } | Self::Changed { .. } => 1,
        }
    }
}

impl fmt::Display for ApplyError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NotRoot => write!(
                f,
                "writing /etc/apt needs root and this run is not root. Re-run it as: sudo apt-scout"
            ),
            Self::Read { path, reason } => write!(f, "cannot read {}: {reason}", path.display()),
            Self::Target { path, reason } => write!(f, "refusing {}: {reason}", path.display()),
            Self::Changed { path, reason } => {
                write!(
                    f,
                    "{} changed: {reason}. Nothing was written.",
                    path.display()
                )
            }
        }
    }
}

/// A planned file paired with the state the plan expected to find on disk.
struct Captured {
    path: PathBuf,
    original: Option<FileState>,
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
            path,
            original: w.original.clone(),
        });
    }
    Ok(captured)
}

/// Re-check every captured path, in the order the plan will write them.
fn validate_targets(captured: &[Captured]) -> Result<(), ApplyError> {
    captured.iter().try_for_each(check_one)
}

/// Re-read one captured path and confirm it still matches the plan.
fn check_one(c: &Captured) -> Result<(), ApplyError> {
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

/// Wrap an IO failure as a read refusal that names the path.
fn read_failed(path: &Path, e: io::Error) -> ApplyError {
    ApplyError::Read {
        path: path.to_path_buf(),
        reason: e.to_string(),
    }
}
