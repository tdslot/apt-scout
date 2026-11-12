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

    let net = Net::new();
    let page = net
        .get(LAUNCHPAD, http::MAX_PAGE, None)
        .map_err(|e| (1, format!("cannot load the mirror list: {e}")))?;
    let html = String::from_utf8_lossy(&page.body);
    let all = mirrors::parse_launchpad(&html).map_err(|e| {
        (
            1,
            format!("cannot read the mirror list from {LAUNCHPAD}: {e}"),
        )
    })?;

    let country_name = countries::launchpad_name(&country).ok_or_else(|| {
        (
            2,
            format!("unknown country code '{country}'; Launchpad lists mirrors in 82 countries, for example LT, NO, DE, GB, US"),
        )
    })?;
    let mut cands: Vec<_> = all
        .into_iter()
        .filter(|m| {
            m.country == country_name
                && mirrors::validate_url(&m.url).is_ok()
                && (!args.https_only || m.https)
        })
        .collect();
    if cands.is_empty() {
        return Err((
            1,
            format!(
                "Launchpad lists no usable mirrors for {country_name}. Try another country with --country, or drop --https-only."
            ),
        ));
    }
    cands.truncate(MAX_CANDIDATES);

    let arch = architecture().map_err(|e| (4, e))?;
    let reference = match probe::fetch_reference(&net, REFERENCE, &codename) {
        Ok(r) => r,
        Err(probe::ReferenceError::NotFound(url)) => {
            return Err((
                4,
                format!(
                    "{url} answered 404: {codename} is no longer on archive.ubuntu.com (older releases move to old-releases.ubuntu.com, which has no mirrors to choose from). Nothing to do for this release."
                ),
            ));
        }
        Err(e) => {
            return Err((
                1,
                format!("cannot read the reference release from {REFERENCE}: {e}"),
            ));
        }
    };

    let mut probed: Vec<(mirrors::Mirror, probe::Outcome)> = Vec::new();
    let mut rejected: Vec<(String, String)> = Vec::new();
    let handles: Vec<_> = cands
        .iter()
        .cloned()
        .map(|m| {
            let (codename, arch) = (codename.clone(), arch.clone());
            let reference = probe::Reference {
                updates: reference.updates,
                security: reference.security,
            };
            thread::spawn(move || {
                let net = Net::new();
                let r = probe::probe(&net, &m.url, &codename, &arch, &reference);
                (m, r)
            })
        })
        .collect();
    for (m, handle) in cands.iter().zip(handles) {
        match handle.join() {
            Ok((m, Ok(o))) if fresh::is_stale(o.updates_lag_secs) => rejected.push((
                m.url,
                format!(
                    "stale: {} h behind the reference",
                    o.updates_lag_secs / 3600
                ),
            )),
            Ok((m, Ok(o))) => probed.push((m, o)),
            Ok((m, Err(e))) => rejected.push((m.url, e.to_string())),
            Err(_) => rejected.push((m.url.clone(), "the probe crashed unexpectedly".to_string())),
        }
    }
    rejected.sort();

    let ranked = fresh::rank(
        probed
            .iter()
            .map(|(_, o)| fresh::Probed {
                url: o.url.clone(),
                lag_secs: o.updates_lag_secs,
                throughput_bps: o.throughput_bps,
                latency_ms: o.latency_ms,
                same_as: false,
            })
            .collect(),
    );
    println!("Ubuntu {codename}, country {country_name}, reference {REFERENCE}\n");
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
fn architecture() -> Result<String, String> {
    let out = std::process::Command::new("dpkg")
        .arg("--print-architecture")
        .output()
        .map_err(|e| format!("cannot run 'dpkg --print-architecture' to find the CPU architecture: {e}. Is dpkg installed?"))?;
    let arch = String::from_utf8_lossy(&out.stdout).trim().to_string();
    match arch.as_str() {
        "amd64" | "i386" => Ok(arch),
        other => Err(format!(
            "architecture '{other}' uses ports.ubuntu.com, which has no mirrors to choose from; apt-scout supports amd64 and i386 only."
        )),
    }
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
