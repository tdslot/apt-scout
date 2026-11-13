//! OpenPGP verification for `InRelease` using the system `gpgv` and Ubuntu archive keyring.
//!
//! Ranking may use an unverified `Date`; a write decision requires `Verified`, and `Verified`
//! cannot be built without this module's `gpgv` pass (D25).
//! No shell is used, the executable and keyring paths are fixed, and the environment is cleared.

use std::fmt;
use std::io::{Read, Write};
#[cfg(unix)]
use std::os::unix::fs::DirBuilderExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};

use crate::fresh;

pub const GPGV: &str = "/usr/bin/gpgv";
pub const UBUNTU_KEYRING: &str = "/usr/share/keyrings/ubuntu-archive-keyring.gpg";
/// Maximum `InRelease` accepted for verification. `probe` uses it as its download cap, so a body
/// can never download successfully and then fail verification for being too large.
pub const MAX_INRELEASE_BYTES: usize = 512 * 1024;
const GPGV_TIMEOUT: Duration = Duration::from_secs(10);
const MAX_STATUS_BYTES: u64 = 1024 * 1024;

/// The token a successful verification hands out: `gpgv` reported `GOODSIG` and `VALIDSIG`, exited
/// zero and reported no bad signature. The field is private to this module and only
/// `verify_inrelease_with_timeout` builds one, so no other code path in this crate — the CLI
/// included — can produce a value `VerifiedDate` accepts as proof (D25, `docs/threat-model.md`
/// 3.10). A `compile_fail` doctest cannot say that (the crate has no `[lib]` target);
/// `the_write_proof_is_minted_only_in_the_verifier` is the executable half of the invariant.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct VerifiedProof(());

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SignatureStatus {
    /// `gpgv` reported a valid signature from the configured Ubuntu archive keyring. It carries the
    /// proof token, so the only way to hold this variant is to have verification hand it to you.
    Verified(VerifiedProof),
    /// The data was readable but is unsigned, malformed, altered, or signed by an unknown key.
    Invalid(String),
    /// Verification could not run (missing tool/keyring, unsafe temp state, size limit, spawn error).
    Unavailable(String),
}

impl SignatureStatus {
    pub fn is_verified(&self) -> bool {
        matches!(self, SignatureStatus::Verified(_))
    }

    /// A proof for the tests and for the write harness, which is a test artifact of its own (W1/W2
    /// pin it out of the shipped binary). Compiled out of every shipped build, so this is not a
    /// second door into `Verified`: the shipped binary has one, and it is the `gpgv` run.
    #[cfg(any(test, apt_scout_write_harness))]
    pub fn verified_for_test() -> SignatureStatus {
        SignatureStatus::Verified(VerifiedProof(()))
    }
}

impl fmt::Display for SignatureStatus {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            SignatureStatus::Verified(_) => write!(f, "verified"),
            SignatureStatus::Invalid(why) => write!(f, "invalid ({why})"),
            SignatureStatus::Unavailable(why) => write!(f, "unavailable ({why})"),
        }
    }
}

/// A freshness `Date` that came out of an `InRelease` whose signature is `Verified`.
/// The field is private, and the only constructor needs both halves of the proof: a `Verified`
/// status and a `fresh::SignedDate`, which only the strict parser inside `fresh` builds. So a write
/// path can take `VerifiedDate` and cannot be handed a date from outside the signed region.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct VerifiedDate(fresh::SignedDate);

impl VerifiedDate {
    pub fn seconds(self) -> i64 {
        self.0.seconds()
    }
}

impl SignatureStatus {
    /// The date is usable as a trusted freshness value only when this status is `Verified`.
    pub fn verified_date(&self, date: fresh::SignedDate) -> Option<VerifiedDate> {
        self.is_verified().then_some(VerifiedDate(date))
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

/// Verify with explicit tool and keyring paths. The private directory is also gpgv's homedir, so no
/// user GnuPG configuration or default keyring can influence the result.
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
            let _ = std::fs::remove_dir_all(&directory);
            return SignatureStatus::Unavailable(format!(
                "cannot create verification temp file: {e}"
            ));
        }
    };
    if let Err(e) = file.write_all(data).and_then(|_| file.sync_all()) {
        drop(file);
        let _ = std::fs::remove_file(&path);
        let _ = std::fs::remove_dir_all(&directory);
        return SignatureStatus::Unavailable(format!("cannot write verification temp file: {e}"));
    }
    drop(file);

    let mut child = match Command::new(gpgv)
        .env_clear()
        .env("LC_ALL", "C")
        .arg("--status-fd")
        .arg("1")
        .arg("--homedir")
        .arg(&directory)
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
            let _ = std::fs::remove_dir_all(&directory);
            return SignatureStatus::Unavailable(format!("cannot run {}: {e}", gpgv.display()));
        }
    };
    let Some(mut stdout) = child.stdout.take() else {
        let _ = child.kill();
        let _ = child.wait();
        let _ = std::fs::remove_file(&path);
        let _ = std::fs::remove_dir_all(&directory);
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
            let _ = std::fs::remove_dir_all(&directory);
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
                let _ = std::fs::remove_dir_all(&directory);
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
                let _ = std::fs::remove_dir_all(&directory);
                return SignatureStatus::Unavailable(format!("cannot wait for gpgv: {e}"));
            }
        }
    };
    let output = reader.join();
    let _ = std::fs::remove_file(&path);
    let _ = std::fs::remove_dir_all(&directory);
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
        SignatureStatus::Verified(VerifiedProof(()))
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
        assert_eq!(SignatureStatus::verified_for_test().to_string(), "verified");
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
            SignatureStatus::verified_for_test()
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

    /// Write an executable fake `gpgv` script and a dummy keyring into a private temp directory.
    #[cfg(unix)]
    fn fake_gpgv(tag: &str, script: &str) -> (PathBuf, PathBuf, PathBuf) {
        use std::os::unix::fs::PermissionsExt;

        let temp =
            std::env::temp_dir().join(format!("apt-scout-test-{tag}-{}", std::process::id()));
        std::fs::create_dir(&temp).unwrap();
        let gpgv = temp.join("gpgv");
        std::fs::write(&gpgv, script).unwrap();
        std::fs::set_permissions(&gpgv, std::fs::Permissions::from_mode(0o700)).unwrap();
        let keyring = temp.join("keyring.gpg");
        std::fs::write(&keyring, b"test").unwrap();
        (temp, gpgv, keyring)
    }

    #[cfg(unix)]
    #[test]
    fn oversized_status_output_is_bounded_and_unavailable() {
        let (temp, fake_gpgv, keyring) = fake_gpgv(
            "output",
            "#!/bin/sh\ni=0\nwhile [ $i -lt 1100 ]; do printf '%1024s\\n' ''; i=$((i+1)); done\n",
        );

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
        let (temp, fake_gpgv, keyring) = fake_gpgv("hang", "#!/bin/sh\nwhile :; do :; done\n");

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

    #[cfg(unix)]
    #[test]
    fn a_good_status_stream_is_verified() {
        let (temp, gpgv, keyring) = fake_gpgv(
            "goodsigs",
            "#!/bin/sh\nprintf '%s\\n' '[GNUPG:] GOODSIG 1234 Ubuntu Archive Automatic Signing Key' '[GNUPG:] VALIDSIG 1234 5678'\nexit 0\n",
        );
        let status =
            verify_inrelease_with_timeout(b"signed data", &gpgv, &keyring, Duration::from_secs(5));
        assert_eq!(status, SignatureStatus::verified_for_test());
        std::fs::remove_dir_all(temp).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn errsig_with_a_zero_exit_is_invalid() {
        let (temp, gpgv, keyring) = fake_gpgv(
            "errsig",
            "#!/bin/sh\nprintf '%s\\n' '[GNUPG:] ERRSIG 1234 1 8 00 1 1234 0'\nexit 0\n",
        );
        let status =
            verify_inrelease_with_timeout(b"signed data", &gpgv, &keyring, Duration::from_secs(5));
        assert!(matches!(status, SignatureStatus::Invalid(_)), "{status}");
        std::fs::remove_dir_all(temp).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn good_lines_with_a_nonzero_exit_are_invalid() {
        let (temp, gpgv, keyring) = fake_gpgv(
            "exit1",
            "#!/bin/sh\nprintf '%s\\n' '[GNUPG:] GOODSIG 1234 Ubuntu Archive Automatic Signing Key' '[GNUPG:] VALIDSIG 1234 5678'\nexit 1\n",
        );
        let status =
            verify_inrelease_with_timeout(b"signed data", &gpgv, &keyring, Duration::from_secs(5));
        assert!(matches!(status, SignatureStatus::Invalid(_)), "{status}");
        std::fs::remove_dir_all(temp).unwrap();
    }

    #[test]
    fn missing_tool_or_keyring_is_unavailable_not_verified() {
        let gpgv = Path::new("/definitely/not/gpgv");
        let keyring = Path::new("/definitely/not/keyring.gpg");
        let status = verify_inrelease_with(b"-----BEGIN PGP SIGNED MESSAGE-----\n", gpgv, keyring);
        assert!(matches!(status, SignatureStatus::Unavailable(_)));
    }

    #[test]
    fn verified_date_requires_a_verified_status() {
        // The strict parse refuses a prefixed body, so no `SignedDate` exists to gate on at all:
        let forged = "Date: Thu, 1 Jan 2099 00:00:00 UTC\n-----BEGIN PGP SIGNED MESSAGE-----\nHash: SHA512\n\nOrigin: Ubuntu\nDate: Thu, 12 Nov 2026  4:23:38 UTC\n";
        assert!(fresh::release_date(forged).is_err());
        let signed = fresh::release_date("-----BEGIN PGP SIGNED MESSAGE-----\nHash: SHA512\n\nDate: Thu, 12 Nov 2026  4:23:38 UTC\n").unwrap();
        assert_eq!(
            SignatureStatus::verified_for_test()
                .verified_date(signed)
                .map(|d| d.seconds()),
            Some(1_794_457_418)
        );
        assert!(
            SignatureStatus::Invalid("bad".into())
                .verified_date(signed)
                .is_none()
        );
        assert!(
            SignatureStatus::Unavailable("missing".into())
                .verified_date(signed)
                .is_none()
        );
    }

    /// Why the parser, not `gpgv`, has to enforce the signed region (P1-1): `gpgv` exits 0 on a
    /// prefixed body, so verification alone reports `Verified` for data whose prefix it never signed.
    #[test]
    fn gpgv_accepts_a_prefixed_body() {
        let Some(gpgv) = test_gpgv() else { return }; // existing helper: skips without a local gpgv
        let keyring = fixture("ubuntu-archive-keyring.gpg");
        let good = std::fs::read(fixture("noble-updates-InRelease")).unwrap();
        let mut prefixed = b"Date: Thu, 1 Jan 2099 00:00:00 UTC\n".to_vec();
        prefixed.extend_from_slice(&good);
        assert_eq!(
            verify_inrelease_with(&prefixed, &gpgv, &keyring),
            SignatureStatus::verified_for_test()
        );
    }

    /// The proof cannot be minted outside this module (D25, `docs/threat-model.md` 3.10).
    ///
    /// `SignatureStatus::Verified` carries a `VerifiedProof` whose only field is private here, so the
    /// token exists only where `gpgv` reported a good signature; every other module has to reach it
    /// through `verified_for_test`, which a `cfg(test)` or `apt_scout_write_harness` build compiles
    /// and the shipped binary does not (W1/W2 pin the harness out of it). A `compile_fail` doctest
    /// cannot state this: the crate has no `[lib]` target, so no documentation test is ever compiled.
    /// This test is the executable half — it reads every source file of the crate and fails when a
    /// second `VerifiedProof(...)` appears outside `src/verify.rs`.
    #[test]
    fn the_write_proof_is_minted_only_in_the_verifier() {
        let src = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
        let mut checked = 0;
        for entry in std::fs::read_dir(&src).expect("the crate's source directory") {
            let path = entry.expect("a source file").path();
            if path.extension().is_none_or(|ext| ext != "rs") || path == src.join("verify.rs") {
                continue;
            }
            let text = std::fs::read_to_string(&path).expect("a source file is readable UTF-8");
            assert!(
                !text.contains("VerifiedProof("),
                "{} mints a write proof; only src/verify.rs may (D25)",
                path.display()
            );
            checked += 1;
        }
        assert!(
            checked >= 5,
            "expected the crate's modules, checked {checked}"
        );
    }
}
