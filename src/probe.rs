//! Probe one mirror (SPEC.md, Pipeline step 7): freshness and OpenPGP status from the full
//! `-updates` `InRelease`, the head of `-security` for the informational date, time to first byte,
//! and throughput on a ranged sample. Uses `Fetch`, so it is testable.

use std::fmt;

use crate::fresh;
use crate::http::{Fetch, HttpError};
use crate::verify::{self, SignatureStatus};

/// The deciding `-updates` pocket is verified, so the whole file is fetched. The cap is the
/// verifier's own limit: a body that downloads but cannot be verified would only mislead.
const INRELEASE_BYTES: u64 = verify::MAX_INRELEASE_BYTES as u64;
/// `-security` is informational, so only the head is read (`Date:` sits in the first lines).
const HEAD_BYTES: u64 = 4096;
const SAMPLE_BYTES: u64 = 1_048_576;
/// Smaller bodies are not a usable throughput sample (the real file is several MB).
const MIN_SAMPLE: u64 = 64 * 1024;

#[derive(Debug, PartialEq)]
pub struct Outcome {
    pub url: String,
    /// The mirror's `-updates` InRelease `Date` (Unix seconds). Lag is computed by the caller against
    /// the reference, or against the newest candidate when the reference is unreachable.
    pub updates_date: i64,
    /// OpenPGP status of the deciding `-updates` InRelease. Ranking can use unverified dates, but
    /// write decisions must require `Verified`.
    pub updates_signature: SignatureStatus,
    /// Informational only (SPEC step 8): `None` when the mirror has no `-security` InRelease.
    pub security_date: Option<i64>,
    /// Time to first byte of the deciding `-updates` request: DNS, TCP, TLS and one round trip,
    /// measured before any body byte arrives (SPEC step 7). Not a download time.
    pub latency_ms: u64,
    pub throughput_bps: u64,
    /// Set when the mirror answered from a different host than the one listed (SPEC step 7).
    pub redirected_to: Option<String>,
}

#[derive(Debug)]
pub enum ProbeError {
    Unreachable(HttpError),
    /// The deciding `-updates` InRelease is missing or unusable.
    BadUpdates(String),
    /// The throughput sample is not a real file.
    BadSample(String),
}

impl fmt::Display for ProbeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ProbeError::Unreachable(e) => write!(f, "{e}"),
            ProbeError::BadUpdates(why) | ProbeError::BadSample(why) => write!(f, "{why}"),
        }
    }
}

/// Reference `Date`s from the canonical archive, fetched once before probing.
pub struct Reference {
    pub updates: i64,
    pub updates_signature: SignatureStatus,
    /// `None` when the archive has no readable `-security` InRelease; it is informational only.
    pub security: Option<i64>,
}

fn inrelease_url(base: &str, suite: &str) -> String {
    format!("{}/dists/{suite}/InRelease", base.trim_end_matches('/'))
}

/// The deciding pocket: the whole file is fetched under the verification cap and its OpenPGP
/// signature is checked. Returns the `Date`, the request's time to first byte, the final URL and
/// the signature status.
fn date_of_verified(
    net: &dyn Fetch,
    base: &str,
    suite: &str,
) -> Result<(i64, u64, String, SignatureStatus), String> {
    let url = inrelease_url(base, suite);
    let got = net
        .get(&url, INRELEASE_BYTES, None)
        .map_err(|e| e.to_string())?;
    let text = String::from_utf8_lossy(&got.body);
    let date = fresh::release_date(&text).map_err(|e| format!("{url}: {e}"))?;
    let signature = verify::verify_inrelease(&got.body);
    Ok((date, got.ttfb.as_millis() as u64, got.final_url, signature))
}

/// The informational `-security` pocket (SPEC step 8): only `Date:` is used, so a ranged read of the
/// head is enough. Its signature is deliberately not checked — nothing may consume it.
fn date_of_cheap(net: &dyn Fetch, base: &str, suite: &str) -> Result<i64, String> {
    let url = inrelease_url(base, suite);
    let got = net
        .get(&url, HEAD_BYTES, Some((0, HEAD_BYTES - 1)))
        .map_err(|e| e.to_string())?;
    let text = String::from_utf8_lossy(&got.body);
    fresh::release_date(&text).map_err(|e| format!("{url}: {e}"))
}

#[derive(Debug, PartialEq)]
pub enum ReferenceError {
    /// The release is not on the canonical archive any more.
    NotFound(String),
    Other(String),
}

impl fmt::Display for ReferenceError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ReferenceError::NotFound(url) => write!(f, "{url} answered HTTP 404"),
            ReferenceError::Other(why) => write!(f, "{why}"),
        }
    }
}

pub fn fetch_reference(
    net: &dyn Fetch,
    base: &str,
    codename: &str,
) -> Result<Reference, ReferenceError> {
    let url = inrelease_url(base, &format!("{codename}-updates"));
    let got = net.get(&url, INRELEASE_BYTES, None).map_err(|e| match e {
        // Only the deciding pocket decides whether the release left the archive.
        HttpError::Status(404, u) => ReferenceError::NotFound(u),
        other => ReferenceError::Other(other.to_string()),
    })?;
    let updates = fresh::release_date(&String::from_utf8_lossy(&got.body))
        .map_err(|e| ReferenceError::Other(format!("{url}: {e}")))?;
    let updates_signature = verify::verify_inrelease(&got.body);
    let security = date_of_cheap(net, base, &format!("{codename}-security")).ok();
    Ok(Reference {
        updates,
        updates_signature,
        security,
    })
}

pub fn probe(
    net: &dyn Fetch,
    mirror: &str,
    codename: &str,
    arch: &str,
) -> Result<Outcome, ProbeError> {
    let base = mirror.trim_end_matches('/');
    let (updates, latency_ms, updates_final, updates_signature) =
        date_of_verified(net, base, &format!("{codename}-updates"))
            .map_err(ProbeError::BadUpdates)?;
    let security_date = date_of_cheap(net, base, &format!("{codename}-security")).ok();

    let sample_url = format!("{base}/dists/{codename}/main/binary-{arch}/Packages.xz");
    let sample = net
        .get(&sample_url, SAMPLE_BYTES, Some((0, SAMPLE_BYTES - 1)))
        .map_err(ProbeError::Unreachable)?;
    // A mirror's error page can come back as HTTP 200; only a real xz file counts as a sample.
    const XZ_MAGIC: [u8; 6] = [0xFD, b'7', b'z', b'X', b'Z', 0];
    if !sample.body.starts_with(&XZ_MAGIC) || (sample.body.len() as u64) < MIN_SAMPLE {
        return Err(ProbeError::BadSample(format!(
            "{sample_url} did not return an xz file ({} bytes read); the mirror may serve an error page with HTTP 200",
            sample.body.len()
        )));
    }
    let secs = sample.elapsed.as_secs_f64().max(0.001);
    let throughput_bps = (sample.body.len() as f64 / secs) as u64;

    Ok(Outcome {
        url: base.to_string(),
        updates_date: updates,
        updates_signature,
        security_date,
        redirected_to: other_host(base, &updates_final),
        latency_ms,
        throughput_bps,
    })
}

/// `Some(host)` when `final_url` is on a different host than the listed mirror `base`.
fn other_host(base: &str, final_url: &str) -> Option<String> {
    let host = |u: &str| {
        u.split_once("://")
            .map(|(_, r)| r)
            .unwrap_or(u)
            .split(['/', ':'])
            .next()
            .unwrap_or_default()
            .to_ascii_lowercase()
    };
    let (a, b) = (host(base), host(final_url));
    (a != b).then_some(b)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::http::{Canned, Fetched, HttpError};

    const UPD: &str = "-----BEGIN PGP SIGNED MESSAGE-----\nHash: SHA512\n\nOrigin: Ubuntu\nDate: Thu, 12 Nov 2026  4:23:38 UTC\n";
    const UPD_OLD: &str = "-----BEGIN PGP SIGNED MESSAGE-----\nHash: SHA512\n\nOrigin: Ubuntu\nDate: Wed, 11 Nov 2026 22:06:33 UTC\n";
    const SEC: &str = "-----BEGIN PGP SIGNED MESSAGE-----\nHash: SHA512\n\nOrigin: Ubuntu\nDate: Thu, 12 Nov 2026  2:19:32 UTC\n";

    /// A body that looks like the start of an xz file and is large enough to be a sample.
    static XZ_OK: &[u8] = &{
        let mut b = [0u8; 70_000];
        b[0] = 0xFD;
        b[1] = b'7';
        b[2] = b'z';
        b[3] = b'X';
        b[4] = b'Z';
        b
    };

    /// One recorded request: URL, byte cap and optional range.
    type Call = (String, u64, Option<(u64, u64)>);

    /// Records each request and rejects an oversized un-ranged body like `Net` does, so tests can
    /// assert which pocket fetches what and that the size cap is really applied at download time.
    struct Recording {
        answers: Vec<(&'static str, Result<&'static [u8], u16>)>,
        calls: std::cell::RefCell<Vec<Call>>,
        ttfb: std::time::Duration,
        elapsed: std::time::Duration,
    }

    impl Recording {
        fn new(answers: Vec<(&'static str, Result<&'static [u8], u16>)>) -> Self {
            Self {
                answers,
                calls: std::cell::RefCell::new(Vec::new()),
                ttfb: std::time::Duration::from_millis(5),
                elapsed: std::time::Duration::from_millis(5),
            }
        }

        fn calls(&self) -> Vec<Call> {
            self.calls.borrow().clone()
        }
    }

    impl Fetch for Recording {
        fn get(
            &self,
            url: &str,
            max: u64,
            range: Option<(u64, u64)>,
        ) -> Result<Fetched, HttpError> {
            self.calls.borrow_mut().push((url.to_string(), max, range));
            match self.answers.iter().find(|(u, _)| *u == url) {
                Some((_, Ok(body))) => {
                    if range.is_none() && body.len() as u64 > max {
                        return Err(HttpError::TooLarge(max, url.to_string()));
                    }
                    Ok(Fetched {
                        body: body.to_vec(),
                        elapsed: self.elapsed,
                        ttfb: self.ttfb,
                        final_url: url.to_string(),
                    })
                }
                Some((_, Err(code))) => Err(HttpError::Status(*code, url.to_string())),
                None => Err(HttpError::Transport(
                    "no canned answer".into(),
                    url.to_string(),
                )),
            }
        }
    }

    const M_UPD: &str = "http://m.example/ubuntu/dists/noble-updates/InRelease";
    const M_SEC: &str = "http://m.example/ubuntu/dists/noble-security/InRelease";
    const M_SAMPLE: &str = "http://m.example/ubuntu/dists/noble/main/binary-amd64/Packages.xz";

    fn recording() -> Recording {
        Recording::new(vec![
            (M_UPD, Ok(UPD.as_bytes())),
            (M_SEC, Ok(SEC.as_bytes())),
            (M_SAMPLE, Ok(XZ_OK)),
        ])
    }

    fn reference() -> Reference {
        Reference {
            updates: fresh::release_date(UPD).unwrap(),
            updates_signature: SignatureStatus::Invalid("test fixture is unsigned".into()),
            security: Some(fresh::release_date(SEC).unwrap()),
        }
    }

    #[test]
    fn reference_survives_a_broken_security_pocket() {
        let c = Canned(vec![
            (
                "http://archive.ubuntu.com/ubuntu/dists/noble-updates/InRelease",
                Ok(UPD.as_bytes()),
            ),
            (
                "http://archive.ubuntu.com/ubuntu/dists/noble-security/InRelease",
                Err(500),
            ),
        ]);
        let r = fetch_reference(&c, "http://archive.ubuntu.com/ubuntu", "noble").unwrap();
        assert_eq!(r.security, None);
        assert_eq!(r.updates, fresh::release_date(UPD).unwrap());
    }

    #[test]
    fn missing_security_pocket_is_not_a_removed_release() {
        let c = Canned(vec![
            (
                "http://archive.ubuntu.com/ubuntu/dists/noble-updates/InRelease",
                Ok(UPD.as_bytes()),
            ),
            (
                "http://archive.ubuntu.com/ubuntu/dists/noble-security/InRelease",
                Err(404),
            ),
        ]);
        assert!(fetch_reference(&c, "http://archive.ubuntu.com/ubuntu", "noble").is_ok());
    }

    #[test]
    fn reference_404_means_release_left_the_archive() {
        let c = Canned(vec![(
            "http://archive.ubuntu.com/ubuntu/dists/bionic-updates/InRelease",
            Err(404),
        )]);
        assert!(matches!(
            fetch_reference(&c, "http://archive.ubuntu.com/ubuntu", "bionic"),
            Err(ReferenceError::NotFound(_))
        ));
    }

    #[test]
    fn reference_network_error_is_not_notfound() {
        let c = Canned(vec![]);
        assert!(matches!(
            fetch_reference(&c, "http://archive.ubuntu.com/ubuntu", "noble"),
            Err(ReferenceError::Other(_))
        ));
    }

    #[test]
    fn reference_from_archive() {
        let c = Canned(vec![
            (
                "http://archive.ubuntu.com/ubuntu/dists/noble-updates/InRelease",
                Ok(UPD.as_bytes()),
            ),
            (
                "http://archive.ubuntu.com/ubuntu/dists/noble-security/InRelease",
                Ok(SEC.as_bytes()),
            ),
        ]);
        let r = fetch_reference(&c, "http://archive.ubuntu.com/ubuntu/", "noble").unwrap();
        assert_eq!(r.updates, fresh::release_date(UPD).unwrap());
    }

    #[test]
    fn fresh_mirror_has_zero_lag_and_throughput() {
        let c = Canned(vec![
            (
                "http://m.example/ubuntu/dists/noble-updates/InRelease",
                Ok(UPD.as_bytes()),
            ),
            (
                "http://m.example/ubuntu/dists/noble-security/InRelease",
                Ok(SEC.as_bytes()),
            ),
            (
                "http://m.example/ubuntu/dists/noble/main/binary-amd64/Packages.xz",
                Ok(XZ_OK),
            ),
        ]);
        let o = probe(&c, "http://m.example/ubuntu/", "noble", "amd64").unwrap();
        assert_eq!(fresh::lag(reference().updates, o.updates_date), 0);
        assert_eq!(
            o.security_date
                .map(|d| fresh::lag(reference().security.unwrap(), d)),
            Some(0)
        );
        assert_eq!(o.url, "http://m.example/ubuntu");
        assert!(o.throughput_bps > 0);
    }

    #[test]
    fn lagging_mirror_reports_measured_lag() {
        let c = Canned(vec![
            (
                "http://m.example/ubuntu/dists/noble-updates/InRelease",
                Ok(UPD_OLD.as_bytes()),
            ),
            (
                "http://m.example/ubuntu/dists/noble-security/InRelease",
                Ok(SEC.as_bytes()),
            ),
            (
                "http://m.example/ubuntu/dists/noble/main/binary-amd64/Packages.xz",
                Ok(XZ_OK),
            ),
        ]);
        let o = probe(&c, "http://m.example/ubuntu", "noble", "amd64").unwrap();
        let lag = fresh::lag(reference().updates, o.updates_date);
        assert_eq!(lag, 6 * 3600 + 17 * 60 + 5); // 04:23:38 - 22:06:33 the day before
        assert!(!fresh::is_stale(lag));
    }

    #[test]
    fn missing_security_is_informational() {
        let c = Canned(vec![
            (
                "http://m.example/ubuntu/dists/noble-updates/InRelease",
                Ok(UPD.as_bytes()),
            ),
            (
                "http://m.example/ubuntu/dists/noble-security/InRelease",
                Err(404),
            ),
            (
                "http://m.example/ubuntu/dists/noble/main/binary-amd64/Packages.xz",
                Ok(XZ_OK),
            ),
        ]);
        let o = probe(&c, "http://m.example/ubuntu", "noble", "amd64").unwrap();
        assert_eq!(o.security_date, None);
    }

    #[test]
    fn redirect_to_another_host_is_reported() {
        assert_eq!(
            other_host("http://a.example/ubuntu", "http://b.example/ubuntu/dists/x"),
            Some("b.example".to_string())
        );
        assert_eq!(
            other_host(
                "http://A.example/ubuntu",
                "https://a.example/ubuntu/dists/x"
            ),
            None
        );
    }

    #[test]
    fn html_error_page_with_200_is_not_a_sample() {
        let c = Canned(vec![
            (
                "http://m.example/ubuntu/dists/noble-updates/InRelease",
                Ok(UPD.as_bytes()),
            ),
            (
                "http://m.example/ubuntu/dists/noble-security/InRelease",
                Ok(SEC.as_bytes()),
            ),
            (
                "http://m.example/ubuntu/dists/noble/main/binary-amd64/Packages.xz",
                Ok(b"<html>404 not found</html>"),
            ),
        ]);
        let e = probe(&c, "http://m.example/ubuntu", "noble", "amd64").unwrap_err();
        assert!(matches!(e, ProbeError::BadSample(_)), "{e}");
        assert!(e.to_string().contains("did not return an xz file"), "{e}");
    }

    #[test]
    fn too_short_xz_is_not_a_sample() {
        let c = Canned(vec![
            (
                "http://m.example/ubuntu/dists/noble-updates/InRelease",
                Ok(UPD.as_bytes()),
            ),
            (
                "http://m.example/ubuntu/dists/noble-security/InRelease",
                Ok(SEC.as_bytes()),
            ),
            (
                "http://m.example/ubuntu/dists/noble/main/binary-amd64/Packages.xz",
                Ok(&[0xFD, b'7', b'z', b'X', b'Z', 0, 1, 2]),
            ),
        ]);
        assert!(matches!(
            probe(&c, "http://m.example/ubuntu", "noble", "amd64",),
            Err(ProbeError::BadSample(_))
        ));
    }

    #[test]
    fn missing_updates_rejects_the_mirror() {
        let c = Canned(vec![(
            "http://m.example/ubuntu/dists/noble-updates/InRelease",
            Err(404),
        )]);
        assert!(matches!(
            probe(&c, "http://m.example/ubuntu", "noble", "amd64",),
            Err(ProbeError::BadUpdates(_))
        ));
    }

    #[test]
    fn unparsable_date_rejects_the_mirror() {
        let c = Canned(vec![(
            "http://m.example/ubuntu/dists/noble-updates/InRelease",
            Ok(b"<html>not a release</html>"),
        )]);
        let e = probe(&c, "http://m.example/ubuntu", "noble", "amd64").unwrap_err();
        assert!(e.to_string().contains("no 'Date:' header"), "{e}");
    }

    #[test]
    fn unreachable_sample_is_reported() {
        let c = Canned(vec![
            (
                "http://m.example/ubuntu/dists/noble-updates/InRelease",
                Ok(UPD.as_bytes()),
            ),
            (
                "http://m.example/ubuntu/dists/noble-security/InRelease",
                Ok(SEC.as_bytes()),
            ),
        ]);
        assert!(matches!(
            probe(&c, "http://m.example/ubuntu", "noble", "amd64",),
            Err(ProbeError::Unreachable(_))
        ));
    }

    #[test]
    fn updates_pocket_is_fetched_whole_and_security_pocket_only_reads_the_head() {
        let rec = recording();
        probe(&rec, "http://m.example/ubuntu", "noble", "amd64").unwrap();
        let calls = rec.calls();
        let find = |url: &str| {
            calls
                .iter()
                .find(|(u, _, _)| u == url)
                .unwrap_or_else(|| panic!("no request for {url}"))
        };
        let (_, max, range) = find(M_UPD);
        assert_eq!(*range, None);
        assert_eq!(*max, verify::MAX_INRELEASE_BYTES as u64);
        let (_, max, range) = find(M_SEC);
        assert_eq!(*range, Some((0, 4095)));
        assert_eq!(*max, 4096);
        let (_, _, range) = find(M_SAMPLE);
        assert_eq!(*range, Some((0, 1_048_575)));

        // the reference fetches the same two pockets, and `-security` is ranged there too
        let rec = recording();
        fetch_reference(&rec, "http://m.example/ubuntu", "noble").unwrap();
        let calls = rec.calls();
        assert_eq!(calls.len(), 2, "{calls:?}");
        let security = calls
            .iter()
            .find(|(u, _, _)| u == M_SEC)
            .unwrap_or_else(|| panic!("no request for {M_SEC}"));
        assert_eq!(security.2, Some((0, 4095)));
    }

    #[test]
    fn latency_is_time_to_first_byte_not_the_download() {
        let mut rec = recording();
        rec.ttfb = std::time::Duration::from_millis(7);
        rec.elapsed = std::time::Duration::from_millis(900);
        let o = probe(&rec, "http://m.example/ubuntu", "noble", "amd64").unwrap();
        assert_eq!(o.latency_ms, 7);
    }

    #[test]
    fn an_inrelease_over_the_verification_cap_is_rejected_at_download() {
        let big: &'static [u8] =
            Box::leak(vec![b'x'; verify::MAX_INRELEASE_BYTES + 1].into_boxed_slice());
        let rec = Recording::new(vec![(M_UPD, Ok(big))]);
        let e = probe(&rec, "http://m.example/ubuntu", "noble", "amd64").unwrap_err();
        let ProbeError::BadUpdates(msg) = &e else {
            panic!("expected BadUpdates, got {e}");
        };
        assert!(msg.contains("524288"), "{msg}");
    }
}
