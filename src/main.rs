mod apply;
mod countries;
mod cymru;
mod digest;
mod distro;
mod fresh;
mod geo;
#[cfg(apt_scout_write_harness)]
mod harness;
mod http;
mod manifest;
mod mirrors;
mod plan;
mod probe;
pub mod sources;
mod verify;

use std::io::{self, BufRead, Write};
use std::os::unix::fs::MetadataExt;
use std::path::Path;
use std::process::ExitCode;
use std::thread;
use std::time::{SystemTime, UNIX_EPOCH};

use http::{Fetch, Net};

const LAUNCHPAD: &str = "https://launchpad.net/ubuntu/+archivemirrors";
const REFERENCE: &str = "http://archive.ubuntu.com/ubuntu";
const TEXT_LIST: &str = "http://mirrors.ubuntu.com";
/// Candidates probed per run (SPEC step 4).
const MAX_CANDIDATES: usize = 30;
/// The `--top` default (SPEC.md CLI: `--top <N>` shows N best mirrors, default 5).
const DEFAULT_TOP: usize = 5;

struct Args {
    country: Option<String>,
    top: usize,
    https_only: bool,
    no_geo: bool,
    list: bool,
    yes: bool,
}

fn parse_args() -> Result<Args, String> {
    parse_args_from(std::env::args().skip(1))
}

fn parse_args_from(args: impl IntoIterator<Item = String>) -> Result<Args, String> {
    let mut a = Args {
        country: None,
        top: DEFAULT_TOP,
        https_only: false,
        no_geo: false,
        list: false,
        yes: false,
    };
    let mut it = args.into_iter();
    while let Some(arg) = it.next() {
        match arg.as_str() {
            "--list" => a.list = true,
            "--yes" => a.yes = true,
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
                    "apt-scout [--country <CC>] [--no-geo] [--top N] [--https-only] [--list] [--yes]\nShows the freshest, fastest Ubuntu mirrors for your country (detected, or given with --country). --no-geo skips every geolocation lookup and needs --country.\nAs root it writes the best mirror to the apt sources after one confirmation; --yes skips that confirmation. As any other user it writes nothing, prints the one line to run under sudo, and exits 3. --list shows the ranked table and the change a write would make, and never writes whatever the user."
                );
                std::process::exit(0);
            }
            other => return Err(format!("unknown option '{other}'; try --help")),
        }
    }
    Ok(a)
}

/// What this run does about writing, from its flags and its uid (SPEC.md CLI: the uid decides
/// whether the run writes). Every flag is an override, never a step in the workflow.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Mode {
    /// `--list`: the table and the change a write would make, nothing written, exit 0.
    List,
    /// A non-root run that would write: the `sudo` line, exit 3.
    Recommend,
    /// A root run that writes; `confirm` is false when `--yes` was given.
    Write { confirm: bool },
}

fn decide(list: bool, root: bool, yes: bool) -> Mode {
    if list {
        Mode::List
    } else if !root {
        Mode::Recommend
    } else {
        Mode::Write { confirm: !yes }
    }
}

/// The line a non-root run prints: the same command under `sudo`, rebuilt from the parsed flags so
/// no argument of the run reaches it as an opaque string.
fn sudo_command(args: &Args) -> String {
    let mut line = String::from("sudo apt-scout");
    if let Some(country) = &args.country {
        line.push_str(&format!(" --country {country}"));
    }
    if args.no_geo {
        line.push_str(" --no-geo");
    }
    if args.top != DEFAULT_TOP {
        line.push_str(&format!(" --top {}", args.top));
    }
    if args.https_only {
        line.push_str(" --https-only");
    }
    if args.yes {
        line.push_str(" --yes");
    }
    line
}

/// What `build_plan` found (SPEC.md Apply): either the writes a run must make, or that every
/// eligible URI already points at the selected mirror — which is success, not an error, so that
/// re-running a run whose write already happened does not fail.
#[derive(Debug)]
struct PlannedChange {
    /// `None` when there is nothing to change: `Plan::validate` refuses an empty plan, and an empty
    /// plan is the one case that is not an error.
    plan: Option<plan::Plan>,
    /// Every file, stanza and URI the rewrite refused, with its reason, for the caller to report
    /// whatever the mode (SPEC.md Apply: report each skipped entry and why).
    skipped: Vec<String>,
}

/// True when `file` already carries the selected mirror as an eligible URI: `rewrite_source` then
/// replaces that URI with itself, so the file yields no write and the run has nothing to do.
fn already_at_mirror(file: &sources::FileReport, selected_uri: &str) -> bool {
    file.entries.iter().any(|entry| {
        entry
            .uris
            .iter()
            .zip(&entry.rewritable)
            .any(|(uri, rewritable)| *rewritable && uri == selected_uri)
    })
}

/// Every skipped entry, one line each, with the reason the rewrite refused it.
fn report_skipped(skipped: &[String]) {
    for reason in skipped {
        println!("skipped {reason}");
    }
}

/// Build the plan a write needs, from the source inventory and the selected mirror.
///
/// This is where the CLI reads the apt files it may later replace, and it writes nothing: each
/// inspected file is rewritten purely, the bytes on disk are captured as the `FileState` the
/// writer re-checks before its first byte, and the whole set goes through `Plan::for_apply`, which
/// refuses a mirror without a verified `-updates` proof. A file that would not change produces no
/// write: when every eligible URI already points at the selected mirror the plan is empty and that is
/// reported as success, and when nothing eligible exists at all the plan is refused.
fn build_plan(
    root: &Path,
    inventory: &sources::Inventory,
    archive_hosts: &[String],
    selected: &fresh::Probed,
    platform: plan::Platform,
) -> Result<PlannedChange, (u8, String)> {
    let nothing_written = "Nothing was written.";
    let mut writes = Vec::new();
    let mut skipped: Vec<String> = Vec::new();
    for file in &inventory.files {
        // A file the inventory could not read is never inspected silently: SPEC.md Apply requires
        // every skipped file to be reported with its reason.
        if let Some(issue) = &file.issue {
            skipped.push(format!("{}: {issue}", file.path.display()));
            continue;
        }
        if !file.path.starts_with(root) {
            skipped.push(format!("{}: outside the apt root", file.path.display()));
            continue;
        }
        let bytes = std::fs::read(&file.path).map_err(|e| {
            (
                1,
                format!(
                    "cannot read {} while building the plan: {e}. {nothing_written}",
                    file.path.display()
                ),
            )
        })?;
        let text = match std::str::from_utf8(&bytes) {
            Ok(text) => text,
            Err(e) => {
                skipped.push(format!("{}: not UTF-8: {e}", file.path.display()));
                continue;
            }
        };
        let rewrite = sources::rewrite_source(
            text,
            &file.path,
            selected.url.as_str(),
            archive_hosts,
            platform == plan::Platform::PopOs,
        )
        .map_err(|e| {
            (
                1,
                format!("cannot plan the rewrite: {e}. {nothing_written}"),
            )
        })?;
        skipped.extend(rewrite.skipped);
        for planned in rewrite.files {
            debug_assert_eq!(planned.path.parent(), file.path.parent());
            let Ok(relative) = planned.path.strip_prefix(root) else {
                skipped.push(format!("{}: outside the apt root", planned.path.display()));
                continue;
            };
            let Ok(target) = plan::Target::parse(&relative.to_string_lossy()) else {
                skipped.push(format!(
                    "{}: not a writable apt target",
                    planned.path.display()
                ));
                continue;
            };
            let new_bytes = planned.content.into_bytes();
            let original = if planned.create_new {
                None
            } else {
                Some(file_state(&planned.path)?)
            };
            // The rewrite is computed from one read and the plan is bound by another: if the file
            // changed between them, the bytes the rewrite describes are no longer on disk.
            if let Some(original) = &original
                && original.bytes != bytes
            {
                return Err((
                    1,
                    format!(
                        "{} changed while the plan was built, so the planned bytes no longer match the disk. Nothing was written; re-run.",
                        planned.path.display()
                    ),
                ));
            }
            if original
                .as_ref()
                .is_some_and(|original| original.bytes == new_bytes)
            {
                continue;
            }
            writes.push(plan::PlannedWrite {
                target,
                digest: digest::sha256(&new_bytes),
                new_bytes,
                original,
                create_new: planned.create_new,
            });
        }
    }
    if writes.is_empty() {
        // Already at the selected mirror: nothing to change, and that is success. Every eligible URI
        // was replaced by itself, so a second run of the same command must not report a failure.
        if inventory
            .files
            .iter()
            .any(|file| already_at_mirror(file, selected.url.as_str()))
        {
            return Ok(PlannedChange {
                plan: None,
                skipped,
            });
        }
        let why = if skipped.is_empty() {
            "no eligible official archive URI was found".to_string()
        } else {
            skipped.join("; ")
        };
        return Err((
            1,
            format!("no apt source file would change: {why}. {nothing_written}"),
        ));
    }
    let plan = plan::Plan::for_apply(selected, platform, writes).map_err(|e| match e {
        plan::PlanError::NoProof => (
            1,
            format!(
                "cannot write {}: its -updates InRelease signature is {}, so the mirror is not proven fresh. {nothing_written}",
                selected.url, selected.signature
            ),
        ),
        other => (
            1,
            format!("cannot write {}: {other}. {nothing_written}", selected.url),
        ),
    })?;
    Ok(PlannedChange {
        plan: Some(plan),
        skipped,
    })
}

/// `st_mode` also carries the file-type bits, and the writer records and compares
/// `st_mode & 0o7777`. A plan that recorded the raw value would disagree with the writer about
/// every file, so the mask is applied here to the same value the writer recomputes.
const MODE_MASK: u32 = 0o7777;

/// The bytes and metadata of a file the plan replaces, read the way the writer re-reads them. A
/// symlink is refused here too: a plan names a target, never a path a link could redirect.
fn file_state(path: &Path) -> Result<plan::FileState, (u8, String)> {
    let meta = std::fs::symlink_metadata(path).map_err(|e| {
        (
            1,
            format!(
                "cannot inspect {} while building the plan: {e}. Nothing was written.",
                path.display()
            ),
        )
    })?;
    if !meta.file_type().is_file() {
        return Err((
            1,
            format!(
                "{} is not a regular file (a symlink or a special file); apt-scout never writes through one. Nothing was written.",
                path.display()
            ),
        ));
    }
    let on_disk = std::fs::read(path).map_err(|e| {
        (
            1,
            format!(
                "cannot read {} while building the plan: {e}. Nothing was written.",
                path.display()
            ),
        )
    })?;
    Ok(plan::FileState {
        dev: meta.dev(),
        ino: meta.ino(),
        mode: meta.mode() & MODE_MASK,
        uid: meta.uid(),
        gid: meta.gid(),
        bytes: reread(path, on_disk),
    })
}

/// The bytes the writer will re-check, which is the second read of a planned file. Identical to the
/// disk in the shipped binary: `#[cfg(test)]` `reread` only lets a test seal in different bytes for
/// one path, which is the only way to reproduce "the file changed while the plan was built" without
/// racing the disk.
#[cfg(test)]
fn reread(path: &Path, on_disk: Vec<u8>) -> Vec<u8> {
    tests::reread_override(path).unwrap_or(on_disk)
}

#[cfg(not(test))]
fn reread(_path: &Path, on_disk: Vec<u8>) -> Vec<u8> {
    on_disk
}

/// The planned change, one line per changed line of a file, for the confirmation and `--list`.
fn plan_report(plan: &plan::Plan) -> Vec<String> {
    let mut out = vec![format!("  mirror: {}", plan.selected_uri())];
    for write in plan.writes() {
        let path = format!("{}/{}", apply::APT_DIR, write.target.relative().display());
        match &write.original {
            None => {
                out.push(format!("  create {path}:"));
                for line in String::from_utf8_lossy(&write.new_bytes).lines() {
                    out.push(format!("    + {line}"));
                }
            }
            Some(original) => {
                out.push(format!("  replace {path}:"));
                let old = String::from_utf8_lossy(&original.bytes);
                let new = String::from_utf8_lossy(&write.new_bytes);
                let old_lines: Vec<&str> = old.lines().collect();
                let new_lines: Vec<&str> = new.lines().collect();
                if old_lines.len() == new_lines.len() {
                    for (i, (before, after)) in old_lines.iter().zip(&new_lines).enumerate() {
                        if before != after {
                            out.push(format!("    line {}: - {before}", i + 1));
                            out.push(format!("    line {}: + {after}", i + 1));
                        }
                    }
                } else {
                    out.push(format!(
                        "    {} bytes replaced with {} bytes",
                        original.bytes.len(),
                        write.new_bytes.len()
                    ));
                }
            }
        }
    }
    out
}

/// The one confirmation before a write. The prompt is a safety rail, not a security control: the
/// guarantee is the plan's validation, and no answer here can bypass it.
fn confirm_write(input: &mut impl BufRead, prompt: &str) -> Result<(), (u8, String)> {
    print!("{prompt}");
    io::stdout().flush().map_err(|e| {
        (
            1,
            format!(
                "cannot write the confirmation prompt to standard output: {e}. Nothing was written."
            ),
        )
    })?;
    let mut answer = String::new();
    input.read_line(&mut answer).map_err(|e| {
        (
            1,
            format!("cannot read the confirmation from standard input: {e}. Nothing was written."),
        )
    })?;
    match answer.trim().to_ascii_lowercase().as_str() {
        "y" | "yes" => Ok(()),
        "" => Err((
            1,
            "no answer on standard input, so nothing was written. Re-run with --yes to skip the confirmation."
                .to_string(),
        )),
        _ => Err((
            1,
            "cancelled at the confirmation, so nothing was written.".to_string(),
        )),
    }
}

/// The wall clock, in seconds, that names this run's backup directory.
fn now_secs() -> Result<u64, (u8, String)> {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|since| since.as_secs())
        .map_err(|e| {
            (
                1,
                format!("cannot read the system clock: {e}. Nothing was written."),
            )
        })
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

/// No apt source file holds an official Ubuntu archive URI, so the run stops before probing. SPEC.md
/// Apply: `--list` exits 0, a non-root run prints the `sudo` line and exits 3, and a root run that was
/// going to write exits 1.
fn no_eligible_uri(mode: Mode, args: &Args) -> Result<u8, (u8, String)> {
    match mode {
        // `--list` is read-only whatever the uid and exits 0 (SPEC.md Apply).
        Mode::List => Ok(0),
        Mode::Recommend => {
            println!(
                "\nNothing was written: this run is not root and apt sources need root to change.\nRun:\n  {}",
                sudo_command(args)
            );
            Ok(3)
        }
        Mode::Write { .. } => Err((
            1,
            "no official Ubuntu apt source URI can be changed, so the write has nothing to do. Nothing was written. Run with --list to see every file that was inspected."
                .to_string(),
        )),
    }
}

/// Nothing survived the freshness and reachability checks (SPEC step 9): there is no mirror to
/// recommend, and a run that cannot recommend writes nothing.
fn no_fresh_mirror() -> (u8, String) {
    (
        1,
        "no mirror passed the freshness and reachability checks; see the rejected list above"
            .to_string(),
    )
}

fn run() -> Result<u8, (u8, String)> {
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
    // The uid decides whether this run may write (SPEC.md CLI). `require_root` is the writer's own
    // check, so the decision and the writer can never disagree about what root is.
    let mode = decide(args.list, apply::require_root().is_ok(), args.yes);
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
        return no_eligible_uri(mode, &args);
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
    // fails closed when nothing verified. The writer holds the selected mirror's proof against it.
    let trusted_baseline = write_baseline(
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
    let Some(best) = ranked.first() else {
        return Err(no_fresh_mirror());
    };
    let best_asn = mirror_asn.get(&best.url).map(String::as_str);
    println!(
        "\nrecommended: {} — {} -updates signature, lag {}m, AS {} ({}), {:.1} MB/s, {} ms",
        best.url,
        signature_label(&best.signature),
        best.lag_secs / 60,
        best_asn.unwrap_or("unknown"),
        as_status(server_asn.as_deref(), best_asn),
        best.throughput_bps as f64 / 1e6,
        best.latency_ms
    );
    let platform = if d.id == "pop" {
        plan::Platform::PopOs
    } else {
        plan::Platform::Ubuntu
    };
    match mode {
        // `--list` prints the change a write would make and never writes, whatever the uid.
        Mode::List => {
            match build_plan(
                Path::new(apply::APT_DIR),
                &apt_sources,
                &archive_hosts,
                best,
                platform,
            ) {
                // Every skipped entry is listed in `--list` too: the mode shows the whole change,
                // including what the write would leave as it is (SPEC.md Apply).
                Ok(change) => {
                    match &change.plan {
                        Some(plan) => {
                            println!("\nA write would make this change:");
                            for line in plan_report(plan) {
                                println!("{line}");
                            }
                        }
                        None => println!(
                            "\nA write would change nothing: the apt sources already point at {}.",
                            best.url
                        ),
                    }
                    report_skipped(&change.skipped);
                }
                // A refused plan names the entries it refused, so there is nothing left to list.
                Err((_code, why)) => println!("\nA write would be refused: {why}"),
            };
            Ok(0)
        }
        // A run that may not write prints the one line that repeats it under `sudo`, and exits 3.
        Mode::Recommend => {
            println!(
                "\nNothing was written: this run is not root and apt sources need root to change.\nRun:\n  {}",
                sudo_command(&args)
            );
            Ok(3)
        }
        Mode::Write { confirm } => {
            let change = build_plan(
                Path::new(apply::APT_DIR),
                &apt_sources,
                &archive_hosts,
                best,
                platform,
            )?;
            let Some(plan) = change.plan else {
                // Re-running a run whose write already happened is success, not a failure.
                println!(
                    "\nNothing to change: the apt sources already point at {}. Nothing was written.",
                    best.url
                );
                report_skipped(&change.skipped);
                return Ok(0);
            };
            // The backup id is fixed before the confirmation so the prompt can name the directory
            // the write will create; the writer is then handed the same instant.
            let now = now_secs()?;
            let backup_id = manifest::BackupId::from_secs(now).map_err(|e| {
                (
                    1,
                    format!(
                        "cannot name the backup directory for {now}: {e}. Nothing was written."
                    ),
                )
            })?;
            println!("\nThe write is planned and its bytes are bound to the files on disk:");
            for line in plan_report(&plan) {
                println!("{line}");
            }
            println!(
                "  backup: {}/{} (every replaced file is copied there, mode 0700, before the first write)",
                apply::BACKUP_ROOT,
                backup_id.as_str()
            );
            println!("  alternatives: none (extra mirrors as comments are a TUI option)");
            // The prompt lists what will stay as it is, too: the user confirms the whole change.
            report_skipped(&change.skipped);
            if confirm {
                confirm_write(&mut io::stdin().lock(), "\nApply this change? [y/N] ")?;
            }
            let outcome = apply::apply(&plan, trusted_baseline.as_ref(), now)
                .map_err(|e| (e.exit_code(), e.to_string()))?;
            println!(
                "\nbackup: {}/{}",
                apply::BACKUP_ROOT,
                outcome.backup_id.as_str()
            );
            let written: Vec<String> = outcome
                .written
                .iter()
                .map(|target| format!("{}/{}", apply::APT_DIR, target.relative().display()))
                .collect();
            for path in &written {
                println!("wrote {path}");
            }
            // The skipped entries are listed again after the write, where the user reads the result.
            report_skipped(&change.skipped);
            println!(
                "\nchanged {}: every eligible official archive URI in those files now points at {}.\n\
                 Next: run 'apt update' to refresh the package lists.",
                written.join(", "),
                plan.selected_uri()
            );
            Ok(0)
        }
    }
}

/// `dpkg --print-architecture`; only amd64 and i386 have mirror choice (SPEC.md Scope). The path is
/// fixed rather than resolved through `PATH`, because this run reaches the write and SPEC.md's
/// Privilege boundary forbids a write-path command resolved from the environment.
const DPKG: &str = "/usr/bin/dpkg";

fn architecture() -> Result<String, (u8, String)> {
    let out = std::process::Command::new(DPKG)
        .arg("--print-architecture")
        .output()
        .map_err(|e| (1, format!("cannot run '{DPKG} --print-architecture' to find the CPU architecture: {e}. Is dpkg installed?")))?;
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
        verify::SignatureStatus::Verified(_) => "verified",
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
    // The write harness exists only in a `--cfg apt_scout_write_harness` build, and it is
    // dispatched before argument parsing. A plain build therefore parses `__write-harness` as an
    // ordinary argument and fails it as an unknown option (W2), so the harness cannot be reached
    // by accident.
    #[cfg(apt_scout_write_harness)]
    if let Some(code) = harness::main(std::env::args().skip(1).collect()) {
        return code;
    }
    match run() {
        Ok(code) => ExitCode::from(code),
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

    /// The one hook into `build_plan`'s second read of a planned file: `(path, the bytes to hand back
    /// instead of the disk's)`. It is consumed by the next read of that path, so it cannot leak into
    /// another test, and no shipped build compiles it at all.
    static REREAD: std::sync::Mutex<Option<(std::path::PathBuf, Vec<u8>)>> =
        std::sync::Mutex::new(None);

    pub(super) fn reread_override(path: &Path) -> Option<Vec<u8>> {
        let mut armed = REREAD.lock().ok()?;
        match armed.as_ref() {
            Some((armed_path, bytes)) if armed_path == path => {
                let bytes = bytes.clone();
                *armed = None;
                Some(bytes)
            }
            _ => None,
        }
    }

    /// Arm the seam for the next read of `path`.
    fn reread_with(path: std::path::PathBuf, bytes: Vec<u8>) {
        *REREAD.lock().expect("the seam is not poisoned") = Some((path, bytes));
    }

    /// The harness is compiled out unless `--cfg apt_scout_write_harness` is passed, so a
    /// `build.rs` that ever set the cfg unconditionally fails here instead of silently shipping
    /// the write cases in a release binary. Weak on its own: the built binaries are what W1/W2
    /// check.
    #[test]
    fn harness_cfg_is_off_in_a_default_build() {
        let compiled_in = cfg!(apt_scout_write_harness);
        assert!(
            !compiled_in,
            "a default build must not carry the write harness"
        );
    }

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
        verify::SignatureStatus::verified_for_test()
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
            updates_signature: verify::SignatureStatus::verified_for_test(),
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

    #[test]
    fn the_uid_and_the_flags_decide_whether_the_run_writes() {
        // `--list` never writes, whatever the uid, and exits 0 (SPEC.md CLI table).
        assert_eq!(decide(true, false, false), Mode::List);
        assert_eq!(decide(true, true, false), Mode::List);
        assert_eq!(decide(true, true, true), Mode::List);
        // A non-root run recommends and exits 3, with or without --yes.
        assert_eq!(decide(false, false, false), Mode::Recommend);
        assert_eq!(decide(false, false, true), Mode::Recommend);
        // A root run writes, and asks once unless --yes skipped the prompt.
        assert_eq!(decide(false, true, false), Mode::Write { confirm: true });
        assert_eq!(decide(false, true, true), Mode::Write { confirm: false });
    }

    #[test]
    fn yes_is_parsed_and_the_sudo_line_repeats_the_flags() {
        let args = parse_args_from(["--yes"].into_iter().map(str::to_string)).unwrap();
        assert!(args.yes);
        assert_eq!(sudo_command(&args), "sudo apt-scout --yes");

        let plain = parse_args_from(std::iter::empty()).unwrap();
        assert_eq!(sudo_command(&plain), "sudo apt-scout");

        let flags = parse_args_from(
            ["--country", "lt", "--no-geo", "--top", "3", "--https-only"]
                .into_iter()
                .map(str::to_string),
        )
        .unwrap();
        assert_eq!(
            sudo_command(&flags),
            "sudo apt-scout --country LT --no-geo --top 3 --https-only"
        );
    }

    #[test]
    fn a_default_answer_or_a_no_at_the_prompt_writes_nothing() {
        let ask = |answer: &str| {
            confirm_write(&mut std::io::Cursor::new(answer.to_string()), "").map_err(|e| e.1)
        };
        assert!(ask("y\n").is_ok());
        assert!(ask("YES\n").is_ok());
        let no = ask("n\n").expect_err("a no must refuse the write");
        assert!(no.contains("nothing was written"), "{no}");
        // A closed stdin is not a yes: the answer is required, or --yes.
        let eof = ask("").expect_err("no answer must refuse the write");
        assert!(eof.contains("--yes"), "{eof}");
    }

    /// A fixture apt root under the temp directory, removed however the test ends. Every plan test
    /// below reads and writes inside it and nowhere else.
    struct TempApt(std::path::PathBuf);

    impl TempApt {
        fn new(tag: &str) -> TempApt {
            let path =
                std::env::temp_dir().join(format!("apt-scout-d11-{tag}-{}", std::process::id()));
            let _ = std::fs::remove_dir_all(&path);
            std::fs::create_dir_all(path.join("sources.list.d")).expect("temp apt root");
            TempApt(path)
        }

        fn write(&self, name: &str, content: &str) {
            std::fs::write(self.0.join(name), content).expect("fixture file");
        }

        fn read(&self, name: &str) -> String {
            std::fs::read_to_string(self.0.join(name)).expect("fixture file")
        }
    }

    impl Drop for TempApt {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    /// A probed mirror shaped the way `probe::probe` builds one: the signed `Date` and the status
    /// meet in the same value, so only a `Verified` status can yield a proof.
    fn probed(url: &str, signature: verify::SignatureStatus) -> fresh::Probed {
        fresh::Probed {
            url: url.to_string(),
            updates_date: 1000,
            signed_updates_date: Some(
                fresh::release_date(
                    "-----BEGIN PGP SIGNED MESSAGE-----\nHash: SHA512\n\nDate: Thu, 01 Jan 1970 00:16:40 UTC\n",
                )
                .expect("the armour and the date parse"),
            ),
            lag_secs: 0,
            throughput_bps: 1_000_000,
            latency_ms: 10,
            same_as: false,
            signature,
        }
    }

    const MIRROR: &str = "https://mirror.example/ubuntu/";

    #[test]
    fn a_rerun_at_the_selected_mirror_is_success_not_a_failure() {
        // The file already points at the mirror this run selected: the rewrite replaces the URI with
        // itself, so the plan is empty. An empty plan here is the re-run, not "nothing eligible".
        let apt = TempApt::new("already");
        let current = format!("deb {MIRROR} noble main\n");
        apt.write("sources.list", &current);
        let hosts = vec!["mirror.example".to_string()];
        let inventory = sources::inventory(&apt.0, &hosts, false);
        assert!(inventory.has_rewritable_uri());
        let change = build_plan(
            &apt.0,
            &inventory,
            &hosts,
            &probed(MIRROR, verify::SignatureStatus::verified_for_test()),
            plan::Platform::Ubuntu,
        )
        .expect("a run whose write already happened must not fail");
        assert!(change.plan.is_none(), "nothing to change, so no plan");
        assert!(change.skipped.is_empty(), "{:?}", change.skipped);
        // Success is still a run that writes nothing.
        assert_eq!(apt.read("sources.list"), current);
    }

    #[test]
    fn the_plan_reports_every_skipped_entry_with_its_reason() {
        let apt = TempApt::new("skipped");
        apt.write(
            "sources.list",
            "deb http://archive.ubuntu.com/ubuntu noble main\ndeb https://security.ubuntu.com/ubuntu noble-security main\n",
        );
        // Not valid UTF-8: the inventory reports the file instead of dropping it, and the plan has to
        // pass that report on (SPEC.md Apply: every skipped file with its reason).
        std::fs::write(apt.0.join("sources.list.d/bad.list"), [0xff, 0xfe]).expect("fixture file");
        let hosts = vec![
            "archive.ubuntu.com".to_string(),
            "mirror.example".to_string(),
        ];
        let inventory = sources::inventory(&apt.0, &hosts, false);
        let change = build_plan(
            &apt.0,
            &inventory,
            &hosts,
            &probed(MIRROR, verify::SignatureStatus::verified_for_test()),
            plan::Platform::Ubuntu,
        )
        .expect("one URI is rewritable");
        assert!(change.plan.is_some(), "the archive URI is rewritten");
        let skipped = change.skipped.join("\n");
        assert!(
            skipped.contains("bad.list: file is not valid UTF-8"),
            "{skipped}"
        );
        assert!(
            skipped.contains("https://security.ubuntu.com/ubuntu was not changed"),
            "{skipped}"
        );
    }

    #[test]
    fn the_plan_refuses_a_file_that_changed_while_it_was_built() {
        let apt = TempApt::new("changed");
        let original = "deb http://archive.ubuntu.com/ubuntu noble main\n";
        apt.write("sources.list", original);
        let hosts = vec![
            "archive.ubuntu.com".to_string(),
            "mirror.example".to_string(),
        ];
        let inventory = sources::inventory(&apt.0, &hosts, false);
        // The rewrite is computed from one read; the plan is bound by a second one. Hand that second
        // read different bytes, which is what a file edited between the two reads looks like.
        reread_with(
            apt.0.join("sources.list"),
            b"deb http://archive.ubuntu.com/ubuntu noble main universe\n".to_vec(),
        );
        let (code, msg) = build_plan(
            &apt.0,
            &inventory,
            &hosts,
            &probed(MIRROR, verify::SignatureStatus::verified_for_test()),
            plan::Platform::Ubuntu,
        )
        .expect_err("a file that changed under the plan must be refused");
        assert_eq!(code, 1);
        assert!(
            msg.contains(
                "changed while the plan was built, so the planned bytes no longer match the disk"
            ),
            "{msg}"
        );
        assert!(msg.contains("Nothing was written"), "{msg}");
        assert!(msg.contains("sources.list"), "{msg}");
        // A refusal writes nothing.
        assert_eq!(apt.read("sources.list"), original);
    }

    #[test]
    fn the_run_level_exit_codes_are_pinned() {
        let args = parse_args_from(["--country", "LT"].into_iter().map(str::to_string)).unwrap();
        // No eligible URI, `--list`: read-only whatever the uid (SPEC.md CLI table).
        assert_eq!(no_eligible_uri(Mode::List, &args), Ok(0));
        // No eligible URI, non-root: the `sudo` line and exit 3.
        assert_eq!(no_eligible_uri(Mode::Recommend, &args), Ok(3));
        // No eligible URI, root that was going to write: exit 1, and it says nothing was written.
        let (code, msg) = no_eligible_uri(Mode::Write { confirm: true }, &args)
            .expect_err("a root run with nothing to write must fail");
        assert_eq!(code, 1);
        assert!(
            msg.contains("no official Ubuntu apt source URI can be changed"),
            "{msg}"
        );
        assert!(msg.contains("Nothing was written"), "{msg}");
        // Nothing survived the checks: exit 1 with the reason, never a panic on an empty ranking.
        let (code, msg) = no_fresh_mirror();
        assert_eq!(code, 1);
        assert!(
            msg.contains("no mirror passed the freshness and reachability checks"),
            "{msg}"
        );
    }

    #[test]
    fn the_plan_is_refused_when_the_selected_mirror_is_not_verified() {
        let apt = TempApt::new("no-proof");
        let original = "deb http://archive.ubuntu.com/ubuntu noble main\n";
        apt.write("sources.list", original);
        let hosts = vec!["archive.ubuntu.com".to_string()];
        let inventory = sources::inventory(&apt.0, &hosts, false);
        assert!(inventory.has_rewritable_uri());
        let selected = probed(
            MIRROR,
            verify::SignatureStatus::Invalid("the signature is from an unknown key".into()),
        );
        let (code, msg) = build_plan(
            &apt.0,
            &inventory,
            &hosts,
            &selected,
            plan::Platform::Ubuntu,
        )
        .expect_err("an unverified mirror must not be written");
        assert_eq!(code, 1);
        assert!(msg.contains("not proven fresh"), "{msg}");
        assert!(msg.contains("Nothing was written"), "{msg}");
        // The refusal is a refusal: the file still holds the bytes it was read from.
        assert_eq!(apt.read("sources.list"), original);
    }

    #[test]
    fn the_plan_is_refused_when_no_uri_is_eligible() {
        let apt = TempApt::new("no-uri");
        apt.write(
            "sources.list",
            "deb https://security.ubuntu.com/ubuntu noble-security main\ndeb https://ppa.example/ubuntu noble main\n",
        );
        let hosts = vec!["archive.ubuntu.com".to_string()];
        let inventory = sources::inventory(&apt.0, &hosts, false);
        assert!(!inventory.has_rewritable_uri());
        let (code, msg) = build_plan(
            &apt.0,
            &inventory,
            &hosts,
            &probed(MIRROR, verify::SignatureStatus::verified_for_test()),
            plan::Platform::Ubuntu,
        )
        .expect_err("an empty plan must be refused, not written");
        assert_eq!(code, 1);
        assert!(msg.contains("no apt source file would change"), "{msg}");
        assert!(msg.contains("Nothing was written"), "{msg}");
    }

    #[test]
    fn the_plan_rewrites_only_the_eligible_uris_and_binds_the_bytes_on_disk() {
        let apt = TempApt::new("plan");
        let legacy = "deb http://archive.ubuntu.com/ubuntu noble main universe\n# keep this comment\ndeb-src https://security.ubuntu.com/ubuntu noble-security main\n";
        apt.write("sources.list", legacy);
        let deb822 = "# noble\nTypes: deb\nURIs: http://archive.ubuntu.com/ubuntu\nSuites: noble noble-updates\nComponents: main\n";
        apt.write("sources.list.d/ubuntu.sources", deb822);
        let hosts = vec![
            "archive.ubuntu.com".to_string(),
            "mirror.example".to_string(),
        ];
        let inventory = sources::inventory(&apt.0, &hosts, false);
        let plan = build_plan(
            &apt.0,
            &inventory,
            &hosts,
            &probed(MIRROR, verify::SignatureStatus::verified_for_test()),
            plan::Platform::Ubuntu,
        )
        .expect("both inspected files have a rewritable URI")
        .plan
        .expect("both inspected files are written");
        assert_eq!(plan.selected_uri(), MIRROR);
        assert_eq!(plan.proof().seconds(), 1000);
        let targets: Vec<String> = plan
            .writes()
            .iter()
            .map(|w| w.target.relative().display().to_string())
            .collect();
        assert_eq!(targets, ["sources.list", "sources.list.d/ubuntu.sources"]);
        for write in plan.writes() {
            // The bytes the writer re-checks are the bytes this plan was computed from, read from
            // the real file rather than from the inventory's parse.
            let original = write.original.as_ref().expect("both files exist");
            assert!(!write.create_new);
            let new = String::from_utf8_lossy(&write.new_bytes).into_owned();
            assert!(new.contains(MIRROR), "{new}");
            assert!(!new.contains("archive.ubuntu.com"), "{new}");
            let on_disk = apt.read(&write.target.relative().display().to_string());
            assert_eq!(original.bytes, on_disk.as_bytes());
            // The writer compares the plan's state against `st_mode & 0o7777`; a mode that still
            // carried the file-type bits would make every file look changed and refuse the write.
            let meta = std::fs::symlink_metadata(apt.0.join(write.target.relative()))
                .expect("fixture metadata");
            assert_eq!(original.mode & !0o7777, 0);
            assert_eq!(original.mode, meta.mode() & 0o7777);
            assert_eq!(write.digest, crate::digest::sha256(&write.new_bytes));
        }
        // Building a plan writes nothing.
        let legacy_new = String::from_utf8_lossy(&plan.writes()[0].new_bytes).into_owned();
        assert!(
            legacy_new.contains("deb-src https://security.ubuntu.com/ubuntu noble-security main"),
            "{legacy_new}"
        );
        assert!(legacy_new.contains("# keep this comment"), "{legacy_new}");
        // The change the confirmation and --list show: one line per changed line, and nothing else.
        assert_eq!(
            plan_report(&plan),
            [
                "  mirror: https://mirror.example/ubuntu/",
                "  replace /etc/apt/sources.list:",
                "    line 1: - deb http://archive.ubuntu.com/ubuntu noble main universe",
                "    line 1: + deb https://mirror.example/ubuntu/ noble main universe",
                "  replace /etc/apt/sources.list.d/ubuntu.sources:",
                "    line 3: - URIs: http://archive.ubuntu.com/ubuntu",
                "    line 3: + URIs: https://mirror.example/ubuntu/",
            ]
        );
        assert_eq!(apt.read("sources.list"), legacy);
        assert_eq!(apt.read("sources.list.d/ubuntu.sources"), deb822);
    }

    #[test]
    fn the_plan_creates_the_pop_os_security_source_without_an_original() {
        let apt = TempApt::new("pop");
        let system = "Types: deb\nURIs: http://apt.pop-os.org/ubuntu\nSuites: jammy jammy-security jammy-updates\nComponents: main\n";
        apt.write("sources.list.d/system.sources", system);
        let hosts = vec!["apt.pop-os.org".to_string(), "mirror.example".to_string()];
        let inventory = sources::inventory(&apt.0, &hosts, true);
        let plan = build_plan(
            &apt.0,
            &inventory,
            &hosts,
            &probed(MIRROR, verify::SignatureStatus::verified_for_test()),
            plan::Platform::PopOs,
        )
        .expect("the Pop!_OS split is planned")
        .plan
        .expect("the Pop!_OS split is written");
        assert_eq!(plan.writes().len(), 2);
        let created = plan
            .writes()
            .iter()
            .find(|w| w.create_new)
            .expect("the security source is a planned creation");
        assert_eq!(
            created.target.relative().display().to_string(),
            "sources.list.d/system-security.sources"
        );
        assert!(created.original.is_none());
        // Planned, never incidental: the file does not exist until the writer creates it.
        assert!(
            !apt.0
                .join("sources.list.d/system-security.sources")
                .exists()
        );
    }
}
