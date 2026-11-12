mod countries;
mod cymru;
mod digest;
mod distro;
mod fresh;
mod geo;
mod http;
mod mirrors;
mod probe;
pub mod sources;
mod verify;

use std::process::ExitCode;
use std::thread;

use http::{Fetch, Net};

const LAUNCHPAD: &str = "https://launchpad.net/ubuntu/+archivemirrors";
const REFERENCE: &str = "http://archive.ubuntu.com/ubuntu";
const TEXT_LIST: &str = "http://mirrors.ubuntu.com";
/// Candidates probed per run (SPEC step 4).
const MAX_CANDIDATES: usize = 30;

struct Args {
    country: Option<String>,
    top: usize,
    https_only: bool,
    no_geo: bool,
}

fn parse_args() -> Result<Args, String> {
    parse_args_from(std::env::args().skip(1))
}

fn parse_args_from(args: impl IntoIterator<Item = String>) -> Result<Args, String> {
    let mut a = Args {
        country: None,
        top: 5,
        https_only: false,
        no_geo: false,
    };
    let mut it = args.into_iter();
    while let Some(arg) = it.next() {
        match arg.as_str() {
            "--list" => {}
            "--https-only" => a.https_only = true,
            "--no-geo" => a.no_geo = true,
            "--country" => {
                let cc = it
                    .next()
                    .ok_or("--country needs a two-letter country code, for example --country LT")?;
                let cc = cc.to_ascii_uppercase();
                if cc.len() != 2 || !cc.chars().all(|c| c.is_ascii_uppercase()) {
                    return Err(format!(
                        "--country '{cc}' is not a two-letter country code, for example LT"
                    ));
                }
                a.country = Some(cc);
            }
            "--top" => {
                let n = it
                    .next()
                    .ok_or("--top needs a number, for example --top 5")?;
                a.top = n
                    .parse()
                    .ok()
                    .filter(|n| *n > 0)
                    .ok_or(format!("--top '{n}' is not a positive number"))?;
            }
            "-h" | "--help" => {
                println!(
                    "apt-scout [--country <CC>] [--no-geo] [--top N] [--https-only] [--list]\nShows the freshest, fastest Ubuntu mirrors for your country (detected, or given with --country). --no-geo skips every geolocation lookup and needs --country. Shows a read-only apt source inventory; nothing is written."
                );
                std::process::exit(0);
            }
            other => return Err(format!("unknown option '{other}'; try --help")),
        }
    }
    Ok(a)
}

fn validate_geo_args(args: &Args) -> Result<(), String> {
    if args.no_geo && args.country.is_none() {
        return Err("--no-geo needs --country <CC>, because nothing else can tell which country you are in. Example: --no-geo --country LT".to_string());
    }
    if let Some(country) = args.country.as_deref()
        && countries::launchpad_name(country).is_none()
    {
        return Err(format!(
            "unknown country code '{country}'; Launchpad lists mirrors in 82 countries, for example LT, NO, DE, GB, US"
        ));
    }
    Ok(())
}

fn validate_preflight(args: &Args, arch: &str) -> Result<(), (u8, String)> {
    validate_geo_args(args).map_err(|e| (2, e))?;
    if !matches!(arch, "amd64" | "i386") {
        return Err((
            4,
            format!(
                "architecture '{arch}' uses ports.ubuntu.com, which has no mirrors to choose from; apt-scout supports amd64 and i386 only."
            ),
        ));
    }
    Ok(())
}

fn run() -> Result<(), (u8, String)> {
    let args = parse_args().map_err(|e| (2, e))?;
    let (d, source) = distro::detect().map_err(|e| (1, e))?;
    if !d.is_ubuntu_family() {
        return Err((
            4,
            format!(
                "unsupported distribution (ID='{}', ID_LIKE='{}' in {source}). apt-scout supports Ubuntu and derivatives: ID or ID_LIKE must contain 'ubuntu'.",
                d.id,
                d.id_like.join(" ")
            ),
        ));
    }
    let codename = d.codename.ok_or_else(|| {
        (1, format!("{source} contains neither UBUNTU_CODENAME nor VERSION_CODENAME, so the Ubuntu release cannot be determined. Check that the distribution's os-release file is intact; report this error with its ID and codename fields at https://github.com/tdslot/apt-scout/issues."))
    })?;
    let arch = architecture()?;
    validate_preflight(&args, &arch)?;
    let net = Net::new();
    // Geolocation is optional (SPEC.md, Resilience): failures are reported and the run continues.
    let mut geo = geo::Geo::default();
    if !args.no_geo {
        let (found, notes) = geo::detect(&net);
        for n in &notes {
            eprintln!("note: geolocation: {n}");
        }
        geo = found;
        if let (None, Some(ip)) = (&geo.asn, geo.ip.as_deref().and_then(|i| i.parse().ok())) {
            match cymru::lookup(ip) {
                Ok(asn) => geo.asn = Some(asn),
                Err(e) => eprintln!("note: server AS lookup via Team Cymru DNS failed: {e}"),
            }
        }
    }
    let country = args.country.clone().or(geo.country.clone()).ok_or((
        2,
        "cannot tell which country you are in (every geolocation source failed or was skipped). Pass it with --country <CC>, for example --country LT.".to_string(),
    ))?;
    let country_origin = if args.country.is_some() {
        "from --country"
    } else {
        "detected"
    };

    let country_name = countries::launchpad_name(&country).ok_or_else(|| {
        (
            2,
            format!("unknown country code '{country}'; Launchpad lists mirrors in 82 countries, for example LT, NO, DE, GB, US"),
        )
    })?;

    let (mut cands, list_source) = load_candidates(&net, &country, country_name, args.https_only)?;
    // Different list sources can name the same mirror twice (with and without a trailing slash).
    let mut seen = std::collections::HashSet::new();
    cands.retain(|m| seen.insert(m.url.trim_end_matches('/').to_ascii_lowercase()));
    cands.truncate(MAX_CANDIDATES);

    let mut archive_hosts = vec!["archive.ubuntu.com".to_string()];
    if d.id == "pop" {
        archive_hosts.push("apt.pop-os.org".to_string());
    }
    archive_hosts.extend(
        cands
            .iter()
            .filter_map(|m| mirror_host_port(&m.url).map(|(host, _)| host)),
    );
    archive_hosts.sort_unstable();
    archive_hosts.dedup_by(|a, b| a.eq_ignore_ascii_case(b));
    let apt_sources = sources::inventory(
        std::path::Path::new("/etc/apt"),
        &archive_hosts,
        d.id == "pop",
    );
    println!(
        "APT source inventory (read-only):\n{}",
        apt_sources.report()
    );
    if !apt_sources.has_rewritable_uri() {
        println!(
            "No official Ubuntu apt source URI can be changed; stopping before mirror probes."
        );
        return Ok(());
    }

    let reference = match probe::fetch_reference(&net, REFERENCE, &codename) {
        Ok(r) => Some(r),
        Err(probe::ReferenceError::NotFound(url)) => {
            return Err((
                4,
                format!(
                    "{url} answered 404: {codename} is no longer on archive.ubuntu.com (older releases move to old-releases.ubuntu.com, which has no mirrors to choose from). Nothing to do for this release."
                ),
            ));
        }
        Err(e) => {
            eprintln!(
                "warning: cannot read the reference release from {REFERENCE}: {e}. Using the newest mirror date as the baseline instead, so lag is relative to the best candidate."
            );
            None
        }
    };
    let handles: Vec<_> = cands
        .iter()
        .cloned()
        .map(|m| {
            let (codename, arch) = (codename.clone(), arch.clone());
            thread::spawn(move || {
                let net = Net::new();
                let r = probe::probe(&net, &m.url, &codename, &arch);
                (m, r)
            })
        })
        .collect();
    let mut results = Vec::new();
    let mut rejected: Vec<(String, String)> = Vec::new();
    for (m, handle) in cands.iter().zip(handles) {
        match handle.join() {
            Ok(done) => results.push(done),
            Err(_) => rejected.push((
                m.url.trim_end_matches('/').to_string(),
                "the probe crashed unexpectedly".to_string(),
            )),
        }
    }

    // Read-only ranking baseline (SPEC step 8): the canonical archive; when it was unreachable, the
    // newest candidate date, verified or not, because ranking must work without `gpgv`.
    let baseline = reference
        .as_ref()
        .map(|r| r.updates)
        .or_else(|| {
            display_baseline_fallback(
                results
                    .iter()
                    .filter_map(|(_, r)| r.as_ref().ok())
                    .map(|o| o.updates_date)
                    .collect(),
            )
        })
        .unwrap_or(0);
    // Write baseline (SPEC: Freshness trust boundary, ruling 5): verified values only, so a write
    // fails closed when nothing verified. D9 consumes it; D6f pins the composition with tests.
    let _write_baseline = write_baseline(
        reference.as_ref(),
        results
            .iter()
            .filter_map(|(_, r)| r.as_ref().ok())
            .filter_map(|o| o.verified_updates())
            .collect(),
    );
    // AS of each candidate (SPEC step 5): informational, never a reason to fail; skipped with --no-geo.
    let server_asn = geo.asn.clone();
    let mirror_asn: std::collections::HashMap<String, String> =
        if args.no_geo || server_asn.is_none() {
            Default::default()
        } else {
            resolve_mirror_asns(
                &results
                    .iter()
                    .map(|(m, _)| m.url.clone())
                    .collect::<Vec<_>>(),
            )
        };
    let mut probed: Vec<fresh::Probed> = Vec::new();
    let mut notes: std::collections::HashMap<String, String> = std::collections::HashMap::new();
    for (m, r) in results {
        match r {
            Ok(o) => {
                let lag = fresh::lag(baseline, o.updates_date);
                let mut note = String::new();
                if let (Some(rsec), Some(sec)) =
                    (reference.as_ref().and_then(|r| r.security), o.security_date)
                {
                    let sl = fresh::lag(rsec, sec);
                    if sl > 0 {
                        note.push_str(&format!(" -security {}m behind", sl / 60));
                    }
                }
                if let Some(host) = &o.redirected_to {
                    note.push_str(&format!(" redirects to {host}"));
                }
                if !o.updates_signature.is_verified() {
                    note.push_str(&format!(" signature {}", o.updates_signature));
                }
                if !note.is_empty() {
                    notes.insert(o.url.clone(), note);
                }
                if fresh::is_stale(lag) {
                    rejected.push((
                        m.url.trim_end_matches('/').to_string(),
                        format!("stale: {} h behind", lag / 3600),
                    ));
                } else {
                    probed.push(fresh::Probed {
                        url: o.url,
                        updates_date: o.updates_date,
                        signed_updates_date: o.signed_updates_date,
                        lag_secs: lag,
                        throughput_bps: o.throughput_bps,
                        latency_ms: o.latency_ms,
                        same_as: server_asn.is_some()
                            && mirror_asn.get(&m.url) == server_asn.as_ref(),
                        signature: o.updates_signature.clone(),
                    });
                }
            }
            Err(e) => rejected.push((m.url.trim_end_matches('/').to_string(), e.to_string())),
        }
    }
    rejected.sort();
    let ranked = fresh::rank(probed);
    let baseline_name = if reference.is_some() {
        REFERENCE
    } else {
        "newest mirror (reference unreachable)"
    };
    println!(
        "Ubuntu {codename}, country {country_name} ({country_origin}), server {}, mirror list: {list_source}, baseline {baseline_name}\n",
        geo.asn.as_deref().unwrap_or("AS unknown")
    );
    if let Some(reference) = reference.as_ref() {
        println!(
            "Reference InRelease signature: {}",
            reference.updates_signature
        );
    }
    println!(
        "Signature status is shown for ranking only; apt writes will require verified InRelease data.\n"
    );
    println!(
        "{:>2}  {:<48} {:>12} {:>9} {:>10} {:>7} {:>9} {:>8}",
        "#", "mirror", "mirror AS", "AS status", "signature", "lag", "speed", "latency"
    );
    for (i, r) in ranked.iter().take(args.top).enumerate() {
        let mirror_asn = mirror_asn.get(&r.url).map(String::as_str);
        println!(
            "{:>2}  {:<48.48} {:>12} {:>9} {:>10} {:>6}m {:>6.1} MB/s {:>6} ms",
            i + 1,
            r.url,
            mirror_asn.unwrap_or("unknown"),
            as_status(server_asn.as_deref(), mirror_asn),
            signature_label(&r.signature),
            r.lag_secs / 60,
            r.throughput_bps as f64 / 1e6,
            r.latency_ms
        );
        if let Some(n) = notes.get(&r.url) {
            println!("    note:{n}");
        }
    }
    for (url, why) in &rejected {
        println!("rejected {url}: {why}");
    }
    if ranked.is_empty() {
        return Err((
            1,
            "no mirror passed the freshness and reachability checks; see the rejected list above"
                .to_string(),
        ));
    }
    Ok(())
}

/// `dpkg --print-architecture`; only amd64 and i386 have mirror choice (SPEC.md Scope).
fn architecture() -> Result<String, (u8, String)> {
    let out = std::process::Command::new("dpkg")
        .arg("--print-architecture")
        .output()
        .map_err(|e| (1, format!("cannot run 'dpkg --print-architecture' to find the CPU architecture: {e}. Is dpkg installed?")))?;
    let arch = String::from_utf8_lossy(&out.stdout).trim().to_string();
    match arch.as_str() {
        "amd64" | "i386" => Ok(arch),
        other => Err((
            4,
            format!(
                "architecture '{other}' uses ports.ubuntu.com, which has no mirrors to choose from; apt-scout supports amd64 and i386 only."
            ),
        )),
    }
}

fn signature_label(status: &verify::SignatureStatus) -> &'static str {
    match status {
        verify::SignatureStatus::Verified => "verified",
        verify::SignatureStatus::Invalid(_) => "invalid",
        verify::SignatureStatus::Unavailable(_) => "unavailable",
    }
}

fn as_status(server_asn: Option<&str>, mirror_asn: Option<&str>) -> &'static str {
    match (server_asn, mirror_asn) {
        (Some(server), Some(mirror)) if server == mirror => "same",
        (Some(_), Some(_)) => "different",
        _ => "unknown",
    }
}

fn mirror_host_port(url: &str) -> Option<(String, u16)> {
    let (scheme, rest) = url.split_once("://")?;
    let default_port = match scheme {
        "http" => 80,
        "https" => 443,
        _ => return None,
    };
    let authority = rest.split(['/', '?', '#']).next()?;
    let authority = authority
        .rsplit_once('@')
        .map_or(authority, |(_, host)| host);
    if let Some(v6) = authority.strip_prefix('[') {
        let (host, tail) = v6.split_once(']')?;
        let port = match tail {
            "" => default_port,
            tail => tail.strip_prefix(':')?.parse().ok()?,
        };
        return Some((host.to_string(), port));
    }
    let (host, port) = match authority.rsplit_once(':') {
        Some((host, port)) if port.bytes().all(|b| b.is_ascii_digit()) => {
            (host, port.parse().ok()?)
        }
        _ => (authority, default_port),
    };
    Some((host.to_string(), port))
}

/// AS of each mirror host via Team Cymru DNS, at most 8 lookups at a time and one total 20 s
/// budget for all resolution and queries. Standard-library hostname resolution is offloaded to a
/// worker thread because it cannot be interrupted; after the budget, results are discarded.
/// Mirrors without AS data are left out (AS is informational).
fn resolve_mirror_asns(urls: &[String]) -> std::collections::HashMap<String, String> {
    use std::net::ToSocketAddrs;
    use std::time::{Duration, Instant};
    const AS_BUDGET: Duration = Duration::from_secs(20);
    let deadline = Instant::now() + AS_BUDGET;
    let mut out = std::collections::HashMap::new();
    for chunk in urls.chunks(8) {
        if Instant::now() >= deadline {
            eprintln!(
                "note: AS lookup time budget ({} s) exhausted; remaining mirrors ranked without AS preference",
                AS_BUDGET.as_secs()
            );
            break;
        }
        let handles: Vec<_> = chunk
            .iter()
            .cloned()
            .map(|url| {
                thread::spawn(move || {
                    let (host, port) = mirror_host_port(&url)?;
                    let ip = (host.as_str(), port)
                        .to_socket_addrs()
                        .ok()?
                        .map(|addr| addr.ip())
                        .find(|ip| !mirrors::is_private(*ip))?;
                    let asn = cymru::lookup(ip).ok()?;
                    Some((url, asn))
                })
            })
            .collect();
        let (tx, rx) = std::sync::mpsc::channel();
        for h in handles {
            let tx = tx.clone();
            thread::spawn(move || {
                let _ = tx.send(h.join().ok().flatten());
            });
        }
        drop(tx);
        loop {
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                break;
            }
            match rx.recv_timeout(remaining) {
                Ok(Some((url, asn))) => {
                    out.insert(url, asn);
                }
                Ok(None) => {}
                Err(std::sync::mpsc::RecvTimeoutError::Timeout)
                | Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => break,
            }
        }
        if Instant::now() >= deadline {
            eprintln!(
                "note: AS lookup time budget ({} s) exhausted; remaining mirrors ranked without AS preference",
                AS_BUDGET.as_secs()
            );
            break;
        }
    }
    out
}

/// Read-only fallback baseline (SPEC step 8): the newest candidate date, but a date more than
/// `MAX_LAG_SECS` ahead of the median is treated as bogus and ignored, so one mirror with a wrong
/// clock cannot make every other mirror look stale. Verified or not: ranking must work without `gpgv`.
fn display_baseline_fallback(mut dates: Vec<i64>) -> Option<i64> {
    if dates.is_empty() {
        return None;
    }
    dates.sort_unstable();
    let median = dates[dates.len() / 2];
    dates
        .into_iter()
        .rfind(|d| *d - median <= fresh::MAX_LAG_SECS)
}

/// Write fallback baseline (SPEC: Freshness trust boundary, ruling 5): the same newest-sane-date rule,
/// restricted to candidate dates whose own `-updates` `InRelease` is `Verified`. `None` when nothing
/// verified, so a write fails closed instead of trusting an unverified date.
fn fallback_baseline(dates: Vec<verify::VerifiedDate>) -> Option<verify::VerifiedDate> {
    let newest = display_baseline_fallback(dates.iter().map(|d| d.seconds()).collect())?;
    dates.into_iter().find(|d| d.seconds() == newest)
}

/// The freshness baseline a write may trust: the canonical reference when it is `Verified`, otherwise
/// the newest verified candidate date. `None` means no write (SPEC: Freshness trust boundary).
fn write_baseline(
    reference: Option<&probe::Reference>,
    candidates: Vec<verify::VerifiedDate>,
) -> Option<verify::VerifiedDate> {
    reference
        .and_then(|r| r.verified_updates())
        .or_else(|| fallback_baseline(candidates))
}

/// SPEC step 3: Launchpad first, then `mirrors.ubuntu.com/<CC>.txt`. Returns the candidates and the
/// name of the source that supplied them; every failed source is reported with its reason.
fn load_candidates(
    net: &dyn Fetch,
    cc: &str,
    country_name: &str,
    https_only: bool,
) -> Result<(Vec<mirrors::Mirror>, &'static str), (u8, String)> {
    let keep =
        |m: &mirrors::Mirror| mirrors::validate_url(&m.url).is_ok() && (!https_only || m.https);
    let mut failures = Vec::new();

    match net.get(LAUNCHPAD, http::MAX_PAGE, None) {
        Ok(page) => match mirrors::parse_launchpad(&String::from_utf8_lossy(&page.body)) {
            Ok(all) => {
                let found: Vec<_> = all
                    .into_iter()
                    .filter(|m| m.country == country_name && keep(m))
                    .collect();
                if !found.is_empty() {
                    return Ok((found, "Launchpad"));
                }
                failures.push(format!(
                    "Launchpad lists no usable mirrors for {country_name}"
                ));
            }
            Err(e) => failures.push(format!("cannot read {LAUNCHPAD}: {e}")),
        },
        Err(e) => failures.push(format!("cannot load the Launchpad mirror list: {e}")),
    }

    let url = format!("{TEXT_LIST}/{cc}.txt");
    match net.get(&url, http::MAX_SMALL, None) {
        Ok(got) => {
            let found: Vec<_> = mirrors::parse_text_list(&String::from_utf8_lossy(&got.body))
                .into_iter()
                .map(|u| mirrors::Mirror {
                    country: country_name.to_string(),
                    name: String::new(),
                    https: u.starts_with("https://"),
                    url: u,
                    speed: None,
                    status: mirrors::Status::Unknown,
                })
                .filter(keep)
                .collect();
            if !found.is_empty() {
                eprintln!(
                    "warning: {}. Using {url} instead (its URLs come over plain HTTP, so they are unverified).",
                    failures.join("; ")
                );
                return Ok((found, "mirrors.ubuntu.com (unverified)"));
            }
            failures.push(format!("{url} has no usable mirrors"));
        }
        Err(e) => failures.push(format!("cannot load {url}: {e}")),
    }
    Err((
        1,
        format!(
            "no mirror list is available: {}. Check the network connection, or try another country with --country.",
            failures.join("; ")
        ),
    ))
}

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err((code, msg)) => {
            eprintln!("error: {msg}");
            ExitCode::from(code)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::http::Canned;

    #[test]
    fn preflight_rejects_unknown_country_and_architecture_before_network() {
        let unknown = parse_args_from(["--country", "ZZ"].into_iter().map(str::to_string)).unwrap();
        let (code, msg) = validate_preflight(&unknown, "amd64").unwrap_err();
        assert_eq!(code, 2);
        assert!(msg.contains("unknown country code 'ZZ'"));

        let unsupported =
            parse_args_from(["--country", "LT"].into_iter().map(str::to_string)).unwrap();
        let (code, msg) = validate_preflight(&unsupported, "arm64").unwrap_err();
        assert_eq!(code, 4);
        assert!(msg.contains("architecture 'arm64'"));
    }

    #[test]
    fn preflight_accepts_supported_country_and_architecture() {
        let args = parse_args_from(["--country", "LT"].into_iter().map(str::to_string)).unwrap();
        assert!(validate_preflight(&args, "amd64").is_ok());
        assert!(validate_preflight(&args, "i386").is_ok());
    }

    #[test]
    fn no_geo_requires_country() {
        let args = parse_args_from(["--no-geo"].into_iter().map(str::to_string)).unwrap();
        assert_eq!(
            validate_geo_args(&args).unwrap_err(),
            "--no-geo needs --country <CC>, because nothing else can tell which country you are in. Example: --no-geo --country LT"
        );
        let args = parse_args_from(
            ["--no-geo", "--country", "LT"]
                .into_iter()
                .map(str::to_string),
        )
        .unwrap();
        assert!(args.no_geo);
        assert_eq!(args.country.as_deref(), Some("LT"));
    }

    #[test]
    fn as_status_reports_same_different_and_unknown() {
        assert_eq!(as_status(Some("AS64500"), Some("AS64500")), "same");
        assert_eq!(as_status(Some("AS64500"), Some("AS64501")), "different");
        assert_eq!(as_status(Some("AS64500"), None), "unknown");
        assert_eq!(as_status(None, Some("AS64500")), "unknown");
        // Lookup unavailable: no ASN values, so never imply an AS preference.
        assert_eq!(as_status(None, None), "unknown");
    }

    #[test]
    fn mirror_host_uses_url_scheme_and_explicit_port() {
        assert_eq!(
            mirror_host_port("https://example.org:8443/ubuntu"),
            Some(("example.org".into(), 8443))
        );
        assert_eq!(
            mirror_host_port("http://[2001:4860:4860::8888]:8080/ubuntu"),
            Some(("2001:4860:4860::8888".into(), 8080))
        );
        assert_eq!(
            mirror_host_port("https://[2001:4860:4860::8888]/ubuntu"),
            Some(("2001:4860:4860::8888".into(), 443))
        );
        assert_eq!(
            mirror_host_port("https://example.org/ubuntu"),
            Some(("example.org".into(), 443))
        );
        assert_eq!(
            mirror_host_port("http://example.org/ubuntu"),
            Some(("example.org".into(), 80))
        );
        assert_eq!(mirror_host_port("not-a-url"), None);
    }

    /// A `VerifiedDate` built the only way one exists: a `Date` the strict parser accepted inside the
    /// armour, plus a `Verified` status.
    fn vd(rfc: &str) -> verify::VerifiedDate {
        let body = format!("-----BEGIN PGP SIGNED MESSAGE-----\nHash: SHA512\n\nDate: {rfc}\n");
        verify::SignatureStatus::Verified
            .verified_date(fresh::release_date(&body).unwrap())
            .unwrap()
    }

    #[test]
    fn display_baseline_fallback_ignores_a_bogus_future_date() {
        assert_eq!(display_baseline_fallback(vec![]), None);
        assert_eq!(display_baseline_fallback(vec![100, 100, 100]), Some(100));
        // one mirror claims a date two days ahead: ignored
        assert_eq!(
            display_baseline_fallback(vec![1000, 1000, 1000 + 2 * 86_400]),
            Some(1000)
        );
        // a normal 6 h spread keeps the newest
        assert_eq!(
            display_baseline_fallback(vec![0, 0, 6 * 3600]),
            Some(6 * 3600)
        );
    }

    #[test]
    fn fallback_baseline_takes_only_verified_dates() {
        assert_eq!(fallback_baseline(vec![]), None);
        let secs = |d: Option<verify::VerifiedDate>| d.map(|d| d.seconds());
        // same arithmetic as the read-only fallback, over trusted values only
        assert_eq!(
            secs(fallback_baseline(vec![
                vd("Thu, 01 Jan 1970 00:16:40 UTC");
                3
            ])),
            Some(1000)
        );
        assert_eq!(
            secs(fallback_baseline(vec![
                vd("Thu, 01 Jan 1970 00:16:40 UTC"),
                vd("Thu, 01 Jan 1970 00:16:40 UTC"),
                vd("Sat, 03 Jan 1970 00:16:40 UTC"),
            ])),
            Some(1000)
        );
        assert_eq!(
            secs(fallback_baseline(vec![
                vd("Thu, 01 Jan 1970 00:00:00 UTC"),
                vd("Thu, 01 Jan 1970 00:00:00 UTC"),
                vd("Thu, 01 Jan 1970 06:00:00 UTC"),
            ])),
            Some(21_600)
        );
        // an unverified date cannot enter at all: only `Verified` + a parsed signed date builds the type
        let signed = fresh::release_date(
            "-----BEGIN PGP SIGNED MESSAGE-----\nHash: SHA512\n\nDate: Thu, 01 Jan 1970 00:16:40 UTC\n",
        )
        .unwrap();
        assert!(
            verify::SignatureStatus::Invalid("bad".into())
                .verified_date(signed)
                .is_none()
        );
        assert!(
            verify::SignatureStatus::Unavailable("missing".into())
                .verified_date(signed)
                .is_none()
        );
    }

    #[test]
    fn write_baseline_prefers_a_verified_reference() {
        let reference_date = fresh::parse_date("Thu, 12 Nov 2026  8:29:02 UTC").unwrap();
        let verified = probe::Reference {
            updates: reference_date,
            signed_updates_date: Some(
                fresh::release_date(
                    "-----BEGIN PGP SIGNED MESSAGE-----\nHash: SHA512\n\nDate: Thu, 12 Nov 2026  8:29:02 UTC\n",
                )
                .unwrap(),
            ),
            updates_signature: verify::SignatureStatus::Verified,
            security: None,
        };
        let unverified = probe::Reference {
            updates: reference_date,
            signed_updates_date: None,
            updates_signature: verify::SignatureStatus::Invalid("unsigned".into()),
            security: None,
        };
        let candidate = vd("Thu, 12 Nov 2026  4:23:38 UTC");
        let candidate_secs = fresh::parse_date("Thu, 12 Nov 2026  4:23:38 UTC").ok();
        // the verified reference wins over any candidate
        assert_eq!(
            write_baseline(Some(&verified), vec![candidate]).map(|d| d.seconds()),
            Some(reference_date)
        );
        // an unverified reference falls back to the newest verified candidate
        assert_eq!(
            write_baseline(Some(&unverified), vec![candidate]).map(|d| d.seconds()),
            candidate_secs
        );
        // nothing verified: a write fails closed
        assert_eq!(write_baseline(Some(&unverified), vec![]), None);
        assert_eq!(write_baseline(None, vec![]), None);
    }

    const LT_TXT: &[u8] = b"https://mirror.vpsnet.com/ubuntu/\nhttp://ubuntu.mirror.vu.lt/ubuntu/\nftp://x.example/\n";
    const LAUNCHPAD_OK: &[u8] = include_bytes!("../tests/fixtures/launchpad-archivemirrors.html");

    #[test]
    fn launchpad_is_preferred() {
        let net = Canned(vec![
            (LAUNCHPAD, Ok(LAUNCHPAD_OK)),
            ("http://mirrors.ubuntu.com/LT.txt", Ok(LT_TXT)),
        ]);
        let (c, src) = load_candidates(&net, "LT", "Lithuania", false).unwrap();
        assert_eq!(src, "Launchpad");
        assert!(c.iter().all(|m| m.country == "Lithuania"));
        assert!(c.len() >= 4);
    }

    #[test]
    fn falls_back_to_text_list_when_launchpad_is_down() {
        let net = Canned(vec![
            (LAUNCHPAD, Err(503)),
            ("http://mirrors.ubuntu.com/LT.txt", Ok(LT_TXT)),
        ]);
        let (c, src) = load_candidates(&net, "LT", "Lithuania", false).unwrap();
        assert_eq!(src, "mirrors.ubuntu.com (unverified)");
        assert_eq!(c.len(), 2); // the ftp:// line is dropped
        assert!(c.iter().any(|m| m.https));
    }

    #[test]
    fn falls_back_when_launchpad_has_no_such_country() {
        let net = Canned(vec![
            (LAUNCHPAD, Ok(LAUNCHPAD_OK)),
            (
                "http://mirrors.ubuntu.com/IS.txt",
                Ok(b"http://ftp.is.example/ubuntu/\n"),
            ),
        ]);
        let (c, src) = load_candidates(&net, "IS", "Iceland", false).unwrap();
        assert_eq!(src, "mirrors.ubuntu.com (unverified)");
        assert_eq!(c.len(), 1);
    }

    #[test]
    fn https_only_filters_the_fallback() {
        let net = Canned(vec![
            (LAUNCHPAD, Err(503)),
            ("http://mirrors.ubuntu.com/LT.txt", Ok(LT_TXT)),
        ]);
        let (c, _) = load_candidates(&net, "LT", "Lithuania", true).unwrap();
        assert_eq!(c.len(), 1);
        assert!(c[0].https);
    }

    #[test]
    fn hostile_entries_in_the_text_list_are_dropped() {
        let txt: &[u8] = b"http://127.0.0.1/ubuntu/\nhttp://169.254.169.254/\nhttp://user:pw@a.example/\nhttp://ok.example/ubuntu/\n";
        let net = Canned(vec![
            (LAUNCHPAD, Err(503)),
            ("http://mirrors.ubuntu.com/LT.txt", Ok(txt)),
        ]);
        let (c, _) = load_candidates(&net, "LT", "Lithuania", false).unwrap();
        assert_eq!(c.len(), 1);
        assert_eq!(c[0].url, "http://ok.example/ubuntu/");
    }

    #[test]
    fn both_sources_failing_names_both_reasons() {
        let net = Canned(vec![
            (LAUNCHPAD, Err(503)),
            ("http://mirrors.ubuntu.com/LT.txt", Err(404)),
        ]);
        let (code, msg) = load_candidates(&net, "LT", "Lithuania", false).unwrap_err();
        assert_eq!(code, 1);
        assert!(msg.contains("Launchpad"), "{msg}");
        assert!(msg.contains("LT.txt"), "{msg}");
        assert!(msg.contains("--country"), "{msg}");
    }
}
