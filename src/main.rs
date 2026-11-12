mod countries;
mod distro;
mod fresh;
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
}

fn parse_args() -> Result<Args, String> {
    let mut a = Args {
        country: None,
        top: 5,
        https_only: false,
    };
    let mut it = std::env::args().skip(1);
    while let Some(arg) = it.next() {
        match arg.as_str() {
            "--list" => {}
            "--https-only" => a.https_only = true,
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
                    "apt-scout --country <CC> [--top N] [--https-only] [--list]\nShows the freshest, fastest Ubuntu mirrors for a country. Read-only: nothing is written."
                );
                std::process::exit(0);
            }
            other => return Err(format!("unknown option '{other}'; try --help")),
        }
    }
    Ok(a)
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
    let country = args.country.ok_or((
        2,
        "no country given. Geolocation is not implemented yet: pass --country <CC>, for example --country LT".to_string(),
    ))?;

    let country_name = countries::launchpad_name(&country).ok_or_else(|| {
        (
            2,
            format!("unknown country code '{country}'; Launchpad lists mirrors in 82 countries, for example LT, NO, DE, GB, US"),
        )
    })?;
    let arch = architecture()?;

    let net = Net::new();
    let (mut cands, list_source) = load_candidates(&net, &country, country_name, args.https_only)?;
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
            Err(_) => rejected.push((m.url.clone(), "the probe crashed unexpectedly".to_string())),
        }
    }

    // Baseline: the canonical archive; when it was unreachable, the newest candidate date.
    let baseline = reference
        .as_ref()
        .map(|r| r.updates)
        .or_else(|| {
            results
                .iter()
                .filter_map(|(_, r)| r.as_ref().ok())
                .map(|o| o.updates_date)
                .max()
        })
        .unwrap_or(0);
    let mut probed: Vec<fresh::Probed> = Vec::new();
    let mut notes: std::collections::HashMap<String, String> = std::collections::HashMap::new();
    for (m, r) in results {
        match r {
            Ok(o) => {
                let lag = fresh::lag(baseline, o.updates_date);
                let mut note = String::new();
                if let (Some(r), Some(sec)) = (reference.as_ref(), o.security_date) {
                    let sl = fresh::lag(r.security, sec);
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
                    rejected.push((m.url, format!("stale: {} h behind", lag / 3600)));
                } else {
                    probed.push(fresh::Probed {
                        url: o.url,
                        lag_secs: lag,
                        throughput_bps: o.throughput_bps,
                        latency_ms: o.latency_ms,
                        same_as: false,
                    });
                }
            }
            Err(e) => rejected.push((m.url, e.to_string())),
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
        "Ubuntu {codename}, country {country_name}, mirror list: {list_source}, baseline {baseline_name}\n"
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
