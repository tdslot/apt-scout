//! OpenPGP verification for `InRelease` using the system `gpgv` and Ubuntu archive keyring.
//!
//! Ranking may use an unverified `Date`; any future write decision must require `Verified`.
//! No shell is used, the executable and keyring paths are fixed, and the environment is cleared.

use std::fmt;
use std::io::{Read, Write};
#[cfg(unix)]
use std::os::unix::fs::DirBuilderExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};

pub const GPGV: &str = "/usr/bin/gpgv";
pub const UBUNTU_KEYRING: &str = "/usr/share/keyrings/ubuntu-archive-keyring.gpg";
const MAX_INRELEASE_BYTES: usize = 512 * 1024;
const GPGV_TIMEOUT: Duration = Duration::from_secs(10);
const MAX_STATUS_BYTES: u64 = 1024 * 1024;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SignatureStatus {
    /// `gpgv` reported a valid signature from the configured Ubuntu archive keyring.
    Verified,
    /// The data was readable but is unsigned, malformed, altered, or signed by an unknown key.
    Invalid(String),
    /// Verification could not run (missing tool/keyring, unsafe temp state, size limit, spawn error).
    Unavailable(String),
}

impl SignatureStatus {
    pub fn is_verified(&self) -> bool {
        matches!(self, SignatureStatus::Verified)
    }
}

impl fmt::Display for SignatureStatus {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            SignatureStatus::Verified => write!(f, "verified"),
            SignatureStatus::Invalid(why) => write!(f, "invalid ({why})"),
            SignatureStatus::Unavailable(why) => write!(f, "unavailable ({why})"),
        }
    }
}

fn private_temp_dir() -> Result<PathBuf, String> {
    let mut random = [0u8; 8];
    std::fs::File::open("/dev/urandom")
        .and_then(|mut f| std::io::Read::read_exact(&mut f, &mut random))
        .map_err(|e| format!("cannot obtain temporary directory name: {e}"))?;
    let dir = std::env::temp_dir().join(format!(
        "apt-scout-inrelease-{}-{:016x}",
        std::process::id(),
        u64::from_be_bytes(random)
    ));
    #[cfg(unix)]
    std::fs::DirBuilder::new()
        .mode(0o700)
        .create(&dir)
        .map_err(|e| format!("cannot create private verification directory: {e}"))?;
    #[cfg(not(unix))]
    std::fs::create_dir(&dir)
        .map_err(|e| format!("cannot create private verification directory: {e}"))?;
    Ok(dir)
}

/// Verify an inline-signed `InRelease` using the fixed system paths.
pub fn verify_inrelease(data: &[u8]) -> SignatureStatus {
    verify_inrelease_with_timeout(
        data,
        Path::new(GPGV),
        Path::new(UBUNTU_KEYRING),
        GPGV_TIMEOUT,
    )
}

#[cfg(test)]
fn verify_inrelease_with(data: &[u8], gpgv: &Path, keyring: &Path) -> SignatureStatus {
    verify_inrelease_with_timeout(data, gpgv, keyring, GPGV_TIMEOUT)
}

fn verify_inrelease_with_timeout(
    data: &[u8],
    gpgv: &Path,
    keyring: &Path,
    timeout: Duration,
) -> SignatureStatus {
    if data.is_empty() {
        return SignatureStatus::Invalid("empty InRelease".into());
    }
    if data.len() > MAX_INRELEASE_BYTES {
        return SignatureStatus::Unavailable(format!(
            "InRelease is larger than the {MAX_INRELEASE_BYTES} byte verification limit"
        ));
    }
    match std::fs::metadata(gpgv) {
        Ok(m) if m.is_file() => {}
        Ok(_) => {
            return SignatureStatus::Unavailable(format!(
                "{} is not a regular file",
                gpgv.display()
            ));
        }
        Err(e) => {
            return SignatureStatus::Unavailable(format!("cannot inspect {}: {e}", gpgv.display()));
        }
    }
    match std::fs::metadata(keyring) {
        Ok(m) if m.is_file() => {}
        Ok(_) => {
            return SignatureStatus::Unavailable(format!(
                "{} is not a regular file",
                keyring.display()
            ));
        }
        Err(e) => {
            return SignatureStatus::Unavailable(format!(
                "cannot inspect {}: {e}",
                keyring.display()
            ));
        }
    }

    let directory = match private_temp_dir() {
        Ok(directory) => directory,
        Err(e) => return SignatureStatus::Unavailable(e),
    };
    let path = directory.join("InRelease");
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    std::os::unix::fs::OpenOptionsExt::mode(&mut options, 0o600);
    let mut file = match options.open(&path) {
        Ok(file) => file,
        Err(e) => {
            let _ = std::fs::remove_dir(&directory);
            return SignatureStatus::Unavailable(format!(
                "cannot create verification temp file: {e}"
            ));
        }
    };
    if let Err(e) = file.write_all(data).and_then(|_| file.sync_all()) {
        drop(file);
        let _ = std::fs::remove_file(&path);
        let _ = std::fs::remove_dir(&directory);
        return SignatureStatus::Unavailable(format!("cannot write verification temp file: {e}"));
    }
    drop(file);

    let mut child = match Command::new(gpgv)
        .env_clear()
        .env("LC_ALL", "C")
        .arg("--status-fd")
        .arg("1")
        .arg("--keyring")
        .arg(keyring)
        .arg(&path)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
    {
        Ok(child) => child,
        Err(e) => {
            let _ = std::fs::remove_file(&path);
            let _ = std::fs::remove_dir(&directory);
            return SignatureStatus::Unavailable(format!("cannot run {}: {e}", gpgv.display()));
        }
    };
    let Some(mut stdout) = child.stdout.take() else {
        let _ = child.kill();
        let _ = child.wait();
        let _ = std::fs::remove_file(&path);
        let _ = std::fs::remove_dir(&directory);
        return SignatureStatus::Unavailable("gpgv status pipe is unavailable".into());
    };
    let reader = match thread::Builder::new().spawn(move || {
        let mut captured = Vec::with_capacity(MAX_STATUS_BYTES as usize + 1);
        let mut buffer = [0u8; 8192];
        loop {
            let count = stdout.read(&mut buffer)?;
            if count == 0 {
                return Ok::<_, std::io::Error>(captured);
            }
            if captured.len() <= MAX_STATUS_BYTES as usize {
                let remaining = MAX_STATUS_BYTES as usize + 1 - captured.len();
                captured.extend_from_slice(&buffer[..count.min(remaining)]);
            }
        }
    }) {
        Ok(reader) => reader,
        Err(e) => {
            let _ = child.kill();
            let _ = child.wait();
            let _ = std::fs::remove_file(&path);
            let _ = std::fs::remove_dir(&directory);
            return SignatureStatus::Unavailable(format!("cannot read gpgv status output: {e}"));
        }
    };
    let deadline = Instant::now() + timeout;
    let exit_status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) if Instant::now() < deadline => thread::sleep(Duration::from_millis(10)),
            Ok(None) => {
                let _ = child.kill();
                let _ = child.wait();
                let _ = reader.join();
                let _ = std::fs::remove_file(&path);
                let _ = std::fs::remove_dir(&directory);
                return SignatureStatus::Unavailable(format!(
                    "{} exceeded the {} second verification timeout",
                    gpgv.display(),
                    timeout.as_secs()
                ));
            }
            Err(e) => {
                let _ = child.kill();
                let _ = child.wait();
                let _ = reader.join();
                let _ = std::fs::remove_file(&path);
                let _ = std::fs::remove_dir(&directory);
                return SignatureStatus::Unavailable(format!("cannot wait for gpgv: {e}"));
            }
        }
    };
    let output = reader.join();
    let _ = std::fs::remove_file(&path);
    let _ = std::fs::remove_dir(&directory);
    let output = match output {
        Ok(Ok(output)) if output.len() <= MAX_STATUS_BYTES as usize => output,
        Ok(Ok(_)) => {
            return SignatureStatus::Unavailable(
                "gpgv status output exceeded its size limit".into(),
            );
        }
        Ok(Err(e)) => {
            return SignatureStatus::Unavailable(format!("cannot read gpgv status output: {e}"));
        }
        Err(_) => return SignatureStatus::Unavailable("gpgv status reader failed".into()),
    };
    let status = String::from_utf8_lossy(&output);
    let has_good = status.lines().any(|l| l.starts_with("[GNUPG:] GOODSIG "));
    let has_valid = status.lines().any(|l| l.starts_with("[GNUPG:] VALIDSIG "));
    let has_bad = status.lines().any(|l| {
        l.starts_with("[GNUPG:] BADSIG ")
            || l.starts_with("[GNUPG:] ERRSIG ")
            || l.starts_with("[GNUPG:] NO_PUBKEY ")
    });
    if exit_status.success() && has_good && has_valid && !has_bad {
        SignatureStatus::Verified
    } else {
        let reason = status
            .lines()
            .find(|l| {
                l.starts_with("[GNUPG:] BADSIG ")
                    || l.starts_with("[GNUPG:] ERRSIG ")
                    || l.starts_with("[GNUPG:] NO_PUBKEY ")
            })
            .unwrap_or("gpgv did not report a good signature");
        SignatureStatus::Invalid(reason.replace("[GNUPG:] ", ""))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn display_is_explicit_for_every_state() {
        assert_eq!(SignatureStatus::Verified.to_string(), "verified");
        assert!(
            SignatureStatus::Invalid("bad".into())
                .to_string()
                .contains("invalid")
        );
        assert!(
            SignatureStatus::Unavailable("missing".into())
                .to_string()
                .contains("unavailable")
        );
    }

    #[test]
    fn empty_data_and_oversize_data_are_not_verified() {
        assert!(matches!(verify_inrelease(&[]), SignatureStatus::Invalid(_)));
        assert!(matches!(
            verify_inrelease(&vec![b'x'; MAX_INRELEASE_BYTES + 1]),
            SignatureStatus::Unavailable(_)
        ));
    }

    fn test_gpgv() -> Option<PathBuf> {
        [
            "/usr/bin/gpgv",
            "/opt/homebrew/bin/gpgv",
            "/usr/local/bin/gpgv",
        ]
        .into_iter()
        .map(PathBuf::from)
        .find(|p| p.is_file())
    }

    fn fixture(name: &str) -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures/signed")
            .join(name)
    }

    #[test]
    fn real_ubuntu_signature_verifies_and_tampering_does_not() {
        let Some(gpgv) = test_gpgv() else { return };
        let keyring = fixture("ubuntu-archive-keyring.gpg");
        let good = std::fs::read(fixture("noble-updates-InRelease")).unwrap();
        let bad = std::fs::read(fixture("noble-updates-InRelease.tampered")).unwrap();
        assert_eq!(
            verify_inrelease_with(&good, &gpgv, &keyring),
            SignatureStatus::Verified
        );
        assert!(matches!(
            verify_inrelease_with(&bad, &gpgv, &keyring),
            SignatureStatus::Invalid(_)
        ));
    }

    #[test]
    fn unsigned_or_wrong_keyring_is_not_verified() {
        let Some(gpgv) = test_gpgv() else { return };
        let keyring = fixture("ubuntu-archive-keyring.gpg");
        let unsigned = b"Origin: Ubuntu\nDate: Thu, 12 Nov 2026  4:23:38 UTC\n";
        assert!(matches!(
            verify_inrelease_with(unsigned, &gpgv, &keyring),
            SignatureStatus::Invalid(_)
        ));
        let good = std::fs::read(fixture("noble-updates-InRelease")).unwrap();
        let empty_keyring =
            std::env::temp_dir().join(format!("apt-scout-empty-keyring-{}", std::process::id()));
        std::fs::write(&empty_keyring, []).unwrap();
        let status = verify_inrelease_with(&good, &gpgv, &empty_keyring);
        let _ = std::fs::remove_file(&empty_keyring);
        assert!(!status.is_verified());
    }

    #[cfg(unix)]
    #[test]
    fn oversized_status_output_is_bounded_and_unavailable() {
        use std::os::unix::fs::PermissionsExt;

        let temp =
            std::env::temp_dir().join(format!("apt-scout-test-output-{}", std::process::id()));
        std::fs::create_dir(&temp).unwrap();
        let fake_gpgv = temp.join("gpgv");
        std::fs::write(
            &fake_gpgv,
            b"#!/bin/sh\ni=0\nwhile [ $i -lt 1100 ]; do printf '%1024s\\n' ''; i=$((i+1)); done\n",
        )
        .unwrap();
        std::fs::set_permissions(&fake_gpgv, std::fs::Permissions::from_mode(0o700)).unwrap();
        let keyring = temp.join("keyring.gpg");
        std::fs::write(&keyring, b"test").unwrap();

        let status = verify_inrelease_with_timeout(
            b"signed data",
            &fake_gpgv,
            &keyring,
            Duration::from_secs(5),
        );
        assert!(
            matches!(status, SignatureStatus::Unavailable(reason) if reason.contains("size limit"))
        );
        std::fs::remove_dir_all(temp).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn a_hung_verifier_is_killed_and_reported_unavailable() {
        use std::os::unix::fs::PermissionsExt;

        let temp = std::env::temp_dir().join(format!("apt-scout-test-hang-{}", std::process::id()));
        std::fs::create_dir(&temp).unwrap();
        let fake_gpgv = temp.join("gpgv");
        std::fs::write(&fake_gpgv, b"#!/bin/sh\nwhile :; do :; done\n").unwrap();
        std::fs::set_permissions(&fake_gpgv, std::fs::Permissions::from_mode(0o700)).unwrap();
        let keyring = temp.join("keyring.gpg");
        std::fs::write(&keyring, b"test").unwrap();

        let started = Instant::now();
        let status = verify_inrelease_with_timeout(
            b"signed data",
            &fake_gpgv,
            &keyring,
            Duration::from_millis(50),
        );
        assert!(matches!(status, SignatureStatus::Unavailable(_)));
        assert!(started.elapsed() < Duration::from_secs(2));
        std::fs::remove_dir_all(temp).unwrap();
    }

    #[test]
    fn missing_tool_or_keyring_is_unavailable_not_verified() {
        let gpgv = Path::new("/definitely/not/gpgv");
        let keyring = Path::new("/definitely/not/keyring.gpg");
        let status = verify_inrelease_with(b"-----BEGIN PGP SIGNED MESSAGE-----\n", gpgv, keyring);
        assert!(matches!(status, SignatureStatus::Unavailable(_)));
    }
}
