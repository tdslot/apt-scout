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
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

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
    /// `--verbose`: per-mirror detail and each rejection's full reason.
    verbose: bool,
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
        verbose: false,
    };
    let mut it = args.into_iter();
    while let Some(arg) = it.next() {
        match arg.as_str() {
            "--list" => a.list = true,
            "--yes" => a.yes = true,
            "--https-only" => a.https_only = true,
            "--no-geo" => a.no_geo = true,
            "--verbose" => a.verbose = true,
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
                println!("{HELP}");
                std::process::exit(0);
            }
            other => return Err(format!("unknown option '{other}'; try --help")),
        }
    }
    Ok(a)
}

/// `--help`, once: the flags, what a run does with them, the exit codes and one example each. The
/// rule that used to repeat under every table lives here instead.
const HELP: &str = "\
apt-scout — pick the fastest Ubuntu mirror and use it

USAGE
  apt-scout [OPTIONS]

DISCOVERY
  --country <CC>   two-letter country; overrides detection
  --no-geo         no geolocation; requires --country
  --https-only     only https mirrors
  --top <N>        rows to show (default 5)
  --verbose        per-mirror detail and full rejection reasons

MODE
  (no flag)        non-root: recommend and print the sudo line, exit 3
                   root:      back up, confirm, rewrite the apt sources
  --list           ranked table and planned change only; never writes
  --yes            skip the one confirmation

TABLE
  sig reads ok only for a verified -updates InRelease; only ok may authorize a write.
  notes reads sec:<n>m for a -security lag and redir:<host> for a redirect to another host;
  a host that does not fit is cut and ends in ~; --verbose prints it in full.

EXIT CODES
  0 success    1 refusal or failure    2 usage    3 not root
  4 unsupported distribution or architecture

EXAMPLES
  apt-scout --list --country LT       preview only
  sudo apt-scout                      detect country, write the best mirror
  sudo apt-scout --country DE --yes   no prompt";

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
    if args.verbose {
        line.push_str(" --verbose");
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
    roots: &[sources::MirrorRoot],
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
            roots,
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

/// What the operator can do when no apt source URI is eligible. A host the loaded mirror list does
/// not name is the usual cause, and the fix is to put the URIs back by hand until `--restore` ships
/// (D10), so the report and the write error carry the same sentence instead of drifting apart.
fn no_eligible_hint(list_source: &str) -> String {
    format!(
        "The sources here point at a host the mirror list from {list_source} does not name, so nothing here can be replaced: set those URIs back to archive.ubuntu.com and run again (--restore arrives with D10)."
    )
}

/// No apt source file holds an official Ubuntu archive URI, so the run stops before probing. SPEC.md
/// Apply: `--list` exits 0, a non-root run prints the `sudo` line and exits 3, and a root run that was
/// going to write exits 1.
fn no_eligible_uri(mode: Mode, args: &Args, list_source: &str) -> Result<u8, (u8, String)> {
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
            format!(
                "no official Ubuntu apt source URI can be changed, so the write has nothing to do. Nothing was written. {}. Run with --list to see every file that was inspected.",
                no_eligible_hint(list_source)
            ),
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

/// The lines a run prints, assembled in section order as the facts arrive and printed once, so a
/// slow probe phase cannot interleave with the report (SPEC.md CLI: the ranked table, the
/// recommendation with its reason, and every rejected mirror with its reason).
#[derive(Default)]
struct Report(Vec<String>);

impl Report {
    /// A line at the top of the report, before any section.
    fn line(&mut self, line: String) {
        self.0.push(line);
    }

    /// A blank line, then a section: sections never run together, and an empty section prints
    /// nothing at all rather than a heading with no content under it.
    fn block(&mut self, lines: Vec<String>) {
        debug_assert!(
            !self.0.is_empty(),
            "a section needs the report's first line"
        );
        if self.0.is_empty() || lines.is_empty() {
            return;
        }
        self.0.push(String::new());
        self.0.extend(lines);
    }

    fn print(&self) {
        for line in &self.0 {
            println!("{line}");
        }
    }
}

/// The run's identity: version, release and the country the run used, with the source of the
/// country, because `--country` is an override and the line says which one won (SPEC.md CLI).
fn header_identity(version: &str, distro: &str, country: &str, origin: &str) -> String {
    format!("apt-scout {version} · {distro} · country: {country} ({origin})")
}

/// The facts about where the answer came from: server AS, mirror list source, freshness reference.
fn header_context(
    server_as: Option<&str>,
    list_source: &str,
    reference: Option<&probe::Reference>,
) -> String {
    let reference = match reference {
        Some(r) => format!(
            "reference: archive.ubuntu.com ({})",
            signature_words(&r.updates_signature)
        ),
        None => "reference: newest mirror date (archive.ubuntu.com unreachable)".to_string(),
    };
    format!(
        "server {} · mirrors: {list_source} · {reference}",
        server_as.unwrap_or("AS unknown")
    )
}

/// A section title with an annotation on the right, so a heading can state the rule it follows
/// without a sentence underneath the table.
fn titled(left: &str, right: &str) -> String {
    if right.is_empty() {
        left.to_string()
    } else {
        format!("{left:<45}  {right}")
    }
}

/// The one line under the table when no row has AS data: the run's own reason for it, because a
/// column of `unknown` would read as an answer (SPEC step 9).
fn as_note(rows: &[Row], reason: Option<&str>) -> Option<String> {
    if rows.is_empty() || rows.iter().any(|row| row.as_cell.is_some()) {
        return None;
    }
    Some(format!(
        "note: AS data unavailable ({})",
        reason.unwrap_or("no AS lookup answered")
    ))
}

/// `Ranked — 5 of 12 usable, 7 rejected`: how much of the ranking is shown, and what did not make it.
fn ranked_title(shown: usize, usable: usize, rejected: usize) -> String {
    titled(
        &format!("Ranked — {shown} of {usable} usable, {rejected} rejected"),
        "lag = -updates age vs freshest",
    )
}

/// One ranked row, as presentation needs it: nothing here decides anything.
struct Row<'a> {
    url: &'a str,
    updates_date: i64,
    lag_secs: i64,
    throughput_bps: u64,
    latency_ms: u64,
    signature: &'a verify::SignatureStatus,
    /// `12929 same` when the run has the mirror's AS; the column exists when any row has one.
    as_cell: Option<String>,
    /// The mirror's `-security` lag, which is informational only and never rejects (SPEC step 8).
    security_lag: Option<i64>,
    /// The host this mirror redirected to, when it is not its own host (SPEC step 7).
    redirect_host: Option<&'a str>,
}

/// The narrowest the `as` column may be: a table with AS data still says `unknown` for a mirror the
/// lookup could not name, and that word must fit the column it shares with the ASNs.
const AS_MIN_WIDTH: usize = 7;

/// The most columns a redirect host may take in the `notes` cell. A host longer than this is cut,
/// so one long name cannot wrap the table; `--verbose` prints it in full.
const REDIR_HOST_MAX: usize = 16;

/// What a cut host ends in. `~` cannot appear in a hostname, so a real host is never read as a cut
/// one, and it costs one column, so `REDIR_HOST_MAX` bounds the cell whether or not it was cut.
const CUT_MARKER: char = '~';

/// The ranked table (SPEC step 9): rank, mirror, lag, speed, delay, AS, signature, notes. The AS
/// column is left out entirely when no row has AS data, so it can never read as "every mirror is
/// unknown"; the notes column is left out when no shown row has a note, so a plain table gains no
/// empty column.
fn ranked_rows(rows: &[Row]) -> Vec<String> {
    // Both facts SPEC step 9 asks for, in one cell: the ASN the lookup answered and whether it is
    // the server's. A fixed printed width would either cut a 6-digit ASN or waste columns on a
    // 4-digit one, so the column is as wide as the widest cell it holds.
    let as_width = rows
        .iter()
        .filter_map(|row| row.as_cell.as_deref())
        .map(str::len)
        .max()
        .map(|widest| widest.max(AS_MIN_WIDTH));
    let notes: Vec<Option<String>> = rows.iter().map(note_cell).collect();
    let with_notes = notes.iter().any(Option::is_some);
    let mut out = Vec::new();
    let mut head = format!(
        "  {:>2}  {:<44} {:>5} {:>10} {:>9}",
        "#", "mirror", "lag", "speed", "delay"
    );
    if let Some(width) = as_width {
        head.push_str(&format!(" {:>width$}", "as"));
    }
    head.push_str("  sig");
    if with_notes {
        head.push_str("  notes");
    }
    out.push(head);
    for (i, row) in rows.iter().enumerate() {
        let mut line = format!(
            "  {:>2}  {:<44.44} {:>5} {:>10} {:>9}",
            i + 1,
            row.url,
            format!("{}m", row.lag_secs / 60),
            format!("{:.1} MB/s", row.throughput_bps as f64 / 1e6),
            format!("{} ms", row.latency_ms)
        );
        if let Some(width) = as_width {
            let cell = row.as_cell.as_deref().unwrap_or("unknown");
            line.push_str(&format!(" {cell:>width$}"));
        }
        line.push_str(&format!("  {}", signature_cell(row.signature)));
        if let Some(note) = &notes[i] {
            line.push_str(&format!("  {note}"));
        }
        out.push(line);
    }
    out
}

/// The trailing `notes` cell, last so the facts it carries never push a row's other columns around:
/// `sec:778m` for a `-security` lag and `redir:<host>` for a redirect to another host. Both are
/// facts SPEC.md asks the default output to show (steps 7 and 8) and both were `--verbose`-only
/// after the report rewrite. `None` when the row has neither, so the column can be omitted.
fn note_cell(row: &Row) -> Option<String> {
    let security = row
        .security_lag
        .filter(|lag| *lag > 0)
        .map(|lag| format!("sec:{}m", lag / 60));
    let redirect = row
        .redirect_host
        .map(|host| format!("redir:{}", host_prefix(host, REDIR_HOST_MAX)));
    match (security, redirect) {
        (None, None) => None,
        (Some(sec), None) => Some(sec),
        (None, Some(redir)) => Some(redir),
        (Some(sec), Some(redir)) => Some(format!("{sec} {redir}")),
    }
}

/// At most `room` columns of a host, ending in `CUT_MARKER` when the host did not fit: a cell that
/// silently stopped at `redir:a-very-long-mir` cannot be told from a host of that name. A host of
/// exactly `room` columns is not cut and carries no marker, so the marker means one thing only.
fn host_prefix(host: &str, room: usize) -> String {
    if host.chars().count() <= room {
        return host.to_string();
    }
    // The marker takes the last column, so the whole cell stays inside `room`.
    let mut cut: String = host.chars().take(room.saturating_sub(1)).collect();
    cut.push(CUT_MARKER);
    cut
}

/// What `--verbose` prints about one mirror, and where the default table's `notes` cell reads the
/// same two facts back: the country, both pocket dates and the redirect the probe saw. The
/// `-security` lag is computed once, here, so `--verbose` and the table cannot disagree about it.
fn detail_of(reference_security: Option<i64>, country: &str, outcome: &probe::Outcome) -> Detail {
    let security_lag = match (reference_security, outcome.security_date) {
        (Some(reference), Some(security)) => Some(fresh::lag(reference, security)),
        _ => None,
    };
    Detail {
        country: country.to_string(),
        security_date: outcome.security_date,
        security_lag,
        redirected_to: outcome.redirected_to.clone(),
    }
}

/// The rows the table prints, in rank order and capped at `top`, each carrying the AS cell and the
/// two facts the `notes` column shows. Both note facts are read from the run's own probe results
/// (`details`) instead of being recomputed here, so the default table and `--verbose` describe the
/// same run (SPEC steps 7 and 8).
fn table_rows<'a>(
    ranked: &'a [fresh::Probed],
    details: &'a std::collections::HashMap<String, Detail>,
    mirror_asn: &std::collections::HashMap<String, ResolvedAs>,
    server_asn: Option<&str>,
    top: usize,
) -> Vec<Row<'a>> {
    ranked
        .iter()
        .take(top)
        .map(|r| {
            let detail = details.get(&r.url);
            Row {
                url: &r.url,
                updates_date: r.updates_date,
                lag_secs: r.lag_secs,
                throughput_bps: r.throughput_bps,
                latency_ms: r.latency_ms,
                signature: &r.signature,
                as_cell: as_cell(mirror_asn, server_asn, &r.url),
                // Neither fact is a reason to reject or to reorder, but both are facts this run
                // saw, so they ride on the row instead of being dropped between the probe and the
                // table.
                security_lag: detail.and_then(|d| d.security_lag),
                redirect_host: detail.and_then(|d| d.redirected_to.as_deref()),
            }
        })
        .collect()
}

/// The recommendation and the facts behind it: host, country, AS, speed, lag and signature status
/// (SPEC.md CLI), in one line so nothing repeats the table. The AS cell is the table's own, so the
/// ASN the tie-break used is named here too instead of the bare match word.
fn recommendation(best: &fresh::Probed, country: &str, as_cell: Option<&str>) -> String {
    format!(
        "  {} — {} {}, {}, AS {}, {}m lag, {:.1} MB/s, {} ms",
        best.url,
        if best.lag_secs == 0 {
            "freshest"
        } else {
            "closest"
        },
        signature_words(&best.signature),
        country,
        as_cell.unwrap_or("unknown"),
        best.lag_secs / 60,
        best.throughput_bps as f64 / 1e6,
        best.latency_ms
    )
}

/// Why a candidate is not in the ranked table (SPEC step 8), normalized so the section groups
/// mirrors instead of repeating one sentence per host. `--verbose` prints the full reason.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Reject {
    NotFound,
    NoDate,
    NoInRelease,
    TooLarge,
    Unreachable,
    NoSample,
    Crashed,
    Other(String),
    Stale(i64),
}

impl Reject {
    /// The line a reason is grouped under.
    fn label(&self) -> String {
        match self {
            Reject::NotFound => "404".to_string(),
            Reject::NoDate => "no Date header".to_string(),
            Reject::NoInRelease => "no usable InRelease".to_string(),
            Reject::TooLarge => "oversized InRelease".to_string(),
            Reject::Unreachable => "unreachable".to_string(),
            Reject::NoSample => "no speed sample".to_string(),
            Reject::Crashed => "probe crashed".to_string(),
            Reject::Other(why) => why.clone(),
            Reject::Stale(_) => "stale".to_string(),
        }
    }
}

/// One mirror that did not make the table, with its full reason kept for `--verbose`.
struct Rejected {
    host: String,
    kind: Reject,
    why: String,
}

/// The short reason a probe failure is grouped under. The strings are the ones `probe` and `fresh`
/// produce, so a wording change there is a test failure here rather than a silent new group.
fn rejection(error: &probe::ProbeError) -> Reject {
    match error {
        probe::ProbeError::Unreachable(http::HttpError::Status(404, _)) => Reject::NotFound,
        probe::ProbeError::Unreachable(http::HttpError::Status(code, _)) => {
            Reject::Other(format!("HTTP {code}"))
        }
        probe::ProbeError::Unreachable(http::HttpError::TooLarge(..)) => Reject::TooLarge,
        probe::ProbeError::Unreachable(_) => Reject::Unreachable,
        probe::ProbeError::BadSample(_) => Reject::NoSample,
        probe::ProbeError::BadUpdates(why) if why.contains("no 'Date:' header") => Reject::NoDate,
        probe::ProbeError::BadUpdates(_) => Reject::NoInRelease,
    }
}

/// `Rejected — 7` and one line per reason with the hosts and their numbers (SPEC step 8: every
/// rejected mirror with the reason it was rejected). Sorted by reason, so two runs that saw the
/// same answers print the same section.
fn rejected_section(rejected: &[Rejected]) -> Vec<String> {
    let mut groups: std::collections::BTreeMap<String, Vec<String>> =
        std::collections::BTreeMap::new();
    for r in rejected {
        let item = match r.kind {
            Reject::Stale(secs) => format!("{} ({} h)", r.host, secs / 3600),
            _ => r.host.clone(),
        };
        groups.entry(r.kind.label()).or_default().push(item);
    }
    let mut out = vec![format!("Rejected — {}", rejected.len())];
    for (label, items) in groups {
        out.push(format!("  {label:<17} {}", items.join(", ")));
    }
    out
}

/// `--verbose` says why for each mirror on its own line, instead of grouping.
fn rejected_detail(rejected: &[Rejected]) -> Vec<String> {
    let mut out = vec![format!("Rejected — {}", rejected.len())];
    for r in rejected {
        out.push(format!("  {:<40} {:<17} {}", r.host, r.kind.label(), r.why));
    }
    out
}

fn rejected_lines(rejected: &[Rejected], verbose: bool) -> Vec<String> {
    if verbose {
        rejected_detail(rejected)
    } else {
        rejected_section(rejected)
    }
}

/// What `--verbose` prints about one mirror beyond the table: country, address, AS holder, both
/// pocket dates and the signature status (SPEC.md CLI: per-mirror detail).
struct Detail {
    country: String,
    security_date: Option<i64>,
    security_lag: Option<i64>,
    redirected_to: Option<String>,
}

fn detail_section(
    rows: &[Row],
    details: &std::collections::HashMap<String, Detail>,
    resolved: &std::collections::HashMap<String, ResolvedAs>,
) -> Vec<String> {
    let mut out = vec!["Detail".to_string()];
    for (i, row) in rows.iter().enumerate() {
        out.push(format!("  {}  {}", i + 1, row.url));
        let detail = details.get(row.url);
        let found = resolved.get(row.url);
        let country = detail.map_or("unknown", |d| d.country.as_str());
        let ip = found.map_or_else(|| "unknown".to_string(), |f| f.ip.to_string());
        let asn = match found {
            Some(f) => match &f.name {
                Some(name) => format!("{} ({name})", f.asn),
                None => f.asn.clone(),
            },
            None => "unknown".to_string(),
        };
        out.push(format!("     country: {country} · ip: {ip} · as: {asn}"));
        let security = match (
            detail.and_then(|d| d.security_date),
            detail.and_then(|d| d.security_lag),
        ) {
            (Some(date), Some(lag)) => {
                format!("-security: {}, {}m lag", utc_stamp(date), lag / 60)
            }
            _ => "-security: no InRelease".to_string(),
        };
        out.push(format!(
            "     -updates: {}, {}m lag · {security}",
            utc_stamp(row.updates_date),
            row.lag_secs / 60
        ));
        out.push(format!(
            "     signature: {}",
            signature_words(row.signature)
        ));
        if let Some(host) = detail.and_then(|d| d.redirected_to.as_ref()) {
            out.push(format!("     redirects to {host}"));
        }
    }
    out
}

/// Unix seconds as `2026-11-12 08:29 UTC`: ASCII digits and one shape in every locale (SPEC.md
/// Localization), so a copied date stays a date.
fn utc_stamp(secs: i64) -> String {
    let days = secs.div_euclid(86_400);
    let rem = secs.rem_euclid(86_400);
    // Howard Hinnant's civil_from_days, the inverse of the parser's days_from_civil.
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    format!(
        "{year:04}-{month:02}-{day:02} {:02}:{:02} UTC",
        rem / 3600,
        (rem % 3600) / 60
    )
}

/// The `Sources` section: every inspected file, and every stanza and URI a write would rewrite or
/// skip, with the reason (SPEC.md Apply: report each inspected file and each skipped entry). One
/// line per file and per URI, so the section is a list, not a paragraph. Paths are relative to the
/// apt root named in the title, which keeps the list narrow and unambiguous.
fn sources_section(root: &Path, inventory: &sources::Inventory) -> Vec<String> {
    let mut out = vec![titled("Sources", &root.display().to_string())];
    if inventory.files.is_empty() {
        out.push("  no apt source files found".to_string());
    }
    for file in &inventory.files {
        let path = file
            .path
            .strip_prefix(root)
            .map(|p| p.display().to_string())
            .unwrap_or_else(|_| file.path.display().to_string());
        if let Some(issue) = &file.issue {
            out.push(format!("  {path:<40}  {:<7}  {issue}", "skip"));
            continue;
        }
        if file.entries.is_empty() {
            out.push(format!("  {path:<40}  no active entries"));
            continue;
        }
        for entry in &file.entries {
            if entry.uris.is_empty() {
                out.push(format!(
                    "  {path:<40}  {:<7}  line {}  {}",
                    "skip",
                    entry.line,
                    entry.issue.as_deref().unwrap_or("no URI")
                ));
                continue;
            }
            for (i, uri) in entry.uris.iter().enumerate() {
                let host = mirror_host_port(uri).map_or_else(|| uri.clone(), |(host, _)| host);
                if entry.rewritable.get(i).copied().unwrap_or(false) {
                    out.push(format!(
                        "  {path:<40}  {:<7}  line {}  {host}",
                        "rewrite", entry.line
                    ));
                } else {
                    out.push(format!(
                        "  {path:<40}  {:<7}  line {}  {host} — {}",
                        "skip",
                        entry.line,
                        skip_reason(entry)
                    ));
                }
            }
        }
    }
    out
}

/// Why the write leaves an entry alone, in the shortest true form. A `-security` stanza says so in
/// its own words, because the inventory's third-party wording would call an official Ubuntu host a
/// stranger (SPEC.md Apply: a security suite is never rewritten).
fn skip_reason(entry: &sources::Entry) -> String {
    let security = entry.suites.iter().any(|s| s.ends_with("-security"));
    let other = entry.suites.iter().any(|s| !s.ends_with("-security"));
    if security && !other {
        return "security suite is never rewritten".to_string();
    }
    entry
        .issue
        .clone()
        .unwrap_or_else(|| "not an official Ubuntu archive mirror host".to_string())
}

/// The change a write would make, per target: the old URI and the new one. URI pairs rather than
/// file lines, because the one thing that changes is the URI, and a long `sources.list` diff buries
/// it (SPEC.md Apply: the old URI and the new one, the files it will change).
fn plan_changes(root: &Path, inventory: &sources::Inventory, plan: &plan::Plan) -> Vec<String> {
    let mut out = Vec::new();
    for write in plan.writes() {
        let relative = write.target.relative().display().to_string();
        if write.create_new {
            out.push(format!("  {relative}  (new file, mode 0600)"));
            for line in String::from_utf8_lossy(&write.new_bytes).lines() {
                out.push(format!("    + {line}"));
            }
            continue;
        }
        let path = root.join(write.target.relative());
        let mut shown = false;
        if let Some(file) = inventory.files.iter().find(|file| file.path == path) {
            for entry in &file.entries {
                for (uri, rewritable) in entry.uris.iter().zip(&entry.rewritable) {
                    if *rewritable {
                        out.push(format!("  {relative}:{}", entry.line));
                        out.push(format!("    - {uri}"));
                        out.push(format!("    + {}", plan.selected_uri()));
                        shown = true;
                    }
                }
            }
        }
        // A planned write always comes from a rewritable URI; this keeps the section honest about
        // the file even if a later change ever plans a different kind of write.
        if !shown {
            out.push(format!("  {relative}  rewritten"));
        }
    }
    out
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
        "--country"
    } else {
        "detected"
    };

    let country_name = countries::launchpad_name(&country).ok_or_else(|| {
        (
            2,
            format!("unknown country code '{country}'; Launchpad lists mirrors in 82 countries, for example LT, NO, DE, GB, US"),
        )
    })?;

    let (mut cands, list_source, list_roots) =
        load_candidates(&net, &country, country_name, args.https_only)?;
    // Different list sources can name the same mirror twice (with and without a trailing slash).
    let mut seen = std::collections::HashSet::new();
    cands.retain(|m| seen.insert(m.url.trim_end_matches('/').to_ascii_lowercase()));
    cands.truncate(MAX_CANDIDATES);

    // The places the loaded list says an Ubuntu archive lives. Eligibility is host **and** path from
    // every country's Launchpad entry, so a mirror this tool wrote survives `--country`; `--https-only`
    // never narrows it, and every entry passed `validate_url` before it entered (SPEC: Apply).
    let mut roots = vec![sources::MirrorRoot {
        host: "archive.ubuntu.com".to_string(),
        path: "/".to_string(),
    }];
    if d.id == "pop" {
        roots.push(sources::MirrorRoot {
            host: "apt.pop-os.org".to_string(),
            path: "/".to_string(),
        });
    }
    roots.extend(list_roots);
    roots.sort_unstable_by(|a, b| a.host.cmp(&b.host).then_with(|| a.path.cmp(&b.path)));
    roots.dedup_by(|a, b| a.host.eq_ignore_ascii_case(&b.host) && a.path == b.path);
    let apt_sources =
        sources::inventory(std::path::Path::new(apply::APT_DIR), &roots, d.id == "pop");
    let distro_label = match d.version_id.as_deref() {
        Some(v) => format!("Ubuntu {v} ({codename})"),
        None => format!("Ubuntu ({codename})"),
    };
    let mut report = Report::default();
    report.line(header_identity(
        env!("CARGO_PKG_VERSION"),
        &distro_label,
        country_name,
        country_origin,
    ));
    report.block(sources_section(Path::new(apply::APT_DIR), &apt_sources));
    if !apt_sources.has_rewritable_uri() {
        report.line(
            "No official Ubuntu apt source URI can be changed; stopping before mirror probes."
                .to_string(),
        );
        report.line(no_eligible_hint(list_source));
        report.print();
        return no_eligible_uri(mode, &args, list_source);
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
    let mut rejected: Vec<Rejected> = Vec::new();
    for (m, handle) in cands.iter().zip(handles) {
        match handle.join() {
            Ok(done) => results.push(done),
            Err(_) => rejected.push(Rejected {
                host: m.url.trim_end_matches('/').to_string(),
                kind: Reject::Crashed,
                why: "the probe crashed unexpectedly".to_string(),
            }),
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
            .filter_map(|o| o.verified_updates)
            .collect(),
    );
    // AS of each candidate (SPEC step 5): informational, never a reason to fail; skipped with --no-geo.
    // A failed lookup is kept as the reason the table has no AS column, instead of a silent `unknown`.
    let server_asn = geo.asn.clone();
    let (mirror_asn, as_error): (
        std::collections::HashMap<String, ResolvedAs>,
        Option<String>,
    ) = if args.no_geo {
        (
            Default::default(),
            Some("--no-geo skips every AS lookup".to_string()),
        )
    } else if server_asn.is_none() {
        (
            Default::default(),
            Some("the server's AS is unknown, so no mirror can match it".to_string()),
        )
    } else {
        resolve_mirror_asns(
            &results
                .iter()
                .map(|(m, _)| m.url.clone())
                .collect::<Vec<_>>(),
            args.verbose,
        )
    };
    let mut probed: Vec<fresh::Probed> = Vec::new();
    let mut details: std::collections::HashMap<String, Detail> = std::collections::HashMap::new();
    let reference_security = reference.as_ref().and_then(|r| r.security);
    for (m, r) in results {
        match r {
            Ok(o) => {
                let lag = fresh::lag(baseline, o.updates_date);
                details.insert(o.url.clone(), detail_of(reference_security, &m.country, &o));
                if fresh::is_stale(lag) {
                    rejected.push(Rejected {
                        host: m.url.trim_end_matches('/').to_string(),
                        kind: Reject::Stale(lag),
                        why: format!(
                            "stale: {} h behind the freshest date; the limit is {} h",
                            lag / 3600,
                            fresh::MAX_LAG_SECS / 3600
                        ),
                    });
                } else {
                    probed.push(fresh::Probed {
                        url: o.url,
                        updates_date: o.updates_date,
                        verified_updates: o.verified_updates,
                        lag_secs: lag,
                        throughput_bps: o.throughput_bps,
                        latency_ms: o.latency_ms,
                        same_as: server_asn.is_some()
                            && mirror_asn.get(as_key(&m.url)).map(|a| &a.asn)
                                == server_asn.as_ref(),
                        signature: o.updates_signature,
                    });
                }
            }
            Err(e) => rejected.push(Rejected {
                host: m.url.trim_end_matches('/').to_string(),
                kind: rejection(&e),
                why: e.to_string(),
            }),
        }
    }
    rejected.sort_by(|a, b| a.host.cmp(&b.host));
    let ranked = fresh::rank(probed);
    let rows = table_rows(
        &ranked,
        &details,
        &mirror_asn,
        server_asn.as_deref(),
        args.top,
    );
    report.block(vec![header_context(
        server_asn.as_deref(),
        list_source,
        reference.as_ref(),
    )]);
    let mut ranked_lines = vec![ranked_title(rows.len(), ranked.len(), rejected.len())];
    ranked_lines.extend(ranked_rows(&rows));
    // A run with no AS data says why under the table, once, instead of printing a column of
    // `unknown` that implies the lookup happened (SPEC step 9).
    ranked_lines.extend(as_note(&rows, as_error.as_deref()));
    report.block(ranked_lines);
    if args.verbose && !rows.is_empty() {
        report.block(detail_section(&rows, &details, &mirror_asn));
    }
    let Some(best) = ranked.first() else {
        report.block(rejected_lines(&rejected, args.verbose));
        report.print();
        return Err(no_fresh_mirror());
    };
    let best_cell = as_cell(&mirror_asn, server_asn.as_deref(), &best.url);
    report.block(vec![
        "Recommendation".to_string(),
        recommendation(best, country_name, best_cell.as_deref()),
    ]);

    let platform = if d.id == "pop" {
        plan::Platform::PopOs
    } else {
        plan::Platform::Ubuntu
    };
    // The backup id is fixed before the plan is printed so the report can name the directory the
    // write will create; the writer is then handed the same instant.
    let write_timing = match mode {
        Mode::Write { .. } => {
            let now = now_secs()?;
            let id = manifest::BackupId::from_secs(now).map_err(|e| {
                (
                    1,
                    format!(
                        "cannot name the backup directory for {now}: {e}. Nothing was written."
                    ),
                )
            })?;
            Some((now, id))
        }
        _ => None,
    };
    let mut pending: Option<plan::Plan> = None;
    if mode != Mode::Recommend {
        let mut planned = vec![titled(
            "Planned change",
            if mode == Mode::List {
                "nothing is written with --list"
            } else {
                "written only after the confirmation"
            },
        )];
        match build_plan(
            Path::new(apply::APT_DIR),
            &apt_sources,
            &roots,
            best,
            platform,
        ) {
            Ok(change) => match change.plan {
                Some(plan) => {
                    planned.extend(plan_changes(Path::new(apply::APT_DIR), &apt_sources, &plan));
                    if let Some((_, id)) = &write_timing {
                        planned.push(format!(
                            "  backup: {}/{} (every replaced file is copied there, mode 0700, before the first write)",
                            apply::BACKUP_ROOT,
                            id.as_str()
                        ));
                        planned.push(
                            "  alternatives: none (extra mirrors as comments are a TUI option)"
                                .to_string(),
                        );
                        // The confirmation lists what the write leaves alone, too: the user is
                        // confirming the whole change, not only the files that move.
                        for reason in &change.skipped {
                            planned.push(format!("  keep as is  {reason}"));
                        }
                    }
                    pending = Some(plan);
                }
                None => planned.push(format!(
                    "  nothing to change: the apt sources already point at {}",
                    best.url
                )),
            },
            // A refused plan names the entries it refused, so there is nothing left to list; a
            // `--list` run reports it and still exits 0 (SPEC.md CLI table).
            Err((code, why)) => match mode {
                Mode::List => planned.push(format!("  a write would be refused: {why}")),
                _ => return Err((code, why)),
            },
        }
        report.block(planned);
    }
    report.block(rejected_lines(&rejected, args.verbose));
    report.print();

    match mode {
        // `--list` prints the change a write would make and never writes, whatever the uid.
        Mode::List => Ok(0),
        // A run that may not write prints the one line that repeats it under `sudo`, and exits 3.
        Mode::Recommend => {
            println!(
                "\nNot root: nothing was written. Run:\n  {}",
                sudo_command(&args)
            );
            Ok(3)
        }
        Mode::Write { confirm } => {
            let Some(plan) = pending else {
                // Re-running a run whose write already happened is success, not a failure.
                return Ok(0);
            };
            let Some((now, _)) = write_timing else {
                return Err((
                    1,
                    "the write has no backup directory name, so nothing was written. This is a bug in apt-scout; re-run it."
                        .to_string(),
                ));
            };
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

/// The `sig` column: the short status, so a row says in one word whether a write may use it. Only
/// `ok` (a verified `-updates` `InRelease`) can authorize a write (SPEC: Freshness trust boundary).
fn signature_cell(status: &verify::SignatureStatus) -> &'static str {
    match status {
        verify::SignatureStatus::Verified(_) => "ok",
        verify::SignatureStatus::Invalid(_) => "no",
        verify::SignatureStatus::Unavailable(_) => "unavailable",
    }
}

/// The same status in words, for the recommendation and `--verbose`.
fn signature_words(status: &verify::SignatureStatus) -> &'static str {
    match status {
        verify::SignatureStatus::Verified(_) => "verified",
        verify::SignatureStatus::Invalid(_) => "unverified (invalid signature)",
        verify::SignatureStatus::Unavailable(_) => "unverified (signature unavailable)",
    }
}

/// The `as` cell value for one mirror: the ASN Team Cymru answered and whether it is the server's,
/// together (`12929 same`), because SPEC step 9 asks for both facts and one cell can carry both.
/// `None` when the run has no AS answer for this mirror at all, which the table prints as
/// `unknown`: never imply an AS preference the lookup did not support.
fn as_cell(
    resolved: &std::collections::HashMap<String, ResolvedAs>,
    server_asn: Option<&str>,
    url: &str,
) -> Option<String> {
    resolved.get(as_key(url)).map(|found| {
        format!(
            "{} {}",
            asn_number(&found.asn),
            as_status(server_asn, Some(&found.asn))
        )
    })
}

/// `AS12929` as `12929`: the cell already says what the number is, and the prefix would spend two
/// columns of every row on nothing.
fn asn_number(asn: &str) -> &str {
    asn.strip_prefix("AS").unwrap_or(asn)
}

fn as_status(server_asn: Option<&str>, mirror_asn: Option<&str>) -> &'static str {
    match (server_asn, mirror_asn) {
        (Some(server), Some(mirror)) if server == mirror => "same",
        (Some(_), Some(_)) => "diff",
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

/// The AS Team Cymru names for one candidate, and (with `--verbose`) its holder name (SPEC step 5).
/// `name` is display detail only: grouping and ranking read `asn`, never `name`.
struct ResolvedAs {
    ip: std::net::IpAddr,
    asn: String,
    name: Option<String>,
}

/// The spelling every AS answer is keyed and looked up by. The probe reports a mirror without its
/// trailing slash, the mirror list may carry one, and a map keyed on the other spelling answers
/// `unknown` for every row it is asked about.
fn as_key(url: &str) -> &str {
    url.trim_end_matches('/')
}

/// One total budget for every AS number. `--verbose` holder names are a separate pass with the
/// smaller `NAME_BUDGET` of their own, so no name can spend this one.
const AS_BUDGET: Duration = Duration::from_secs(20);
/// The budget for `--verbose` holder names, spent after every AS number is recorded: a run that
/// loses this detail loses only a name.
const NAME_BUDGET: Duration = Duration::from_secs(5);

/// AS of each mirror host via Team Cymru DNS, in two phases that cannot spend each other's budget:
/// first every AS number (`resolve_asns`, `AS_BUDGET`), then, for `--verbose` only, the holder names
/// (`attach_names`, `NAME_BUDGET`). The names start from the answers the first phase has already
/// returned and can only fill in a name, so `--verbose` can add a name and can lose a name but
/// cannot change which mirror has an AS — and therefore cannot change how any row groups or ranks.
/// Returns the answers it got and, when any lookup failed, the first real reason (sorted by URL, so
/// a run is repeatable) so the table can say why it has no AS data instead of printing `unknown`.
fn resolve_mirror_asns(
    urls: &[String],
    with_names: bool,
) -> (
    std::collections::HashMap<String, ResolvedAs>,
    Option<String>,
) {
    let (mut out, failure) = resolve_asns(urls);
    if with_names {
        attach_names(&mut out, NAME_BUDGET, |asn| cymru::lookup_name(asn).ok());
    }
    (out, failure)
}

/// Every AS number, at most 8 lookups at a time and one total `AS_BUDGET` for all resolution and
/// queries. Standard-library hostname resolution is offloaded to a worker thread because it cannot
/// be interrupted; after the budget, results are discarded.
fn resolve_asns(
    urls: &[String],
) -> (
    std::collections::HashMap<String, ResolvedAs>,
    Option<String>,
) {
    let deadline = Instant::now() + AS_BUDGET;
    let mut out = std::collections::HashMap::new();
    let mut failures: std::collections::BTreeMap<String, String> =
        std::collections::BTreeMap::new();
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
            .map(|url| thread::spawn(move || resolve_one(&url)))
            .collect();
        let (tx, rx) = std::sync::mpsc::channel();
        for h in handles {
            let tx = tx.clone();
            thread::spawn(move || {
                let _ = tx.send(h.join().ok());
            });
        }
        drop(tx);
        loop {
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                break;
            }
            match rx.recv_timeout(remaining) {
                Ok(Some((url, Ok(resolved)))) => {
                    out.insert(as_key(&url).to_string(), resolved);
                }
                Ok(Some((url, Err(why)))) => {
                    failures.insert(url, why);
                }
                // The probe thread panicked: its URL has no AS, and the run continues.
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
    (out, failures.into_iter().next().map(|(_, why)| why))
}

/// Resolve one candidate's host, then its AS. Every failure is returned with its URL so the caller
/// can name the first one: an AS answer is information, never a reason to stop the run (SPEC.md,
/// Resilience).
fn resolve_one(url: &str) -> (String, Result<ResolvedAs, String>) {
    use std::net::ToSocketAddrs;
    let fail = |why: String| (url.to_string(), Err(why));
    let Some((host, port)) = mirror_host_port(url) else {
        return fail(format!("{url} has no http(s) host to resolve"));
    };
    let addrs = match (host.as_str(), port).to_socket_addrs() {
        Ok(addrs) => addrs,
        Err(e) => return fail(format!("cannot resolve {host}: {e}")),
    };
    let Some(ip) = addrs.map(|a| a.ip()).find(|ip| !mirrors::is_private(*ip)) else {
        return fail(format!("{host} resolves only to a private address"));
    };
    let asn = match cymru::lookup(ip) {
        Ok(asn) => asn,
        Err(e) => return fail(format!("Team Cymru names no AS for {ip}: {e}")),
    };
    (
        url.to_string(),
        Ok(ResolvedAs {
            ip,
            asn,
            name: None,
        }),
    )
}

/// `--verbose` holder names, filled in on answers that already exist and inside `budget` of their
/// own. `lookup` answers `None` when Cymru has no name, and a lookup that does not answer before
/// the budget is spent leaves `name` empty: a name is display detail, so this pass adds detail or
/// drops detail, and the AS numbers it decorates are already recorded and cannot be lost here.
fn attach_names(
    out: &mut std::collections::HashMap<String, ResolvedAs>,
    budget: Duration,
    lookup: impl Fn(&str) -> Option<String>,
) {
    let deadline = Instant::now() + budget;
    // Sorted, so two runs that only fit some names inside the budget fill the same ones.
    let mut keys: Vec<String> = out.keys().cloned().collect();
    keys.sort();
    for key in keys {
        if Instant::now() >= deadline {
            break;
        }
        let Some(asn) = out.get(&key).map(|resolved| resolved.asn.clone()) else {
            continue;
        };
        if let Some(entry) = out.get_mut(&key) {
            entry.name = lookup(&asn);
        }
    }
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
        .and_then(|r| r.verified_updates)
        .or_else(|| fallback_baseline(candidates))
}

/// What one mirror-list source produced: the candidates, the name of the list that supplied them,
/// and the run's mirror roots (see `load_candidates` for the contract between the first and last).
type LoadedCandidates = (Vec<mirrors::Mirror>, &'static str, Vec<sources::MirrorRoot>);

/// The `(host, path)` pairs of `urls`, in the order the list gave them; the caller sorts and dedups.
/// A URL whose authority or path does not parse contributes nothing, so a tree is never guessed from
/// a broken link.
fn mirror_roots<'a>(urls: impl Iterator<Item = &'a str>) -> Vec<sources::MirrorRoot> {
    urls.filter_map(sources::mirror_root).collect()
}

/// SPEC step 3: Launchpad first, then `mirrors.ubuntu.com/<CC>.txt`. Returns the candidates, the
/// name of the source that supplied them, and the run's mirror roots; every failed source is
/// reported with its reason.
///
/// Contract for the third element: every mirror in the returned `Vec<Mirror>` has its `(host, path)`
/// in it. From Launchpad that set is every host and path the page names, in every country, collected
/// before the country filter and before `keep`, so a mirror a previous run wrote stays rewritable
/// after the operator changes country and `--https-only` does not narrow what is recognized (SPEC:
/// Apply). When the run falls back to `<CC>.txt`, the set is that file's validated entries, because
/// that list arrives over plain HTTP: the set fails closed rather than widening on an unverified list.
fn load_candidates(
    net: &dyn Fetch,
    cc: &str,
    country_name: &str,
    https_only: bool,
) -> Result<LoadedCandidates, (u8, String)> {
    let keep =
        |m: &mirrors::Mirror| mirrors::validate_url(&m.url).is_ok() && (!https_only || m.https);
    let mut failures = Vec::new();

    match net.get(LAUNCHPAD, http::MAX_PAGE, None) {
        Ok(page) => match mirrors::parse_launchpad(&String::from_utf8_lossy(&page.body)) {
            Ok(all) => {
                // Every country's entry, not only this run's, and before `keep`: eligibility answers
                // recognizability, never use.
                let roots = mirror_roots(all.iter().map(|m| m.url.as_str()));
                let found: Vec<_> = all
                    .into_iter()
                    .filter(|m| m.country == country_name && keep(m))
                    .collect();
                if !found.is_empty() {
                    return Ok((found, "Launchpad", roots));
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
            let all: Vec<_> = mirrors::parse_text_list(&String::from_utf8_lossy(&got.body))
                .into_iter()
                .map(|u| mirrors::Mirror {
                    country: country_name.to_string(),
                    name: String::new(),
                    https: u.starts_with("https://"),
                    url: u,
                    speed: None,
                    status: mirrors::Status::Unknown,
                })
                .collect();
            // Validated, not `keep`: `--https-only` picks what may be selected, never what may be
            // recognized, so a mirror written over `http` stays rewritable in a later run. This
            // list arrives over plain HTTP, so a host that fails candidate validation is still
            // dropped and never becomes rewritable.
            let roots = mirror_roots(
                all.iter()
                    .filter(|m| mirrors::validate_url(&m.url).is_ok())
                    .map(|m| m.url.as_str()),
            );
            let found: Vec<_> = all.into_iter().filter(keep).collect();
            if !found.is_empty() {
                eprintln!(
                    "warning: {}. Using {url} instead (its URLs come over plain HTTP, so they are unverified).",
                    failures.join("; ")
                );
                return Ok((found, "mirrors.ubuntu.com (unverified)", roots));
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

    /// Every host listed at `/` — the whole-host case — as the eligibility set these tests use.
    fn listed(hosts: &[&str]) -> Vec<sources::MirrorRoot> {
        hosts
            .iter()
            .map(|host| sources::MirrorRoot {
                host: (*host).to_string(),
                path: "/".to_string(),
            })
            .collect()
    }

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
    fn as_status_reports_same_diff_and_unknown() {
        assert_eq!(as_status(Some("AS64500"), Some("AS64500")), "same");
        assert_eq!(as_status(Some("AS64500"), Some("AS64501")), "diff");
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

    /// A `VerifiedDate` built the only way one exists: the verifier's in-region read of the body, with
    /// a proof attached. A test that could name the date without that read would be the abuse this
    /// type exists to make inexpressible.
    fn vd(rfc: &str) -> verify::VerifiedDate {
        verify::verified_date_for_test(&format!(
            "-----BEGIN PGP SIGNED MESSAGE-----\nHash: SHA512\n\nDate: {rfc}\n"
        ))
        .expect("the armour and the date parse")
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
        // an unverified date cannot enter at all: only a verification pass hands a `VerifiedDate` out
        assert_eq!(
            verify::SignatureStatus::Invalid("bad".into())
                .proof_date()
                .map(|d| d.seconds()),
            None
        );
        assert_eq!(
            verify::SignatureStatus::Unavailable("missing".into())
                .proof_date()
                .map(|d| d.seconds()),
            None
        );
    }

    #[test]
    fn write_baseline_prefers_a_verified_reference() {
        let reference_date = fresh::parse_date("Thu, 12 Nov 2026  8:29:02 UTC").unwrap();
        let verified = probe::Reference {
            updates: reference_date,
            verified_updates: Some(vd("Thu, 12 Nov 2026  8:29:02 UTC")),
            updates_signature: verify::SignatureStatus::verified_for_test(),
            security: None,
        };
        let unverified = probe::Reference {
            updates: reference_date,
            verified_updates: None,
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
    /// A page with no country section at all: present, but not a mirror list.
    const LAUNCHPAD_EMPTY: &[u8] = b"<html></html>";

    #[test]
    fn launchpad_is_preferred() {
        let net = Canned(vec![
            (LAUNCHPAD, Ok(LAUNCHPAD_OK)),
            ("http://mirrors.ubuntu.com/LT.txt", Ok(LT_TXT)),
        ]);
        let (c, src, _) = load_candidates(&net, "LT", "Lithuania", false).unwrap();
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
        let (c, src, _) = load_candidates(&net, "LT", "Lithuania", false).unwrap();
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
        let (c, src, _) = load_candidates(&net, "IS", "Iceland", false).unwrap();
        assert_eq!(src, "mirrors.ubuntu.com (unverified)");
        assert_eq!(c.len(), 1);
    }

    #[test]
    fn https_only_filters_the_fallback() {
        let net = Canned(vec![
            (LAUNCHPAD, Err(503)),
            ("http://mirrors.ubuntu.com/LT.txt", Ok(LT_TXT)),
        ]);
        let (c, _, _) = load_candidates(&net, "LT", "Lithuania", true).unwrap();
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
        let (c, _, roots) = load_candidates(&net, "LT", "Lithuania", false).unwrap();
        assert_eq!(c.len(), 1);
        assert_eq!(c[0].url, "http://ok.example/ubuntu/");
        // The fallback list arrives over plain HTTP, so only its validated entries join the set: a
        // loopback or metadata address named there can never become rewritable.
        assert_eq!(
            roots,
            [sources::MirrorRoot {
                host: "ok.example".to_string(),
                path: "/ubuntu/".to_string(),
            }]
        );
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

    /// The rewrite-eligibility set is every host and path the Launchpad page names, in every country,
    /// so a mirror this tool wrote stays rewritable after the operator changes country (SPEC: Apply).
    #[test]
    fn the_host_list_names_every_country_not_only_the_run_s_country() {
        let net = Canned(vec![(LAUNCHPAD, Ok(LAUNCHPAD_OK))]);
        let (cands, src, roots) = load_candidates(&net, "DE", "Germany", false).unwrap();
        assert_eq!(src, "Launchpad");
        assert!(cands.iter().all(|m| m.country == "Germany"));
        assert!(
            roots.iter().any(|r| r.host == "ftp.uni-stuttgart.de"),
            "a German mirror's host: {roots:?}"
        );
        assert!(
            roots.iter().any(|r| r.host == "ubuntu.uib.no"),
            "a Norwegian mirror written by an earlier run must stay recognizable: {roots:?}"
        );
        assert!(
            roots.iter().any(|r| r.host == "ftp.litnet.lt"),
            "a Lithuanian mirror written by an earlier run must stay recognizable: {roots:?}"
        );
        // The path travels with the host: `ubuntu.uib.no` is listed under `/archive/`.
        assert!(
            roots
                .iter()
                .any(|r| r.host == "ubuntu.uib.no" && r.path == "/archive/"),
            "the listed path must be the one carried in the set: {roots:?}"
        );
    }

    /// `--https-only` filters candidates, never eligibility: a mirror written over `http` stays
    /// recognizable in a later `--https-only` run, so the fix is not order-dependent.
    #[test]
    fn https_only_does_not_narrow_the_host_list() {
        let net = Canned(vec![(LAUNCHPAD, Ok(LAUNCHPAD_OK))]);
        let (cands, _, roots) = load_candidates(&net, "DE", "Germany", true).unwrap();
        assert!(cands.iter().all(|m| m.https));
        // ftp.litnet.lt's only HTTP link in the fixture is `http`, so no candidate may carry it,
        // while its host must still be in the set.
        assert!(
            cands
                .iter()
                .all(|m| m.url != "http://ftp.litnet.lt/ubuntu/")
        );
        assert!(
            roots.iter().any(|r| r.host == "ftp.litnet.lt"),
            "an http-only mirror host must stay recognizable: {roots:?}"
        );
    }

    /// The same rule on the fallback list: `--https-only` drops the `http` candidates, and a mirror
    /// an earlier run wrote over `http` stays recognizable. Candidate validation still applies,
    /// because this list arrives over plain HTTP.
    #[test]
    fn https_only_does_not_narrow_the_fallback_host_list() {
        let net = Canned(vec![
            (LAUNCHPAD, Err(503)),
            ("http://mirrors.ubuntu.com/LT.txt", Ok(LT_TXT)),
        ]);
        let (cands, _, roots) = load_candidates(&net, "LT", "Lithuania", true).unwrap();
        assert_eq!(cands.len(), 1);
        assert!(cands.iter().all(|m| m.https));
        assert!(
            roots.iter().any(|r| r.host == "ubuntu.mirror.vu.lt"),
            "an http mirror host must stay recognizable: {roots:?}"
        );
        // The `ftp://` line: not a candidate and not an entry, whatever the flags say.
        assert_eq!(roots.len(), 2, "{roots:?}");
    }

    /// The contract `main` relies on when it builds the set from the third element alone:
    /// every returned candidate's `(host, path)` is in the returned set, in both branches.
    #[test]
    fn every_candidate_host_is_in_the_returned_host_list() {
        let cases = vec![
            (Canned(vec![(LAUNCHPAD, Ok(LAUNCHPAD_OK))]), "DE", "Germany"),
            (
                Canned(vec![
                    (LAUNCHPAD, Err(503)),
                    ("http://mirrors.ubuntu.com/LT.txt", Ok(LT_TXT)),
                ]),
                "LT",
                "Lithuania",
            ),
        ];
        for (net, cc, name) in cases {
            let (cands, src, roots) = load_candidates(&net, cc, name, false).unwrap();
            assert!(!cands.is_empty(), "{src}");
            for m in &cands {
                let mine = sources::mirror_root(&m.url).expect("a validated candidate has a host");
                assert!(roots.contains(&mine), "{src}: {mine:?} is not in {roots:?}");
            }
        }
    }

    /// Launchpad is the only list that names every country, so when it is unavailable or unreadable
    /// the set is the entries of the list the run did load: it shrinks, and a foreign host stays
    /// unrecognized rather than becoming rewritable on an unverified list.
    #[test]
    fn the_host_list_shrinks_to_the_fallback_when_launchpad_fails() {
        for launchpad in [Err(500), Ok(LAUNCHPAD_EMPTY)] {
            let net = Canned(vec![
                (LAUNCHPAD, launchpad),
                ("http://mirrors.ubuntu.com/LT.txt", Ok(LT_TXT)),
            ]);
            let (cands, src, roots) = load_candidates(&net, "LT", "Lithuania", false).unwrap();
            assert_eq!(src, "mirrors.ubuntu.com (unverified)");
            let mut expected: Vec<sources::MirrorRoot> = cands
                .iter()
                .map(|m| sources::mirror_root(&m.url).expect("a validated candidate has a host"))
                .collect();
            expected.sort_unstable_by(|a, b| a.host.cmp(&b.host).then_with(|| a.path.cmp(&b.path)));
            let mut got = roots.clone();
            got.sort_unstable_by(|a, b| a.host.cmp(&b.host).then_with(|| a.path.cmp(&b.path)));
            assert_eq!(got, expected);
            // No entry of the Launchpad fixture may leak in through the fallback.
            for foreign in ["ubuntu.uib.no", "ftp.litnet.lt", "ftp.uni-stuttgart.de"] {
                assert!(!roots.iter().any(|r| r.host == foreign), "{roots:?}");
            }
        }
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

    /// A probed mirror shaped the way `probe::probe` builds one: the verified date is the one the
    /// verification pass handed out, so a mirror whose status did not verify carries none.
    fn probed(url: &str, signature: verify::SignatureStatus) -> fresh::Probed {
        fresh::Probed {
            url: url.to_string(),
            updates_date: 1000,
            verified_updates: signature
                .is_verified()
                .then(|| vd("Thu, 01 Jan 1970 00:16:40 UTC")),
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
        let hosts = listed(&["mirror.example"]);
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
        let hosts = listed(&["archive.ubuntu.com", "mirror.example"]);
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
        let hosts = listed(&["archive.ubuntu.com", "mirror.example"]);
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
        assert_eq!(no_eligible_uri(Mode::List, &args, "Launchpad"), Ok(0));
        // No eligible URI, non-root: the `sudo` line and exit 3.
        assert_eq!(no_eligible_uri(Mode::Recommend, &args, "Launchpad"), Ok(3));
        // No eligible URI, root that was going to write: exit 1, and it says nothing was written.
        let (code, msg) = no_eligible_uri(Mode::Write { confirm: true }, &args, "Launchpad")
            .expect_err("a root run with nothing to write must fail");
        assert_eq!(code, 1);
        assert!(
            msg.contains("no official Ubuntu apt source URI can be changed"),
            "{msg}"
        );
        assert!(msg.contains("Nothing was written"), "{msg}");
        // The refusal says what to do: name the host as the problem and point at the URIs to set
        // back, with `--restore` explicitly still to come (D10).
        assert!(
            msg.contains("set those URIs back to archive.ubuntu.com"),
            "{msg}"
        );
        assert!(msg.contains("--restore"), "{msg}");
        // The hint is one line and names the list the run did load, so the reason is checkable.
        for source in ["Launchpad", "mirrors.ubuntu.com (unverified)"] {
            let hint = no_eligible_hint(source);
            assert!(hint.contains(source), "{hint}");
            assert!(hint.contains("archive.ubuntu.com"), "{hint}");
            assert!(!hint.contains('\n'), "the hint stays one line: {hint}");
        }
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
        let hosts = listed(&["archive.ubuntu.com"]);
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
        let hosts = listed(&["archive.ubuntu.com"]);
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
        let hosts = listed(&["archive.ubuntu.com", "mirror.example"]);
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
        // The change the confirmation and --list show: the old URI and the new one per target,
        // and nothing else.
        assert_eq!(
            plan_changes(&apt.0, &inventory, &plan),
            [
                "  sources.list:1",
                "    - http://archive.ubuntu.com/ubuntu",
                "    + https://mirror.example/ubuntu/",
                "  sources.list.d/ubuntu.sources:2",
                "    - http://archive.ubuntu.com/ubuntu",
                "    + https://mirror.example/ubuntu/",
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
        let hosts = listed(&["apt.pop-os.org", "mirror.example"]);
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

    /// The `sig` column's whole job: a verified `-updates` `InRelease` is the only `ok`.
    #[test]
    fn signature_cell_is_ok_only_for_verified() {
        assert_eq!(
            signature_cell(&verify::SignatureStatus::verified_for_test()),
            "ok"
        );
        assert_eq!(
            signature_cell(&verify::SignatureStatus::Invalid("bad".into())),
            "no"
        );
        assert_eq!(
            signature_cell(&verify::SignatureStatus::Unavailable("no gpgv".into())),
            "unavailable"
        );
        // The words the recommendation and --verbose use say what the cell cannot.
        assert_eq!(
            signature_words(&verify::SignatureStatus::Invalid("bad".into())),
            "unverified (invalid signature)"
        );
        assert_eq!(
            signature_words(&verify::SignatureStatus::Unavailable("no gpgv".into())),
            "unverified (signature unavailable)"
        );
    }

    fn row<'a>(
        url: &'a str,
        lag_secs: i64,
        signature: &'a verify::SignatureStatus,
        as_cell: Option<&'a str>,
    ) -> Row<'a> {
        Row {
            url,
            updates_date: 1_000,
            lag_secs,
            throughput_bps: 1_200_000,
            latency_ms: 1235,
            signature,
            as_cell: as_cell.map(str::to_string),
            security_lag: None,
            redirect_host: None,
        }
    }

    /// The same row with the two facts the `notes` column carries. `--verbose` prints them long; the
    /// table prints them compact, and both read the same row.
    fn noted<'a>(
        mut row: Row<'a>,
        security_lag: Option<i64>,
        redirect_host: Option<&'a str>,
    ) -> Row<'a> {
        row.security_lag = security_lag;
        row.redirect_host = redirect_host;
        row
    }

    /// The AS column exists only when the run has AS data for at least one row: a table of
    /// `unknown` would read as "every lookup answered", which is exactly the silent failure the
    /// column used to have. When it does exist, every row carries a cell, so the columns stay
    /// aligned and a mirror without an answer says `unknown` rather than writing nothing.
    #[test]
    fn the_ranked_table_renders_the_as_column_only_with_as_data() {
        let ok = verify::SignatureStatus::verified_for_test();
        let without = ranked_rows(&[
            row("http://a.example/ubuntu", 0, &ok, None),
            row("http://b.example/ubuntu", 600, &ok, None),
        ]);
        assert_eq!(
            without,
            [
                "   #  mirror                                         lag      speed     delay  sig"
                    .to_string(),
                "   1  http://a.example/ubuntu                         0m   1.2 MB/s   1235 ms  ok"
                    .to_string(),
                "   2  http://b.example/ubuntu                        10m   1.2 MB/s   1235 ms  ok"
                    .to_string(),
            ]
        );
        let with = ranked_rows(&[
            row("http://a.example/ubuntu", 0, &ok, Some("12929 same")),
            row("http://b.example/ubuntu", 0, &ok, None),
        ]);
        assert_eq!(
            with,
            [
                "   #  mirror                                         lag      speed     delay         as  sig"
                    .to_string(),
                "   1  http://a.example/ubuntu                         0m   1.2 MB/s   1235 ms 12929 same  ok"
                    .to_string(),
                "   2  http://b.example/ubuntu                         0m   1.2 MB/s   1235 ms    unknown  ok"
                    .to_string(),
            ]
        );
    }

    /// The `notes` column: the two facts SPEC.md asks the default output to show, compact, and only
    /// when a shown row has one — a run with no lag and no redirect gains no empty column.
    #[test]
    fn the_notes_column_carries_each_fact_and_only_when_present() {
        let ok = verify::SignatureStatus::verified_for_test();
        let plain = ranked_rows(&[row("http://a.example/ubuntu", 0, &ok, None)]);
        assert_eq!(
            plain[0],
            "   #  mirror                                         lag      speed     delay  sig"
        );
        assert_eq!(
            plain[1],
            "   1  http://a.example/ubuntu                         0m   1.2 MB/s   1235 ms  ok"
        );

        // A `-security` lag is informational only (it never rejects), but it is shown.
        let lagging = ranked_rows(&[noted(
            row("http://a.example/ubuntu", 0, &ok, None),
            Some(778 * 60),
            None,
        )]);
        assert_eq!(
            lagging[0],
            "   #  mirror                                         lag      speed     delay  sig  notes"
        );
        assert_eq!(
            lagging[1],
            "   1  http://a.example/ubuntu                         0m   1.2 MB/s   1235 ms  ok  sec:778m"
        );

        // A mirror that redirects to another host says which one, by default.
        let redirected = ranked_rows(&[noted(
            row("http://a.example/ubuntu", 0, &ok, None),
            None,
            Some("mirror2.example"),
        )]);
        assert_eq!(
            redirected[1],
            "   1  http://a.example/ubuntu                         0m   1.2 MB/s   1235 ms  ok  redir:mirror2.example"
        );

        // Both facts, one row; a zero lag is not one of them.
        let both = ranked_rows(&[noted(
            row("http://a.example/ubuntu", 0, &ok, None),
            Some(60),
            Some("mirror2.example"),
        )]);
        assert_eq!(
            both[1],
            "   1  http://a.example/ubuntu                         0m   1.2 MB/s   1235 ms  ok  sec:1m redir:mirror2.example"
        );
        let zero = ranked_rows(&[noted(
            row("http://a.example/ubuntu", 0, &ok, None),
            Some(0),
            None,
        )]);
        assert!(!zero[0].contains("notes") && !zero[1].contains("sec:"));
    }

    /// One long host cannot wrap the table: the cell keeps the fact and cuts the name, ends the cut
    /// in `CUT_MARKER` so it cannot be read as a host of that name, and `--verbose` is where the
    /// whole name is printed.
    #[test]
    fn a_long_redirect_host_is_cut_and_verbose_prints_it_whole() {
        let ok = verify::SignatureStatus::verified_for_test();
        let long = "a-very-long-mirror-host.example.org";
        let cell = noted(
            row("http://a.example/ubuntu", 0, &ok, None),
            Some(778 * 60),
            Some(long),
        );
        assert_eq!(
            note_cell(&cell),
            Some("sec:778m redir:a-very-long-mir~".to_string())
        );
        // The cut is bounded, so a run of long hostnames cannot widen the table without limit.
        let cut = note_cell(&noted(
            row("http://a.example/ubuntu", 0, &ok, None),
            None,
            Some(long),
        ))
        .expect("a redirect is a note");
        assert_eq!(cut.len(), "redir:".len() + REDIR_HOST_MAX);

        // A host of exactly the limit is not cut, so it carries no marker: the marker means "cut"
        // and nothing else.
        let exact = "a".repeat(REDIR_HOST_MAX);
        assert_eq!(
            note_cell(&noted(
                row("http://a.example/ubuntu", 0, &ok, None),
                None,
                Some(&exact),
            )),
            Some(format!("redir:{exact}"))
        );
        // A non-ASCII host is cut on a character boundary, so it cannot panic, and still fits.
        // A multi-byte character that is not a Lithuanian letter: the CI English-only guard
        // rejects the Lithuanian alphabet in `*.rs`, and this test only needs a char boundary.
        let wide = "→".repeat(REDIR_HOST_MAX + 2);
        let wide_cell = note_cell(&noted(
            row("http://a.example/ubuntu", 0, &ok, None),
            None,
            Some(&wide),
        ))
        .expect("a redirect is a note");
        assert_eq!(wide_cell.chars().count(), "redir:".len() + REDIR_HOST_MAX);
        assert!(wide_cell.ends_with(CUT_MARKER), "{wide_cell}");

        // `--verbose` prints the same fact long, from the same row data.
        let mut details = std::collections::HashMap::new();
        details.insert(
            "http://a.example/ubuntu".to_string(),
            Detail {
                country: "Lithuania".to_string(),
                security_date: Some(400),
                security_lag: Some(778 * 60),
                redirected_to: Some(long.to_string()),
            },
        );
        let verbose = detail_section(&[cell], &details, &std::collections::HashMap::new());
        assert!(
            verbose
                .iter()
                .any(|l| l == &format!("     redirects to {long}")),
            "{verbose:?}"
        );
        assert!(
            verbose.iter().any(|l| l.contains("778m lag")),
            "{verbose:?}"
        );
    }

    /// The `-updates` fixture: a real signed `InRelease`, so the probe path parses a real body.
    const UPDATES_INRELEASE: &[u8] =
        include_bytes!("../tests/fixtures/signed/noble-updates-InRelease");
    /// A `-security` body whose `Date` is 6 h 09 m 30 s behind the fixture's, so its lag is a real
    /// number instead of one the test wrote into a `Row`.
    const SECURITY_INRELEASE: &[u8] =
        b"-----BEGIN PGP SIGNED MESSAGE-----\nHash: SHA512\n\nOrigin: Ubuntu\nDate: Thu, 12 Nov 2026  2:19:32 UTC\n";

    /// One mirror answering every pocket a probe asks for. `http::Canned` always ends where it
    /// started, so a redirect (SPEC step 7) can only come from a fake that owns the `-updates`
    /// final URL.
    struct Mirror {
        security: Option<&'static [u8]>,
        sample: Vec<u8>,
        updates_final: Option<String>,
    }

    fn answered(body: &[u8], final_url: String) -> http::Fetched {
        http::Fetched {
            body: body.to_vec(),
            elapsed: std::time::Duration::from_millis(50),
            ttfb: std::time::Duration::from_millis(5),
            final_url,
        }
    }

    impl http::Fetch for Mirror {
        fn get(
            &self,
            url: &str,
            _max: u64,
            _range: Option<(u64, u64)>,
        ) -> Result<http::Fetched, http::HttpError> {
            if url.ends_with("-updates/InRelease") {
                let final_url = self
                    .updates_final
                    .clone()
                    .unwrap_or_else(|| url.to_string());
                return Ok(answered(UPDATES_INRELEASE, final_url));
            }
            if url.ends_with("-security/InRelease") {
                return match self.security {
                    Some(body) => Ok(answered(body, url.to_string())),
                    None => Err(http::HttpError::Status(404, url.to_string())),
                };
            }
            if url.ends_with("Packages.xz") {
                return Ok(answered(&self.sample, url.to_string()));
            }
            Err(http::HttpError::Status(404, url.to_string()))
        }
    }

    /// The rendered table for one probe result, through the wiring the run uses: `probe` →
    /// `detail_of` → `table_rows` → `ranked_rows`.
    fn table_for(outcome: &probe::Outcome, reference_security: Option<i64>) -> Vec<String> {
        let details = std::collections::HashMap::from([(
            outcome.url.clone(),
            detail_of(reference_security, "Lithuania", outcome),
        )]);
        let no_as: std::collections::HashMap<String, ResolvedAs> = std::collections::HashMap::new();
        let ranked = [fresh::Probed {
            url: outcome.url.clone(),
            updates_date: outcome.updates_date,
            verified_updates: outcome.verified_updates,
            lag_secs: 0,
            throughput_bps: outcome.throughput_bps,
            latency_ms: outcome.latency_ms,
            same_as: false,
            signature: outcome.updates_signature.clone(),
        }];
        ranked_rows(&table_rows(&ranked, &details, &no_as, None, 5))
    }

    /// The table's `notes` cell is filled from the run's own probe result, not from a `Row` a test
    /// built: a `-security` lag and a redirect both reach the rendered table here, and neither
    /// appears when the mirror served neither. Cutting this test off from the probe is what let the
    /// two facts regress silently, so it stays on the real path.
    #[test]
    fn the_notes_come_from_the_probe_result() {
        let mut sample = vec![0u8; 70_000];
        sample[..6].copy_from_slice(&[0xFD, b'7', b'z', b'X', b'Z', 0]);

        let redirecting = Mirror {
            security: Some(SECURITY_INRELEASE),
            sample: sample.clone(),
            updates_final: Some(
                "http://mirror2.example/ubuntu/dists/noble-updates/InRelease".to_string(),
            ),
        };
        let outcome = probe::probe(&redirecting, "http://a.example/ubuntu", "noble", "amd64")
            .expect("the fake answers every pocket the probe asks for");
        let security_date = fresh::release_date_tolerant(
            std::str::from_utf8(SECURITY_INRELEASE).expect("the body is ascii"),
        )
        .expect("-security has a parsable Date");
        let table = table_for(&outcome, Some(outcome.updates_date));
        assert!(
            table[0].contains("notes"),
            "the notes column is missing: {table:?}"
        );
        assert!(
            table[1].contains(&format!(
                "sec:{}m",
                (outcome.updates_date - security_date) / 60
            )),
            "the -security lag is missing: {table:?}"
        );
        assert!(
            table[1].contains("redir:mirror2.example"),
            "the redirect is missing: {table:?}"
        );

        // The same mirror without a `-security` pocket and without a redirect: no notes column, and
        // no token that would read as one.
        let quiet = Mirror {
            security: None,
            sample,
            updates_final: None,
        };
        let outcome = probe::probe(&quiet, "http://a.example/ubuntu", "noble", "amd64")
            .expect("the fake answers every pocket the probe asks for");
        assert_eq!(
            (outcome.security_date, outcome.redirected_to.clone()),
            (None, None)
        );
        let table = table_for(&outcome, Some(outcome.updates_date));
        assert!(!table[0].contains("notes"), "{table:?}");
        assert!(
            !table[1].contains("sec:") && !table[1].contains("redir:"),
            "{table:?}"
        );
    }

    /// The table is a terminal table, so its width is pinned: one column that grows turns every row
    /// into a wrapped row. Measured on the shapes the default report prints, each with the longest
    /// fact its cells can carry: a `--no-geo` row (no AS column — what the 44-column mirror column
    /// was sized for), a row with an AS cell and a note, and the widest row this report can produce
    /// (an `unavailable` signature and a full-length note). The arithmetic of the widest row is
    /// `6 prefix + 44 mirror + 6 lag + 11 speed + 10 delay + 10 as + 2 + 11 sig + 2 + 31 notes`; the
    /// redirect host is the only cell that can grow with input, and `REDIR_HOST_MAX` is what bounds
    /// it, so the `notes` column costs a fixed 33 columns instead of an unbounded amount. Reaching
    /// 80 for this row would mean dropping the full mirror URL (which the container matrix anchors
    /// on) or one of the numeric columns — a layout decision, not a note-column one.
    #[test]
    fn the_ranked_table_width_is_pinned() {
        let ok = verify::SignatureStatus::verified_for_test();
        let bad = verify::SignatureStatus::Unavailable("no gpgv".into());
        let url = "http://lt.archive.ubuntu.com/ubuntu";
        let longest_note = "a-very-long-mirror-host.example.org";
        let width = |rows: &[Row]| {
            ranked_rows(rows)
                .iter()
                .map(|line| line.chars().count())
                .max()
                .unwrap_or(0)
        };
        let no_as = width(&[row(url, 0, &ok, None)]);
        let a_note = width(&[noted(row(url, 0, &ok, None), Some(778 * 60), None)]);
        let with_as = width(&[row(url, 0, &ok, Some("12929 same"))]);
        let widest = width(&[noted(
            row(url, 0, &bad, Some("12929 diff")),
            Some(10_080 * 60),
            Some(longest_note),
        )]);
        assert_eq!((no_as, a_note, with_as, widest), (82, 91, 93, 136));
    }

    /// `--help` is where the table's shorthand is defined: without the two token names the `notes`
    /// column is unreadable, without the marker a cut host reads as a host of that name, and without
    /// the pointer a reader cannot find the cut host again.
    #[test]
    fn the_help_defines_the_notes_column() {
        assert!(HELP.contains("notes reads sec:<n>m"), "{HELP}");
        assert!(HELP.contains("redir:<host>"), "{HELP}");
        assert!(HELP.contains("cut and ends in ~"), "{HELP}");
        assert!(HELP.contains("--verbose prints it in full"), "{HELP}");
        // The rule that only a verified InRelease may authorize a write stays where it was.
        assert!(HELP.contains("only ok may authorize a write"), "{HELP}");
    }

    /// "N of M usable" counts what is shown against what survived, and the right-hand note says what
    /// the lag compares: without it `lag` has no baseline.
    #[test]
    fn the_ranked_title_counts_what_is_shown() {
        assert_eq!(
            ranked_title(3, 12, 7),
            "Ranked — 3 of 12 usable, 7 rejected            lag = -updates age vs freshest"
        );
    }

    /// The one line under the table: only when no row has AS data, and it names the run's own
    /// reason rather than a bare `unknown`.
    #[test]
    fn the_as_note_appears_only_when_no_row_has_as_data() {
        let ok = verify::SignatureStatus::verified_for_test();
        assert_eq!(as_note(&[], Some("whatever")), None);
        assert_eq!(
            as_note(
                &[row("http://a.example/ubuntu", 0, &ok, Some("12929 same"))],
                Some("x")
            ),
            None
        );
        assert_eq!(
            as_note(&[row("http://a.example/ubuntu", 0, &ok, None)], Some("Team Cymru names no AS for 192.0.2.1: no AS data for this address")),
            Some("note: AS data unavailable (Team Cymru names no AS for 192.0.2.1: no AS data for this address)".to_string())
        );
        // No reason was kept (nothing was ever asked): the note still says so, it does not vanish.
        assert_eq!(
            as_note(&[row("http://a.example/ubuntu", 0, &ok, None)], None),
            Some("note: AS data unavailable (no AS lookup answered)".to_string())
        );
    }

    /// The recommendation's reason carries every fact SPEC.md CLI asks for in it, and it names the
    /// AS the tie-break used: the bare match word cannot be checked against the table.
    #[test]
    fn the_recommendation_carries_country_as_lag_speed_and_signature() {
        let best = probed(MIRROR, verify::SignatureStatus::verified_for_test());
        assert_eq!(
            recommendation(&best, "Lithuania", Some("12929 same")),
            "  https://mirror.example/ubuntu/ — freshest verified, Lithuania, AS 12929 same, 0m lag, 1.0 MB/s, 10 ms"
        );
        // A mirror one generation behind says so instead of claiming to be the freshest.
        let mut lagging = best;
        lagging.lag_secs = 6 * 3600;
        assert_eq!(
            recommendation(&lagging, "Lithuania", None),
            "  https://mirror.example/ubuntu/ — closest verified, Lithuania, AS unknown, 360m lag, 1.0 MB/s, 10 ms"
        );
    }

    /// The rejected section is grouped: a count in the header, one line per reason, the numbers a
    /// stale mirror needs, and a reason that caught one host still prints its line.
    #[test]
    fn rejected_mirrors_are_grouped_by_reason_with_a_count() {
        let rejected = vec![
            Rejected {
                host: "mirror.b.example".into(),
                kind: Reject::NotFound,
                why:
                    "http://mirror.b.example/ubuntu/dists/noble-updates/InRelease answered HTTP 404"
                        .into(),
            },
            Rejected {
                host: "mirror.a.example".into(),
                kind: Reject::NotFound,
                why:
                    "http://mirror.a.example/ubuntu/dists/noble-updates/InRelease answered HTTP 404"
                        .into(),
            },
            Rejected {
                host: "mirror.c.example".into(),
                kind: Reject::NoDate,
                why: "no Date".into(),
            },
            Rejected {
                host: "mirror.e.example".into(),
                kind: Reject::Stale(162 * 3600),
                why: "stale: 162 h behind the freshest date; the limit is 24 h".into(),
            },
            Rejected {
                host: "mirror.d.example".into(),
                kind: Reject::Unreachable,
                why: "cannot reach http://mirror.d.example/ubuntu: connection refused".into(),
            },
        ];
        assert_eq!(
            rejected_section(&rejected),
            [
                "Rejected — 5",
                "  404               mirror.b.example, mirror.a.example",
                "  no Date header    mirror.c.example",
                "  stale             mirror.e.example (162 h)",
                "  unreachable       mirror.d.example",
            ]
        );
        // --verbose trades the groups for each mirror's own line and its full reason.
        let verbose = rejected_detail(&rejected);
        assert_eq!(verbose.len(), rejected.len() + 1);
        assert!(
            verbose.iter().any(|l| l.contains("mirror.a.example")
                && l.contains("404")
                && l.contains("answered HTTP 404")),
            "{verbose:?}"
        );
        assert!(
            verbose.iter().any(|l| l.contains("mirror.e.example")
                && l.contains("stale")
                && l.contains("162 h behind")),
            "{verbose:?}"
        );
    }

    /// The classifier reads the real error values the probe and the date parser produce, so a
    /// rewording there fails here instead of quietly becoming one big "other" group.
    #[test]
    fn a_probe_failure_is_grouped_by_what_went_wrong() {
        let url = "http://m.example/ubuntu/dists/noble-updates/InRelease".to_string();
        let cases = [
            (
                probe::ProbeError::Unreachable(http::HttpError::Status(404, url.clone())),
                "404",
            ),
            (
                probe::ProbeError::Unreachable(http::HttpError::Status(403, url.clone())),
                "HTTP 403",
            ),
            (
                probe::ProbeError::Unreachable(http::HttpError::Transport(
                    "connection refused".into(),
                    url.clone(),
                )),
                "unreachable",
            ),
            (
                probe::ProbeError::Unreachable(http::HttpError::TooLarge(524_288, url.clone())),
                "oversized InRelease",
            ),
            (
                probe::ProbeError::BadSample("not xz".into()),
                "no speed sample",
            ),
            (
                probe::ProbeError::BadUpdates(format!("{url}: {}", fresh::DateError::NoDateHeader)),
                "no Date header",
            ),
            (
                probe::ProbeError::BadUpdates(format!(
                    "{url}: {}",
                    fresh::DateError::NotCleartextSigned
                )),
                "no usable InRelease",
            ),
        ];
        for (error, label) in cases {
            assert_eq!(rejection(&error).label(), label, "{error}");
        }
        assert_eq!(Reject::Stale(3600).label(), "stale");
        assert_eq!(Reject::Crashed.label(), "probe crashed");
    }

    /// A URL with no host never reaches a resolver, so the failure reason is deterministic and the
    /// first one (by URL) is the one reported. No network is touched.
    #[test]
    fn an_as_lookup_failure_keeps_its_reason() {
        let (resolved, reason) = resolve_mirror_asns(&["not-a-url".to_string()], false);
        assert!(resolved.is_empty());
        let reason = reason.expect("a failed lookup must leave a reason");
        assert!(reason.contains("not-a-url"), "{reason}");
        assert!(reason.contains("no http(s) host"), "{reason}");
    }

    fn resolved(asn: &str) -> ResolvedAs {
        ResolvedAs {
            ip: "192.0.2.1".parse().expect("a literal address"),
            asn: asn.to_string(),
            name: None,
        }
    }

    /// `--verbose` names are a pass of their own, with a budget that only a name can spend: spent
    /// out, no name is even asked for and every AS answer stands exactly as it was. This is what
    /// keeps `--verbose` from changing which mirror has an AS, and therefore from changing a group
    /// or a rank.
    #[test]
    fn verbose_names_cannot_take_an_as_answer_with_them() {
        let mut answers = std::collections::HashMap::from([
            ("http://a.example".to_string(), resolved("12929")),
            ("http://b.example".to_string(), resolved("64500")),
        ]);
        let before = answers
            .iter()
            .map(|(url, resolved)| (url.clone(), resolved.asn.clone()))
            .collect::<Vec<_>>();

        // No name may be asked for once the name budget is gone.
        attach_names(&mut answers, Duration::ZERO, |_| {
            panic!("a name was asked for after the budget")
        });
        assert!(answers.values().all(|resolved| resolved.name.is_none()));

        // A generous budget fills both names, and a name that Cymru does not know leaves `None`.
        attach_names(&mut answers, Duration::from_secs(30), |asn| {
            (asn == "12929").then(|| "TELEKOM SLOVENIJE".to_string())
        });
        assert_eq!(
            answers["http://a.example"].name.as_deref(),
            Some("TELEKOM SLOVENIJE")
        );
        assert!(answers["http://b.example"].name.is_none());

        // Whatever the names did, every AS number is the one the AS pass recorded.
        for (url, asn) in before {
            assert_eq!(answers[&url].asn, asn, "{url}");
        }
    }

    /// `2026-11-12 08:29 UTC` is what a reader needs; the round trip pins it to the same civil
    /// arithmetic the freshness parser uses.
    #[test]
    fn utc_stamps_round_trip_through_the_date_parser() {
        let text = "Thu, 12 Nov 2026  8:29:02 UTC";
        let secs = fresh::parse_date(text).expect("the fixture parses");
        assert_eq!(utc_stamp(secs), "2026-11-12 08:29 UTC");
        assert_eq!(utc_stamp(0), "1970-01-01 00:00 UTC");
        assert_eq!(utc_stamp(1_700_000_000), "2023-11-14 22:13 UTC");
    }

    /// The Sources section is the inventory, one line per file and per URI, with the reason a URI
    /// is skipped (SPEC.md Apply). A host another country's Launchpad entry names is `rewrite`, a
    /// listed host at a path outside the listed tree is `skip`, while the PPA and `-security` lines
    /// keep their own reasons.
    #[test]
    fn the_sources_section_lists_every_file_entry_and_reason() {
        let apt = TempApt::new("sources-section");
        apt.write(
            "sources.list",
            "deb http://archive.ubuntu.com/ubuntu noble main\ndeb http://ubuntu.uib.no/archive/ noble-updates main\ndeb https://security.ubuntu.com/ubuntu noble-security main\ndeb https://ppa.example/ubuntu noble main\ndeb http://ubuntu.uib.no/debian trixie main\n",
        );
        apt.write("sources.list.d/empty.list", "# nothing active\n");
        std::fs::write(apt.0.join("sources.list.d/bad.list"), [0xff, 0xfe]).expect("fixture file");
        // `ubuntu.uib.no` is a Norwegian mirror listed at `/archive/`: a run in another country still
        // recognizes its Ubuntu tree, and only that tree.
        let hosts = vec![
            sources::MirrorRoot {
                host: "archive.ubuntu.com".to_string(),
                path: "/".to_string(),
            },
            sources::MirrorRoot {
                host: "mirror.example".to_string(),
                path: "/".to_string(),
            },
            sources::MirrorRoot {
                host: "ubuntu.uib.no".to_string(),
                path: "/archive/".to_string(),
            },
        ];
        let inventory = sources::inventory(&apt.0, &hosts, false);
        let lines = sources_section(&apt.0, &inventory);
        // The title names the root, so the list itself carries only paths relative to it.
        assert!(lines[0].starts_with("Sources"), "{lines:?}");
        assert_eq!(
            &lines[1..],
            [
                "  sources.list                              rewrite  line 1  archive.ubuntu.com",
                "  sources.list                              rewrite  line 2  ubuntu.uib.no",
                "  sources.list                              skip     line 3  security.ubuntu.com — security suite is never rewritten",
                "  sources.list                              skip     line 4  ppa.example — URI 'https://ppa.example/ubuntu' is not an official Ubuntu archive mirror",
                "  sources.list                              skip     line 5  ubuntu.uib.no — URI 'http://ubuntu.uib.no/debian' is not an official Ubuntu archive mirror",
                "  sources.list.d/bad.list                   skip     file is not valid UTF-8; skipped without changes",
                "  sources.list.d/empty.list                 no active entries",
            ]
        );
    }

    /// The header says which country won and where the answer came from; a run whose reference was
    /// unreachable says that instead of pretending it had one.
    #[test]
    fn the_header_names_the_release_country_and_reference() {
        assert_eq!(
            header_identity(
                "0.1.0",
                "Ubuntu 24.04 (noble)",
                "United States",
                "--country"
            ),
            "apt-scout 0.1.0 · Ubuntu 24.04 (noble) · country: United States (--country)"
        );
        let reference = probe::Reference {
            updates: 1_000,
            verified_updates: Some(vd("Thu, 01 Jan 1970 00:16:40 UTC")),
            updates_signature: verify::SignatureStatus::verified_for_test(),
            security: None,
        };
        assert_eq!(
            header_context(Some("AS12929"), "Launchpad", Some(&reference)),
            "server AS12929 · mirrors: Launchpad · reference: archive.ubuntu.com (verified)"
        );
        assert_eq!(
            header_context(None, "mirrors.ubuntu.com (unverified)", None),
            "server AS unknown · mirrors: mirrors.ubuntu.com (unverified) · reference: newest mirror date (archive.ubuntu.com unreachable)"
        );
    }

    /// `--verbose` detail: country, address, AS holder, both pocket dates with their lag, and the
    /// signature status. Unknown answers stay visible as unknown instead of disappearing.
    #[test]
    fn the_verbose_detail_names_dates_as_and_country() {
        let ok = verify::SignatureStatus::verified_for_test();
        let rows = vec![row("http://a.example/ubuntu", 0, &ok, Some("same"))];
        let mut resolved = std::collections::HashMap::new();
        resolved.insert(
            "http://a.example/ubuntu".to_string(),
            ResolvedAs {
                ip: "192.0.2.10".parse().expect("a fixture address"),
                asn: "AS12929".to_string(),
                name: Some("EXAMPLE-AS, US".to_string()),
            },
        );
        let mut details = std::collections::HashMap::new();
        details.insert(
            "http://a.example/ubuntu".to_string(),
            Detail {
                country: "United States".to_string(),
                security_date: Some(1_000 - 600),
                security_lag: Some(600),
                redirected_to: Some("mirror2.example".to_string()),
            },
        );
        assert_eq!(
            detail_section(&rows, &details, &resolved),
            [
                "Detail",
                "  1  http://a.example/ubuntu",
                "     country: United States · ip: 192.0.2.10 · as: AS12929 (EXAMPLE-AS, US)",
                "     -updates: 1970-01-01 00:16 UTC, 0m lag · -security: 1970-01-01 00:06 UTC, 10m lag",
                "     signature: verified",
                "     redirects to mirror2.example",
            ]
        );
        // Nothing looked up: the three answers say unknown, and the section still prints.
        assert_eq!(
            detail_section(
                &rows,
                &std::collections::HashMap::new(),
                &std::collections::HashMap::new()
            ),
            [
                "Detail",
                "  1  http://a.example/ubuntu",
                "     country: unknown · ip: unknown · as: unknown",
                "     -updates: 1970-01-01 00:16 UTC, 0m lag · -security: no InRelease",
                "     signature: verified",
            ]
        );
    }

    /// `--verbose` is a display flag: it changes no decision and repeats in the `sudo` line.
    #[test]
    fn verbose_is_parsed_and_repeats_in_the_sudo_line() {
        let args = parse_args_from(
            ["--verbose", "--country", "LT"]
                .into_iter()
                .map(str::to_string),
        )
        .unwrap();
        assert!(args.verbose);
        assert_eq!(sudo_command(&args), "sudo apt-scout --country LT --verbose");
        let plain = parse_args_from(std::iter::empty()).unwrap();
        assert!(!plain.verbose);
    }

    /// The whole `--list` block, section by section, from fixture data — the shape the human
    /// approved. It pins the section order and the column widths together; the container matrix is
    /// what proves a live run prints the same block.
    #[test]
    fn the_list_report_reads_as_one_block() {
        let apt = TempApt::new("report");
        apt.write(
            "sources.list.d/ubuntu.sources",
            "# noble\nTypes: deb\nURIs: http://archive.ubuntu.com/ubuntu\nSuites: noble noble-updates\nComponents: main\n\nTypes: deb\nURIs: http://security.ubuntu.com/ubuntu\nSuites: noble-security\nComponents: main\n",
        );
        let hosts = listed(&["archive.ubuntu.com", "mirror.arizona.edu"]);
        let inventory = sources::inventory(&apt.0, &hosts, false);
        let selected = MIRROR;
        let plan = build_plan(
            &apt.0,
            &inventory,
            &hosts,
            &probed(selected, verify::SignatureStatus::verified_for_test()),
            plan::Platform::Ubuntu,
        )
        .expect("the archive stanza is rewritable")
        .plan
        .expect("one file changes");
        let verified = verify::SignatureStatus::verified_for_test();
        let unverified = verify::SignatureStatus::Unavailable("no gpgv in this image".into());
        let rows = vec![
            row(selected, 0, &verified, Some("12929 same")),
            noted(
                row(
                    "http://mirrors.usinternet.com/ubuntu",
                    0,
                    &unverified,
                    Some("3356 diff"),
                ),
                Some(778 * 60),
                Some("mirror2.example"),
            ),
        ];
        let rejected = vec![
            Rejected {
                host: "mirror.lostpacket.org".into(),
                kind: Reject::NotFound,
                why: "answered HTTP 404".into(),
            },
            Rejected {
                host: "mirror.pit.teraswitch.com".into(),
                kind: Reject::Stale(162 * 3600),
                why: "stale: 162 h behind".into(),
            },
        ];
        let mut resolved = std::collections::HashMap::new();
        resolved.insert(
            selected.to_string(),
            ResolvedAs {
                ip: "192.0.2.10".parse().expect("a fixture address"),
                asn: "AS12929".to_string(),
                name: Some("ARIZONA, US".to_string()),
            },
        );
        let mut report = Report::default();
        report.line(header_identity(
            "0.1.0",
            "Ubuntu 24.04 (noble)",
            "United States",
            "--country",
        ));
        report.block(sources_section(&apt.0, &inventory));
        report.block(vec![header_context(Some("AS12929"), "Launchpad", None)]);
        let mut ranked_lines = vec![ranked_title(rows.len(), rows.len(), rejected.len())];
        ranked_lines.extend(ranked_rows(&rows));
        ranked_lines.extend(as_note(&rows, None));
        report.block(ranked_lines);
        report.block(vec![
            "Recommendation".to_string(),
            recommendation(
                &probed(selected, verify::SignatureStatus::verified_for_test()),
                "United States",
                Some("12929 same"),
            ),
        ]);
        let mut planned = vec![titled("Planned change", "nothing is written with --list")];
        planned.extend(plan_changes(&apt.0, &inventory, &plan));
        report.block(planned);
        report.block(rejected_section(&rejected));
        let expected = vec![
            "apt-scout 0.1.0 · Ubuntu 24.04 (noble) · country: United States (--country)".to_string(),
            String::new(),
            format!("{:<45}  {}", "Sources", apt.0.display()),
            "  sources.list.d/ubuntu.sources             rewrite  line 2  archive.ubuntu.com"
                .to_string(),
            "  sources.list.d/ubuntu.sources             skip     line 7  security.ubuntu.com — security suite is never rewritten"
                .to_string(),
            String::new(),
            "server AS12929 · mirrors: Launchpad · reference: newest mirror date (archive.ubuntu.com unreachable)"
                .to_string(),
            String::new(),
            "Ranked — 2 of 2 usable, 2 rejected             lag = -updates age vs freshest"
                .to_string(),
            "   #  mirror                                         lag      speed     delay         as  sig  notes"
                .to_string(),
            "   1  https://mirror.example/ubuntu/                  0m   1.2 MB/s   1235 ms 12929 same  ok"
                .to_string(),
            "   2  http://mirrors.usinternet.com/ubuntu            0m   1.2 MB/s   1235 ms  3356 diff  unavailable  sec:778m redir:mirror2.example"
                .to_string(),
            String::new(),
            "Recommendation".to_string(),
            "  https://mirror.example/ubuntu/ — freshest verified, United States, AS 12929 same, 0m lag, 1.0 MB/s, 10 ms"
                .to_string(),
            String::new(),
            "Planned change                                 nothing is written with --list"
                .to_string(),
            "  sources.list.d/ubuntu.sources:2".to_string(),
            "    - http://archive.ubuntu.com/ubuntu".to_string(),
            "    + https://mirror.example/ubuntu/".to_string(),
            String::new(),
            "Rejected — 2".to_string(),
            "  404               mirror.lostpacket.org".to_string(),
            "  stale             mirror.pit.teraswitch.com (162 h)".to_string(),
        ];
        assert_eq!(report.0, expected);
    }

    /// The AS map is keyed by `as_key`, which is the spelling the probe reports. A mirror listed
    /// with a trailing slash must still find its answer, or the column reads `unknown` for every
    /// row while the lookup succeeded.
    #[test]
    fn the_as_column_finds_a_mirror_listed_with_a_trailing_slash() {
        let mut resolved = std::collections::HashMap::new();
        resolved.insert(
            as_key("http://a.example/ubuntu/").to_string(),
            ResolvedAs {
                ip: "192.0.2.10".parse().expect("a fixture address"),
                asn: "AS64500".to_string(),
                name: None,
            },
        );
        assert_eq!(
            as_key("http://a.example/ubuntu/"),
            "http://a.example/ubuntu"
        );
        assert_eq!(
            as_cell(&resolved, Some("AS64500"), "http://a.example/ubuntu"),
            Some("64500 same".to_string())
        );
        assert_eq!(
            as_cell(&resolved, Some("AS64500"), "http://a.example/ubuntu/"),
            Some("64500 same".to_string())
        );
        assert_eq!(
            as_cell(&resolved, Some("AS64501"), "http://a.example/ubuntu"),
            Some("64500 diff".to_string())
        );
        // No answer for that host at all: no column value, so the note explains the gap.
        assert_eq!(
            as_cell(&resolved, Some("AS64500"), "http://b.example/ubuntu"),
            None
        );
        // The lookup answered but the server's own AS is unknown: the ASN is still a fact.
        assert_eq!(
            as_cell(&resolved, None, "http://a.example/ubuntu"),
            Some("64500 unknown".to_string())
        );
    }
}
