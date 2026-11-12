//! Probe one mirror (SPEC.md, Pipeline step 7): freshness from the head of `-updates`/`-security`
//! InRelease, connect latency, and throughput on a ranged sample. Uses `Fetch`, so it is testable.
#![allow(dead_code)] // ponytail: not called by main yet; remove this allow when the pipeline uses it.

use std::fmt;

use crate::fresh;
use crate::http::{Fetch, HttpError};

/// `Date:` is within the first lines of InRelease; 4 KiB is enough (measured on LT/NO mirrors).
const HEAD_BYTES: u64 = 4096;
const SAMPLE_BYTES: u64 = 1_048_576;

#[derive(Debug, PartialEq)]
pub struct Outcome {
    pub url: String,
    /// Reference minus mirror `-updates` Date, clamped to 0.
    pub updates_lag_secs: i64,
    /// Informational only (SPEC step 8): `None` when the mirror has no `-security` InRelease.
    pub security_lag_secs: Option<i64>,
    pub latency_ms: u64,
    pub throughput_bps: u64,
}

#[derive(Debug)]
pub enum ProbeError {
    Unreachable(HttpError),
    /// The deciding `-updates` InRelease is missing or unusable.
    BadUpdates(String),
}

impl fmt::Display for ProbeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ProbeError::Unreachable(e) => write!(f, "{e}"),
            ProbeError::BadUpdates(why) => write!(f, "{why}"),
        }
    }
}

/// Reference `Date`s from the canonical archive, fetched once before probing.
pub struct Reference {
    pub updates: i64,
    pub security: i64,
}

fn date_of(net: &dyn Fetch, base: &str, suite: &str) -> Result<(i64, u64), String> {
    let url = format!("{}/dists/{suite}/InRelease", base.trim_end_matches('/'));
    let got = net
        .get(&url, HEAD_BYTES, Some((0, HEAD_BYTES - 1)))
        .map_err(|e| e.to_string())?;
    let text = String::from_utf8_lossy(&got.body);
    let date = fresh::release_date(&text).map_err(|e| format!("{url}: {e}"))?;
    Ok((date, got.elapsed.as_millis() as u64))
}

pub fn fetch_reference(net: &dyn Fetch, base: &str, codename: &str) -> Result<Reference, String> {
    Ok(Reference {
        updates: date_of(net, base, &format!("{codename}-updates"))?.0,
        security: date_of(net, base, &format!("{codename}-security"))?.0,
    })
}

pub fn probe(
    net: &dyn Fetch,
    mirror: &str,
    codename: &str,
    arch: &str,
    reference: &Reference,
) -> Result<Outcome, ProbeError> {
    let base = mirror.trim_end_matches('/');
    let (updates, latency_ms) =
        date_of(net, base, &format!("{codename}-updates")).map_err(ProbeError::BadUpdates)?;
    let security_lag_secs = date_of(net, base, &format!("{codename}-security"))
        .ok()
        .map(|(d, _)| fresh::lag(reference.security, d));

    let sample_url = format!("{base}/dists/{codename}/main/binary-{arch}/Packages.xz");
    let sample = net
        .get(&sample_url, SAMPLE_BYTES, Some((0, SAMPLE_BYTES - 1)))
        .map_err(ProbeError::Unreachable)?;
    let secs = sample.elapsed.as_secs_f64().max(0.001);
    let throughput_bps = (sample.body.len() as f64 / secs) as u64;

    Ok(Outcome {
        url: base.to_string(),
        updates_lag_secs: fresh::lag(reference.updates, updates),
        security_lag_secs,
        latency_ms,
        throughput_bps,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::http::Canned;

    const UPD: &str = "-----BEGIN PGP SIGNED MESSAGE-----\nHash: SHA512\n\nOrigin: Ubuntu\nDate: Thu, 12 Nov 2026  4:23:38 UTC\n";
    const UPD_OLD: &str = "-----BEGIN PGP SIGNED MESSAGE-----\nHash: SHA512\n\nOrigin: Ubuntu\nDate: Wed, 11 Nov 2026 22:06:33 UTC\n";
    const SEC: &str = "-----BEGIN PGP SIGNED MESSAGE-----\nHash: SHA512\n\nOrigin: Ubuntu\nDate: Thu, 12 Nov 2026  2:19:32 UTC\n";

    fn reference() -> Reference {
        Reference {
            updates: fresh::release_date(UPD).unwrap(),
            security: fresh::release_date(SEC).unwrap(),
        }
    }

    #[test]
    fn reference_from_archive() {
        let c = Canned(vec![
            (
                "http://archive.ubuntu.com/ubuntu/dists/noble-updates/InRelease",
                Ok(UPD),
            ),
            (
                "http://archive.ubuntu.com/ubuntu/dists/noble-security/InRelease",
                Ok(SEC),
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
                Ok(UPD),
            ),
            (
                "http://m.example/ubuntu/dists/noble-security/InRelease",
                Ok(SEC),
            ),
            (
                "http://m.example/ubuntu/dists/noble/main/binary-amd64/Packages.xz",
                Ok("0123456789"),
            ),
        ]);
        let o = probe(
            &c,
            "http://m.example/ubuntu/",
            "noble",
            "amd64",
            &reference(),
        )
        .unwrap();
        assert_eq!(o.updates_lag_secs, 0);
        assert_eq!(o.security_lag_secs, Some(0));
        assert_eq!(o.url, "http://m.example/ubuntu");
        assert!(o.throughput_bps > 0);
    }

    #[test]
    fn lagging_mirror_reports_measured_lag() {
        let c = Canned(vec![
            (
                "http://m.example/ubuntu/dists/noble-updates/InRelease",
                Ok(UPD_OLD),
            ),
            (
                "http://m.example/ubuntu/dists/noble-security/InRelease",
                Ok(SEC),
            ),
            (
                "http://m.example/ubuntu/dists/noble/main/binary-amd64/Packages.xz",
                Ok("x"),
            ),
        ]);
        let o = probe(
            &c,
            "http://m.example/ubuntu",
            "noble",
            "amd64",
            &reference(),
        )
        .unwrap();
        assert_eq!(o.updates_lag_secs, 6 * 3600 + 17 * 60 + 5); // 04:23:38 - 22:06:33 the day before
        assert!(!fresh::is_stale(o.updates_lag_secs));
    }

    #[test]
    fn missing_security_is_informational() {
        let c = Canned(vec![
            (
                "http://m.example/ubuntu/dists/noble-updates/InRelease",
                Ok(UPD),
            ),
            (
                "http://m.example/ubuntu/dists/noble-security/InRelease",
                Err(404),
            ),
            (
                "http://m.example/ubuntu/dists/noble/main/binary-amd64/Packages.xz",
                Ok("x"),
            ),
        ]);
        let o = probe(
            &c,
            "http://m.example/ubuntu",
            "noble",
            "amd64",
            &reference(),
        )
        .unwrap();
        assert_eq!(o.security_lag_secs, None);
    }

    #[test]
    fn missing_updates_rejects_the_mirror() {
        let c = Canned(vec![(
            "http://m.example/ubuntu/dists/noble-updates/InRelease",
            Err(404),
        )]);
        assert!(matches!(
            probe(
                &c,
                "http://m.example/ubuntu",
                "noble",
                "amd64",
                &reference()
            ),
            Err(ProbeError::BadUpdates(_))
        ));
    }

    #[test]
    fn unparsable_date_rejects_the_mirror() {
        let c = Canned(vec![(
            "http://m.example/ubuntu/dists/noble-updates/InRelease",
            Ok("<html>not a release</html>"),
        )]);
        let e = probe(
            &c,
            "http://m.example/ubuntu",
            "noble",
            "amd64",
            &reference(),
        )
        .unwrap_err();
        assert!(e.to_string().contains("no 'Date:' header"), "{e}");
    }

    #[test]
    fn unreachable_sample_is_reported() {
        let c = Canned(vec![
            (
                "http://m.example/ubuntu/dists/noble-updates/InRelease",
                Ok(UPD),
            ),
            (
                "http://m.example/ubuntu/dists/noble-security/InRelease",
                Ok(SEC),
            ),
        ]);
        assert!(matches!(
            probe(
                &c,
                "http://m.example/ubuntu",
                "noble",
                "amd64",
                &reference()
            ),
            Err(ProbeError::Unreachable(_))
        ));
    }
}
