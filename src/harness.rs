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

/// What the `apply-pop` case puts in the file it creates: the security suite the plan restores.
/// The creation's mode is the writer's own (`apply::FILE_MODE`), never a plan field.
const SECURITY_SOURCES: &[u8] =
    b"Types: deb\nURIs: http://archive.ubuntu.com/ubuntu\nSuites: noble-security\nComponents: main universe\n";

/// Every case this build implements; the dispatch in `main` has one arm per entry.
const CASES: [&str; 9] = [
    "selftest",
    "apply",
    "apply-pop",
    "apply-pause",
    "hostile-names",
    "no-baseline",
    "stale",
    "digest-mismatch",
    "prefixed",
];

/// The names `hostile-names` hands the real parsers: the last one must be accepted, the rest not.
///
/// One accepted name is in the list on purpose, so the `UNDER` half of the printed contract is
/// exercised by a case instead of being a format nothing prints. The 65-byte entry is one byte
/// past `SourceName::parse`'s 64-byte cap, so length is the only rule it can fail; the empty entry
/// prints `REFUSED ` and is a refused name, not a missing one.
const HOSTILE_NAMES: [&str; 10] = [
    "..",
    "../../etc/passwd",
    "/etc/shadow",
    "foo.txt",
    ".list",
    ".hidden.sources",
    "a/b.list",
    "",
    "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa.sources",
    "custom.sources",
];

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
        "apply-pop" => write_case(Case::ApplyPop),
        "apply-pause" => write_case(Case::ApplyPause),
        "hostile-names" => hostile_names(),
        "no-baseline" => write_case(Case::NoBaseline),
        "stale" => write_case(Case::Stale),
        "digest-mismatch" => digest_mismatch(),
        "prefixed" => prefixed(),
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
/// A variant per case that builds a plan through `plan_for`. A case that reads no apt file
/// (`selftest`), writes no plan (`hostile-names`) or prints before it refuses (`digest-mismatch`,
/// `prefixed`) has none, and each of those is a function of its own.
#[derive(Clone, Copy)]
enum Case {
    Apply,
    ApplyPause,
    /// The replace write plus the one creation a stock Pop!_OS install needs.
    ApplyPop,
    /// `apply` with no baseline at all.
    NoBaseline,
    /// `apply` with a baseline `fresh::MAX_LAG_SECS + 1` ahead of the mirror's proof.
    Stale,
    /// `apply`'s write, carrying a digest that does not describe its bytes.
    DigestMismatch,
}

impl Case {
    /// The platform this case's plan claims.
    ///
    /// A case never reads `/etc/os-release`: `apply` takes a plan and never looks at the
    /// distribution, so a Pop case needs a platform value and nothing else.
    fn platform(self) -> Platform {
        match self {
            // The only case whose plan creates a file: `plan::validate` refuses a creation on any
            // other platform, so this value is what makes the two-write plan representable.
            Case::ApplyPop => Platform::PopOs,
            _ => Platform::Ubuntu,
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
    let baseline = baseline_of(case)?;
    let outcome = apply::apply(&plan, baseline.as_ref(), NOW_SECS).map_err(Fault::Apply)?;
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

/// Print every listed name's verdict, taken from the real parsers and the real path builder.
///
/// The spelling a name has as a target is the writer's own (`sources.list.d/<name>`), and the path
/// an accepted one prints is the one `Target::relative` builds, never the input echoed back with a
/// prefix. A runtime demonstration, not a regression guard: `plan.rs`'s enum makes an out-of-root
/// target unrepresentable, so no case can detect a writer that opened a path outside the two
/// roots — the type is the control. Nothing here writes.
fn hostile_names() -> Result<String, Fault> {
    require_container()?;
    let mut lines = Vec::with_capacity(HOSTILE_NAMES.len());
    for name in HOSTILE_NAMES {
        match Target::parse(&format!("sources.list.d/{name}")) {
            Ok(target) => lines.push(format!("UNDER {}", target.relative().display())),
            Err(_) => lines.push(format!("REFUSED {name}")),
        }
    }
    Ok(lines.join("\n"))
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
    let proof = proof_date()?;
    let probed = probed_from(proof.seconds(), Some(proof));
    Plan::for_apply(&probed, case.platform(), writes_for(case)?).map_err(Fault::Plan)
}

/// The mirror every case plans against. Its URL is unresolvable, so no case can depend on it
/// answering, and `signed` is the date the proof was taken from, or `None` when the only date the
/// mirror showed was outside the signed region.
fn probed_from(updates_date: i64, signed: Option<SignedDate>) -> Probed {
    Probed {
        url: MIRROR_URL.to_string(),
        updates_date,
        signed_updates_date: signed,
        lag_secs: 0,
        throughput_bps: 1,
        latency_ms: 1,
        same_as: false,
        signature: SignatureStatus::verified_for_test(),
    }
}

/// Every write this case's plan carries, in the order `apply` reaches them.
fn writes_for(case: Case) -> Result<Vec<PlannedWrite>, Fault> {
    let target = replace_target()?;
    let original = original_of(&target)?;
    let new_bytes = rewritten(&original.bytes);
    // `digest-mismatch` records the digest of the bytes being *replaced*: a real digest of the
    // wrong content, which is the confusion the digest rule exists to refuse. No other case can
    // reach it, because the rewrite always changes the bytes it describes.
    let digest = match case {
        Case::DigestMismatch => sha256(&original.bytes),
        _ => sha256(&new_bytes),
    };
    let replace = PlannedWrite {
        target,
        new_bytes,
        digest,
        original: Some(original),
        create_new: false,
    };
    match case {
        // Creations run first, so the security source is the plan's first write.
        Case::ApplyPop => Ok(vec![security_sources()?, replace]),
        _ => Ok(vec![replace]),
    }
}

/// The write that repairs a Pop!_OS install: `system-security.sources`, absent and unused.
///
/// `original: None` is a claim about the disk, not a bug: `capture` is what finds a file where the
/// plan says there is none, so a pre-existing or symlinked create target is refused by the writer
/// with its own variant instead of being pre-empted here.
fn security_sources() -> Result<PlannedWrite, Fault> {
    let name = SourceName::parse(EXCLUDED_NAME).map_err(Fault::Plan)?;
    let new_bytes = SECURITY_SOURCES.to_vec();
    Ok(PlannedWrite {
        target: Target::SourcesListD(name),
        digest: sha256(&new_bytes),
        new_bytes,
        original: None,
        create_new: true,
    })
}

/// The one case that aims `plan::validate`'s digest rule at a plan that breaks it.
///
/// `Plan::for_apply` is the only constructor and the fields are private, so no writer code can hold
/// a plan whose digest is not its bytes: such a plan is refused where it is built, before `apply`
/// is called and before any disk operation.
fn digest_mismatch() -> Result<String, Fault> {
    require_container()?;
    match plan_for(Case::DigestMismatch) {
        Err(fault) => Err(fault),
        // Unreachable unless the construction stops checking the bytes against the digest.
        Ok(_) => Err(Fault::Harness(
            "a plan whose recorded digest is not the digest of its bytes was accepted, so nothing \
             refuses a write of one content behind a proof issued for another"
                .to_string(),
        )),
    }
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

/// The fixture with a `Date:` line the signature does not cover placed before the armour.
///
/// The line is the same one `stale` rewrites the body with, so the forgery claims to be a day and a
/// second newer than the body it is glued to: a reader that took it would call the mirror fresher
/// than the mirror's own signature says it is.
fn prefixed_body() -> Result<String, Fault> {
    Ok(format!("{STALE_DATE_LINE}\n{}", fixture_text()?))
}

/// The run's verified baseline: a `Verified` status and the strict-parsed proof date.
fn baseline() -> Result<VerifiedDate, Fault> {
    verified(proof_date()?)
}

/// The baseline this case runs with, if it has one.
fn baseline_of(case: Case) -> Result<Option<VerifiedDate>, Fault> {
    match case {
        // The writer's own refusal: a run that cannot say how old its mirror is may not write.
        Case::NoBaseline => Ok(None),
        // `MAX_LAG_SECS + 1` ahead of the mirror's proof, so `check_freshness` must refuse it.
        Case::Stale => Ok(Some(verified(stale_date()?)?)),
        _ => Ok(Some(baseline()?)),
    }
}

/// A verified date for one strict-parsed date, or a harness fault when the status cannot give one.
fn verified(date: SignedDate) -> Result<VerifiedDate, Fault> {
    let Some(verified) = SignatureStatus::verified_for_test().verified_date(date) else {
        return Err(Fault::Harness(
            "harness baseline is unusable: a verified status and a strict-parsed date always yield \
             a verified date"
                .to_string(),
        ));
    };
    Ok(verified)
}

/// The one case that proves the strict in-region parse is what authorizes a write.
///
/// The same body is refused by the strict read and accepted by the tolerant one (the read-only
/// ranking's), and a mirror whose only date came from the tolerant read has no proof to write with.
fn prefixed() -> Result<String, Fault> {
    require_container()?;
    let body = prefixed_body()?;
    let Err(refusal) = fresh::release_date(&body) else {
        return Err(Fault::Harness(
            "the strict parse accepted a body with a Date: line before the armour, so a date the \
             signature does not cover could authorize a write"
                .to_string(),
        ));
    };
    println!("STRICT refused: {refusal}");
    let tolerated = fresh::release_date_tolerant(&body).map_err(|e| {
        Fault::Harness(format!(
            "harness fixture is unusable: the tolerant parse refused the prefixed body: {e}"
        ))
    })?;
    println!("TOLERANT {tolerated}");
    let writes = writes_for(Case::Apply)?;
    match Plan::for_apply(
        &probed_from(tolerated, None),
        Case::Apply.platform(),
        writes,
    ) {
        Err(err) => Err(Fault::Plan(err)),
        // Unreachable unless the proof rule names the wrong half of the pair.
        Ok(_) => Err(Fault::Harness(
            "a mirror whose only date sits outside the signed region was given a proof, so the \
             tolerant read can authorize a write"
                .to_string(),
        )),
    }
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
