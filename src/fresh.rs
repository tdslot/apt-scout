//! Freshness from InRelease `Date:` (SPEC.md, Pipeline steps 6-9). Pure functions, no network.
#![allow(dead_code)] // ponytail: not called by main yet; remove this allow when the pipeline uses it.

use std::fmt;

/// Stale mirrors are rejected when `-updates` lags the reference by more than this (SPEC step 8).
pub const MAX_LAG_SECS: i64 = 24 * 3600;

#[derive(Debug, PartialEq)]
pub enum DateError {
    NoDateHeader,
    Malformed(String),
}

impl fmt::Display for DateError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            DateError::NoDateHeader => write!(
                f,
                "InRelease has no 'Date:' header; the mirror may not serve a real InRelease"
            ),
            DateError::Malformed(d) => write!(
                f,
                "cannot parse InRelease date '{d}'; expected 'Thu, 12 Nov 2026  4:23:38 UTC'"
            ),
        }
    }
}

fn days_from_civil(y: i64, m: i64, d: i64) -> i64 {
    // Howard Hinnant's algorithm: days since 1970-01-01
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let mp = (m + 9) % 12;
    let doy = (153 * mp + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146097 + doe - 719468
}

/// Parse an RFC 1123-style date as used by apt: `Thu, 12 Nov 2026  4:23:38 UTC` -> Unix seconds.
pub fn parse_date(value: &str) -> Result<i64, DateError> {
    let bad = || DateError::Malformed(value.to_string());
    let rest = value.split_once(',').map(|(_, r)| r).unwrap_or(value);
    let parts: Vec<&str> = rest.split_whitespace().collect();
    let [day, month, year, time, zone] = parts.as_slice() else {
        return Err(bad());
    };
    if !matches!(*zone, "UTC" | "GMT" | "+0000") {
        return Err(bad());
    }
    let month = [
        "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
    ]
    .iter()
    .position(|m| m.eq_ignore_ascii_case(month))
    .ok_or_else(bad)? as i64
        + 1;
    let (day, year): (i64, i64) = (
        day.parse().map_err(|_| bad())?,
        year.parse().map_err(|_| bad())?,
    );
    let t: Vec<i64> = time
        .split(':')
        .map(|x| x.parse().map_err(|_| bad()))
        .collect::<Result<_, _>>()?;
    let [h, mi, s] = t.as_slice() else {
        return Err(bad());
    };
    if !(1..=31).contains(&day)
        || !(0..24).contains(h)
        || !(0..60).contains(mi)
        || !(0..61).contains(s)
        || year < 1970
    {
        return Err(bad());
    }
    Ok(days_from_civil(year, month, day) * 86400 + h * 3600 + mi * 60 + s)
}

/// Extract the `Date:` header of an InRelease (clearsigned text; headers come first).
pub fn release_date(text: &str) -> Result<i64, DateError> {
    for line in text.lines() {
        if let Some(v) = line.strip_prefix("Date:") {
            return parse_date(v.trim());
        }
        if line.starts_with("-----BEGIN PGP SIGNATURE") {
            break;
        }
    }
    Err(DateError::NoDateHeader)
}

/// Seconds the mirror is behind the reference, clamped to 0 (the reference is fetched first,
/// so a mirror can be newer).
pub fn lag(reference: i64, mirror: i64) -> i64 {
    (reference - mirror).max(0)
}

pub fn is_stale(lag_secs: i64) -> bool {
    lag_secs > MAX_LAG_SECS
}

/// What the ranker needs to know about a probed mirror (SPEC step 9).
#[derive(Debug, Clone, PartialEq)]
pub struct Probed {
    pub url: String,
    pub lag_secs: i64,
    pub throughput_bps: u64,
    pub latency_ms: u64,
    pub same_as: bool,
}

/// Rank: lag ascending (equal `Date` forms a group), then within a group throughput descending and
/// latency ascending, except that a same-AS mirror within 20% of the group's fastest wins.
pub fn rank(mut mirrors: Vec<Probed>) -> Vec<Probed> {
    mirrors.sort_by(|a, b| {
        a.lag_secs
            .cmp(&b.lag_secs)
            .then(b.throughput_bps.cmp(&a.throughput_bps))
            .then(a.latency_ms.cmp(&b.latency_ms))
    });
    let mut out = Vec::with_capacity(mirrors.len());
    while !mirrors.is_empty() {
        let lag = mirrors[0].lag_secs;
        let end = mirrors
            .iter()
            .position(|m| m.lag_secs != lag)
            .unwrap_or(mirrors.len());
        let mut group: Vec<Probed> = mirrors.drain(..end).collect();
        let top = group[0].throughput_bps; // sorted: fastest first
        // Stable partition: same-AS mirrors within the band move ahead of the rest of the band.
        let in_band = |m: &Probed| m.throughput_bps.saturating_mul(10) >= top.saturating_mul(8);
        let band_end = group
            .iter()
            .position(|m| !in_band(m))
            .unwrap_or(group.len());
        let (mut band, rest) = {
            let rest = group.split_off(band_end);
            (group, rest)
        };
        let (same, other): (Vec<_>, Vec<_>) = band.drain(..).partition(|m| m.same_as);
        out.extend(same);
        out.extend(other);
        out.extend(rest);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_apt_date_with_double_space() {
        // 2026-11-12 04:23:38 UTC
        assert_eq!(
            parse_date("Thu, 12 Nov 2026  4:23:38 UTC"),
            Ok(1_794_457_418)
        );
    }

    #[test]
    fn epoch_and_leap_day() {
        assert_eq!(parse_date("Thu, 01 Jan 1970 00:00:00 UTC"), Ok(0));
        assert_eq!(
            parse_date("Thu, 29 Feb 2024 12:00:00 UTC"),
            Ok(1_709_208_000)
        );
    }

    #[test]
    fn rejects_garbage_dates() {
        for d in [
            "",
            "yesterday",
            "Thu, 12 Foo 2026 04:23:38 UTC",
            "Thu, 12 Nov 2026 25:00:00 UTC",
            "Thu, 12 Nov 2026 04:23:38 PST",
            "Thu, 32 Nov 2026 04:23:38 UTC",
        ] {
            assert!(matches!(parse_date(d), Err(DateError::Malformed(_))), "{d}");
        }
    }

    #[test]
    fn release_date_from_real_fixtures() {
        let t = include_str!("../tests/fixtures/inrelease-noble-updates.txt");
        assert_eq!(release_date(t), Ok(1_794_457_418));
        let j = include_str!("../tests/fixtures/inrelease-jammy-security.txt");
        assert_eq!(
            release_date(j),
            Ok(parse_date("Thu, 12 Nov 2026  2:19:32 UTC").unwrap())
        );
    }

    #[test]
    fn missing_date_header() {
        assert_eq!(
            release_date("-----BEGIN PGP SIGNED MESSAGE-----\n\nOrigin: Ubuntu\n"),
            Err(DateError::NoDateHeader)
        );
    }

    #[test]
    fn lag_is_clamped_and_staleness_uses_24h() {
        assert_eq!(lag(1000, 400), 600);
        assert_eq!(lag(1000, 1200), 0);
        assert!(!is_stale(MAX_LAG_SECS));
        assert!(is_stale(MAX_LAG_SECS + 1));
        // the measured case: a mirror one signing generation (about 6 h) behind is not stale
        assert!(!is_stale(6 * 3600));
    }

    fn p(url: &str, lag: i64, mbps: u64, ms: u64, same_as: bool) -> Probed {
        Probed {
            url: url.into(),
            lag_secs: lag,
            throughput_bps: mbps * 1_000_000,
            latency_ms: ms,
            same_as,
        }
    }

    fn urls(v: &[Probed]) -> Vec<&str> {
        v.iter().map(|m| m.url.as_str()).collect()
    }

    #[test]
    fn exact_date_beats_faster_lagging_mirror() {
        let r = rank(vec![
            p("fast-behind", 21_600, 900, 5, false),
            p("slow-exact", 0, 50, 60, false),
        ]);
        assert_eq!(urls(&r), ["slow-exact", "fast-behind"]);
    }

    #[test]
    fn within_group_fastest_first_then_latency() {
        let r = rank(vec![
            p("b", 0, 100, 30, false),
            p("a", 0, 100, 10, false),
            p("c", 0, 400, 50, false),
        ]);
        assert_eq!(urls(&r), ["c", "a", "b"]);
    }

    #[test]
    fn same_as_wins_inside_speed_band_only() {
        // 85% of the top speed: inside the band, so the same-AS mirror wins
        let r = rank(vec![
            p("top", 0, 100, 10, false),
            p("as-near", 0, 85, 10, true),
        ]);
        assert_eq!(urls(&r), ["as-near", "top"]);
        // 50% of the top speed: outside the band, no preference
        let r = rank(vec![
            p("top", 0, 100, 10, false),
            p("as-slow", 0, 50, 10, true),
        ]);
        assert_eq!(urls(&r), ["top", "as-slow"]);
    }

    #[test]
    fn same_as_does_not_cross_lag_groups() {
        let r = rank(vec![
            p("exact", 0, 10, 10, false),
            p("as-behind", 21_600, 10, 10, true),
        ]);
        assert_eq!(urls(&r), ["exact", "as-behind"]);
    }

    #[test]
    fn empty_input() {
        assert!(rank(vec![]).is_empty());
    }
}
