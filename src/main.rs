mod countries;
mod cymru;
mod distro;
mod fresh;
mod geo;
mod http;
mod mirrors;
mod probe;

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
                    "apt-scout [--country <CC>] [--no-geo] [--top N] [--https-only] [--list]\nShows the freshest, fastest Ubuntu mirrors for your country (detected, or given with --country). --no-geo skips every geolocation lookup and needs --country. Read-only: nothing is written."
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
    validate_geo_args(&args).map_err(|e| (2, e))?;
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
    let arch = architecture()?;

    let (mut cands, list_source) = load_candidates(&net, &country, country_name, args.https_only)?;
    // Different list sources can name the same mirror twice (with and without a trailing slash).
    let mut seen = std::collections::HashSet::new();
    cands.retain(|m| seen.insert(m.url.trim_end_matches('/').to_ascii_lowercase()));
    cands.truncate(MAX_CANDIDATES);

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

    // Baseline: the canonical archive; when it was unreachable, the newest candidate date.
    let baseline = reference
        .as_ref()
        .map(|r| r.updates)
        .or_else(|| {
            fallback_baseline(
                results
                    .iter()
                    .filter_map(|(_, r)| r.as_ref().ok())
                    .map(|o| o.updates_date)
                    .collect(),
            )
        })
        .unwrap_or(0);
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
                        lag_secs: lag,
                        throughput_bps: o.throughput_bps,
                        latency_ms: o.latency_ms,
                        same_as: server_asn.is_some()
                            && mirror_asn.get(&m.url) == server_asn.as_ref(),
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
    println!(
        "{:>2}  {:<48} {:>7} {:>9} {:>8}",
        "#", "mirror", "lag", "speed", "latency"
    );
    for (i, r) in ranked.iter().take(args.top).enumerate() {
        println!(
            "{:>2}  {:<48} {:>6}m {:>6.1} MB/s {:>6} ms",
            i + 1,
            r.url,
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

fn mirror_host(url: &str) -> Option<String> {
    let authority = url.split_once("://")?.1.split(['/', '?', '#']).next()?;
    let authority = authority
        .rsplit_once('@')
        .map_or(authority, |(_, host)| host);
    if let Some(v6) = authority.strip_prefix('[') {
        return Some(v6.split_once(']')?.0.to_string());
    }
    Some(
        authority
            .rsplit_once(':')
            .map_or(authority, |(host, _)| host)
            .to_string(),
    )
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
                    let host = mirror_host(&url)?;
                    let ip = (host.as_str(), 80)
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

/// Baseline when the canonical archive is unreachable: the newest mirror date, but a date that is
/// more than `MAX_LAG_SECS` ahead of the median is treated as bogus and ignored, so one mirror with a
/// wrong clock cannot make every other mirror look stale.
fn fallback_baseline(mut dates: Vec<i64>) -> Option<i64> {
    if dates.is_empty() {
        return None;
    }
    dates.sort_unstable();
    let median = dates[dates.len() / 2];
    dates
        .into_iter()
        .rfind(|d| *d - median <= fresh::MAX_LAG_SECS)
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
    fn mirror_host_handles_ports_and_ipv6() {
        assert_eq!(
            mirror_host("https://example.org:8443/ubuntu"),
            Some("example.org".into())
        );
        assert_eq!(
            mirror_host("http://[2001:4860:4860::8888]:8080/ubuntu"),
            Some("2001:4860:4860::8888".into())
        );
        assert_eq!(
            mirror_host("https://example.org/ubuntu"),
            Some("example.org".into())
        );
        assert_eq!(mirror_host("not-a-url"), None);
    }

    #[test]
    fn fallback_baseline_ignores_a_bogus_future_date() {
        assert_eq!(fallback_baseline(vec![]), None);
        assert_eq!(fallback_baseline(vec![100, 100, 100]), Some(100));
        // one mirror claims a date two days ahead: ignored
        assert_eq!(
            fallback_baseline(vec![1000, 1000, 1000 + 2 * 86_400]),
            Some(1000)
        );
        // a normal 6 h spread keeps the newest
        assert_eq!(fallback_baseline(vec![0, 0, 6 * 3600]), Some(6 * 3600));
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
