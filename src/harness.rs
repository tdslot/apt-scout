//! The write harness: the case runner `scripts/docker-test.sh --write-only` drives.
//!
//! Compiled only into a binary built with `--cfg apt_scout_write_harness`, so a plain release
//! binary carries neither this code nor the `__write-harness` option (the shell's W2 case asserts
//! exactly that). One process handles one case, against the real `/etc/apt` of a disposable
//! container, and prints exactly one machine-readable line.
//!
//! It reads no environment variable and takes no path: every path is compiled in, and every date
//! comes from `tests/fixtures/signed/noble-updates-InRelease`, so two runs in one container see the
//! same backup id and the same bytes.

use std::fs;
use std::io::{self, Write as _};
use std::os::unix::fs::MetadataExt;
use std::path::Path;
use std::process::ExitCode;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread;
use std::time::{Duration, Instant};

use crate::apply::{self, APT_DIR, ApplyError};
use crate::digest::sha256;
use crate::fresh::{self, Probed, SignedDate};
use crate::plan::{FileState, Plan, PlanError, PlannedWrite, Platform, SourceName, Target};
use crate::verify::{SignatureStatus, VerifiedDate};

/// The `now_secs` every case hands `apply::apply`: 2026-11-12T00:00:00Z.
///
/// A wall clock would make the backup directory's name differ between two runs in one container,
/// so a second run would either collide there or depend on elapsed time. Here
/// `manifest::BackupId::from_secs` always yields `20261112T000000Z`, a name a case can spell.
const NOW_SECS: u64 = 1_794_441_600;

/// The fixture: a real `noble-updates` `InRelease`, and the proof of the mirror below.
const FIXTURE: &[u8] = include_bytes!("../tests/fixtures/signed/noble-updates-InRelease");
/// The fixture's `Date:` line, verbatim; `stale_inrelease` replaces exactly this.
const FIXTURE_DATE_LINE: &str = "Date: Thu, 12 Nov 2026  8:29:02 UTC";
/// The same date one second past `fresh::MAX_LAG_SECS`, which `fresh::is_stale` must refuse.
const STALE_DATE_LINE: &str = "Date: Fri, 13 Nov 2026 08:29:03 UTC";

/// The mirror every case plans against: unresolvable, so no case can depend on it answering.
const MIRROR_URL: &str = "http://mirror.example.invalid/ubuntu";
/// The host the rewrite replaces in the target file, and what it replaces it with.
const ARCHIVE_HOST: &[u8] = b"archive.ubuntu.com";
const MIRROR_HOST: &[u8] = b"mirror.example.invalid";
/// The line appended when the target does not name the archive host at all.
const APPENDED_LINE: &[u8] = b"# apt-scout harness\n";

/// The one file the `apply-pop` case creates. A case whose create target was also its replace
/// target would build a plan `Plan::validate` has to refuse, so this name is never selected.
const EXCLUDED_NAME: &str = "system-security.sources";

/// Every case this build implements; the dispatch in `main` has one arm per entry.
const CASES: [&str; 3] = ["selftest", "apply", "apply-pause"];

/// How many cases the harness contract defines, `selftest` included.
///
/// Printed by `selftest`, which is the one line the shell can pin before it runs anything. It is
/// the count in the contract, not the number of arms compiled into one unit: grow the arms, leave
/// this alone.
const CONTRACT_CASES: usize = 9;

/// The pause protocol's paths, compiled in; no argument and no environment can move them.
const PAUSE_DIR: &str = "/tmp/apt-scout-harness";
const PAUSE_MARKER: &str = "/tmp/apt-scout-harness/paused";
const PAUSE_RELEASE: &str = "/tmp/apt-scout-harness/go";
/// How long a pause waits for its release, and how often it looks. The shell waits 15 s for the
/// marker, so a harness that never reaches the pause fails there instead of hanging the run.
const PAUSE_LIMIT: Duration = Duration::from_secs(30);
const PAUSE_POLL: Duration = Duration::from_millis(20);
/// The exit code of a pause that was never released: no other path in this process produces it.
const PAUSE_FAILED: u8 = 97;

/// Set by the `apply-pause` case only, before `apply::apply` runs.
static PAUSE_ENABLED: AtomicBool = AtomicBool::new(false);

/// Run one case, or return `None` when this is not a harness invocation.
///
/// `None` is what keeps a plain build honest: there `__write-harness` is an ordinary argument and
/// fails as an unknown option (W2), so the harness cannot be reached by accident.
pub fn main(args: Vec<String>) -> Option<ExitCode> {
    let mut args = args.iter();
    if args.next().map(String::as_str) != Some("__write-harness") {
        return None;
    }
    let name = args.next().map(String::as_str).unwrap_or("");
    let outcome = match name {
        "selftest" => selftest(),
        "apply" => write_case(Case::Apply),
        "apply-pause" => write_case(Case::ApplyPause),
        _ => {
            eprintln!(
                "error: unknown write-harness case '{name}'; cases are {}",
                CASES.join(", ")
            );
            return Some(ExitCode::from(2));
        }
    };
    Some(report(outcome))
}

/// Which case is running: the inputs its plan and its baseline depend on.
///
/// A variant per case that needs a plan. A case that differs only in a fixture, a baseline or a
/// write list gets its own variant when it lands, and one dispatch arm beside the ones above.
#[derive(Clone, Copy)]
enum Case {
    Apply,
    ApplyPause,
}

impl Case {
    /// The platform this case's plan claims.
    ///
    /// A case never reads `/etc/os-release`: `apply` takes a plan and never looks at the
    /// distribution, so a Pop case needs a platform value and nothing else.
    fn platform(self) -> Platform {
        match self {
            Case::Apply | Case::ApplyPause => Platform::Ubuntu,
        }
    }
}

/// Why a case produced no `OK` line.
enum Fault {
    /// The harness could not build the case at all: fixture, image layout, or IO.
    Harness(String),
    /// A broken harness constant. A `SELFTEST …` line, so a moved fixture fails loudly.
    Selftest(String),
    /// A write the plan itself refused, before any disk operation.
    Plan(PlanError),
    /// The writer refused the run, with its own exit code.
    Apply(ApplyError),
}

/// Print the one line this fault reports, on the stream the shell reads it from, and return the
/// process's exit code.
fn report(outcome: Result<String, Fault>) -> ExitCode {
    match outcome {
        Ok(line) => {
            println!("{line}");
            ExitCode::SUCCESS
        }
        Err(Fault::Harness(message)) => {
            eprintln!("error: {message}");
            ExitCode::from(1)
        }
        Err(Fault::Selftest(line)) => {
            println!("{line}");
            ExitCode::from(1)
        }
        Err(Fault::Plan(err)) => {
            println!("ERR Plan: {err}");
            ExitCode::from(1)
        }
        Err(Fault::Apply(err)) => {
            let code = err.exit_code();
            println!("ERR {}: {err}", token(&err));
            ExitCode::from(code)
        }
    }
}

/// The token that names this refusal in the one machine-readable line.
fn token(err: &ApplyError) -> &'static str {
    match err {
        ApplyError::NotRoot => "NotRoot",
        ApplyError::Plan(_) => "Plan",
        ApplyError::NoBaseline => "NoBaseline",
        ApplyError::Stale { .. } => "Stale",
        ApplyError::Read { .. } => "Read",
        ApplyError::TooLarge { .. } => "TooLarge",
        ApplyError::Target { .. } => "Target",
        ApplyError::Owner { .. } => "Owner",
        ApplyError::Changed { .. } => "Changed",
        ApplyError::Exists { .. } => "Exists",
        ApplyError::Backup { .. } => "Backup",
        ApplyError::Write { .. } => "Write",
        ApplyError::Time { .. } => "Time",
    }
}

/// One case that writes: guard, plan, baseline, apply.
fn write_case(case: Case) -> Result<String, Fault> {
    require_container()?;
    // Armed before the plan is built, so the hook inside `apply` sees it.
    if matches!(case, Case::ApplyPause) {
        PAUSE_ENABLED.store(true, Ordering::SeqCst);
    }
    let plan = plan_for(case)?;
    let baseline = baseline()?;
    let outcome = apply::apply(&plan, Some(&baseline), NOW_SECS).map_err(Fault::Apply)?;
    Ok(format!(
        "OK backup={} wrote={}",
        outcome.backup_id.as_str(),
        written_of(&outcome.written)
    ))
}

/// The targets one run wrote, relative to `/etc/apt`, comma-separated and in write order.
fn written_of(targets: &[Target]) -> String {
    targets
        .iter()
        .map(|t| t.relative().display().to_string())
        .collect::<Vec<String>>()
        .join(",")
}

/// The one case that touches no file.
fn selftest() -> Result<String, Fault> {
    let proof = proof_date()?;
    let stale = stale_date()?;
    let stale_lag = fresh::lag(stale.seconds(), proof.seconds());
    if !fresh::is_stale(stale_lag) {
        return Err(Fault::Selftest(format!(
            "SELFTEST stale constant is {stale_lag} seconds, needs more than {}",
            fresh::MAX_LAG_SECS
        )));
    }
    Ok(format!(
        "HARNESS 1 cases={CONTRACT_CASES} proof={} baseline={} stale=yes",
        proof.seconds(),
        baseline()?.seconds()
    ))
}

/// Refuse every write case outside a disposable container.
///
/// This is the only thing between a mistaken host run and the host's `/etc/apt`, so it comes
/// before `plan_for` reads anything: a refusal here means no apt file was even opened.
fn require_container() -> Result<(), Fault> {
    if Path::new("/.dockerenv").exists() || Path::new("/run/.containerenv").exists() {
        return Ok(());
    }
    Err(Fault::Harness(
        "refusing to run a write case outside a container: /.dockerenv and /run/.containerenv \
         are both absent. Write cases exist only in a disposable container, so nothing was written."
            .to_string(),
    ))
}

/// Build this case's plan from the apt configuration on disk. No network, no parameters.
fn plan_for(case: Case) -> Result<Plan, Fault> {
    let target = replace_target()?;
    let original = original_of(&target)?;
    let new_bytes = rewritten(&original.bytes);
    let digest = sha256(&new_bytes);
    let proof = proof_date()?;
    let probed = Probed {
        url: MIRROR_URL.to_string(),
        updates_date: proof.seconds(),
        signed_updates_date: Some(proof),
        lag_secs: 0,
        throughput_bps: 1,
        latency_ms: 1,
        same_as: false,
        signature: SignatureStatus::Verified,
    };
    let writes = vec![PlannedWrite {
        target,
        new_bytes,
        digest,
        original: Some(original),
        create_new: false,
    }];
    Plan::for_apply(&probed, case.platform(), writes).map_err(Fault::Plan)
}

/// The file this case replaces, by the one selection rule every case shares.
///
/// `/etc/apt/sources.list` when it exists at all (any type); otherwise the first `.sources` entry
/// of `/etc/apt/sources.list.d` in byte order, minus `EXCLUDED_NAME`. Symlinks are followed on
/// purpose, here and at `sources.list.d` alike: refusing a symlinked target is `apply`'s job, and
/// a case asserts exactly that refusal, so this function must not pre-empt it.
fn replace_target() -> Result<Target, Fault> {
    let apt = Path::new(APT_DIR);
    if fs::symlink_metadata(apt.join("sources.list")).is_ok() {
        return Ok(Target::SourcesList);
    }
    let dir = apt.join("sources.list.d");
    let entries = match fs::read_dir(&dir) {
        Ok(entries) => entries,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Err(no_source_file()),
        Err(e) => return Err(listing_failed(&dir, &e)),
    };
    let mut names = Vec::new();
    for entry in entries {
        let entry = entry.map_err(|e| listing_failed(&dir, &e))?;
        if let Some(name) = entry.file_name().to_str() {
            names.push(name.to_string());
        }
    }
    names.sort();
    for name in names {
        if !name.ends_with(".sources") || name == EXCLUDED_NAME {
            continue;
        }
        // A name `SourceName::parse` refuses cannot be a target; that is a plan refusal, not a
        // missing file, so it is reported as one.
        return SourceName::parse(&name)
            .map(Target::SourcesListD)
            .map_err(Fault::Plan);
    }
    Err(no_source_file())
}

fn no_source_file() -> Fault {
    Fault::Harness("no apt source file to plan against".to_string())
}

fn listing_failed(dir: &Path, err: &io::Error) -> Fault {
    Fault::Harness(format!("cannot list {}: {err}", dir.display()))
}

/// The state `apply` compares the plan against, read the way `apply` reads it.
///
/// `fs::metadata` follows a symlink and `fs::read` follows it too, which is what `apply`'s own
/// capture compares against once it has taken its `lstat`.
fn original_of(target: &Target) -> Result<FileState, Fault> {
    let path = target.from_under(Path::new(APT_DIR));
    let failed = |e: io::Error| Fault::Harness(format!("cannot read {}: {e}", path.display()));
    let meta = fs::metadata(&path).map_err(failed)?;
    let bytes = fs::read(&path).map_err(failed)?;
    Ok(FileState {
        dev: meta.dev(),
        ino: meta.ino(),
        mode: meta.mode() & 0o7777,
        uid: meta.uid(),
        gid: meta.gid(),
        bytes,
    })
}

/// The bytes this case writes: the archive host replaced once, or one appended comment when the
/// file does not name it. Deterministic, so the digest is too.
fn rewritten(bytes: &[u8]) -> Vec<u8> {
    let Some(at) = bytes
        .windows(ARCHIVE_HOST.len())
        .position(|w| w == ARCHIVE_HOST)
    else {
        let mut out = bytes.to_vec();
        if !out.ends_with(b"\n") {
            out.push(b'\n');
        }
        out.extend_from_slice(APPENDED_LINE);
        return out;
    };
    let mut out = Vec::with_capacity(bytes.len());
    out.extend_from_slice(&bytes[..at]);
    out.extend_from_slice(MIRROR_HOST);
    out.extend_from_slice(&bytes[at + ARCHIVE_HOST.len()..]);
    out
}

/// The fixture's text.
fn fixture_text() -> Result<&'static str, Fault> {
    std::str::from_utf8(FIXTURE)
        .map_err(|e| Fault::Harness(format!("harness fixture is unusable: it is not UTF-8: {e}")))
}

/// The strict parse of one `InRelease` body, as the freshness baseline needs it.
fn fixture_date(text: &str) -> Result<SignedDate, Fault> {
    fresh::release_date(text)
        .map_err(|e| Fault::Harness(format!("harness fixture is unusable: {e}")))
}

/// The mirror's proof, and the baseline of every case but `stale`: `1_794_472_142`.
fn proof_date() -> Result<SignedDate, Fault> {
    fixture_date(fixture_text()?)
}

/// The fixture with its `Date:` line moved one second past `fresh::MAX_LAG_SECS`.
fn stale_inrelease() -> Result<String, Fault> {
    let text = fixture_text()?;
    if !text.contains(FIXTURE_DATE_LINE) {
        return Err(Fault::Harness(format!(
            "harness fixture is unusable: it has no '{FIXTURE_DATE_LINE}' line to replace"
        )));
    }
    Ok(text.replacen(FIXTURE_DATE_LINE, STALE_DATE_LINE, 1))
}

/// The stale body's date: `1_794_558_543`, exactly `MAX_LAG_SECS + 1` after the proof.
fn stale_date() -> Result<SignedDate, Fault> {
    fixture_date(&stale_inrelease()?)
}

/// The run's verified baseline: a `Verified` status and the strict-parsed proof date.
fn baseline() -> Result<VerifiedDate, Fault> {
    let Some(baseline) = SignatureStatus::Verified.verified_date(proof_date()?) else {
        return Err(Fault::Harness(
            "harness baseline is unusable: a verified status and a strict-parsed date always yield \
             a verified date"
                .to_string(),
        ));
    };
    Ok(baseline)
}

/// The hook `apply::run` calls between its last validation and its first sweep.
///
/// Absent from every non-harness build (`apply` gates the call, not this function), and a no-op
/// here until the `apply-pause` case arms it. It reads no environment: both paths are compiled in.
pub fn pause_after_validation() {
    if !PAUSE_ENABLED.load(Ordering::SeqCst) {
        return;
    }
    if let Err(e) = pause() {
        eprintln!("error: the pause could not be taken: {e}; the case fails");
        std::process::exit(i32::from(PAUSE_FAILED));
    }
}

/// Land the marker the shell waits for, then hold the write zone until it is released.
///
/// The wait is bounded and monotonic: a release that never comes exits with a code no other path
/// produces, so a broken handshake fails its case instead of passing it on a substituted file.
fn pause() -> io::Result<()> {
    fs::create_dir_all(PAUSE_DIR)?;
    let mut marker = fs::File::create(PAUSE_MARKER)?;
    marker.write_all(b"paused\n")?;
    marker.sync_all()?;
    let started = Instant::now();
    while !Path::new(PAUSE_RELEASE).exists() {
        if started.elapsed() >= PAUSE_LIMIT {
            eprintln!("error: the pause was never released after 30 s; the case fails");
            std::process::exit(i32::from(PAUSE_FAILED));
        }
        thread::sleep(PAUSE_POLL);
    }
    Ok(())
}
