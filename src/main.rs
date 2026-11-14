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
mod restore;
pub mod sources;
mod verify;

use std::io::{self, BufRead, IsTerminal, Write};
use std::os::unix::fs::MetadataExt;
use std::path::Path;
use std::process::ExitCode;
use std::sync::{Arc, Mutex, mpsc};
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

#[derive(Debug, Clone, PartialEq, Eq)]
struct Args {
    country: Option<String>,
    top: usize,
    https_only: bool,
    no_geo: bool,
    /// `--any`: the country stops constraining the candidate set and becomes display-only.
    any: bool,
    list: bool,
    yes: bool,
    /// `--verbose`: per-mirror detail and each rejection's full reason.
    verbose: bool,
    /// `--no-color`: no escapes in the report, whatever the terminal is (SPEC.md Output).
    no_color: bool,
    /// `--restore`: put the newest backup back instead of applying a mirror.
    restore: bool,
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
        any: false,
        list: false,
        yes: false,
        verbose: false,
        no_color: false,
        restore: false,
    };
    // The first flag of the mirror pipeline this run was given, spelled as the user typed it. A
    // restore sets none of them, so the two can never be in one run (SPEC.md CLI).
    let mut pipeline: Option<String> = None;
    let mut it = args.into_iter();
    while let Some(arg) = it.next() {
        // Each short alias is the same arm as its long form, and the value-taking ones name the
        // spelling the user typed in their error, so a message never points at a flag they did not
        // write. Only these aliases exist; `-R` stays reserved for a flag that is not implemented
        // yet, and bundling (`-lyc NO`) stays an unknown option.
        match arg.as_str() {
            "--list" | "-l" => {
                a.list = true;
                note_pipeline(&mut pipeline, &arg);
            }
            "--restore" | "-r" => a.restore = true,
            "--yes" | "-y" => a.yes = true,
            "--https-only" | "-s" => {
                a.https_only = true;
                note_pipeline(&mut pipeline, &arg);
            }
            "--no-geo" | "-n" => {
                a.no_geo = true;
                note_pipeline(&mut pipeline, &arg);
            }
            "--any" | "-a" => {
                a.any = true;
                note_pipeline(&mut pipeline, &arg);
            }
            "--verbose" | "-v" => a.verbose = true,
            // The one implemented flag without a short form, so `-N` stays an unknown option.
            "--no-color" => a.no_color = true,
            "--country" | "-c" => {
                note_pipeline(&mut pipeline, &arg);
                let cc = it.next().ok_or(format!(
                    "{arg} needs a two-letter country code, for example {arg} LT"
                ))?;
                let cc = cc.to_ascii_uppercase();
                if cc.len() != 2 || !cc.chars().all(|c| c.is_ascii_uppercase()) {
                    return Err(format!(
                        "{arg} '{cc}' is not a two-letter country code, for example LT"
                    ));
                }
                a.country = Some(cc);
            }
            "--top" | "-t" => {
                note_pipeline(&mut pipeline, &arg);
                let n = it
                    .next()
                    .ok_or(format!("{arg} needs a number, for example {arg} 5"))?;
                a.top = n
                    .parse()
                    .ok()
                    .filter(|n| *n > 0)
                    .ok_or(format!("{arg} '{n}' is not a positive number"))?;
            }
            arg if help_requested(arg) => {
                println!("{HELP}");
                std::process::exit(0);
            }
            other => return Err(format!("unknown option '{other}'; try --help")),
        }
    }
    // A restore and a mirror flag are one usage error, checked once here so the answer does not
    // depend on which of the two the user typed first.
    if a.restore
        && let Some(flag) = pipeline
    {
        return Err(restore_conflict(&flag));
    }
    Ok(a)
}

/// Remember the first pipeline flag a run was given, spelled the way it was typed: the one a
/// restore's usage error names.
fn note_pipeline(slot: &mut Option<String>, flag: &str) {
    if slot.is_none() {
        *slot = Some(flag.to_string());
    }
}

/// The usage error a restore and a flag of the mirror pipeline make together. A restore replaces
/// the source of the rewrite, so there is no mirror to look up, rank or plan with (SPEC.md CLI).
fn restore_conflict(flag: &str) -> String {
    format!(
        "{flag} cannot be combined with --restore: a restore reads the newest backup and never \
         probes, ranks or rewrites. Run 'sudo apt-scout --restore', adding --yes to skip the \
         confirmation."
    )
}

/// `-h` and `--help` are one request, kept out of the flag match so both spellings print the same
/// text and exit 0 before any run starts.
fn help_requested(arg: &str) -> bool {
    matches!(arg, "-h" | "--help")
}

/// `--help`, once: the flags (long form first, the short alias beside it), what a run does with
/// them, the exit codes and one example each. The rule that used to repeat under every table lives
/// here instead. Examples and the `sudo` line stay long-form, so a copied command is unambiguous.
const HELP: &str = "\
apt-scout — pick the fastest Ubuntu mirror and use it

USAGE
  apt-scout [OPTIONS]

  -h, --help          this text, then exit 0

DISCOVERY
  -c, --country <CC>  two-letter country; overrides detection
  -a, --any           ignore the country; rank every country together
  -n, --no-geo        no geolocation; requires --country or --any
  -s, --https-only    only https mirrors
  -t, --top <N>       rows to show (default 5)
  -v, --verbose       per-mirror detail and full rejection reasons

MODE
  (no flag)           non-root: recommend and print the sudo line, exit 3
                      root:      back up, confirm, rewrite the apt sources
  -l, --list          ranked table and planned change only; never writes
  -y, --yes           skip the one confirmation

RESTORE
  -r, --restore       put the newest backup back; root only
                      non-root: prints sudo apt-scout --restore and exits 3
                      not with the discovery flags or --list

TABLE
  behind    how far this mirror's -updates package lists trail the freshest date the run
            knows, in minutes; 0m means up to date, and it is freshness, never latency.
  latency   the network round trip to the mirror's -updates request, never freshness.
  verified  the -updates InRelease signature: yes = gpgv accepted it with the Ubuntu archive
            keyring; no = readable but unsigned, altered or signed by an unknown key;
            ? = verification could not run (no gpgv or keyring, oversized body, timeout).
            Only yes may authorize a write; no and ? never do.
  notes     redir:<host> for a redirect to another host; a host that does not fit is cut
            and ends in ~.
  --verbose adds the proof behind these words: each row's AS number and its same/diff/unknown
  match with the server (and the chosen mirror's in the recommendation), the server's AS on the
  context line, each row's country and resolved address, both pocket dates in full, the
  -security lag, the exact signature status, a redirect host uncut, and the full text of every
  rejection reason.

DISPLAY
  --no-color          no colour in the report, even on a terminal
  colour and the progress line are terminal-only: colour paints stdout when it is a
  terminal and NO_COLOR is unset, and progress rewrites one line on stderr when that
  is a terminal. A pipe gets neither, so a captured run prints the plain words.

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
    /// `--restore`: the newest backup instead of a mirror, re-derived and written by the one
    /// writer; `confirm` is false in a non-root run and when `--yes` was given.
    Restore { confirm: bool },
}

fn decide(restore: bool, list: bool, root: bool, yes: bool) -> Mode {
    if restore {
        // A restore never reaches the mirror pipeline, so `--list` cannot narrow it and the uid is
        // the only thing that decides whether it writes (SPEC.md CLI).
        Mode::Restore {
            confirm: root && !yes,
        }
    } else if list {
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
    // A restore's line is exactly this whatever else the run was given: the backup and the write
    // are the whole command, and a copied line that carried a shortened or widened run would be a
    // different command than the one it repeats (SPEC.md CLI).
    if args.restore {
        return "sudo apt-scout --restore".to_string();
    }
    let mut line = String::from("sudo apt-scout");
    if let Some(country) = &args.country {
        line.push_str(&format!(" --country {country}"));
    }
    if args.no_geo {
        line.push_str(" --no-geo");
    }
    // A `sudo` line that dropped this one would run a different candidate set, so the flag that
    // decided the set has to travel with it (SPEC.md CLI: the line repeats the flags of the run).
    if args.any {
        line.push_str(" --any");
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
    // The report's display is part of the run the line reproduces: a copy-pasted command must not
    // colour what the user asked it not to colour.
    if args.no_color {
        line.push_str(" --no-color");
    }
    line
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
/// write: when every eligible URI already points at the selected mirror the answer is `None` and that
/// is reported as success, and when nothing eligible exists at all the plan is refused.
///
/// The skips it collects are read here and nowhere else: they name the refusal in the error, and
/// `Sources` has already listed them per entry before probing (SPEC.md Apply), so the report repeats
/// none of them.
fn build_plan(
    root: &Path,
    inventory: &sources::Inventory,
    roots: &[sources::MirrorRoot],
    selected: &fresh::Probed,
    platform: plan::Platform,
) -> Result<Option<plan::Plan>, (u8, String)> {
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
                effect: if planned.create_new {
                    plan::Effect::Create
                } else {
                    plan::Effect::Replace
                },
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
            return Ok(None);
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
    Ok(Some(plan))
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

/// What the user answered at the one confirmation.
///
/// `No` is a decision, not a failure: the mode that asked decides what it means, and the mirror
/// write treats it as a refusal while a restore treats it as a successful no-op. A question that
/// could not be asked or answered is the `Err`, which no answer can turn into a `Yes`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Answer {
    Yes,
    No,
}

/// The one confirmation. The prompt is a safety rail, not a security control: the guarantee is the
/// plan's validation, and no answer here can bypass it.
fn confirm(input: &mut impl BufRead, prompt: &str) -> Result<Answer, (u8, String)> {
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
        "y" | "yes" => Ok(Answer::Yes),
        "" => Err((
            1,
            "no answer on standard input, so nothing was written. Re-run with --yes to skip the confirmation."
                .to_string(),
        )),
        _ => Ok(Answer::No),
    }
}

/// The confirmation a write asks: the only answer it accepts is a yes.
fn confirm_write(input: &mut impl BufRead, prompt: &str) -> Result<(), (u8, String)> {
    match confirm(input, prompt)? {
        Answer::Yes => Ok(()),
        Answer::No => Err((
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
    if args.no_geo && args.country.is_none() && !args.any {
        return Err("--no-geo needs --country <CC> or --any, because nothing else can tell which country you are in. Example: --no-geo --country LT".to_string());
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
/// not name is the usual cause, and the fix is either to put the URIs back by hand or to put the
/// newest backup back with `--restore`; the report and the write error carry the same sentence
/// instead of drifting apart.
fn no_eligible_hint(list_source: &str) -> String {
    format!(
        "The sources here point at a host the mirror list from {list_source} does not name, so nothing here can be replaced: set those URIs back to archive.ubuntu.com, or put the newest backup back with 'sudo apt-scout --restore', and run again."
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
        Mode::Write { .. } | Mode::Restore { .. } => Err((
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

/// The inks the report uses: the whole vocabulary of SPEC.md Output, so no render site can invent a
/// sixth one. Display only — a decision, a comparison, an ordering, a path, a written byte and an
/// exit code never read it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Style {
    Bold,
    Green,
    Yellow,
    Red,
    Dim,
}

impl Style {
    /// The SGR parameter that turns this ink on; `paint` always closes it with a reset.
    fn code(self) -> &'static str {
        match self {
            Style::Bold => "1",
            Style::Green => "32",
            Style::Yellow => "33",
            Style::Red => "31",
            Style::Dim => "2",
        }
    }
}

/// SPEC.md Output: colour appears only when all three inputs agree — stdout is a terminal, `--no-color`
/// was not given, and `NO_COLOR` is unset. Pure, so a test proves every combination without owning a
/// terminal.
fn colour_enabled(stdout_is_terminal: bool, no_color_flag: bool, no_color_set: bool) -> bool {
    stdout_is_terminal && !no_color_flag && !no_color_set
}

/// The one environment read colour makes. `var_os`, because SPEC.md Output counts `NO_COLOR` as set
/// whatever its value is, the empty one included, and a non-UTF-8 value is still a value. It reaches
/// no decision, no path, no written byte and no exit code.
fn no_color_from_env() -> bool {
    std::env::var_os("NO_COLOR").is_some()
}

/// `text` in `style`, or `text` unchanged when colour is off. The only place an escape sequence is
/// made, so a plain run prints exactly the bytes a colour run prints minus the escapes (SPEC.md
/// Output). Pure.
fn paint(style: Style, text: &str, enabled: bool) -> String {
    if !enabled {
        return text.to_string();
    }
    format!("\x1b[{}m{text}\x1b[0m", style.code())
}

/// The style set one run paints its report with, threaded into every render site. The plain one
/// returns its input, so a render function needs no branch of its own and the plain text cannot
/// drift from the coloured one. A styled cell is always painted **after** it is padded: `{:<44.44}`
/// counts bytes, so painting first would break every column.
#[derive(Clone, Copy)]
struct Painter {
    colour: bool,
}

impl Painter {
    fn new(colour: bool) -> Self {
        Painter { colour }
    }

    /// What a piped run and every test use: no escapes, ever.
    #[cfg(test)]
    fn plain() -> Self {
        Painter { colour: false }
    }

    fn bold(&self, text: &str) -> String {
        paint(Style::Bold, text, self.colour)
    }

    fn green(&self, text: &str) -> String {
        paint(Style::Green, text, self.colour)
    }

    fn yellow(&self, text: &str) -> String {
        paint(Style::Yellow, text, self.colour)
    }

    fn red(&self, text: &str) -> String {
        paint(Style::Red, text, self.colour)
    }

    fn dim(&self, text: &str) -> String {
        paint(Style::Dim, text, self.colour)
    }
}

/// Where a transient progress line goes. The shipped sink writes to standard error; a test records,
/// so the phase sequence is asserted without a terminal anywhere and the no-terminal case is proven to
/// be silent rather than merely unobserved.
trait Sink {
    /// Rewrite the one live line in place.
    fn rewrite(&mut self, line: &str);

    /// Erase the live line, leaving the cursor where it was: a warning that follows keeps its own line
    /// (SPEC.md Output).
    fn clear(&mut self);

    /// Write the line that ends the progress and stays: it stops being transient.
    fn settle(&mut self, line: &str);
}

/// The one real sink. `\r` returns to the start of the line and `\x1b[K` erases what was there, so a
/// shorter line never leaves the tail of a longer one behind. A failed write is dropped rather than
/// turned into an error: progress is a convenience, never a result.
struct StderrSink<W: Write> {
    err: W,
}

impl StderrSink<io::Stderr> {
    /// The only constructor a run has, so the progress line cannot be pointed at standard output,
    /// where it would land in the report a pipe or a log captures (SPEC.md Output).
    fn new() -> Self {
        StderrSink { err: io::stderr() }
    }
}

#[cfg(test)]
impl<W: Write> StderrSink<W> {
    /// A test's own buffer, so the sink's exact bytes are asserted without a terminal anywhere.
    fn with_writer(err: W) -> Self {
        StderrSink { err }
    }
}

impl<W: Write> Sink for StderrSink<W> {
    fn rewrite(&mut self, line: &str) {
        let _ = write!(self.err, "\r{line}\x1b[K");
        let _ = self.err.flush();
    }

    fn clear(&mut self) {
        let _ = write!(self.err, "\r\x1b[K");
        let _ = self.err.flush();
    }

    fn settle(&mut self, line: &str) {
        let _ = write!(self.err, "\r{line}\x1b[K\n");
        let _ = self.err.flush();
    }
}

/// The line that ends the progress and stays: the counts the ranked title carries, and the time from
/// the loading phase to the end of ranking, with one decimal place (SPEC.md Output). Pure.
fn done_line(usable: usize, rejected: usize, elapsed: Duration) -> String {
    format!(
        "done — {usable} usable, {rejected} rejected in {:.1} s",
        elapsed.as_secs_f64()
    )
}

/// One Braille cell of the animation, advanced one frame about every `SPIN_INTERVAL` for as long as a
/// phase is active (SPEC.md Output). Ten cells, so the loop takes about a second to turn over.
const SPIN: [char; 10] = ['⠋', '⠙', '⠹', '⠸', '⠼', '⠴', '⠦', '⠧', '⠇', '⠏'];

/// About ten frames a second: fast enough to read as motion, cheap enough that a frame costs nothing
/// beside the probes it is drawn over.
const SPIN_INTERVAL: Duration = Duration::from_millis(100);

/// The frame the animation shows once a phase has been active for `elapsed`. A pure function of the
/// time, so the animation can be asserted without a clock, a thread or a terminal (SPEC.md Output).
fn spinner_frame(elapsed: Duration) -> char {
    let frame = (elapsed.as_millis() / SPIN_INTERVAL.as_millis()) as usize;
    SPIN[frame % SPIN.len()]
}

/// The one line the animation draws: the frame and the phase's own text, so the probing count rides
/// inside the same frame instead of becoming a second line (SPEC.md Output). Pure.
fn spinner_line(elapsed: Duration, text: &str) -> String {
    format!("{} {text}", spinner_frame(elapsed))
}

/// What the run tells the animation. Both drawing messages carry an acknowledgement, so the caller
/// learns that the line it asked for is on the screen — a phase is visible the moment the run names
/// it, not on the next frame — and that a line is gone before a warning takes one of its own.
/// `Phase` is the run naming a phase and `Text` is the same phase's line changing under it: the frame
/// is a pure function of the time **this** phase has been active (SPEC.md Output), so a phase starts
/// the clock and a count does not.
enum Tick {
    Phase(String, mpsc::SyncSender<()>),
    Text(String, mpsc::SyncSender<()>),
    Hide(mpsc::SyncSender<()>),
    Stop,
}

/// What one wait on the animation's channel produced. Its own type so the loop below can be driven by
/// a scripted wait in a test: no thread, no sleep and no terminal (SPEC.md Output).
enum Wake {
    Message(Tick),
    Closed,
    Timeout,
}

/// The animation's whole loop, and the only place a frame is drawn: draw the line the phase handed it,
/// then wait up to `SPIN_INTERVAL`. A `Phase` replaces the line, starts the frame's clock over and is
/// drawn at once; a `Text` replaces the line without touching that clock, because a count changed and
/// the phase did not; a `Hide` erases the line and acknowledges, `Stop` or a closed channel ends the
/// loop, and a timeout is what advances the frame. So the frame moves with time and not with a change
/// of count — the whole point of the ticker: a phase that runs for five seconds visibly moves — and a
/// long run still opens every phase on the first cell (SPEC.md Output). A frame does nothing but
/// draw, so no probe's timeout, measurement, rejection or join order changes because one was drawn.
fn run_ticker<S: Sink>(sink: &mut S, mut wait: impl FnMut(Duration) -> Wake) {
    let mut text: Option<String> = None;
    // The draw a message asked for, whose sender is released once the line is on the screen.
    let mut ack: Option<mpsc::SyncSender<()>> = None;
    let mut elapsed = Duration::ZERO;
    loop {
        if let Some(text) = text.as_deref() {
            sink.rewrite(&spinner_line(elapsed, text));
        }
        if let Some(ack) = ack.take() {
            let _ = ack.send(());
        }
        match wait(SPIN_INTERVAL) {
            Wake::Message(Tick::Phase(next, drawn)) => {
                text = Some(next);
                elapsed = Duration::ZERO;
                ack = Some(drawn);
            }
            Wake::Message(Tick::Text(next, drawn)) => {
                text = Some(next);
                ack = Some(drawn);
            }
            Wake::Message(Tick::Hide(hidden)) => {
                sink.clear();
                text = None;
                let _ = hidden.send(());
            }
            Wake::Message(Tick::Stop) | Wake::Closed => return,
            Wake::Timeout => elapsed += SPIN_INTERVAL,
        }
    }
}

/// The sink the animation thread draws through: the run's own sink, behind the mutex both of them
/// write through, so a phase line and a frame can never split one line between them.
struct Locked<S> {
    sink: Arc<Mutex<S>>,
}

impl<S: Sink> Sink for Locked<S> {
    fn rewrite(&mut self, line: &str) {
        lock(&self.sink).rewrite(line);
    }

    fn clear(&mut self) {
        lock(&self.sink).clear();
    }

    fn settle(&mut self, line: &str) {
        lock(&self.sink).settle(line);
    }
}

/// The one mutex, with poisoning read as "take the data anyway": progress is decoration, so a panic
/// somewhere else must not turn a dropped line into a second panic. Never `unwrap`, as everywhere else
/// outside a test.
fn lock<S>(sink: &Mutex<S>) -> std::sync::MutexGuard<'_, S> {
    match sink.lock() {
        Ok(guard) => guard,
        Err(poisoned) => poisoned.into_inner(),
    }
}

/// The animation thread and the channel to it. `hide` blocks until the line is off the screen, `stop`
/// joins, and `Drop` stops it, so an early `return Err` in `run()` cannot leave a thread drawing a
/// frame into the report that follows.
struct Ticker {
    tx: mpsc::Sender<Tick>,
    handle: Option<thread::JoinHandle<()>>,
}

impl Ticker {
    /// Start the animation over the run's sink. Standard library only: a thread, a channel, the
    /// mutex and the clock, so no terminal or progress crate is added for one cell of Braille.
    fn start<S: Sink + Send + 'static>(sink: Arc<Mutex<S>>) -> Self {
        let (tx, rx) = mpsc::channel();
        let handle = thread::spawn(move || {
            let mut locked = Locked { sink };
            run_ticker(&mut locked, |interval| match rx.recv_timeout(interval) {
                Ok(message) => Wake::Message(message),
                Err(mpsc::RecvTimeoutError::Timeout) => Wake::Timeout,
                Err(mpsc::RecvTimeoutError::Disconnected) => Wake::Closed,
            });
        });
        Ticker {
            tx,
            handle: Some(handle),
        }
    }

    /// Hand the animation a new line and wait for it to be drawn, so the phase is on screen before the
    /// work it names starts. One writer draws the line, so a phase and a frame can never split it.
    fn draw(&self, tick: fn(String, mpsc::SyncSender<()>) -> Tick, line: String) {
        let (ack, drawn) = mpsc::sync_channel(0);
        if self.tx.send(tick(line, ack)).is_err() {
            // The animation is already gone; there is no line to put up and nothing to wait for.
            return;
        }
        let _ = drawn.recv();
    }

    /// A phase starts: the line goes up and the frame's clock starts over.
    fn phase(&self, line: String) {
        self.draw(Tick::Phase, line);
    }

    /// The phase's line changes without the phase changing, so the clock keeps running.
    fn text(&self, line: String) {
        self.draw(Tick::Text, line);
    }

    /// Erase the line and wait until it is gone, so a warning that follows starts on a line of its own
    /// and nothing the animation drew can overwrite it (SPEC.md Output).
    fn hide(&self) {
        let (ack, acked) = mpsc::sync_channel(0);
        if self.tx.send(Tick::Hide(ack)).is_err() {
            // The animation is already gone, so it has nothing left on the screen to erase.
            return;
        }
        let _ = acked.recv();
    }

    /// Stop and join: after this call no frame can be drawn, so the summary and the report that follow
    /// are the last things on that line.
    fn stop(&mut self) {
        let Some(handle) = self.handle.take() else {
            return;
        };
        let _ = self.tx.send(Tick::Stop);
        let _ = handle.join();
    }
}

impl Drop for Ticker {
    fn drop(&mut self) {
        self.stop();
    }
}

/// The probing line: `probing 3 mirrors… 1/3`. `done` is how many probes the whole run has finished
/// and `total` how many it has to probe, which `await_probes` reads from the probes themselves rather
/// than from the joins. Both grow when the one widening adds candidates, so the line never runs
/// backwards (SPEC.md Output). Pure, so the count can be asserted without a terminal.
fn probing_line(done: usize, total: usize) -> String {
    let noun = if total == 1 { "mirror" } else { "mirrors" };
    format!("probing {total} {noun}… {done}/{total}")
}

/// The status line a run shows while it works: one line, on standard error, rewritten in place, and
/// only when standard error is a terminal (SPEC.md Output). With no terminal there is no `Ticker` at
/// all — nothing is started to draw a line — so a piped run takes the same code path and writes
/// nothing: no line, no frame and no "still working" notice.
struct Progress<S: Sink + Send + 'static> {
    /// The run's sink, shared with the animation thread, so the summary the run writes and a frame the
    /// animation draws can never split one line between them.
    sink: Arc<Mutex<S>>,
    /// The animation, when there is one. `None` is a piped run.
    ticker: Option<Ticker>,
    started: Instant,
    /// How many probes this run has finished. Pass 1 and the one widening of SPEC step 4 both count
    /// into it, because both probe the run's one candidate set: a count that restarted would show a
    /// viewer `30/30` and then `0/30` for the same set.
    probed: usize,
    /// Whether probing has named itself yet. The first probing line of a run starts that phase, so the
    /// frame's clock starts with it; the count updates after it are the same phase and are not.
    probing: bool,
}

impl<S: Sink + Send + 'static> Progress<S> {
    /// The clock starts here, so the summary spans the run's work and not only its last phase.
    /// `enabled` is whether standard error is a terminal: false starts no thread at all.
    fn new(sink: S, enabled: bool) -> Self {
        let sink = Arc::new(Mutex::new(sink));
        let ticker = enabled.then(|| Ticker::start(Arc::clone(&sink)));
        Progress {
            sink,
            ticker,
            started: Instant::now(),
            probed: 0,
            probing: false,
        }
    }

    fn enabled(&self) -> bool {
        self.ticker.is_some()
    }

    /// Name the phase now — a person sees it the moment it starts, not on the next frame — and let the
    /// animation redraw it with each frame from then on. The frame's clock starts here, because it is
    /// a function of how long this phase has been active and of nothing earlier in the run.
    fn phase(&mut self, line: String) {
        if let Some(ticker) = &self.ticker {
            ticker.phase(line);
        }
    }

    /// The phase's line changes and the phase does not, so the frame keeps the age it has.
    fn show(&mut self, line: String) {
        if let Some(ticker) = &self.ticker {
            ticker.text(line);
        }
    }

    /// The four phases SPEC.md Output names, in the order a run reaches them.
    fn load(&mut self) {
        self.phase("loading the mirror list".to_string());
    }

    fn reference(&mut self) {
        self.phase("verifying the reference".to_string());
    }

    /// The candidates probed **so far** (SPEC.md Output). The first line of the run starts the probing
    /// phase; every later one is the same phase counting higher.
    fn probe(&mut self, done: usize, total: usize) {
        let line = probing_line(done, total);
        if self.probing {
            self.show(line);
        } else {
            self.probing = true;
            self.phase(line);
        }
    }

    fn ranked(&mut self) {
        self.phase("ranking".to_string());
    }

    /// Erase the live line before something that needs a line of its own, such as a warning. It waits
    /// for the erase, so the warning cannot land on a line the animation redraws.
    fn clear(&mut self) {
        if let Some(ticker) = &self.ticker {
            ticker.hide();
        }
    }

    /// End the progress where no summary belongs: the line is erased and the animation stopped and
    /// joined, so the report that prints next cannot have a frame written over it (SPEC.md Output). A
    /// run that stops before ranking prints no `done` line, because there is no count to give.
    fn stop(&mut self) {
        self.clear();
        self.halt();
    }

    /// Stop and join the animation and leave the line as it is, for a caller that replaces it itself —
    /// the summary does — or has erased it already.
    fn halt(&mut self) {
        if let Some(ticker) = &mut self.ticker {
            ticker.stop();
        }
    }

    /// End the progress at the end of ranking: the animation is stopped first, so no frame may follow
    /// the summary or the report's first line, and the line becomes the one summary that stays
    /// (SPEC.md Output).
    fn finish(&mut self, usable: usize, rejected: usize) {
        if !self.enabled() {
            return;
        }
        self.halt();
        let line = done_line(usable, rejected, self.started.elapsed());
        lock(&self.sink).settle(&line);
    }
}

/// How often the run asks the probes whether they have finished: short enough that the count it hands
/// the animation keeps up with the mirrors landing, long enough that asking costs nothing. The frames
/// between two counts come from the animation's own clock, not from this poll.
const PROBE_POLL: Duration = Duration::from_millis(50);

/// Wait for every probe with the line counting the probes that have **finished**, and hand back the
/// joins in candidate order. `handle.join()` blocks in candidate order, so a loop that joins first
/// would count the joins: a slow first mirror would freeze the line while the other twenty-eight are
/// already done, which is not "the candidates probed so far" (SPEC.md Output). Polling the probes
/// gives the animation a new count as each one lands, and the results still come back in candidate
/// order, so nothing downstream depends on which probe answered first.
fn await_probes<T, S: Sink + Send + 'static>(
    handles: Vec<thread::JoinHandle<T>>,
    progress: &mut Progress<S>,
) -> Vec<thread::Result<T>> {
    // `n/m` counts the candidates probed so far in the run, not in this batch: the one widening adds
    // to the same candidate set, so the second pass continues at `30/60` instead of restarting at
    // `0/30` after the viewer already saw `30/30` (SPEC.md Output).
    let batch = handles.len();
    let done_before = progress.probed;
    let total = done_before + batch;
    // The line goes up before the first wait, so the probing phase is visible from its first
    // millisecond instead of after the first probe comes back. An empty candidate set has nothing
    // to count and shows no line at all.
    if total > 0 {
        progress.probe(done_before, total);
    }
    let mut shown = 0;
    loop {
        let done = handles.iter().filter(|h| h.is_finished()).count();
        if done != shown {
            progress.probe(done_before + done, total);
            shown = done;
        }
        if done == batch {
            break;
        }
        thread::sleep(PROBE_POLL);
    }
    progress.probed = total;
    handles.into_iter().map(|h| h.join()).collect()
}

/// Probe one batch of candidates, each on its own thread with its own `Net`, and hand the answers back
/// in candidate order. Both passes use this, so the one widening probes exactly as pass 1 does. A
/// probe thread that crashed is reported as a rejection instead of becoming a silent gap: `probe`
/// has no panic path of its own, so a crash is a bug and the run says so.
fn probe_candidates<S: Sink + Send + 'static>(
    candidates: &[mirrors::Mirror],
    codename: &str,
    arch: &str,
    progress: &mut Progress<S>,
    rejected: &mut Vec<Rejected>,
) -> ProbeBatch {
    let handles: Vec<_> = candidates
        .iter()
        .cloned()
        .map(|m| {
            let (codename, arch) = (codename.to_string(), arch.to_string());
            thread::spawn(move || {
                let net = Net::new();
                let r = probe::probe(&net, &m.url, &codename, &arch);
                (m, r)
            })
        })
        .collect();
    let mut batch = Vec::new();
    for (m, joined) in candidates.iter().zip(await_probes(handles, progress)) {
        match joined {
            Ok(done) => batch.push(done),
            Err(_) => rejected.push(Rejected {
                host: m.url.trim_end_matches('/').to_string(),
                kind: Reject::Crashed,
                why: "the probe crashed unexpectedly".to_string(),
            }),
        }
    }
    batch
}

/// Where the country in the header came from, and whether it constrains anything. Under `--any` the
/// country is display-only, so the header says `not a constraint` instead of reading like a filter
/// that decided the candidate set (SPEC step 4); with no country at all it reads `any (--any)`.
fn origin_label(any: bool, country_known: bool, given: bool) -> &'static str {
    if any {
        match (country_known, given) {
            (false, _) => "--any",
            (true, true) => "--country, not a constraint",
            (true, false) => "detected, not a constraint",
        }
    } else if given {
        "--country"
    } else {
        "detected"
    }
}

/// The country a recommendation names: the selected mirror's own, which `detail_of` recorded from the
/// list. It is the mirror's and not the run's, because the two differ once a run is not narrowed to
/// its own country (SPEC.md CLI); `unknown` covers a mirror whose probe recorded none.
fn recommendation_country<'a>(
    details: &'a std::collections::HashMap<String, Detail>,
    url: &str,
) -> &'a str {
    details.get(url).map_or("unknown", |d| d.country.as_str())
}

/// The run's identity: version, release and the country the run used, with the source of the
/// country, because `--country` is an override and the line says which one won (SPEC.md CLI).
fn header_identity(
    version: &str,
    distro: &str,
    country: &str,
    origin: &str,
    ink: Painter,
) -> String {
    ink.bold(&format!(
        "apt-scout {version} · {distro} · country: {country} ({origin})"
    ))
}

/// The facts about where the answer came from: the server's AS, the mirror list source and the
/// freshness reference. The server's AS is proof that the AS comparison happened at all and not a
/// fact a person choosing a mirror acts on, so the default line keeps `mirrors:` and `reference:` and
/// the AS is `--verbose`'s (SPEC.md Output).
fn header_context(
    server_as: Option<&str>,
    list_source: &str,
    reference: Option<&probe::Reference>,
    verbose: bool,
) -> String {
    let reference = match reference {
        Some(r) => format!(
            "reference: archive.ubuntu.com ({})",
            signature_words(&r.updates_signature)
        ),
        None => "reference: newest mirror date (archive.ubuntu.com unreachable)".to_string(),
    };
    if verbose {
        return format!(
            "server {} · mirrors: {list_source} · {reference}",
            server_as.unwrap_or("AS unknown")
        );
    }
    format!("mirrors: {list_source} · {reference}")
}

/// A section title with an annotation on the right, so a heading can state the rule it follows
/// without a sentence underneath the table. The heading is bold, and painted after its padding so the
/// annotation stays in its column.
fn titled(left: &str, right: &str, ink: Painter) -> String {
    if right.is_empty() {
        ink.bold(left)
    } else {
        format!("{}  {right}", ink.bold(&format!("{left:<45}")))
    }
}

/// The one line under the table when no row has AS data: the run's own reason for it, because a
/// column of `unknown` would read as an answer (SPEC step 9). Only `--verbose` has that column, so
/// only a `--verbose` run has the gap to explain: the default table never shows an AS, and a note
/// apologising for a column it does not print would be a fact with nothing behind it.
fn as_note(rows: &[Row], reason: Option<&str>, verbose: bool) -> Option<String> {
    if !verbose || rows.is_empty() || rows.iter().any(|row| row.as_cell.is_some()) {
        return None;
    }
    Some(format!(
        "note: AS data unavailable ({})",
        reason.unwrap_or("no AS lookup answered")
    ))
}

/// `Ranked — 5 of 12 usable, 7 rejected`, with one clause per column that needs one underneath the
/// title (SPEC.md Output): the column words are the only place a reader learns that `behind` is
/// freshness and `latency` is the network. `verified`'s clause says what the three cell values mean
/// without repeating the `yes`/`no`/`?` rule, which `--help` spells out.
fn ranked_title(shown: usize, usable: usize, rejected: usize, ink: Painter) -> String {
    titled(
        &format!("Ranked — {shown} of {usable} usable, {rejected} rejected"),
        "behind = -updates age vs freshest · latency = round trip · verified = InRelease signature",
        ink,
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
    /// `12929 same` when the run has the mirror's AS; the column exists when any row has one. The
    /// default report never fills it in, because the AS number and its match are proof that the
    /// tie-break was auditable and not a fact a reader acts on (SPEC.md Output).
    as_cell: Option<String>,
    /// The host this mirror redirected to, when it is not its own host (SPEC step 7).
    redirect_host: Option<&'a str>,
}

/// The narrowest the `as` column may be: a table with AS data still says `unknown` for a mirror the
/// lookup could not name, and that word must fit the column it shares with the ASNs.
const AS_MIN_WIDTH: usize = 7;

/// The width of the `verified` column: the heading word itself, so the three short cells (`yes`, `no`,
/// `?`) all fit and a note that follows a cell starts under the `notes` heading instead of five
/// columns left of it.
const VERIFIED_WIDTH: usize = 8;

/// The most columns a redirect host may take in the `notes` cell. A host longer than this is cut,
/// so one long name cannot wrap the table; `--verbose` prints it in full.
const REDIR_HOST_MAX: usize = 16;

/// What a cut host ends in. `~` cannot appear in a hostname, so a real host is never read as a cut
/// one, and it costs one column, so `REDIR_HOST_MAX` bounds the cell whether or not it was cut.
const CUT_MARKER: char = '~';

/// The ranked table (SPEC step 9): rank, mirror, behind, speed, latency, AS, verified, notes. The AS
/// column is left out entirely when no row has AS data, so it can never read as "every mirror is
/// unknown"; the notes column is left out when no shown row has a note, so a plain table gains no
/// empty column.
fn ranked_rows(rows: &[Row], ink: Painter) -> Vec<String> {
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
    // Each heading word is padded first and dimmed second: the width format counts bytes, so a
    // dimmed word would lose its padding and shift every column under it.
    //
    // `behind` is six columns where `lag` was three, so the numeric field grew to six in the same
    // edit: a heading wider than its field overflows it and pushes every column after it one column
    // to the right of the rows below, which is exactly the drift this table cannot afford.
    let mut head = format!("  {:>2}  {:<44} ", "#", "mirror");
    head.push_str(&ink.dim(&format!("{:>6}", "behind")));
    head.push(' ');
    head.push_str(&ink.dim(&format!("{:>10}", "speed")));
    head.push(' ');
    head.push_str(&ink.dim(&format!("{:>9}", "latency")));
    if let Some(width) = as_width {
        head.push_str(&format!(" {:>width$}", "as"));
    }
    head.push_str(&format!("  {:<VERIFIED_WIDTH$}", "verified"));
    if with_notes {
        head.push_str("  notes");
    }
    out.push(head);
    for (i, row) in rows.iter().enumerate() {
        let mut line = format!(
            "  {:>2}  {:<44.44} {:>6} {:>10} {:>9}",
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
        if let Some(note) = &notes[i] {
            // The cell is painted first and padded second, and only a row with a note pads: the
            // escape sequence would spend the padding on itself, and a row that ends at its cell
            // must not carry trailing blanks a captured line would still show. The two columns of
            // separation the heading has before `notes` are part of the width, so a note starts
            // under `notes` and not two columns left of it.
            let pad = VERIFIED_WIDTH + 2 - signature_cell(row.signature).chars().count();
            line.push_str("  ");
            line.push_str(&signature_cell_painted(row.signature, ink));
            line.push_str(&" ".repeat(pad));
            line.push_str(note);
        } else {
            line.push_str("  ");
            line.push_str(&signature_cell_painted(row.signature, ink));
        }
        out.push(line);
    }
    out
}

/// The trailing `notes` cell, last so the facts it carries never push a row's other columns around:
/// `redir:<host>` for a redirect to another host, cut so one long name cannot wrap the table. The
/// `-security` lag this cell used to carry is proof about a pocket that decides nothing, and
/// `Detail` prints that pocket's date and lag in full, so the default report spends no column on it
/// (SPEC.md Output). `None` when the row has no note, so the column can be omitted.
fn note_cell(row: &Row) -> Option<String> {
    row.redirect_host
        .map(|host| format!("redir:{}", host_prefix(host, REDIR_HOST_MAX)))
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

/// What `--verbose` prints about one mirror: the country, the resolved address, the AS and its
/// holder, both pocket dates with their lag and the redirect the probe saw. The default report shows
/// only the redirect, cut, so this is where the rest of the proof lives (SPEC.md Output). The
/// `-security` lag is computed once, here, so no two places can disagree about it.
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

/// The rows the table prints, in rank order and capped at `top`, each carrying the AS cell (a
/// `--verbose` fact) and the redirect the `notes` column shows. The note fact and the facts `Detail`
/// prints are read from the run's own probe results (`details`) instead of being recomputed here, so
/// the table and `--verbose` describe the same run (SPEC steps 7 and 8). No decision reads `verbose`:
/// it decides only whether the AS cell is filled in at all.
fn table_rows<'a>(
    ranked: &'a [fresh::Probed],
    details: &'a std::collections::HashMap<String, Detail>,
    mirror_asn: &std::collections::HashMap<String, ResolvedAs>,
    server_asn: Option<&str>,
    top: usize,
    verbose: bool,
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
                as_cell: if verbose {
                    as_cell(mirror_asn, server_asn, &r.url)
                } else {
                    None
                },
                // A redirect is a fact about how the request was answered, not a decision, so the
                // row carries it and the table prints it cut; `--verbose` prints it whole.
                redirect_host: detail.and_then(|d| d.redirected_to.as_deref()),
            }
        })
        .collect()
}

/// The recommendation and the facts behind it: host, country, speed, `behind`, `latency` and the
/// signature status (SPEC.md CLI), in one line so nothing repeats the table, and in the table's own
/// words for the two numbers. The chosen mirror's AS and its match are proof that the tie-break was
/// auditable, not facts a reader acts on when choosing a mirror, so only a `--verbose` run names them
/// (SPEC.md Output). The signature is the same split: the default line names the column and its cell
/// — `verified yes`, `verified no`, `verified ?` — and the exact status the cell stands for is
/// `--verbose`'s, because it is the proof and not the decision (SPEC.md Output).
fn recommendation(
    best: &fresh::Probed,
    country: &str,
    as_cell: Option<&str>,
    verbose: bool,
    ink: Painter,
) -> String {
    let as_fact = if verbose {
        format!(", AS {}", as_cell.unwrap_or("unknown"))
    } else {
        String::new()
    };
    let freshness = if best.lag_secs == 0 {
        "freshest"
    } else {
        "closest"
    };
    let signature = if verbose {
        format!("{freshness} {}", signature_words(&best.signature))
    } else {
        format!("{freshness}, verified {}", signature_cell(&best.signature))
    };
    format!(
        "  {} — {signature}, {country}{as_fact}, {}m behind, {:.1} MB/s, {} ms latency",
        ink.bold(&best.url),
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
fn rejected_section(rejected: &[Rejected], ink: Painter) -> Vec<String> {
    let mut groups: std::collections::BTreeMap<String, Vec<String>> =
        std::collections::BTreeMap::new();
    for r in rejected {
        let item = match r.kind {
            Reject::Stale(secs) => format!("{} ({} h)", r.host, secs / 3600),
            _ => r.host.clone(),
        };
        groups.entry(r.kind.label()).or_default().push(item);
    }
    let mut out = vec![ink.bold(&format!("Rejected — {}", rejected.len()))];
    for (label, items) in groups {
        out.push(ink.red(&format!("  {label:<17} {}", items.join(", "))));
    }
    out
}

/// `--verbose` says why for each mirror on its own line, instead of grouping.
fn rejected_detail(rejected: &[Rejected], ink: Painter) -> Vec<String> {
    let mut out = vec![ink.bold(&format!("Rejected — {}", rejected.len()))];
    for r in rejected {
        out.push(ink.red(&format!(
            "  {:<40} {:<17} {}",
            r.host,
            r.kind.label(),
            r.why
        )));
    }
    out
}

/// The `Rejected` section, or nothing at all: a run that rejected nobody prints no heading and no
/// `0` count (SPEC.md Output). `Report::block` skips an empty section, so the caller needs no case.
fn rejected_lines(rejected: &[Rejected], verbose: bool, ink: Painter) -> Vec<String> {
    if rejected.is_empty() {
        return Vec::new();
    }
    if verbose {
        rejected_detail(rejected, ink)
    } else {
        rejected_section(rejected, ink)
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
    ink: Painter,
) -> Vec<String> {
    let mut out = vec![ink.bold("Detail")];
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
fn sources_section(root: &Path, inventory: &sources::Inventory, ink: Painter) -> Vec<String> {
    let mut out = vec![titled("Sources", &root.display().to_string(), ink)];
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
fn plan_changes(
    root: &Path,
    inventory: &sources::Inventory,
    plan: &plan::Plan,
    ink: Painter,
) -> Vec<String> {
    let mut out = Vec::new();
    for write in plan.writes() {
        let relative = write.target.relative().display().to_string();
        if write.effect == plan::Effect::Create {
            out.push(format!("  {relative}  (new file, mode 0600)"));
            for line in String::from_utf8_lossy(&write.new_bytes).lines() {
                out.push(ink.green(&format!("    + {line}")));
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
                        out.push(ink.red(&format!("    - {uri}")));
                        match plan.authority() {
                            plan::Authority::Mirror { uri, .. } => {
                                out.push(ink.green(&format!("    + {uri}")));
                            }
                            // a restore plan has no mirror URI; D11b prints its own block
                            plan::Authority::Restore { .. } => {}
                        }
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

/// The body of the `Planned change` section for a plan that will be written: the URI pairs and the
/// backup directory the write creates. `backup` is `None` for a `--list` run, which creates no backup
/// directory. What the write leaves alone is not here: `Sources` already reported every skip, and the
/// confirmation repeats none of them (SPEC.md CLI). Alternatives appear only when the run has some,
/// and this run has none — extra mirrors as comments are a TUI option.
fn planned_change_lines(
    root: &Path,
    inventory: &sources::Inventory,
    plan: &plan::Plan,
    backup: Option<&manifest::BackupId>,
    ink: Painter,
) -> Vec<String> {
    let mut out = plan_changes(root, inventory, plan, ink);
    if let Some(id) = backup {
        out.push(format!(
            "  backup: {}/{} (every replaced file is copied there, mode 0700, before the first write)",
            apply::BACKUP_ROOT,
            id.as_str()
        ));
    }
    out
}

fn run() -> Result<u8, (u8, String)> {
    let args = parse_args().map_err(|e| (2, e))?;
    // SPEC.md Output: colour is decided once, at startup, from the three inputs that all have to
    // agree. Nothing below re-reads a terminal or an environment variable, so a report cannot be
    // half painted.
    let ink = Painter::new(colour_enabled(
        io::stdout().is_terminal(),
        args.no_color,
        no_color_from_env(),
    ));
    // The uid decides whether this run may write (SPEC.md CLI). `require_root` is the writer's own
    // check, so the decision and the writer can never disagree about what root is.
    let root = apply::require_root().is_ok();
    let mode = decide(args.restore, args.list, root, args.yes);
    // `--restore` replaces the source of the rewrite: the newest backup instead of the mirror
    // pipeline. It reaches no distribution, no geolocation and no network, so it branches here,
    // before anything the mirror run would do (SPEC.md CLI).
    if let Mode::Restore { confirm } = mode {
        return run_restore(&args, ink, root, confirm);
    }
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
    // `--any` needs no country at all: the country is display-only there, so a failed detection is
    // not a reason to stop (SPEC step 2 and step 4).
    let country = match (args.country.clone().or(geo.country.clone()), args.any) {
        (Some(found), _) => Some(found),
        (None, true) => None,
        (None, false) => {
            return Err((
                2,
                "cannot tell which country you are in (every geolocation source failed or was skipped). Pass it with --country <CC>, for example --country LT.".to_string(),
            ));
        }
    };
    // Where the country in the header came from, and whether it constrains anything. `--any` says so
    // in the header itself, because a run that ignores the country must not look narrowed to it.
    let country_origin = origin_label(args.any, country.is_some(), args.country.is_some());

    let country_name = match &country {
        Some(cc) => Some(countries::launchpad_name(cc).ok_or_else(|| {
            (
                2,
                format!("unknown country code '{cc}'; Launchpad lists mirrors in 82 countries, for example LT, NO, DE, GB, US"),
            )
        })?),
        None => None,
    };
    // What the header prints, and the name a widened run's note repeats.
    let country_label = country_name.unwrap_or("any");

    // SPEC.md Output: progress is for a person watching the run work, so the sink is inert unless
    // standard error is a terminal. The clock starts here, before the list loads, so the summary
    // spans the four phases it names.
    let mut progress = Progress::new(StderrSink::new(), io::stderr().is_terminal());
    let LoadedCandidates {
        found,
        source: list_source,
        roots: list_roots,
        rest,
    } = load_with_progress(
        &net,
        country.as_deref().map(|cc| Country {
            cc,
            name: country_name.unwrap_or(""),
        }),
        !args.any,
        args.https_only,
        &mut progress,
    )?;
    // Different list sources can name the same mirror twice (with and without a trailing slash).
    let mut seen = std::collections::HashSet::new();
    let mut kept = found;
    kept.retain(|m| seen.insert(candidate_key(&m.url)));
    // A narrowed set keeps list order and takes the first 30; a set that is not narrowed to one
    // country is capped by Launchpad's declared link speed, then URL, because the page names about
    // 560 mirrors and probing them all is not a burst any run makes (SPEC step 4).
    let cands = if args.any {
        global_selection(&kept, MAX_CANDIDATES)
    } else {
        kept.truncate(MAX_CANDIDATES);
        kept
    };
    // What this pass probes, so the one widening can never probe a mirror twice.
    let probed_keys: std::collections::HashSet<String> =
        cands.iter().map(|m| candidate_key(&m.url)).collect();

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
        country_label,
        country_origin,
        ink,
    ));
    report.block(sources_section(
        Path::new(apply::APT_DIR),
        &apt_sources,
        ink,
    ));
    if !apt_sources.has_rewritable_uri() {
        report.line(
            "No official Ubuntu apt source URI can be changed; stopping before mirror probes."
                .to_string(),
        );
        report.line(no_eligible_hint(list_source));
        // No phase after the list load ran, so there is no summary to print: the line is erased and the
        // animation joined, so the report that follows starts on its own line and nothing can draw
        // over it (SPEC.md Output).
        progress.stop();
        report.print();
        return no_eligible_uri(mode, &args, list_source);
    }

    progress.reference();
    let reference = match probe::fetch_reference(&net, REFERENCE, &codename) {
        Ok(r) => Some(r),
        Err(probe::ReferenceError::NotFound(url)) => {
            progress.clear();
            return Err((
                4,
                format!(
                    "{url} answered 404: {codename} is no longer on archive.ubuntu.com (older releases move to old-releases.ubuntu.com, which has no mirrors to choose from). Nothing to do for this release."
                ),
            ));
        }
        Err(e) => {
            // A warning keeps its own line: the live progress line is erased first, so the two never
            // share one (SPEC.md Output).
            progress.clear();
            eprintln!(
                "warning: cannot read the reference release from {REFERENCE}: {e}. Using the newest mirror date as the baseline instead, so lag is relative to the best candidate."
            );
            None
        }
    };
    let mut rejected: Vec<Rejected> = Vec::new();
    let mut results = probe_candidates(&cands, &codename, &arch, &mut progress, &mut rejected);

    // SPEC step 4: one extra pass, and only from an empty set. `--any` already made the set every
    // country, so an empty one means nothing usable anywhere. Everything below ranks one union and
    // never learns that a widening happened, except the `note:` line the header block carries.
    let widening = if args.any {
        Widening::NotAsked
    } else {
        widen_if_empty(
            &mut results,
            rest.as_deref(),
            &probed_keys,
            country_label,
            reference.as_ref(),
            MAX_CANDIDATES,
            &mut |batch| probe_candidates(batch, &codename, &arch, &mut progress, &mut rejected),
        )
    };

    // Read-only ranking baseline (SPEC step 8): the canonical archive; when it was unreachable, the
    // newest candidate date, verified or not, because ranking must work without `gpgv`. Read over the
    // union, so a widened mirror is not called stale against pass 1's newest date.
    let baseline = ranking_baseline(reference.as_ref(), &results);
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
        // The AS lookup is not one of the four phases SPEC.md Output names, and it may print a note of
        // its own; the live line goes first so a note keeps its own line.
        progress.clear();
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
    progress.ranked();
    let ranked = fresh::rank(probed);
    let rows = table_rows(
        &ranked,
        &details,
        &mirror_asn,
        server_asn.as_deref(),
        args.top,
        args.verbose,
    );
    report.block({
        let mut header = vec![header_context(
            server_asn.as_deref(),
            list_source,
            reference.as_ref(),
            args.verbose,
        )];
        // SPEC.md Output: the reason a run widened is one line directly under the header and before
        // the table. It belongs to this block, not the ranked one, so the no-fresh-mirror return
        // prints it too.
        header.extend(set_note(
            &widening,
            args.any,
            rest.is_none(),
            country_label,
            country.as_deref().unwrap_or("unknown"),
            list_source,
        ));
        header
    });
    let mut ranked_lines = vec![ranked_title(rows.len(), ranked.len(), rejected.len(), ink)];
    ranked_lines.extend(ranked_rows(&rows, ink));
    // A `--verbose` run with no AS data says why under the table, once, instead of printing a column
    // of `unknown` that implies the lookup happened (SPEC step 9). The default table has no such
    // column, so it has no gap to explain.
    ranked_lines.extend(as_note(&rows, as_error.as_deref(), args.verbose));
    report.block(ranked_lines);
    // SPEC.md Output: the status line ends with the ranking it was counting, and becomes the one line
    // that stays. Everything below only assembles what the report will say, so a failure there cannot
    // glue its message onto a live line.
    progress.finish(ranked.len(), rejected.len());
    if args.verbose && !rows.is_empty() {
        report.block(detail_section(&rows, &details, &mirror_asn, ink));
    }
    let Some(best) = ranked.first() else {
        report.block(rejected_lines(&rejected, args.verbose, ink));
        report.print();
        return Err(no_fresh_mirror());
    };
    // The AS cell is only asked for in a `--verbose` run: the default recommendation names no AS, so
    // the map lookup that would fill it in is not even made (SPEC.md Output).
    let best_cell = args
        .verbose
        .then(|| as_cell(&mirror_asn, server_asn.as_deref(), &best.url))
        .flatten();
    // The mirror's own country, not the run's: only the mirror's is a fact about the mirror (SPEC.md
    // CLI), and the two differ once a run is not narrowed to one country.
    let best_country = recommendation_country(&details, &best.url);
    report.block(vec![
        ink.bold("Recommendation"),
        recommendation(best, best_country, best_cell.as_deref(), args.verbose, ink),
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
            ink,
        )];
        match build_plan(
            Path::new(apply::APT_DIR),
            &apt_sources,
            &roots,
            best,
            platform,
        ) {
            Ok(wanted) => match wanted {
                Some(plan) => {
                    planned.extend(planned_change_lines(
                        Path::new(apply::APT_DIR),
                        &apt_sources,
                        &plan,
                        write_timing.as_ref().map(|(_, id)| id),
                        ink,
                    ));
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
    report.block(rejected_lines(&rejected, args.verbose, ink));
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
                confirm_write(
                    &mut io::stdin().lock(),
                    &format!("\n{} ", ink.bold("Apply this change? [y/N]")),
                )?;
            }
            let outcome = apply::apply(&plan, trusted_baseline.as_ref(), now)
                .map_err(|e| (e.exit_code(), e.to_string()))?;
            println!(
                "\nbackup: {}/{}",
                apply::BACKUP_ROOT,
                outcome.backup_id.as_str()
            );
            let touched: Vec<String> = outcome
                .touched
                .iter()
                .map(|t| {
                    let path = format!("{}/{}", apply::APT_DIR, t.target.relative().display());
                    match t.effect {
                        plan::Effect::Remove => format!("removed {path}"),
                        plan::Effect::Create | plan::Effect::Replace => format!("wrote {path}"),
                    }
                })
                .collect();
            for line in &touched {
                println!("{line}");
            }
            match plan.authority() {
                plan::Authority::Mirror { uri, .. } => println!(
                    "\nchanged {}: every eligible official archive URI in those files now points at \
                     {uri}.\nNext: run 'apt update' to refresh the package lists.",
                    touched.join(", ")
                ),
                // a restore plan has no mirror URI; D11b prints its own block
                plan::Authority::Restore { .. } => {}
            }
            Ok(0)
        }
        // `--restore` returned before the mirror pipeline started; it can never reach this match.
        Mode::Restore { .. } => Err((
            1,
            "a restore does not build a mirror report, so this is a bug in apt-scout; nothing was written."
                .to_string(),
        )),
    }
}

/// The restore mode, from the parsed flags and the uid: the one line for a non-root run, then — as
/// root — re-derive the newest backup, confirm once unless `--yes`, restore it and report.
///
/// The plan is built here for the confirmation and built once more inside `restore::run`, which
/// re-reads the backup and hands its own plan to the writer: the prompt is a safety rail, not a
/// control, and the second derivation re-checks every record it is about to write, so a backup that
/// changed after the prompt is refused or re-derived, never written unchecked.
fn run_restore(args: &Args, ink: Painter, root: bool, ask: bool) -> Result<u8, (u8, String)> {
    if !root {
        // SPEC.md CLI: a non-root restore prints the one line and exits 3 before the backup is
        // opened. Nothing below this point has read a byte of it.
        println!("{}", sudo_command(args));
        return Ok(3);
    }
    let backup_root = Path::new(apply::BACKUP_ROOT);
    let apt_root = Path::new(apply::APT_DIR);
    // The id this restore creates is fixed before the confirmation, so the prompt and the write
    // name the same directory, exactly as the mirror path names its own backup.
    let now = now_secs()?;
    let plan = restore::build(backup_root, apt_root).map_err(|e| (e.exit_code(), e.to_string()))?;
    let plan::Authority::Restore { backup } = plan.authority() else {
        return Err((
            1,
            "the restore plan is authorized by no backup, so nothing was written. This is a bug in apt-scout; re-run it."
                .to_string(),
        ));
    };
    let created = manifest::BackupId::from_secs(now).map_err(|e| {
        (
            1,
            format!("cannot name the backup directory for {now}: {e}. Nothing was written."),
        )
    })?;
    if ask {
        for line in restore_confirm_lines(backup, &plan, &created) {
            println!("{line}");
        }
        match confirm(
            &mut io::stdin().lock(),
            &format!("\n{} ", ink.bold("Restore this backup? [y/N]")),
        )? {
            Answer::Yes => {}
            // A no is the user's decision, not a failure: the run did what it was told and there
            // is a backup id to show for it, so it exits 0. The write path refuses a no instead,
            // because there the write is the whole point of the run.
            Answer::No => {
                println!("Nothing was written: the backup is still the newest one.");
                return Ok(0);
            }
        }
    }
    let outcome =
        restore::run(backup_root, apt_root, now).map_err(|e| (e.exit_code(), e.to_string()))?;
    let mut report = Report::default();
    report.line(ink.bold("Restore"));
    report.block(restore_lines(backup, &plan, &outcome, args.verbose));
    report.print();
    Ok(0)
}

/// What the confirmation names before a restore writes anything: the backup it reads, the files it
/// writes or removes, and the backup it creates for what it replaces.
fn restore_confirm_lines(
    restored: &manifest::BackupId,
    plan: &plan::Plan,
    created: &manifest::BackupId,
) -> Vec<String> {
    let mut lines = vec![format!("Restore backup {}", restored.as_str())];
    for write in plan.writes() {
        let path = format!("{}/{}", apply::APT_DIR, write.target.relative().display());
        let verb = match write.effect {
            plan::Effect::Remove => "remove",
            plan::Effect::Create | plan::Effect::Replace => "write",
        };
        lines.push(format!("  {verb} {path}"));
    }
    lines.push(format!(
        "  copies: {}/{}",
        apply::BACKUP_ROOT,
        created.as_str()
    ));
    lines
}

/// The one block a finished restore prints: the backup it put back, the files it wrote and removed,
/// and where it copied what it replaced. `--verbose` adds the digest, mode and owner the writer
/// already holds for each target, and nothing here is a mirror report's heading or a second style.
fn restore_lines(
    restored: &manifest::BackupId,
    plan: &plan::Plan,
    outcome: &apply::Outcome,
    verbose: bool,
) -> Vec<String> {
    let mut lines = vec![format!("  backup: {}", restored.as_str())];
    for touched in &outcome.touched {
        let path = format!("{}/{}", apply::APT_DIR, touched.target.relative().display());
        let verb = match touched.effect {
            plan::Effect::Remove => "removed",
            plan::Effect::Create | plan::Effect::Replace => "wrote",
        };
        lines.push(format!("  {verb} {path}"));
        if !verbose {
            continue;
        }
        // The detail is the plan's, because the same write set is what the writer was handed; a
        // target the writer changed that the plan does not name cannot happen (the plan is the
        // write set) and prints only the line above.
        let Some(write) = plan.writes().iter().find(|w| w.target == touched.target) else {
            continue;
        };
        if !matches!(touched.effect, plan::Effect::Remove) {
            lines.push(format!(
                "    now sha256:{} mode {:04o} owner {}:{}",
                digest::hex(&write.digest),
                write.original.as_ref().map_or(0, |s| s.mode),
                write.original.as_ref().map_or(0, |s| s.uid),
                write.original.as_ref().map_or(0, |s| s.gid),
            ));
        }
        if let Some(original) = &write.original {
            lines.push(format!(
                "    was sha256:{} mode {:04o} owner {}:{}",
                digest::hex(&digest::sha256(&original.bytes)),
                original.mode,
                original.uid,
                original.gid,
            ));
        }
    }
    lines.push(format!(
        "  copies: {}/{}",
        apply::BACKUP_ROOT,
        outcome.backup_id.as_str()
    ));
    lines
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

/// The `verified` column: three values, and only one of them authorizes a write (SPEC: Freshness
/// trust boundary). `yes` is an `InRelease` that `/usr/bin/gpgv` accepted with the Ubuntu archive
/// keyring, `no` is a body that was readable but unsigned, altered or signed by an unknown key, and
/// `?` is verification that could not run at all — no `gpgv` or keyring, an oversized body, a
/// timeout. So `?` is never a pass: the cell says "could not check", not "checked and fine".
fn signature_cell(status: &verify::SignatureStatus) -> &'static str {
    match status {
        verify::SignatureStatus::Verified(_) => "yes",
        verify::SignatureStatus::Invalid(_) => "no",
        verify::SignatureStatus::Unavailable(_) => "?",
    }
}

/// The `verified` column in its ink: `yes` green, `no` and `?` yellow (SPEC.md Output). The cell
/// names itself; the ink repeats it for a reader who scans the column, and it never paints `?` the
/// colour of a pass.
fn signature_cell_painted(status: &verify::SignatureStatus, ink: Painter) -> String {
    let cell = signature_cell(status);
    match status {
        verify::SignatureStatus::Verified(_) => ink.green(cell),
        verify::SignatureStatus::Invalid(_) | verify::SignatureStatus::Unavailable(_) => {
            ink.yellow(cell)
        }
    }
}

/// The same status in words, for `--verbose` alone: the exact status is the proof behind the table's
/// `yes`/`no`/`?`, and proof is what `-v` adds (SPEC.md Output). A default run asks
/// `signature_cell` for the column's word instead.
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

/// The country a run narrows to: the code the `mirrors.ubuntu.com` fallback needs and the name
/// Launchpad groups its entries under. `None` only under `--any` with no country known, where the
/// country is display-only and nothing needs narrowing (SPEC step 4).
struct Country<'a> {
    cc: &'a str,
    name: &'a str,
}

/// What one mirror-list source produced. `found` is what this run probes; `rest` is the same loaded
/// list un-narrowed, kept so the one widening of SPEC step 4 can add every other country without a
/// second fetch, and `None` when the list names one country and cannot widen. `roots` is the rewrite
/// eligibility set (see `load_candidates` for its contract).
#[derive(Debug)]
struct LoadedCandidates {
    found: Vec<mirrors::Mirror>,
    source: &'static str,
    roots: Vec<sources::MirrorRoot>,
    rest: Option<Vec<mirrors::Mirror>>,
}

/// `cap` mirrors out of a set that is **not** narrowed to one country, chosen by Launchpad's own
/// declared link speed, descending, then by URL so the answer is deterministic. `None` sorts last:
/// a mirror that declares no speed is not read as a fast one (SPEC step 4). This decides what is
/// probed and never what ranks.
fn global_selection(all: &[mirrors::Mirror], cap: usize) -> Vec<mirrors::Mirror> {
    let speed = |m: &mirrors::Mirror| mirrors::declared_speed_bps(m.speed.as_deref().unwrap_or(""));
    let mut sorted: Vec<&mirrors::Mirror> = all.iter().collect();
    sorted.sort_by(|a, b| speed(b).cmp(&speed(a)).then_with(|| a.url.cmp(&b.url)));
    sorted.into_iter().take(cap).cloned().collect()
}

/// The dedupe key of one candidate: a trailing slash and the case of the host do not make a second
/// mirror, so the same entry can never be probed twice (SPEC step 4).
fn candidate_key(url: &str) -> String {
    url.trim_end_matches('/').to_ascii_lowercase()
}

/// What pass 1 found out about SPEC step 4's one widening: whether it was not asked for, not needed,
/// done, or impossible with the list this run loaded.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Widening {
    /// `--any`: the candidate set already is every country, so an empty one means nothing usable
    /// anywhere and there is nothing to widen from or to.
    NotAsked,
    /// The run's own country had a usable mirror. The set is untouched and nothing is spawned.
    NotNeeded,
    /// `added` mirrors from other countries were probed and appended to the same results.
    Widened { added: usize },
    /// The only list this run loaded names one country, so there is nothing to widen to.
    Impossible,
}

/// One probe batch's answers, one per mirror that produced a result: a crashed probe thread is
/// reported by whoever ran the batch, so a batch answer is never ambiguous about a mirror.
type ProbeBatch = Vec<(mirrors::Mirror, Result<probe::Outcome, probe::ProbeError>)>;

/// The freshness baseline a read-only run ranks against (SPEC step 8): the canonical reference;
/// when it was unreachable, the newest sane candidate date, verified or not, because ranking must
/// work without `gpgv`. It is a function of the candidate set, so a widened run ranks the union
/// against the union's newest date instead of pass 1's.
fn ranking_baseline(
    reference: Option<&probe::Reference>,
    results: &[(mirrors::Mirror, Result<probe::Outcome, probe::ProbeError>)],
) -> i64 {
    reference
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
        .unwrap_or(0)
}

/// How many of a pass's candidates produced a usable mirror: a probe that answered and whose
/// `-updates` date is not stale against `baseline`. Every other outcome — unreachable, non-200, no
/// usable `InRelease` or `Date`, no speed sample, a crashed probe — is not usable.
fn usable_mirrors(
    results: &[(mirrors::Mirror, Result<probe::Outcome, probe::ProbeError>)],
    baseline: i64,
) -> usize {
    results
        .iter()
        .filter(|(_, answer)| match answer {
            Ok(o) => !fresh::is_stale(fresh::lag(baseline, o.updates_date)),
            Err(_) => false,
        })
        .count()
}

/// SPEC step 4's one automatic widening, and the only place it may happen: when the run's own
/// country yielded nothing usable, add the mirrors the **already loaded** list names for every other
/// country, probe them in one extra pass and append them to the same results, so one pipeline ranks
/// one union. `--any` never widens. A run that found one usable mirror in its own country never
/// widens, and no candidate is probed twice: both are the difference between an answer to an empty
/// set and a preference for foreign mirrors.
fn widen_if_empty(
    results: &mut Vec<(mirrors::Mirror, Result<probe::Outcome, probe::ProbeError>)>,
    rest: Option<&[mirrors::Mirror]>,
    already: &std::collections::HashSet<String>,
    own_country: &str,
    reference: Option<&probe::Reference>,
    cap: usize,
    probe_batch: &mut dyn FnMut(&[mirrors::Mirror]) -> ProbeBatch,
) -> Widening {
    if usable_mirrors(results, ranking_baseline(reference, results)) > 0 {
        return Widening::NotNeeded;
    }
    let Some(rest) = rest else {
        return Widening::Impossible;
    };
    // The same dedupe key as pass 1, seeded with what pass 1 probed, so a mirror that is in the
    // un-narrowed list *and* in the country's own set is never probed twice.
    let mut seen = already.clone();
    let mut others: Vec<mirrors::Mirror> = Vec::new();
    for m in rest {
        if m.country == own_country || !seen.insert(candidate_key(&m.url)) {
            continue;
        }
        others.push(m.clone());
    }
    let selection = global_selection(&others, cap);
    if selection.is_empty() {
        // Nothing outside the country is left to probe, so the candidate set does not change and
        // there is nothing to announce: the run fails exactly as it did before.
        return Widening::Widened { added: 0 };
    }
    let mut added = 0;
    for (m, answer) in probe_batch(&selection) {
        results.push((m, answer));
        added += 1;
    }
    Widening::Widened { added }
}

/// The one line a widened run owes the user, in `as_note`'s voice, printed under the report header
/// and before the table: the one thing this feature must not do is silently prefer a foreign mirror
/// (SPEC step 4). `--any` never widens, and a run whose candidate set did not change says nothing.
fn set_note(
    widening: &Widening,
    any: bool,
    rest_is_none: bool,
    country: &str,
    cc: &str,
    source: &str,
) -> Option<String> {
    match widening {
        Widening::Widened { added } if *added > 0 => Some(format!(
            "note: no usable mirror in {country} ({cc}), so every country's mirrors were added and ranked together"
        )),
        Widening::Impossible => Some(format!(
            "note: no usable mirror in {country} ({cc}), and the list from {source} names only that country, so there is nothing to widen to"
        )),
        // `--any` cannot widen, so a run whose only list names one country says which set it read
        // instead of presenting one country's mirrors as every country's.
        _ if any && rest_is_none => Some(format!(
            "note: --any, but the list from {source} names only one country"
        )),
        _ => None,
    }
}

/// The `(host, path)` pairs of `urls`, in the order the list gave them, plus every URL that failed
/// candidate validation. A URL whose authority or path does not parse contributes nothing, so a tree
/// is never guessed from a broken link.
fn mirror_roots<'a>(
    urls: impl Iterator<Item = &'a str>,
) -> (Vec<sources::MirrorRoot>, Vec<String>) {
    let mut roots = Vec::new();
    let mut rejected = Vec::new();
    for url in urls {
        match mirrors::validate_url(url) {
            Ok(()) => match sources::mirror_root(url) {
                Some(root) => roots.push(root),
                None => rejected.push(url.to_string()),
            },
            Err(_) => rejected.push(url.to_string()),
        }
    }
    (roots, rejected)
}

/// The warning for the entries a list named that never entered the rewrite set, or `None` when every
/// entry entered it. An entry that fails candidate validation is left out **and said so**, on the same
/// channel as the fallback warning, rather than silently trusted (SPEC: Apply). Returned as text, so
/// the caller decides when the line lands: it may have a live progress line to erase first
/// (SPEC.md Output).
fn rejected_note(list: &str, rejected: &[String]) -> Option<String> {
    if rejected.is_empty() {
        return None;
    }
    Some(format!(
        "warning: {list} names {} mirror URL(s) that cannot make an apt source rewritable, so they are left out of that set: {}",
        rejected.len(),
        rejected.join(", ")
    ))
}

/// The run's list phase: name it, do the load, and let what the load has to say land on lines of its
/// own. Extracted from `run()` so the order is pinned by a test rather than by reading it: the line
/// goes up **before** the work (the load is the longest silent stretch of a run — Launchpad up to
/// 10 s, then `mirrors.ubuntu.com` up to another 10 s) and is erased after it, so a warning or an
/// error that follows never lands on top of a live line (SPEC.md Output).
fn load_with_progress<S: Sink + Send + 'static>(
    net: &dyn Fetch,
    country: Option<Country<'_>>,
    narrow: bool,
    https_only: bool,
    progress: &mut Progress<S>,
) -> Result<LoadedCandidates, (u8, String)> {
    progress.load();
    let mut notes = Vec::new();
    let loaded = load_candidates(net, country, narrow, https_only, &mut notes);
    // The load is over whether it worked or not, so its notes get lines of their own and the error
    // that may follow starts where the live line was.
    progress.clear();
    for note in &notes {
        eprintln!("{note}");
    }
    loaded
}

/// SPEC step 3: Launchpad first, then `mirrors.ubuntu.com/<CC>.txt`. Returns the candidates, the
/// name of the source that supplied them, the run's mirror roots and the un-narrowed list; every
/// failed source is reported with its reason.
///
/// `narrow` is false under `--any`, where the country stops constraining the candidate set: `found`
/// is then every kept mirror the list names, whatever country it puts it in (SPEC step 4).
///
/// A Launchpad page that parsed is a loaded list even when the country filter kept nothing: `found`
/// is then empty and `rest` is every other country's entry, so a run whose own country the page does
/// not name reaches the one widening of SPEC step 4 instead of failing with no list at all. The
/// `<CC>.txt` fallback still has its turn first (SPEC step 3), and only a fallback that names
/// nothing of its own hands the page back.
///
/// The lines this load has to tell the user are pushed onto `notes` instead of written here, because
/// the caller owns a live progress line that has to be erased before the first of them lands
/// (SPEC.md Output). A note is produced whether the load succeeds or not, so the caller prints them
/// before it handles the result.
///
/// Contract for `roots`: every mirror in the returned `Vec<Mirror>` has its `(host, path)` in it.
/// From Launchpad that set is every host and path the page names, in every country, collected
/// before the country filter and before `keep`, so a mirror a previous run wrote stays rewritable
/// after the operator changes country and `--https-only` does not narrow what is recognized (SPEC:
/// Apply). When the run falls back to `<CC>.txt`, the set is that file's validated entries, because
/// that list arrives over plain HTTP: the set fails closed rather than widening on an unverified list.
fn load_candidates(
    net: &dyn Fetch,
    country: Option<Country<'_>>,
    narrow: bool,
    https_only: bool,
    notes: &mut Vec<String>,
) -> Result<LoadedCandidates, (u8, String)> {
    let keep =
        |m: &mirrors::Mirror| mirrors::validate_url(&m.url).is_ok() && (!https_only || m.https);
    let mut failures = Vec::new();
    // A page that answered stays in hand even when the country filter kept nothing, because a country
    // with no entry at all is exactly the case SPEC step 4's widening exists for and that page names
    // every other country. It is held, not returned: the fallback below still has its turn (SPEC step
    // 3 reads it when Launchpad returns no mirror for the country), and this page is what the run
    // falls back to when the fallback yields nothing.
    let mut answered_by_launchpad: Option<LoadedCandidates> = None;
    match net.get(LAUNCHPAD, http::MAX_PAGE, None) {
        Ok(page) => match mirrors::parse_launchpad(&String::from_utf8_lossy(&page.body)) {
            Ok(all) => {
                // Every country's entry, not only this run's, and before `keep`: eligibility answers
                // recognizability, never use. Both list sources pass the same validation here.
                let (roots, rejected) = mirror_roots(all.iter().map(|m| m.url.as_str()));
                notes.extend(rejected_note("Launchpad", &rejected));
                // The same list un-narrowed, so the one widening can add another country's mirrors
                // without a second fetch of this page. `--https-only` already applied by `keep`.
                let kept: Vec<_> = all.into_iter().filter(|m| keep(m)).collect();
                let found: Vec<_> = if narrow {
                    let own = country.as_ref().map(|c| c.name);
                    kept.iter()
                        .filter(|m| Some(m.country.as_str()) == own)
                        .cloned()
                        .collect()
                } else {
                    kept.clone()
                };
                if !found.is_empty() {
                    return Ok(LoadedCandidates {
                        found,
                        source: "Launchpad",
                        roots,
                        rest: Some(kept),
                    });
                }
                failures.push(match country.as_ref() {
                    Some(c) if narrow => {
                        format!("Launchpad lists no usable mirrors for {}", c.name)
                    }
                    _ => "Launchpad lists no usable mirrors".to_string(),
                });
                // The country kept nothing while the page names mirrors somewhere: those mirrors are
                // every country but this run's, so the page is what a widening would add.
                if !kept.is_empty() {
                    answered_by_launchpad = Some(LoadedCandidates {
                        found,
                        source: "Launchpad",
                        roots,
                        rest: Some(kept),
                    });
                }
            }
            Err(e) => failures.push(format!("cannot read {LAUNCHPAD}: {e}")),
        },
        Err(e) => failures.push(format!("cannot load the Launchpad mirror list: {e}")),
    }

    // Anything the fallback pushed here belongs to a list this run does not end up using, so it goes
    // when the page that already answered is the answer.
    let notes_before_fallback = notes.len();

    // `mirrors.ubuntu.com` names one country per run and no run fetches another country's file, so
    // without a country there is no file to ask for and nothing to widen to (SPEC step 4). The list
    // that is left cannot supply every country, and the run says so instead of pretending.
    let Some(country) = country else {
        failures.push(
            "the mirrors.ubuntu.com fallback needs a country code, and --any without a country has none"
                .to_string(),
        );
        return Err((1, no_mirror_list(&failures)));
    };
    let url = format!("{TEXT_LIST}/{}.txt", country.cc);
    match net.get(&url, http::MAX_SMALL, None) {
        Ok(got) => {
            let all: Vec<_> = mirrors::parse_text_list(&String::from_utf8_lossy(&got.body))
                .into_iter()
                .map(|u| mirrors::Mirror {
                    country: country.name.to_string(),
                    name: String::new(),
                    https: u.starts_with("https://"),
                    url: u,
                    speed: None,
                    status: mirrors::Status::Unknown,
                })
                .collect();
            // One validation for both sources: `mirror_roots` is the only place an entry enters the
            // set, so a list cannot skip the candidate check the other list applies. `--https-only`
            // still does not narrow it — it picks what may be selected, never what may be recognized,
            // so a mirror written over `http` stays rewritable in a later run.
            let (roots, rejected) = mirror_roots(all.iter().map(|m| m.url.as_str()));
            notes.extend(rejected_note("mirrors.ubuntu.com", &rejected));
            let found: Vec<_> = all.into_iter().filter(keep).collect();
            if !found.is_empty() {
                notes.push(format!(
                    "warning: {}. Using {url} instead (its URLs come over plain HTTP, so they are unverified).",
                    failures.join("; ")
                ));
                return Ok(LoadedCandidates {
                    found,
                    source: "mirrors.ubuntu.com (unverified)",
                    roots,
                    // One country in this file: widening has nothing to reach, and the run says so.
                    rest: None,
                });
            }
            failures.push(format!("{url} has no usable mirrors"));
        }
        Err(e) => failures.push(format!("cannot load {url}: {e}")),
    }
    // Neither list gave this run a candidate, but one of them answered with the whole world: the run
    // ranks it after its one widening instead of failing on a country the page happens not to name.
    if let Some(page) = answered_by_launchpad {
        notes.truncate(notes_before_fallback);
        return Ok(page);
    }
    Err((1, no_mirror_list(&failures)))
}

/// The one sentence a run with no usable list fails with, so both exits from `load_candidates` say
/// the same thing.
fn no_mirror_list(failures: &[String]) -> String {
    format!(
        "no mirror list is available: {}. Check the network connection, or try another country with --country.",
        failures.join("; ")
    )
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

    /// `--no-geo` has nothing but `--country` or `--any` to tell it where the server is, so the usage
    /// error names both and either satisfies it (SPEC.md CLI: `--any` needs no country at all).
    #[test]
    fn no_geo_needs_a_country_or_any() {
        let args = parse_args_from(["--no-geo"].into_iter().map(str::to_string)).unwrap();
        assert_eq!(
            validate_geo_args(&args).unwrap_err(),
            "--no-geo needs --country <CC> or --any, because nothing else can tell which country you are in. Example: --no-geo --country LT"
        );
        let args = parse_args_from(
            ["--no-geo", "--country", "LT"]
                .into_iter()
                .map(str::to_string),
        )
        .unwrap();
        assert!(args.no_geo);
        assert_eq!(args.country.as_deref(), Some("LT"));
        // `--any` needs no country, so it is the other way to satisfy `--no-geo`.
        let args = parse_args_from(["-n", "-a"].into_iter().map(str::to_string)).unwrap();
        assert!(args.any);
        assert!(validate_geo_args(&args).is_ok());
        let args = parse_args_from(
            ["-n", "-a", "--country", "LT"]
                .into_iter()
                .map(str::to_string),
        )
        .unwrap();
        assert!(validate_geo_args(&args).is_ok());
        // The country code is still validated when one is given.
        let args =
            parse_args_from(["-a", "--country", "ZZ"].into_iter().map(str::to_string)).unwrap();
        assert!(validate_geo_args(&args).is_err());
    }

    /// Every approved short alias is the same flag as its long form: the same `Args`, so the same
    /// decisions. The pairing is the contract — a row that drifted from its long form would be a
    /// flag that silently changed meaning.
    #[test]
    fn short_aliases_parse_to_the_same_args_as_their_long_forms() {
        let parse = |argv: &[&str]| {
            parse_args_from(argv.iter().copied().map(str::to_string))
                .unwrap_or_else(|e| panic!("{argv:?} must parse: {e}"))
        };
        for (long, short) in [
            (&["--country", "LT"][..], &["-c", "LT"][..]),
            (&["--top", "3"][..], &["-t", "3"][..]),
            (&["--list"][..], &["-l"][..]),
            (&["--yes"][..], &["-y"][..]),
            (
                &["--no-geo", "--country", "LT"][..],
                &["-n", "-c", "LT"][..],
            ),
            (&["--https-only"][..], &["-s"][..]),
            (&["--verbose"][..], &["-v"][..]),
            (&["--any"][..], &["-a"][..]),
            (&["--restore"][..], &["-r"][..]),
        ] {
            assert_eq!(parse(short), parse(long), "{short:?} must be {long:?}");
        }
        // A run that typed only aliases still recommends a long-form command to copy.
        assert_eq!(
            sudo_command(&parse(&["-c", "LT", "-y", "-t", "3"])),
            "sudo apt-scout --country LT --top 3 --yes"
        );
        // A `sudo` line that dropped `--any` would run against a different candidate set.
        assert_eq!(
            sudo_command(&parse(&["-a", "-c", "DE"])),
            "sudo apt-scout --country DE --any"
        );
        // A restore's line is exactly the long form, whatever else the run was given: the backup
        // and the write are the whole command, and `-r` is spelled out for a reader copying it.
        for argv in [
            &["-r"][..],
            &["--restore", "--yes"][..],
            &["-r", "-v", "--no-color"][..],
        ] {
            assert_eq!(
                sudo_command(&parse(argv)),
                "sudo apt-scout --restore",
                "{argv:?}"
            );
        }
    }

    /// `-h` and `--help` are one request: both spellings print the same text and exit 0, and that
    /// text lists the short form beside every long one, `-h` included.
    #[test]
    fn help_is_recognized_in_both_spellings_and_lists_every_short_form() {
        assert!(help_requested("-h"));
        assert!(help_requested("--help"));
        // Neither a bundled flag nor a misspelling is a help request.
        assert!(!help_requested("-hx"));
        assert!(!help_requested("--helpp"));
        for (long, short) in [
            ("--help", "-h"),
            ("--country", "-c"),
            ("--any", "-a"),
            ("--no-geo", "-n"),
            ("--https-only", "-s"),
            ("--top", "-t"),
            ("--verbose", "-v"),
            ("--list", "-l"),
            ("--yes", "-y"),
            ("--restore", "-r"),
        ] {
            let pair = format!("{short}, {long}");
            assert!(
                HELP.contains(&pair),
                "{pair} is missing from --help:\n{HELP}"
            );
        }
    }

    /// One alias and one value per argument, never a bundle: `-lyc NO` stays an unknown option. The
    /// one reserved short stays unknown too, so `--refresh` can still claim it.
    #[test]
    fn unknown_and_bundled_short_options_are_refused() {
        for argv in [&["-lyc", "NO"][..], &["-ly"][..], &["-R"][..]] {
            let err = parse_args_from(argv.iter().copied().map(str::to_string))
                .expect_err(&format!("{argv:?} must be refused"));
            assert_eq!(
                err,
                format!("unknown option '{}'; try --help", argv[0]),
                "{argv:?}"
            );
        }
    }

    /// A value-taking alias names itself in its error, so the message points at the spelling the
    /// user typed instead of one they did not.
    #[test]
    fn a_missing_or_invalid_value_names_the_spelling_that_was_typed() {
        let err = |argv: &[&str]| {
            parse_args_from(argv.iter().copied().map(str::to_string))
                .expect_err(&format!("{argv:?} must be refused"))
        };
        assert_eq!(
            err(&["-c"]),
            "-c needs a two-letter country code, for example -c LT"
        );
        assert_eq!(
            err(&["--country"]),
            "--country needs a two-letter country code, for example --country LT"
        );
        assert_eq!(err(&["-t"]), "-t needs a number, for example -t 5");
        assert_eq!(err(&["--top"]), "--top needs a number, for example --top 5");
        assert_eq!(
            err(&["-c", "L1"]),
            "-c 'L1' is not a two-letter country code, for example LT"
        );
        assert_eq!(err(&["-t", "0"]), "-t '0' is not a positive number");
        // The value is the next argument, never a flag glued on: `-c -y` reads `-y` as its country.
        assert_eq!(
            err(&["-c", "-y"]),
            "-c '-Y' is not a two-letter country code, for example LT"
        );
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
        let LoadedCandidates {
            found: c,
            source: src,
            ..
        } = load_candidates(
            &net,
            Some(Country {
                cc: "LT",
                name: "Lithuania",
            }),
            true,
            false,
            &mut Vec::new(),
        )
        .unwrap();
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
        let LoadedCandidates {
            found: c,
            source: src,
            ..
        } = load_candidates(
            &net,
            Some(Country {
                cc: "LT",
                name: "Lithuania",
            }),
            true,
            false,
            &mut Vec::new(),
        )
        .unwrap();
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
        let LoadedCandidates {
            found: c,
            source: src,
            ..
        } = load_candidates(
            &net,
            Some(Country {
                cc: "IS",
                name: "Iceland",
            }),
            true,
            false,
            &mut Vec::new(),
        )
        .unwrap();
        assert_eq!(src, "mirrors.ubuntu.com (unverified)");
        assert_eq!(c.len(), 1);
    }

    #[test]
    fn https_only_filters_the_fallback() {
        let net = Canned(vec![
            (LAUNCHPAD, Err(503)),
            ("http://mirrors.ubuntu.com/LT.txt", Ok(LT_TXT)),
        ]);
        let LoadedCandidates { found: c, .. } = load_candidates(
            &net,
            Some(Country {
                cc: "LT",
                name: "Lithuania",
            }),
            true,
            true,
            &mut Vec::new(),
        )
        .unwrap();
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
        let LoadedCandidates {
            found: c, roots, ..
        } = load_candidates(
            &net,
            Some(Country {
                cc: "LT",
                name: "Lithuania",
            }),
            true,
            false,
            &mut Vec::new(),
        )
        .unwrap();
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

    /// The real page with one mirror's only `http` link pointed at a loopback address: a list entry
    /// that fails candidate validation. Fixture-derived on purpose, so the page keeps the shape the
    /// parser reads instead of being a hand-written one that could drift from it.
    fn launchpad_one_bad() -> &'static [u8] {
        Box::leak(
            String::from_utf8_lossy(LAUNCHPAD_OK)
                .replace("http://ftp.litnet.lt/ubuntu/", "http://127.0.0.1/ubuntu/")
                .into_bytes()
                .into_boxed_slice(),
        )
    }

    /// An entry that fails candidate validation, or whose path carries a dot segment, never enters the
    /// rewrite set, and the run reports it instead of trusting it: one `validate_url` filter for the
    /// Launchpad page and the fallback alike (SPEC: Apply).
    #[test]
    fn mirror_roots_drops_and_reports_invalid_entries() {
        let (roots, rejected) = mirror_roots(
            [
                "http://mirror.example/ubuntu/",
                "http://127.0.0.1/ubuntu/",
                "http://2130706433/ubuntu/",
                "http://mirror.example:0/ubuntu/",
                "http://mirror.example/ubuntu/../debian",
                "http://mirror.example/ubuntu/%2e%2e/debian",
            ]
            .into_iter(),
        );
        assert_eq!(
            roots,
            [sources::MirrorRoot {
                host: "mirror.example".to_string(),
                path: "/ubuntu/".to_string(),
            }]
        );
        assert_eq!(
            rejected,
            [
                "http://127.0.0.1/ubuntu/",
                "http://2130706433/ubuntu/",
                "http://mirror.example:0/ubuntu/",
                "http://mirror.example/ubuntu/../debian",
                "http://mirror.example/ubuntu/%2e%2e/debian",
            ]
        );

        // The same rule on the Launchpad side: its valid entries are still in the set, the invalid
        // one is in neither the set nor silence.
        let net = Canned(vec![(LAUNCHPAD, Ok(launchpad_one_bad()))]);
        let LoadedCandidates {
            source: src, roots, ..
        } = load_candidates(
            &net,
            Some(Country {
                cc: "DE",
                name: "Germany",
            }),
            true,
            false,
            &mut Vec::new(),
        )
        .unwrap();
        assert_eq!(src, "Launchpad");
        assert!(
            roots.iter().all(|r| r.host != "127.0.0.1"),
            "a loopback entry must not enter the set: {roots:?}"
        );
        assert!(
            roots.iter().any(|r| r.host == "ubuntu.uib.no"),
            "its valid entries are still there: {roots:?}"
        );
    }

    #[test]
    fn both_sources_failing_names_both_reasons() {
        let net = Canned(vec![
            (LAUNCHPAD, Err(503)),
            ("http://mirrors.ubuntu.com/LT.txt", Err(404)),
        ]);
        let (code, msg) = load_candidates(
            &net,
            Some(Country {
                cc: "LT",
                name: "Lithuania",
            }),
            true,
            false,
            &mut Vec::new(),
        )
        .unwrap_err();
        assert_eq!(code, 1);
        assert!(msg.contains("Launchpad"), "{msg}");
        assert!(msg.contains("LT.txt"), "{msg}");
        assert!(msg.contains("--country"), "{msg}");
    }

    /// One ordered log of what a run showed and what it asked the network for, so a test can pin which
    /// came first.
    /// Shared and `Send`, because the same log is written from two threads: the run draws its phase
    /// lines and the animation redraws them with each frame.
    #[derive(Clone, Default)]
    struct Trace(Arc<Mutex<Vec<String>>>);

    impl Trace {
        fn note(&self, what: impl Into<String>) {
            lock(&self.0).push(what.into());
        }

        fn lines(&self) -> Vec<String> {
            lock(&self.0).clone()
        }
    }

    /// The real sink's contract, recorded into the shared log.
    struct TracingSink(Trace);

    impl Sink for TracingSink {
        fn rewrite(&mut self, line: &str) {
            self.0.note(format!("show: {line}"));
        }

        fn clear(&mut self) {
            self.0.note("clear");
        }

        fn settle(&mut self, line: &str) {
            self.0.note(format!("settle: {line}"));
        }
    }

    /// A `Canned` net that enters every request into the same log.
    struct TracedNet {
        canned: Canned,
        trace: Trace,
    }

    impl Fetch for TracedNet {
        fn get(
            &self,
            url: &str,
            max_bytes: u64,
            range: Option<(u64, u64)>,
        ) -> Result<http::Fetched, http::HttpError> {
            self.trace.note(format!("fetch: {url}"));
            self.canned.get(url, max_bytes, range)
        }
    }

    /// The phase goes up **before** the work it names. The list load is up to 20 s of silence — 10 s of
    /// Launchpad, then 10 s of `mirrors.ubuntu.com` — so a line printed after it returns is the one
    /// phase a person never sees happen, which is the complaint the progress line exists to answer
    /// (SPEC.md Output).
    #[test]
    fn the_list_phase_is_shown_before_the_list_is_loaded() {
        let trace = Trace::default();
        let net = TracedNet {
            canned: Canned(vec![
                (LAUNCHPAD, Err(503)),
                ("http://mirrors.ubuntu.com/LT.txt", Ok(LT_TXT)),
            ]),
            trace: trace.clone(),
        };
        let mut progress = Progress::new(TracingSink(trace.clone()), true);
        let LoadedCandidates {
            found: cands,
            source: src,
            ..
        } = load_with_progress(
            &net,
            Some(Country {
                cc: "LT",
                name: "Lithuania",
            }),
            true,
            false,
            &mut progress,
        )
        .unwrap();
        assert!(!cands.is_empty(), "{src}");
        // The order is the contract, not the number of frames: the animation redraws the same phase
        // line while the work runs, so the assertions are about what came first and what came last.
        let events = trace.lines();
        let phase = events
            .iter()
            .position(|e| e.starts_with("show: ") && e.ends_with("loading the mirror list"))
            .unwrap_or_else(|| panic!("no phase line: {events:?}"));
        assert_eq!(
            phase, 0,
            "the phase line goes up before the work: {events:?}"
        );
        let first_fetch = events
            .iter()
            .position(|e| e.starts_with("fetch: "))
            .unwrap_or_else(|| panic!("nothing was fetched: {events:?}"));
        assert!(phase < first_fetch, "{events:?}");
        // Launchpad first, then the fallback, in that order.
        let launchpad = events
            .iter()
            .position(|e| e == "fetch: https://launchpad.net/ubuntu/+archivemirrors")
            .unwrap_or_else(|| panic!("Launchpad was not tried: {events:?}"));
        let fallback = events
            .iter()
            .position(|e| e == "fetch: http://mirrors.ubuntu.com/LT.txt")
            .unwrap_or_else(|| panic!("the fallback was not tried: {events:?}"));
        assert!(launchpad < fallback, "{events:?}");
        // The line is erased once the load is over, before the warning it produced prints, and that
        // erase is the last thing the sink sees.
        let cleared = events
            .iter()
            .position(|e| e == "clear")
            .unwrap_or_else(|| panic!("the line was never erased: {events:?}"));
        assert_eq!(
            cleared,
            events.len() - 1,
            "the erase comes last: {events:?}"
        );
    }

    /// The load's own lines are data now, not a side effect: the caller decides when they land, and a
    /// test can check what the run would say.
    #[test]
    fn a_fallback_load_reports_its_reasons_as_notes() {
        let net = Canned(vec![
            (LAUNCHPAD, Err(503)),
            ("http://mirrors.ubuntu.com/LT.txt", Ok(LT_TXT)),
        ]);
        let mut notes = Vec::new();
        let LoadedCandidates {
            found: cands,
            source: src,
            ..
        } = load_candidates(
            &net,
            Some(Country {
                cc: "LT",
                name: "Lithuania",
            }),
            true,
            false,
            &mut notes,
        )
        .unwrap();
        assert_eq!(src, "mirrors.ubuntu.com (unverified)");
        assert!(!cands.is_empty());
        assert_eq!(notes.len(), 1, "{notes:?}");
        assert!(
            notes[0].contains("cannot load the Launchpad mirror list"),
            "{}",
            notes[0]
        );
        assert!(
            notes[0].contains("Using http://mirrors.ubuntu.com/LT.txt instead"),
            "{}",
            notes[0]
        );
    }

    /// The rewrite-eligibility set is every host and path the Launchpad page names, in every country,
    /// so a mirror this tool wrote stays rewritable after the operator changes country (SPEC: Apply).
    #[test]
    fn the_host_list_names_every_country_not_only_the_run_s_country() {
        let net = Canned(vec![(LAUNCHPAD, Ok(LAUNCHPAD_OK))]);
        let LoadedCandidates {
            found: cands,
            source: src,
            roots,
            ..
        } = load_candidates(
            &net,
            Some(Country {
                cc: "DE",
                name: "Germany",
            }),
            true,
            false,
            &mut Vec::new(),
        )
        .unwrap();
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
        let LoadedCandidates {
            found: cands,
            roots,
            ..
        } = load_candidates(
            &net,
            Some(Country {
                cc: "DE",
                name: "Germany",
            }),
            true,
            true,
            &mut Vec::new(),
        )
        .unwrap();
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
        let LoadedCandidates {
            found: cands,
            roots,
            ..
        } = load_candidates(
            &net,
            Some(Country {
                cc: "LT",
                name: "Lithuania",
            }),
            true,
            true,
            &mut Vec::new(),
        )
        .unwrap();
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
            let LoadedCandidates {
                found: cands,
                source: src,
                roots,
                ..
            } = load_candidates(
                &net,
                Some(Country { cc, name }),
                true,
                false,
                &mut Vec::new(),
            )
            .unwrap();
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
            let LoadedCandidates {
                found: cands,
                source: src,
                roots,
                ..
            } = load_candidates(
                &net,
                Some(Country {
                    cc: "LT",
                    name: "Lithuania",
                }),
                true,
                false,
                &mut Vec::new(),
            )
            .unwrap();
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

    /// A candidate as a list names it, with the speed the cap sorts by when one is given.
    fn candidate(country: &str, url: &str, speed: Option<&str>) -> mirrors::Mirror {
        mirrors::Mirror {
            country: country.to_string(),
            name: String::new(),
            url: url.to_string(),
            https: url.starts_with("https://"),
            speed: speed.map(str::to_string),
            status: mirrors::Status::Up,
        }
    }

    /// A known instant every probe answer in these tests is dated around, so a lag is arithmetic
    /// rather than a real clock.
    const NOW: i64 = 1_760_000_000;

    /// A probe answer a test can hold: the fields the widening, the note and the report read.
    fn answer_at(url: &str, updates_date: i64) -> probe::Outcome {
        probe::Outcome {
            url: format!("{}/", url.trim_end_matches('/')),
            updates_date,
            verified_updates: None,
            updates_signature: verify::SignatureStatus::Unavailable(
                "not verified in a test".to_string(),
            ),
            security_date: None,
            latency_ms: 10,
            throughput_bps: 1_000_000,
            redirected_to: None,
        }
    }

    /// `--any` removes the country from the candidate set instead of replacing it: every country's
    /// kept entry is what the run probes, and the same list is still on hand for the one widening
    /// (SPEC step 4).
    #[test]
    fn any_does_not_narrow_the_candidates_to_one_country() {
        let net = Canned(vec![(LAUNCHPAD, Ok(LAUNCHPAD_OK))]);
        let LoadedCandidates { found, rest, .. } = load_candidates(
            &net,
            Some(Country {
                cc: "LT",
                name: "Lithuania",
            }),
            false,
            false,
            &mut Vec::new(),
        )
        .unwrap();
        let countries: std::collections::BTreeSet<&str> =
            found.iter().map(|m| m.country.as_str()).collect();
        assert!(
            countries.len() > 1,
            "every country's mirrors enter the set: {countries:?}"
        );
        assert!(found.iter().any(|m| m.country == "Lithuania"));
        assert!(found.iter().any(|m| m.country == "Germany"));
        assert_eq!(
            rest.map(|r| r.len()),
            Some(found.len()),
            "one list, read once, kept un-narrowed"
        );

        // The same list narrowed to Lithuania is a different set, so the country still decides the
        // candidates whenever it is allowed to.
        let net = Canned(vec![(LAUNCHPAD, Ok(LAUNCHPAD_OK))]);
        let narrowed = load_candidates(
            &net,
            Some(Country {
                cc: "LT",
                name: "Lithuania",
            }),
            true,
            false,
            &mut Vec::new(),
        )
        .unwrap();
        assert!(narrowed.found.iter().all(|m| m.country == "Lithuania"));
        assert!(narrowed.found.len() < found.len());

        // No country known at all: Launchpad needs no country code, so the set is every country.
        let net = Canned(vec![(LAUNCHPAD, Ok(LAUNCHPAD_OK))]);
        let LoadedCandidates {
            found: everywhere,
            rest,
            ..
        } = load_candidates(&net, None, false, false, &mut Vec::new()).unwrap();
        assert_eq!(everywhere.len(), found.len());
        assert!(rest.is_some());
    }

    /// Without a country the `mirrors.ubuntu.com` fallback has no file to ask for, so a `--any` run
    /// with no country and no Launchpad says so instead of reading one country's list as every
    /// country's (SPEC step 4).
    #[test]
    fn any_without_a_country_says_the_fallback_needs_one() {
        let net = Canned(vec![(LAUNCHPAD, Err(503))]);
        let (code, msg) = load_candidates(&net, None, false, false, &mut Vec::new()).unwrap_err();
        assert_eq!(code, 1);
        assert!(
            msg.contains("the mirrors.ubuntu.com fallback needs a country code"),
            "{msg}"
        );
        assert!(msg.contains("--any without a country has none"), "{msg}");
    }

    /// The no-preference rule: one usable mirror in the run's own country and the set is left exactly
    /// as it was, so widening can never top up a working run (SPEC step 4).
    #[test]
    fn widening_is_not_triggered_when_one_usable_mirror_exists() {
        let mut results = vec![(
            candidate("Lithuania", "http://lt.example/ubuntu/", None),
            Ok(answer_at("http://lt.example/ubuntu", NOW)),
        )];
        let rest = vec![
            candidate("Lithuania", "http://lt.example/ubuntu/", None),
            candidate("Germany", "http://de.example/ubuntu/", None),
            candidate("Norway", "http://no.example/ubuntu/", None),
        ];
        let mut asked: Vec<String> = Vec::new();
        let widening = widen_if_empty(
            &mut results,
            Some(&rest),
            &std::collections::HashSet::new(),
            "Lithuania",
            None,
            MAX_CANDIDATES,
            &mut |batch| {
                asked.extend(batch.iter().map(|m| m.url.clone()));
                Vec::new()
            },
        );
        assert_eq!(widening, Widening::NotNeeded);
        assert_eq!(results.len(), 1, "the candidate set is not touched");
        assert!(asked.is_empty(), "nothing is probed: {asked:?}");
        assert_eq!(
            set_note(&widening, false, false, "Lithuania", "LT", "Launchpad"),
            None,
            "a run that did not widen owes no line"
        );
    }

    /// Every candidate rejected and the run widens once, to the other countries only, and says why:
    /// the note is the announcement the feature is not allowed to skip.
    #[test]
    fn every_candidate_rejected_widens_once_and_the_note_says_why() {
        let mut results = vec![
            (
                candidate("Lithuania", "http://lt.example/ubuntu/", None),
                Err(probe::ProbeError::BadSample("no sample".to_string())),
            ),
            (
                candidate("Lithuania", "http://lt2.example/ubuntu/", None),
                Err(probe::ProbeError::Unreachable(http::HttpError::Status(
                    404,
                    "http://lt2.example/ubuntu/dists/noble-updates/InRelease".to_string(),
                ))),
            ),
        ];
        let rest = vec![
            candidate("Lithuania", "http://lt.example/ubuntu/", None),
            candidate("Lithuania", "http://lt2.example/ubuntu/", None),
            candidate("Germany", "http://de.example/ubuntu/", None),
            candidate("Norway", "http://no.example/ubuntu/", None),
        ];
        let mut asked: Vec<String> = Vec::new();
        let widening = widen_if_empty(
            &mut results,
            Some(&rest),
            &std::collections::HashSet::from([
                candidate_key("http://lt.example/ubuntu/"),
                candidate_key("http://lt2.example/ubuntu/"),
            ]),
            "Lithuania",
            None,
            MAX_CANDIDATES,
            &mut |batch| {
                asked.extend(batch.iter().map(|m| m.url.clone()));
                batch
                    .iter()
                    .map(|m| (m.clone(), Ok(answer_at(&m.url, NOW))))
                    .collect()
            },
        );
        assert_eq!(widening, Widening::Widened { added: 2 });
        assert_eq!(
            asked,
            ["http://de.example/ubuntu/", "http://no.example/ubuntu/"],
            "only the other countries' mirrors are added"
        );
        assert_eq!(results.len(), 4, "one results vector holds the union");
        assert_eq!(
            set_note(&widening, false, false, "Lithuania", "LT", "Launchpad").as_deref(),
            Some(
                "note: no usable mirror in Lithuania (LT), so every country's mirrors were added and ranked together"
            )
        );
    }

    /// A widened run never probes a candidate twice: a mirror the country's own pass already saw is
    /// skipped whatever its spelling, and a list that names the same mirror twice adds it once.
    #[test]
    fn a_widened_run_never_probes_a_candidate_twice() {
        let mut results = vec![(
            candidate("Lithuania", "http://lt.example/ubuntu/", None),
            Err(probe::ProbeError::BadSample("no sample".to_string())),
        )];
        let rest = vec![
            // The same mirror pass 1 probed, spelled without its trailing slash.
            candidate("Lithuania", "http://lt.example/ubuntu", None),
            // The same mirror listed twice under two countries, with a different case and a
            // trailing slash: one entry, probed once.
            candidate("Germany", "http://de.example/ubuntu/", None),
            candidate("Germany", "http://DE.example/ubuntu", Some("1 Mbps")),
        ];
        let mut asked: Vec<String> = Vec::new();
        let widening = widen_if_empty(
            &mut results,
            Some(&rest),
            &std::collections::HashSet::from([candidate_key("http://lt.example/ubuntu/")]),
            "Lithuania",
            None,
            MAX_CANDIDATES,
            &mut |batch| {
                asked.extend(batch.iter().map(|m| candidate_key(&m.url)));
                batch
                    .iter()
                    .map(|m| (m.clone(), Ok(answer_at(&m.url, NOW))))
                    .collect()
            },
        );
        assert_eq!(widening, Widening::Widened { added: 1 });
        assert_eq!(asked, [candidate_key("http://de.example/ubuntu/")]);
        let keys: Vec<String> = results.iter().map(|(m, _)| candidate_key(&m.url)).collect();
        let mut unique = keys.clone();
        unique.sort();
        unique.dedup();
        assert_eq!(
            unique.len(),
            keys.len(),
            "no candidate appears twice: {keys:?}"
        );
        assert!(
            !asked.iter().any(|k| k.contains("lt.example")),
            "the run's own country is never re-probed: {asked:?}"
        );
    }

    /// A run whose only list is the one-country `mirrors.ubuntu.com` file cannot widen, and it says
    /// so instead of silently ranking one country as if it were every country (SPEC step 4).
    #[test]
    fn a_run_whose_only_list_names_one_country_cannot_widen() {
        let mut results = vec![(
            candidate("Lithuania", "http://lt.example/ubuntu/", None),
            Err(probe::ProbeError::BadSample("no sample".to_string())),
        )];
        let mut asked = 0;
        let widening = widen_if_empty(
            &mut results,
            None,
            &std::collections::HashSet::new(),
            "Lithuania",
            None,
            MAX_CANDIDATES,
            &mut |_batch| {
                asked += 1;
                Vec::new()
            },
        );
        assert_eq!(widening, Widening::Impossible);
        assert_eq!(asked, 0, "there is nothing to probe");
        assert_eq!(results.len(), 1);
        assert_eq!(
            set_note(
                &widening,
                false,
                true,
                "Lithuania",
                "LT",
                "mirrors.ubuntu.com (unverified)"
            )
            .as_deref(),
            Some(
                "note: no usable mirror in Lithuania (LT), and the list from mirrors.ubuntu.com (unverified) names only that country, so there is nothing to widen to"
            )
        );
        // The list the fallback branch hands back really is the one-country one.
        let net = Canned(vec![
            (LAUNCHPAD, Err(503)),
            ("http://mirrors.ubuntu.com/LT.txt", Ok(LT_TXT)),
        ]);
        let LoadedCandidates { source, rest, .. } = load_candidates(
            &net,
            Some(Country {
                cc: "LT",
                name: "Lithuania",
            }),
            true,
            false,
            &mut Vec::new(),
        )
        .unwrap();
        assert_eq!(source, "mirrors.ubuntu.com (unverified)");
        assert!(
            rest.is_none(),
            "this list names one country and nothing else"
        );
    }

    /// The case the feature exists for: a country Launchpad has no section for. The page that
    /// answered is still a loaded list, the fallback that names one country has nothing, and the run
    /// widens from that page and ranks the other countries' mirrors — without a second fetch and
    /// without the "no mirror list is available" exit (SPEC step 4).
    #[test]
    fn a_country_the_page_does_not_name_widens_from_the_page_in_hand() {
        // The fixture's country sections do not include Iceland, and nothing is canned for
        // `mirrors.ubuntu.com/IS.txt`, so the fallback answers as it does for an empty file.
        let net = Canned(vec![(LAUNCHPAD, Ok(LAUNCHPAD_OK))]);
        let LoadedCandidates {
            found: own,
            source,
            rest,
            ..
        } = load_candidates(
            &net,
            Some(Country {
                cc: "IS",
                name: "Iceland",
            }),
            true,
            false,
            &mut Vec::new(),
        )
        .expect("a page that answered is a loaded list, even with no entry for the country");
        assert!(own.is_empty(), "no Icelandic mirror is listed: {own:?}");
        assert_eq!(source, "Launchpad");
        let rest = rest.expect("the page names every other country, so the run can widen to them");
        assert!(
            rest.iter().any(|m| m.country == "Germany"),
            "the un-narrowed page is what the widening reads"
        );

        // One widening from that page, exactly as `run()` makes it: an empty candidate set, the page
        // in hand, and every answer appended to the same results vector.
        let mut results = Vec::new();
        let widening = widen_if_empty(
            &mut results,
            Some(&rest),
            &std::collections::HashSet::new(),
            "Iceland",
            None,
            MAX_CANDIDATES,
            &mut |batch| {
                batch
                    .iter()
                    .map(|m| (m.clone(), Ok(answer_at(&m.url, NOW))))
                    .collect()
            },
        );
        assert!(
            matches!(widening, Widening::Widened { added } if added > 0),
            "the empty country is what triggers the widening: {widening:?}"
        );
        let countries: std::collections::BTreeSet<&str> =
            results.iter().map(|(m, _)| m.country.as_str()).collect();
        assert!(
            !countries.is_empty(),
            "the union is what ranks, not the empty set"
        );
        assert!(
            !countries.contains("Iceland"),
            "a widened set is the other countries: {countries:?}"
        );
        // The union is ranked as one set, which is the only thing the table is built from.
        let ranked = fresh::rank(
            results
                .iter()
                .filter_map(|(_, answer)| answer.as_ref().ok())
                .map(|o| fresh::Probed {
                    url: o.url.clone(),
                    updates_date: o.updates_date,
                    verified_updates: o.verified_updates,
                    lag_secs: 0,
                    throughput_bps: o.throughput_bps,
                    latency_ms: o.latency_ms,
                    same_as: false,
                    signature: o.updates_signature.clone(),
                })
                .collect(),
        );
        assert_eq!(ranked.len(), results.len(), "every widened mirror ranks");
    }

    /// `--any` never widens: its set already is every country, so an empty one means nothing usable
    /// anywhere, and an empty widening would only cost a second pass and a false note.
    #[test]
    fn any_never_widens() {
        assert_eq!(
            set_note(
                &Widening::NotAsked,
                true,
                false,
                "Lithuania",
                "LT",
                "Launchpad"
            ),
            None
        );
        // The one line `--any` does owe: its only list named one country, so it is not "every
        // country's mirrors" and must not read as if it were.
        assert_eq!(
            set_note(
                &Widening::NotAsked,
                true,
                true,
                "any",
                "unknown",
                "mirrors.ubuntu.com (unverified)"
            )
            .as_deref(),
            Some(
                "note: --any, but the list from mirrors.ubuntu.com (unverified) names only one country"
            )
        );
    }

    /// A set that is not narrowed to one country is capped by Launchpad's declared link speed,
    /// descending, then by URL: the page names about 560 mirrors and probing them all is not a burst
    /// any run makes (SPEC step 4). A mirror that declares no speed sorts last.
    #[test]
    fn the_global_selection_is_capped_by_declared_speed_then_url() {
        let page = mirrors::parse_launchpad(&String::from_utf8_lossy(LAUNCHPAD_OK))
            .expect("fixture parses");
        let picked = global_selection(&page, MAX_CANDIDATES);
        assert_eq!(picked.len(), MAX_CANDIDATES);
        let speeds: Vec<Option<u64>> = picked
            .iter()
            .map(|m| mirrors::declared_speed_bps(m.speed.as_deref().unwrap_or("")))
            .collect();
        assert!(
            speeds.windows(2).all(|pair| pair[0] >= pair[1]),
            "declared speed descending: {speeds:?}"
        );
        let fastest = page
            .iter()
            .filter_map(|m| mirrors::declared_speed_bps(m.speed.as_deref().unwrap_or("")))
            .max();
        assert_eq!(speeds[0], fastest, "the fastest declared mirror is first");
        // Two runs read the same page and probe the same mirrors.
        assert_eq!(global_selection(&page, MAX_CANDIDATES), picked);

        let tied = vec![
            candidate("A", "http://b.example/ubuntu/", Some("10 Mbps")),
            candidate("A", "http://a.example/ubuntu/", Some("10 Mbps")),
            candidate("A", "http://z.example/ubuntu/", Some("100 Gbps")),
            candidate("A", "http://u.example/ubuntu/", None),
        ];
        let picked_ties = global_selection(&tied, 3);
        let urls: Vec<&str> = picked_ties.iter().map(|m| m.url.as_str()).collect();
        assert_eq!(
            urls,
            [
                "http://z.example/ubuntu/",
                "http://a.example/ubuntu/",
                "http://b.example/ubuntu/",
            ],
            "ties break by URL, and an unknown speed sorts last"
        );
        assert_eq!(
            global_selection(&tied, 10).len(),
            4,
            "a cap above the set keeps it whole"
        );
    }

    /// Under `--any` the country stays visible in the header and says it decides nothing: a run that
    /// ignores the country must not read as one narrowed to it (SPEC step 4).
    #[test]
    fn any_is_not_a_constraint_on_the_header() {
        assert_eq!(
            origin_label(true, true, true),
            "--country, not a constraint"
        );
        assert_eq!(
            origin_label(true, true, false),
            "detected, not a constraint"
        );
        assert_eq!(origin_label(true, false, false), "--any");
        // Without `--any` the country is what narrowed the set, and the header says which won.
        assert_eq!(origin_label(false, true, true), "--country");
        assert_eq!(origin_label(false, true, false), "detected");
        assert_eq!(
            header_identity(
                "0.1.0",
                "Ubuntu 24.04 (noble)",
                "Lithuania",
                origin_label(true, true, true),
                Painter::plain(),
            ),
            "apt-scout 0.1.0 · Ubuntu 24.04 (noble) · country: Lithuania (--country, not a constraint)"
        );
        assert_eq!(
            header_identity(
                "0.1.0",
                "Ubuntu 24.04 (noble)",
                "any",
                origin_label(true, false, false),
                Painter::plain(),
            ),
            "apt-scout 0.1.0 · Ubuntu 24.04 (noble) · country: any (--any)"
        );
    }

    /// The recommendation names the **mirror's** country, not the run's: the two are the same value
    /// only while every run is narrowed to its own country, so a widened or `--any` run would
    /// otherwise report a country the selected mirror is not in (SPEC.md CLI).
    #[test]
    fn the_recommendation_names_the_mirrors_country() {
        let best = probed(MIRROR, verify::SignatureStatus::verified_for_test());
        let mut details = std::collections::HashMap::new();
        details.insert(
            MIRROR.to_string(),
            Detail {
                country: "Germany".to_string(),
                security_date: None,
                security_lag: None,
                redirected_to: None,
            },
        );
        // A Lithuanian run whose best mirror is German.
        let country = recommendation_country(&details, MIRROR);
        assert_eq!(country, "Germany");
        assert_eq!(
            recommendation(&best, country, None, false, Painter::plain()),
            "  https://mirror.example/ubuntu/ — freshest, verified yes, Germany, 0m behind, 1.0 MB/s, 10 ms latency"
        );
        // A mirror whose probe recorded no country still gets a word, never a blank.
        assert_eq!(
            recommendation_country(&details, "http://elsewhere.example/ubuntu/"),
            "unknown"
        );
    }

    /// A run writes the selected mirror URI without the trailing slash the page spells
    /// (`probe::probe` trims it), so a later run must still recognize it: the roots of a German run
    /// name a Norwegian mirror at `/archive/`, and the URI that first run wrote is `/archive`. Checked
    /// at the seam the run itself uses — the inventory — so this fails if the whole-host or
    /// own-country-only version of the rule ever comes back, not only if one helper regresses.
    #[test]
    fn a_mirror_a_run_wrote_stays_rewritable_under_a_country_change() {
        let net = Canned(vec![(LAUNCHPAD, Ok(LAUNCHPAD_OK))]);
        let LoadedCandidates {
            source: src, roots, ..
        } = load_candidates(
            &net,
            Some(Country {
                cc: "DE",
                name: "Germany",
            }),
            true,
            false,
            &mut Vec::new(),
        )
        .unwrap();
        assert_eq!(src, "Launchpad");
        assert!(
            roots
                .iter()
                .any(|r| r.host == "ubuntu.uib.no" && r.path == "/archive/"),
            "the fixture lists this mirror in Norway, so a German run's set must still name it: \
             {roots:?}"
        );
        let apt = TempApt::new("country-change");
        apt.write(
            "sources.list",
            "deb http://ubuntu.uib.no/archive noble main\n",
        );
        let inventory = sources::inventory(&apt.0, &roots, false);
        assert!(
            inventory.has_rewritable_uri(),
            "run 2 cannot recognize the mirror run 1 wrote: {:?}",
            inventory.files[0].entries
        );
        // The seeded lines that made W24 fail stay refused while that line is eligible.
        apt.write(
            "sources.list",
            "deb http://ubuntu.uib.no/archive noble main\ndeb http://deb.example.com/stable ./\ndeb https://ppa.launchpadcontent.net/x/y/ubuntu noble main\n",
        );
        let mixed = sources::inventory(&apt.0, &roots, false);
        let rewritable: Vec<bool> = mixed.files[0]
            .entries
            .iter()
            .flat_map(|e| e.rewritable.iter().copied())
            .collect();
        assert_eq!(rewritable, [true, false, false]);
    }

    #[test]
    fn the_uid_and_the_flags_decide_whether_the_run_writes() {
        // `--list` never writes, whatever the uid, and exits 0 (SPEC.md CLI table).
        assert_eq!(decide(false, true, false, false), Mode::List);
        assert_eq!(decide(false, true, true, false), Mode::List);
        assert_eq!(decide(false, true, true, true), Mode::List);
        // A non-root run recommends and exits 3, with or without --yes.
        assert_eq!(decide(false, false, false, false), Mode::Recommend);
        assert_eq!(decide(false, false, false, true), Mode::Recommend);
        // A root run writes, and asks once unless --yes skipped the prompt.
        assert_eq!(
            decide(false, false, true, false),
            Mode::Write { confirm: true }
        );
        assert_eq!(
            decide(false, false, true, true),
            Mode::Write { confirm: false }
        );
        // `--restore` is its own mode whatever the other flags say; a non-root run has nothing to
        // confirm, and a root run confirms once unless --yes.
        assert_eq!(
            decide(true, false, false, false),
            Mode::Restore { confirm: false }
        );
        assert_eq!(
            decide(true, false, true, false),
            Mode::Restore { confirm: true }
        );
        assert_eq!(
            decide(true, false, true, true),
            Mode::Restore { confirm: false }
        );
    }

    #[test]
    fn yes_is_parsed_and_the_sudo_line_repeats_the_flags() {
        let args = parse_args_from(["--yes"].into_iter().map(str::to_string)).unwrap();
        assert!(args.yes);
        assert_eq!(sudo_command(&args), "sudo apt-scout --yes");

        let plain = parse_args_from(std::iter::empty()).unwrap();
        assert_eq!(sudo_command(&plain), "sudo apt-scout");

        let flags = parse_args_from(
            [
                "--country",
                "lt",
                "--no-geo",
                "--top",
                "3",
                "--https-only",
                "--no-color",
            ]
            .into_iter()
            .map(str::to_string),
        )
        .unwrap();
        assert_eq!(
            sudo_command(&flags),
            "sudo apt-scout --country LT --no-geo --top 3 --https-only --no-color"
        );
    }

    /// `--restore` is a mode of its own and a run may not combine it with a flag the mirror
    /// pipeline runs on: a restore never probes, ranks or rewrites, so the pair is a usage error
    /// that names the flag the user typed and the command that does work.
    #[test]
    fn restore_parses_and_refuses_every_mirror_flag() {
        let parse = |argv: &[&str]| parse_args_from(argv.iter().copied().map(str::to_string));
        for argv in [&["--restore"][..], &["-r"][..]] {
            let args = parse(argv).unwrap_or_else(|e| panic!("{argv:?} must parse: {e}"));
            assert!(args.restore, "{argv:?}");
            assert_eq!(
                decide(args.restore, args.list, false, args.yes),
                Mode::Restore { confirm: false },
                "{argv:?}"
            );
        }
        // Both orders, and every short spelling, so the error can never depend on which flag came
        // first or on how it was written.
        for (argv, flag) in [
            (&["--restore", "--country", "LT"][..], "--country"),
            (&["-c", "LT", "--restore"][..], "-c"),
            (&["--restore", "--any"][..], "--any"),
            (&["-a", "-r"][..], "-a"),
            (&["--restore", "--top", "3"][..], "--top"),
            // An explicit default is still the flag the user typed.
            (&["--top", "5", "-r"][..], "--top"),
            (&["--restore", "--https-only"][..], "--https-only"),
            (&["-s", "-r"][..], "-s"),
            (&["--restore", "--list"][..], "--list"),
            (&["-l", "-r"][..], "-l"),
            (&["--restore", "--no-geo"][..], "--no-geo"),
            (&["-n", "-r"][..], "-n"),
        ] {
            let err = parse(argv).expect_err(&format!("{argv:?} must be a usage error"));
            assert!(err.contains(flag), "{argv:?}: {err}");
            assert!(
                err.contains("cannot be combined with --restore"),
                "{argv:?}: {err}"
            );
            assert!(err.contains("sudo apt-scout --restore"), "{argv:?}: {err}");
        }
        // `--yes` is the unattended form, and the display flags are display only, so a restore
        // accepts all three.
        let args =
            parse(&["--restore", "--yes", "--verbose", "--no-color"]).expect("restore flags");
        assert!(args.restore && args.yes && args.verbose && args.no_color);
    }

    /// The restore prints its own block, not the mirror report's: the backup it put back, the
    /// files it wrote and removed, and where it copied what it replaced. `--verbose` adds the
    /// digest, mode and owner the writer already holds; the default block carries no digest.
    #[test]
    fn a_restore_block_names_the_backup_the_files_and_the_copies() {
        let original = plan::FileState {
            dev: 1,
            ino: 2,
            mode: 0o644,
            uid: 0,
            gid: 0,
            bytes: b"deb http://archive.ubuntu.com/ubuntu noble main\n".to_vec(),
        };
        let new_bytes = b"Types: deb\nURIs: http://archive.ubuntu.com/ubuntu\n".to_vec();
        let replaced =
            plan::Target::SourcesListD(plan::SourceName::parse("ubuntu.sources").unwrap());
        let removed =
            plan::Target::SourcesListD(plan::SourceName::parse("system-security.sources").unwrap());
        let writes = vec![
            plan::PlannedWrite {
                target: replaced.clone(),
                digest: digest::sha256(&new_bytes),
                new_bytes,
                original: Some(original.clone()),
                effect: plan::Effect::Replace,
            },
            plan::PlannedWrite {
                target: removed.clone(),
                digest: digest::sha256(&[]),
                new_bytes: Vec::new(),
                original: Some(original.clone()),
                effect: plan::Effect::Remove,
            },
        ];
        let restored = manifest::BackupId::parse("20261112T000000Z").expect("id");
        let created = manifest::BackupId::parse("20261113T101500Z").expect("id");
        let plan = plan::Plan::for_restore(
            &restore::validated_backup_for_test(restored.clone()),
            writes,
        )
        .expect("a restore plan");
        assert_eq!(
            restore_confirm_lines(&restored, &plan, &created),
            [
                "Restore backup 20261112T000000Z".to_string(),
                "  write /etc/apt/sources.list.d/ubuntu.sources".to_string(),
                "  remove /etc/apt/sources.list.d/system-security.sources".to_string(),
                "  copies: /var/backups/apt-scout/20261113T101500Z".to_string(),
            ]
        );
        let outcome = apply::Outcome {
            backup_id: created,
            touched: vec![
                apply::Touched {
                    target: replaced,
                    effect: plan::Effect::Replace,
                },
                apply::Touched {
                    target: removed,
                    effect: plan::Effect::Remove,
                },
            ],
        };
        let plain = restore_lines(&restored, &plan, &outcome, false);
        assert_eq!(
            plain,
            [
                "  backup: 20261112T000000Z".to_string(),
                "  wrote /etc/apt/sources.list.d/ubuntu.sources".to_string(),
                "  removed /etc/apt/sources.list.d/system-security.sources".to_string(),
                "  copies: /var/backups/apt-scout/20261113T101500Z".to_string(),
            ]
        );
        assert!(!plain.iter().any(|line| line.contains("sha256")));
        let verbose = restore_lines(&restored, &plan, &outcome, true);
        let was = format!(
            "    was sha256:{} mode 0644 owner 0:0",
            digest::hex(&digest::sha256(&original.bytes))
        );
        assert_eq!(
            verbose.iter().filter(|line| *line == &was).count(),
            2,
            "{verbose:#?}"
        );
        let now = format!(
            "    now sha256:{} mode 0644 owner 0:0",
            digest::hex(&plan.writes()[0].digest)
        );
        assert!(verbose.contains(&now), "{verbose:#?}");
        // A removal has no new content, so it carries the old state only.
        assert_eq!(
            verbose
                .iter()
                .filter(|line| line.starts_with("    now "))
                .count(),
            1
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
        assert!(change.is_none(), "nothing to change, so no plan");
        // Success is still a run that writes nothing.
        assert_eq!(apt.read("sources.list"), current);
    }

    /// Every entry the plan refuses is still reported to the user — in the `Sources` section, before
    /// probing, which is where SPEC.md Apply puts it — and the confirmation that follows repeats none
    /// of them (SPEC.md CLI). The two reports read the same apt files, so a skip cannot fall between
    /// them.
    #[test]
    fn a_skip_is_named_in_sources_and_never_again_in_the_confirmation() {
        let apt = TempApt::new("skipped");
        apt.write(
            "sources.list",
            "deb http://archive.ubuntu.com/ubuntu noble main\ndeb https://security.ubuntu.com/ubuntu noble-security main\n",
        );
        // Not valid UTF-8: the inventory reports the file instead of dropping it (SPEC.md Apply:
        // every skipped file with its reason).
        std::fs::write(apt.0.join("sources.list.d/bad.list"), [0xff, 0xfe]).expect("fixture file");
        let hosts = listed(&["archive.ubuntu.com", "mirror.example"]);
        let inventory = sources::inventory(&apt.0, &hosts, false);
        let plan = build_plan(
            &apt.0,
            &inventory,
            &hosts,
            &probed(MIRROR, verify::SignatureStatus::verified_for_test()),
            plan::Platform::Ubuntu,
        )
        .expect("one URI is rewritable")
        .expect("the archive URI is rewritten");
        let sources = sources_section(&apt.0, &inventory, Painter::plain()).join("\n");
        assert!(
            sources.contains("bad.list                   skip     file is not valid UTF-8"),
            "{sources}"
        );
        assert!(
            sources.contains("security.ubuntu.com — security suite is never rewritten"),
            "{sources}"
        );
        let confirmation = planned_change_lines(
            &apt.0,
            &inventory,
            &plan,
            Some(&manifest::BackupId::from_secs(1_700_000_000).expect("a valid backup id")),
            Painter::plain(),
        )
        .join("\n");
        assert!(!confirmation.contains("bad.list"), "{confirmation}");
        assert!(!confirmation.contains("security"), "{confirmation}");
    }

    /// The confirmation names the files, the URI pairs and the backup directory it will create — and
    /// nothing it would only repeat. `Sources` already listed every skip, alternatives appear only
    /// when the run has some, and this run has none (SPEC.md CLI).
    #[test]
    fn the_planned_change_names_the_uri_pairs_and_the_backup_and_no_skip() {
        let apt = TempApt::new("planned");
        apt.write(
            "sources.list.d/ubuntu.sources",
            "# noble\nTypes: deb\nURIs: http://archive.ubuntu.com/ubuntu\nSuites: noble noble-updates\nComponents: main\n\nTypes: deb\nURIs: http://security.ubuntu.com/ubuntu\nSuites: noble-security\nComponents: main\n",
        );
        let hosts = listed(&["archive.ubuntu.com", "mirror.example"]);
        let inventory = sources::inventory(&apt.0, &hosts, false);
        let plan = build_plan(
            &apt.0,
            &inventory,
            &hosts,
            &probed(MIRROR, verify::SignatureStatus::verified_for_test()),
            plan::Platform::Ubuntu,
        )
        .expect("the archive stanza is rewritable")
        .expect("one file changes");
        let id = manifest::BackupId::from_secs(1_700_000_000).expect("a valid backup id");
        let lines = planned_change_lines(&apt.0, &inventory, &plan, Some(&id), Painter::plain());
        assert_eq!(
            lines,
            [
                "  sources.list.d/ubuntu.sources:2".to_string(),
                "    - http://archive.ubuntu.com/ubuntu".to_string(),
                "    + https://mirror.example/ubuntu/".to_string(),
                format!(
                    "  backup: {}/{} (every replaced file is copied there, mode 0700, before the first write)",
                    apply::BACKUP_ROOT,
                    id.as_str()
                ),
            ]
        );
        let text = lines.join("\n");
        assert!(!text.contains("keep as is"), "{text}");
        assert!(!text.contains("was not changed"), "{text}");
        assert!(!text.contains("alternatives"), "{text}");
        // A `--list` run creates no backup directory, so it prints the URI pairs and no backup line.
        let listed = planned_change_lines(&apt.0, &inventory, &plan, None, Painter::plain());
        assert_eq!(listed, lines[..3].to_vec());
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
        // The refusal says what to do: name the host as the problem and point at the two ways back
        // that exist now, the URIs by hand and the newest backup through `--restore`.
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
        // Both halves of the refusal: the reason each inspected entry was left alone, and the fact
        // that the run wrote nothing. The skips reach the user only through this message.
        assert!(
            msg.contains("https://security.ubuntu.com/ubuntu was not changed"),
            "{msg}"
        );
        assert!(
            msg.contains("https://ppa.example/ubuntu was not changed"),
            "{msg}"
        );
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
        .expect("both inspected files are written");
        let plan::Authority::Mirror { uri, proof } = plan.authority() else {
            panic!("an apply plan is authorized by its mirror");
        };
        assert_eq!(uri, MIRROR);
        assert_eq!(proof.seconds(), 1000);
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
            assert_eq!(write.effect, plan::Effect::Replace);
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
            plan_changes(&apt.0, &inventory, &plan, Painter::plain()),
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
        .expect("the Pop!_OS split is written");
        assert_eq!(plan.writes().len(), 2);
        let created = plan
            .writes()
            .iter()
            .find(|w| w.effect == plan::Effect::Create)
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

    /// The `verified` column's whole job: a verified `-updates` `InRelease` is the only `yes`, and
    /// the writer's own gate agrees with the label it prints.
    #[test]
    fn signature_cell_is_yes_only_for_verified() {
        assert_eq!(
            signature_cell(&verify::SignatureStatus::verified_for_test()),
            "yes"
        );
        // `no` is a body that was readable and did not verify; `?` is verification that could not
        // run at all — no gpgv, no keyring, an oversized body, a timeout. Neither is a pass, and
        // neither label may read like one.
        assert_eq!(
            signature_cell(&verify::SignatureStatus::Invalid("bad".into())),
            "no"
        );
        assert_eq!(
            signature_cell(&verify::SignatureStatus::Unavailable("no gpgv".into())),
            "?"
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
        // Only `yes` may authorize a write (SPEC: Freshness trust boundary). The writer's own gate
        // is `is_verified`, so this pins the labels to exactly what the writer grants: a `no` or a
        // `?` cell never reads as one it would accept.
        assert!(verify::SignatureStatus::verified_for_test().is_verified());
        assert!(!verify::SignatureStatus::Invalid("bad".into()).is_verified());
        assert!(!verify::SignatureStatus::Unavailable("no gpgv".into()).is_verified());
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
            redirect_host: None,
        }
    }

    /// The same row with the one fact the default `notes` column carries. `--verbose` prints the
    /// redirect whole and the table prints it cut, and both read the same row.
    fn noted<'a>(mut row: Row<'a>, redirect_host: Option<&'a str>) -> Row<'a> {
        row.redirect_host = redirect_host;
        row
    }

    /// The AS column exists only when the run has AS data for at least one row: a table of
    /// `unknown` would read as "every lookup answered", which is exactly the silent failure the
    /// column used to have. When it does exist, every row carries a cell, so the columns stay
    /// aligned and a mirror without an answer says `unknown` rather than writing nothing. Only a
    /// `--verbose` run fills the column in at all (`table_rows`), so a default table has none.
    #[test]
    fn the_ranked_table_renders_the_as_column_only_with_as_data() {
        let ok = verify::SignatureStatus::verified_for_test();
        let without = ranked_rows(
            &[
                row("http://a.example/ubuntu", 0, &ok, None),
                row("http://b.example/ubuntu", 600, &ok, None),
            ],
            Painter::plain(),
        );
        assert_eq!(
            without,
            [
                "   #  mirror                                       behind      speed   latency  verified"
                    .to_string(),
                "   1  http://a.example/ubuntu                          0m   1.2 MB/s   1235 ms  yes"
                    .to_string(),
                "   2  http://b.example/ubuntu                         10m   1.2 MB/s   1235 ms  yes"
                    .to_string(),
            ]
        );
        let with = ranked_rows(
            &[
                row("http://a.example/ubuntu", 0, &ok, Some("12929 same")),
                row("http://b.example/ubuntu", 0, &ok, None),
            ],
            Painter::plain(),
        );
        assert_eq!(
            with,
            [
                "   #  mirror                                       behind      speed   latency         as  verified"
                    .to_string(),
                "   1  http://a.example/ubuntu                          0m   1.2 MB/s   1235 ms 12929 same  yes"
                    .to_string(),
                "   2  http://b.example/ubuntu                          0m   1.2 MB/s   1235 ms    unknown  yes"
                    .to_string(),
            ]
        );
    }

    /// The `notes` column: the redirect, compact and cut, and only when a shown row redirected — a
    /// plain table gains no empty column. The `-security` lag used to ride here and is a `--verbose`
    /// fact now (`Detail` prints it in full), so no default row carries `sec:`.
    #[test]
    fn the_notes_column_carries_the_redirect_and_only_when_present() {
        let ok = verify::SignatureStatus::verified_for_test();
        let plain = ranked_rows(
            &[row("http://a.example/ubuntu", 0, &ok, None)],
            Painter::plain(),
        );
        assert_eq!(
            plain[0],
            "   #  mirror                                       behind      speed   latency  verified"
        );
        assert_eq!(
            plain[1],
            "   1  http://a.example/ubuntu                          0m   1.2 MB/s   1235 ms  yes"
        );

        // A mirror that redirects to another host says which one, by default.
        let redirected = ranked_rows(
            &[noted(
                row("http://a.example/ubuntu", 0, &ok, None),
                Some("mirror2.example"),
            )],
            Painter::plain(),
        );
        assert_eq!(
            redirected[0],
            "   #  mirror                                       behind      speed   latency  verified  notes"
        );
        assert_eq!(
            redirected[1],
            "   1  http://a.example/ubuntu                          0m   1.2 MB/s   1235 ms  yes       redir:mirror2.example"
        );
        // The note starts under its own heading: `verified` is as wide as the heading word, so a
        // note does not drift left by the length of the cell before it.
        assert_eq!(redirected[0].find("notes"), redirected[1].find("redir:"));
    }

    /// One long host cannot wrap the table: the cell keeps the fact and cuts the name, ends the cut
    /// in `CUT_MARKER` so it cannot be read as a host of that name, and `--verbose` is where the
    /// whole name is printed.
    #[test]
    fn a_long_redirect_host_is_cut_and_verbose_prints_it_whole() {
        let ok = verify::SignatureStatus::verified_for_test();
        let long = "a-very-long-mirror-host.example.org";
        let cell = noted(row("http://a.example/ubuntu", 0, &ok, None), Some(long));
        assert_eq!(note_cell(&cell), Some("redir:a-very-long-mir~".to_string()));
        // The cut is bounded, so a run of long hostnames cannot widen the table without limit.
        let cut = note_cell(&cell).expect("a redirect is a note");
        assert_eq!(cut.len(), "redir:".len() + REDIR_HOST_MAX);

        // A host of exactly the limit is not cut, so it carries no marker: the marker means "cut"
        // and nothing else.
        let exact = "a".repeat(REDIR_HOST_MAX);
        assert_eq!(
            note_cell(&noted(
                row("http://a.example/ubuntu", 0, &ok, None),
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
        let verbose = detail_section(
            &[cell],
            &details,
            &std::collections::HashMap::new(),
            Painter::plain(),
        );
        assert!(
            verbose
                .iter()
                .any(|l| l == &format!("     redirects to {long}")),
            "{verbose:?}"
        );
        // The `-security` lag moved here with the note: printed in full, in minutes, and never in the
        // table's `sec:` shorthand.
        assert!(
            verbose.iter().any(|l| l.contains("778m lag")),
            "{verbose:?}"
        );
        assert!(
            !verbose.iter().any(|l| l.contains("sec:")),
            "the shorthand is the table's, and the table no longer prints it: {verbose:?}"
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

    /// Both reports for one probe result, through the wiring the run uses: `probe` → `detail_of` →
    /// `table_rows` → `ranked_rows` for the default table, and `detail_section` over the same rows
    /// for the `--verbose` run.
    fn reports_for(
        outcome: &probe::Outcome,
        reference_security: Option<i64>,
    ) -> (Vec<String>, Vec<String>) {
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
        let table = ranked_rows(
            &table_rows(&ranked, &details, &no_as, None, 5, false),
            Painter::plain(),
        );
        let rows = table_rows(&ranked, &details, &no_as, None, 5, true);
        let detail = detail_section(&rows, &details, &no_as, Painter::plain());
        (table, detail)
    }

    /// The table's `notes` cell and `Detail`'s `-security` line are filled from the run's own probe
    /// result, not from a `Row` a test built: the redirect reaches the default table here, and the
    /// `-security` date and lag reach the `--verbose` one, and neither appears when the mirror served
    /// neither. Cutting this test off from the probe is what let the two facts regress silently, so
    /// it stays on the real path.
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
        let lag = (outcome.updates_date - security_date) / 60;
        let (table, detail) = reports_for(&outcome, Some(outcome.updates_date));
        assert!(
            table[0].contains("notes"),
            "the notes column is missing: {table:?}"
        );
        assert!(
            table[1].contains("redir:mirror2.example"),
            "the redirect is missing: {table:?}"
        );
        // The `-security` lag is `--verbose`'s: the default row carries no `sec:`, and the detail
        // line carries the same number the probe saw, on the pocket nobody writes from.
        assert!(
            !table[1].contains("sec:"),
            "the -security lag is not a default fact: {table:?}"
        );
        assert!(
            detail.iter().any(|l| l.contains(&format!(
                "-security: {}, {lag}m lag",
                utc_stamp(security_date)
            ))),
            "the -security date and lag are missing from --verbose: {detail:?}"
        );

        // The same mirror without a `-security` pocket and without a redirect: no notes column, no
        // token that would read as one, and a detail line that says the pocket was not served.
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
        let (table, detail) = reports_for(&outcome, Some(outcome.updates_date));
        assert!(!table[0].contains("notes"), "{table:?}");
        assert!(
            !table[1].contains("sec:") && !table[1].contains("redir:"),
            "{table:?}"
        );
        assert!(
            detail.iter().any(|l| l.contains("-security: no InRelease")),
            "{detail:?}"
        );
    }

    /// The table is a terminal table, so its width is pinned: one column that grows turns every row
    /// into a wrapped row. Measured on the shapes the default report prints, each with the longest
    /// fact its cells can carry: a `--no-geo` row (no AS column — what the 44-column mirror column
    /// was sized for), a row with a note, a `--verbose` row with an AS cell, and the widest row this
    /// report can produce (a `?` cell, an AS cell and a full-length redirect). The widest row adds up
    /// as `6 prefix, 44 mirror, 1, 6 behind, 1, 10 speed, 1, 9 latency, 1, 10 as, 2, 8 verified, 2,
    /// 22 notes`, which is 123 columns; the redirect host is the only cell that can grow with input,
    /// and `REDIR_HOST_MAX` is what bounds it, so the `notes` column costs a fixed 24 columns instead
    /// of an unbounded amount. The two renamed words are why the numeric field is six wide and the
    /// cell eight: `behind` is one column past `lag` and `verified` five past `sig`, and a heading
    /// wider than its field drifts every column after it. Reaching 80 would mean dropping the full
    /// mirror URL (which the container matrix anchors on) or one of the numeric columns — a layout
    /// decision, not a note-column one.
    #[test]
    fn the_ranked_table_width_is_pinned() {
        let ok = verify::SignatureStatus::verified_for_test();
        let bad = verify::SignatureStatus::Unavailable("no gpgv".into());
        let url = "http://lt.archive.ubuntu.com/ubuntu";
        let longest_note = "a-very-long-mirror-host.example.org";
        let width = |rows: &[Row]| {
            ranked_rows(rows, Painter::plain())
                .iter()
                .map(|line| line.chars().count())
                .max()
                .unwrap_or(0)
        };
        let no_as = width(&[row(url, 0, &ok, None)]);
        let a_note = width(&[noted(row(url, 0, &ok, None), Some(longest_note))]);
        let with_as = width(&[row(url, 0, &ok, Some("12929 same"))]);
        let widest = width(&[noted(
            row(url, 10_080 * 60, &bad, Some("12929 diff")),
            Some(longest_note),
        )]);
        assert_eq!((no_as, a_note, with_as, widest), (88, 112, 99, 123));
    }

    /// `--help` is where the table's shorthand is defined: without the column words a reader cannot
    /// tell freshness from latency, without the `yes`/`no`/`?` mapping the `verified` cell is a guess
    /// about a write, without the token name the `notes` column is unreadable, and without the
    /// pointer a reader cannot find the cut host again.
    #[test]
    fn the_help_defines_the_table_columns() {
        for column in ["behind", "latency", "verified"] {
            assert!(HELP.contains(column), "{column} is not in --help: {HELP}");
        }
        assert!(HELP.contains("yes = gpgv accepted it"), "{HELP}");
        assert!(HELP.contains("? = verification could not run"), "{HELP}");
        assert!(HELP.contains("notes"), "{HELP}");
        assert!(HELP.contains("redir:<host>"), "{HELP}");
        assert!(HELP.contains("cut"), "{HELP}");
        assert!(HELP.contains("ends in ~"), "{HELP}");
        assert!(HELP.contains("--verbose"), "{HELP}");
        // The rule that only a verified InRelease may authorize a write stays where it was, in the
        // table's own new words: `no` and `?` never do either, and the help says so.
        assert!(HELP.contains("Only yes may authorize a write"), "{HELP}");
        assert!(HELP.contains("no and ? never do"), "{HELP}");
        assert!(
            !HELP.contains("only ok"),
            "`ok` is not a cell value: {HELP}"
        );
    }

    /// `--help` promises only what the binary prints. It promised "the alternatives when the run has
    /// any" and no code path printed one: an alternative mirror is another **checked** mirror, written
    /// as a `# alternative:` comment, and a non-interactive run checks exactly one, so it has none to
    /// print — the promise belongs to the TUI and to no line of this report (GLOSSARY: Alternative
    /// mirror; SPEC.md Output). The confirmation's own test is what proves the report prints none.
    #[test]
    fn the_help_promises_no_alternatives_the_report_never_prints() {
        assert!(
            HELP.contains("--verbose adds the proof behind these words"),
            "{HELP}"
        );
        assert!(!HELP.contains("alternative"), "{HELP}");
    }

    /// "N of M usable" counts what is shown against what survived, and the legend underneath states
    /// one clause per column that needs one, in the columns' own words (SPEC.md Output): without it
    /// `behind` has no baseline and `latency` could be read as freshness.
    #[test]
    fn the_ranked_title_counts_what_is_shown() {
        assert_eq!(
            ranked_title(3, 12, 7, Painter::plain()),
            "Ranked — 3 of 12 usable, 7 rejected            behind = -updates age vs freshest · latency = round trip · verified = InRelease signature"
        );
    }

    /// The one line under the table: only in a `--verbose` run, only when no row has AS data, and it
    /// names the run's own reason rather than a bare `unknown`. The default table has no AS column, so
    /// it has no gap to explain and gets no note.
    #[test]
    fn the_as_note_appears_only_when_no_row_has_as_data() {
        let ok = verify::SignatureStatus::verified_for_test();
        assert_eq!(as_note(&[], Some("whatever"), true), None);
        assert_eq!(
            as_note(
                &[row("http://a.example/ubuntu", 0, &ok, Some("12929 same"))],
                Some("x"),
                true
            ),
            None
        );
        assert_eq!(
            as_note(&[row("http://a.example/ubuntu", 0, &ok, None)], Some("Team Cymru names no AS for 192.0.2.1: no AS data for this address"), true),
            Some("note: AS data unavailable (Team Cymru names no AS for 192.0.2.1: no AS data for this address)".to_string())
        );
        // No reason was kept (nothing was ever asked): the note still says so, it does not vanish.
        assert_eq!(
            as_note(&[row("http://a.example/ubuntu", 0, &ok, None)], None, true),
            Some("note: AS data unavailable (no AS lookup answered)".to_string())
        );
        // The default report never prints an AS column, so it never explains its absence either.
        assert_eq!(
            as_note(&[row("http://a.example/ubuntu", 0, &ok, None)], None, false),
            None
        );
    }

    /// Nothing was rejected, so there is no `Rejected — 0` heading: a section with nothing to say
    /// prints nothing at all, no heading and no `0` count (SPEC.md Output).
    #[test]
    fn a_report_with_no_rejection_prints_no_rejected_heading() {
        assert!(rejected_lines(&[], false, Painter::plain()).is_empty());
        assert!(rejected_lines(&[], true, Painter::plain()).is_empty());
        // The report it feeds gains no line and no gap for it: `Report::block` skips an empty section,
        // so nothing needs a case here.
        let mut report = Report::default();
        report.line("apt-scout 0.1.0 · Ubuntu 24.04 (noble)".to_string());
        report.block(rejected_lines(&[], false, Painter::plain()));
        assert_eq!(report.0, ["apt-scout 0.1.0 · Ubuntu 24.04 (noble)"]);
    }

    /// The recommendation's reason carries every fact SPEC.md CLI asks for in it, in the table's own
    /// words for the two numbers, and never a second spelling of the same fact — the signature
    /// included: the default line names the column and its cell, and the exact status is `--verbose`'s
    /// (SPEC.md Output).
    #[test]
    fn the_recommendation_carries_country_behind_latency_and_signature() {
        let best = probed(MIRROR, verify::SignatureStatus::verified_for_test());
        // The default line: no AS, because the AS the tie-break used is proof of how the ranking was
        // audited, not a fact a person choosing a mirror acts on, and the signature in the table's
        // own words (SPEC.md Output).
        assert_eq!(
            recommendation(
                &best,
                "Lithuania",
                Some("12929 same"),
                false,
                Painter::plain()
            ),
            "  https://mirror.example/ubuntu/ — freshest, verified yes, Lithuania, 0m behind, 1.0 MB/s, 10 ms latency"
        );
        // The same mirror under --verbose names the AS the tie-break used: the bare match word
        // cannot be checked against the table.
        assert_eq!(
            recommendation(
                &best,
                "Lithuania",
                Some("12929 same"),
                true,
                Painter::plain()
            ),
            "  https://mirror.example/ubuntu/ — freshest verified, Lithuania, AS 12929 same, 0m behind, 1.0 MB/s, 10 ms latency"
        );
        // A mirror one generation behind says so instead of claiming to be the freshest.
        let mut lagging = best;
        lagging.lag_secs = 6 * 3600;
        assert_eq!(
            recommendation(&lagging, "Lithuania", None, false, Painter::plain()),
            "  https://mirror.example/ubuntu/ — closest, verified yes, Lithuania, 360m behind, 1.0 MB/s, 10 ms latency"
        );
        assert_eq!(
            recommendation(&lagging, "Lithuania", None, true, Painter::plain()),
            "  https://mirror.example/ubuntu/ — closest verified, Lithuania, AS unknown, 360m behind, 1.0 MB/s, 10 ms latency"
        );
    }

    /// The default recommendation speaks the table's word for the signature and never the exact
    /// status: a mirror whose check could not run reads `verified ?` and one whose signature was
    /// invalid reads `verified no`, while the phrases that say *why* stay `--verbose`'s (SPEC.md
    /// Output). Without this the default line printed the same `unverified (…)` text `Detail` does.
    #[test]
    fn the_default_recommendation_says_the_tables_word_never_the_proof() {
        let unchecked = &verify::SignatureStatus::Unavailable("no gpgv in this image".into());
        let invalid = &verify::SignatureStatus::Invalid("altered".into());
        let line = |status: &verify::SignatureStatus, verbose: bool| {
            recommendation(
                &probed(MIRROR, status.clone()),
                "Lithuania",
                None,
                verbose,
                Painter::plain(),
            )
        };
        assert_eq!(
            line(unchecked, false),
            "  https://mirror.example/ubuntu/ — freshest, verified ?, Lithuania, 0m behind, 1.0 MB/s, 10 ms latency"
        );
        assert_eq!(
            line(invalid, false),
            "  https://mirror.example/ubuntu/ — freshest, verified no, Lithuania, 0m behind, 1.0 MB/s, 10 ms latency"
        );
        // The cell word is the table's: the same three words, in the same order as the column.
        assert_eq!(
            [
                unchecked,
                invalid,
                &verify::SignatureStatus::verified_for_test()
            ]
            .map(signature_cell),
            ["?", "no", "yes"]
        );
        // Not one exact status in either default line, and both in the `--verbose` ones.
        for (status, phrase) in [
            (unchecked, "unverified (signature unavailable)"),
            (invalid, "unverified (invalid signature)"),
        ] {
            let plain = line(status, false);
            assert!(!plain.contains(phrase), "the proof leaked: {plain}");
            assert!(!plain.contains("unverified"), "the proof leaked: {plain}");
            assert!(line(status, true).contains(phrase), "{status:?}");
        }
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
            rejected_section(&rejected, Painter::plain()),
            [
                "Rejected — 5",
                "  404               mirror.b.example, mirror.a.example",
                "  no Date header    mirror.c.example",
                "  stale             mirror.e.example (162 h)",
                "  unreachable       mirror.d.example",
            ]
        );
        // --verbose trades the groups for each mirror's own line and its full reason.
        let verbose = rejected_detail(&rejected, Painter::plain());
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
        let lines = sources_section(&apt.0, &inventory, Painter::plain());
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
    /// unreachable says that instead of pretending it had one. The server's AS is `--verbose`'s, so
    /// the default line keeps `mirrors:` and `reference:` and nothing else (SPEC.md Output).
    #[test]
    fn the_header_names_the_release_country_and_reference() {
        assert_eq!(
            header_identity(
                "0.1.0",
                "Ubuntu 24.04 (noble)",
                "United States",
                "--country",
                Painter::plain(),
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
            header_context(Some("AS12929"), "Launchpad", Some(&reference), true),
            "server AS12929 · mirrors: Launchpad · reference: archive.ubuntu.com (verified)"
        );
        assert_eq!(
            header_context(Some("AS12929"), "Launchpad", Some(&reference), false),
            "mirrors: Launchpad · reference: archive.ubuntu.com (verified)"
        );
        assert_eq!(
            header_context(None, "mirrors.ubuntu.com (unverified)", None, true),
            "server AS unknown · mirrors: mirrors.ubuntu.com (unverified) · reference: newest mirror date (archive.ubuntu.com unreachable)"
        );
        assert_eq!(
            header_context(None, "mirrors.ubuntu.com (unverified)", None, false),
            "mirrors: mirrors.ubuntu.com (unverified) · reference: newest mirror date (archive.ubuntu.com unreachable)"
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
            detail_section(&rows, &details, &resolved, Painter::plain()),
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
                &std::collections::HashMap::new(),
                Painter::plain(),
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

    /// The long redirect host every report test below uses: longer than `REDIR_HOST_MAX`, so the
    /// default table has to cut it and `--verbose` is the only place it appears whole.
    const LONG_HOST: &str = "a-very-long-mirror-host.example.org";

    /// The whole `--list` block a run prints, from fixture data, with `verbose` threaded exactly
    /// where `run()` threads it: one run, two reports. `the_list_report_reads_as_one_block` pins the
    /// default one line by line; the two tests after it read the pair for what moved.
    fn sample_report(verbose: bool, tag: &str) -> (Vec<String>, TempApt) {
        sample_report_with(verify::SignatureStatus::verified_for_test(), verbose, tag)
    }

    /// The same block with the **recommended** mirror's signature status chosen by the caller, so a
    /// test can make the run whose recommendation is unverified — the run a leak of the exact status
    /// would show up in.
    fn sample_report_with(
        best_signature: verify::SignatureStatus,
        verbose: bool,
        tag: &str,
    ) -> (Vec<String>, TempApt) {
        let apt = TempApt::new(tag);
        apt.write(
            "sources.list.d/ubuntu.sources",
            "# noble\nTypes: deb\nURIs: http://archive.ubuntu.com/ubuntu\nSuites: noble noble-updates\nComponents: main\n\nTypes: deb\nURIs: http://security.ubuntu.com/ubuntu\nSuites: noble-security\nComponents: main\n",
        );
        let hosts = listed(&["archive.ubuntu.com", "mirror.arizona.edu"]);
        let inventory = sources::inventory(&apt.0, &hosts, false);
        // The spelling a probe reports: the mirror's own URL without the trailing slash, which is
        // what the detail section keys the resolved address by.
        let selected = "https://mirror.example/ubuntu";
        let remote = "http://mirrors.usinternet.com/ubuntu";
        let plan = build_plan(
            &apt.0,
            &inventory,
            &hosts,
            &probed(selected, verify::SignatureStatus::verified_for_test()),
            plan::Platform::Ubuntu,
        )
        .expect("the archive stanza is rewritable")
        .expect("one file changes");
        // One verified mirror one generation behind and one whose signature could not be checked, so
        // the three cell values and both arms of the `verified` ink are in the same report.
        let mut second = probed(
            remote,
            verify::SignatureStatus::Unavailable("no gpgv in this image".into()),
        );
        second.lag_secs = 6 * 3600;
        let ranked = vec![probed(selected, best_signature), second];
        // The chosen mirror redirects to a long host and has a `-security` lag worth printing; the
        // other mirror did neither, so its row must end at its own cell.
        let details = std::collections::HashMap::from([
            (
                selected.to_string(),
                Detail {
                    country: "United States".to_string(),
                    security_date: Some(600),
                    security_lag: Some(778 * 60),
                    redirected_to: Some(LONG_HOST.to_string()),
                },
            ),
            (
                remote.to_string(),
                Detail {
                    country: "United States".to_string(),
                    security_date: None,
                    security_lag: None,
                    redirected_to: None,
                },
            ),
        ]);
        let mirror_asn = std::collections::HashMap::from([
            (
                as_key(selected).to_string(),
                ResolvedAs {
                    ip: "192.0.2.10".parse().expect("a fixture address"),
                    asn: "AS12929".to_string(),
                    name: Some("ARIZONA, US".to_string()),
                },
            ),
            (
                as_key(remote).to_string(),
                ResolvedAs {
                    ip: "192.0.2.20".parse().expect("a fixture address"),
                    asn: "AS3356".to_string(),
                    name: None,
                },
            ),
        ]);
        let rejected = vec![
            Rejected {
                host: "mirror.lostpacket.org".into(),
                kind: Reject::NotFound,
                why: "http://mirror.lostpacket.org/ubuntu/dists/noble-updates/InRelease answered HTTP 404".into(),
            },
            Rejected {
                host: "mirror.pit.teraswitch.com".into(),
                kind: Reject::Stale(162 * 3600),
                why: "stale: 162 h behind the freshest date; the limit is 24 h".into(),
            },
        ];
        let rows = table_rows(&ranked, &details, &mirror_asn, Some("AS12929"), 5, verbose);
        let ink = Painter::plain();
        let mut report = Report::default();
        report.line(header_identity(
            "0.1.0",
            "Ubuntu 24.04 (noble)",
            "United States",
            "--country",
            ink,
        ));
        report.block(sources_section(&apt.0, &inventory, ink));
        report.block(vec![header_context(
            Some("AS12929"),
            "Launchpad",
            None,
            verbose,
        )]);
        let mut ranked_lines = vec![ranked_title(rows.len(), ranked.len(), rejected.len(), ink)];
        ranked_lines.extend(ranked_rows(&rows, ink));
        ranked_lines.extend(as_note(&rows, None, verbose));
        report.block(ranked_lines);
        if verbose && !rows.is_empty() {
            report.block(detail_section(&rows, &details, &mirror_asn, ink));
        }
        report.block(vec![
            ink.bold("Recommendation"),
            recommendation(
                &ranked[0],
                "United States",
                as_cell(&mirror_asn, Some("AS12929"), selected).as_deref(),
                verbose,
                ink,
            ),
        ]);
        let mut planned = vec![titled(
            "Planned change",
            "nothing is written with --list",
            ink,
        )];
        planned.extend(plan_changes(&apt.0, &inventory, &plan, ink));
        report.block(planned);
        report.block(rejected_lines(&rejected, verbose, ink));
        (report.0, apt)
    }

    /// The whole `--list` block, section by section, from fixture data — the shape the human
    /// approved, as the default report prints it. It pins the section order and the column widths
    /// together; the container matrix is what proves a live run prints the same block.
    #[test]
    fn the_list_report_reads_as_one_block() {
        let (report, apt) = sample_report(false, "report");
        let expected = vec![
            "apt-scout 0.1.0 · Ubuntu 24.04 (noble) · country: United States (--country)".to_string(),
            String::new(),
            format!("{:<45}  {}", "Sources", apt.0.display()),
            "  sources.list.d/ubuntu.sources             rewrite  line 2  archive.ubuntu.com"
                .to_string(),
            "  sources.list.d/ubuntu.sources             skip     line 7  security.ubuntu.com — security suite is never rewritten"
                .to_string(),
            String::new(),
            "mirrors: Launchpad · reference: newest mirror date (archive.ubuntu.com unreachable)"
                .to_string(),
            String::new(),
            "Ranked — 2 of 2 usable, 2 rejected             behind = -updates age vs freshest · latency = round trip · verified = InRelease signature"
                .to_string(),
            "   #  mirror                                       behind      speed   latency  verified  notes"
                .to_string(),
            "   1  https://mirror.example/ubuntu                    0m   1.0 MB/s     10 ms  yes       redir:a-very-long-mir~"
                .to_string(),
            "   2  http://mirrors.usinternet.com/ubuntu           360m   1.0 MB/s     10 ms  ?"
                .to_string(),
            String::new(),
            "Recommendation".to_string(),
            "  https://mirror.example/ubuntu — freshest, verified yes, United States, 0m behind, 1.0 MB/s, 10 ms latency"
                .to_string(),
            String::new(),
            "Planned change                                 nothing is written with --list"
                .to_string(),
            "  sources.list.d/ubuntu.sources:2".to_string(),
            "    - http://archive.ubuntu.com/ubuntu".to_string(),
            "    + https://mirror.example/ubuntu".to_string(),
            String::new(),
            "Rejected — 2".to_string(),
            "  404               mirror.lostpacket.org".to_string(),
            "  stale             mirror.pit.teraswitch.com (162 h)".to_string(),
        ];
        assert_eq!(report, expected);
    }

    /// The default report is the decision, and this is the test that keeps it so: one assertion per
    /// fact `SPEC.md` Output moved to `--verbose`, so a leak names itself instead of hiding in a
    /// diff of the whole block. Each of these would fail on the report the previous release printed.
    /// The recommended mirror's own signature is **invalid** here on purpose: it is the run in which
    /// the default recommendation printed the exact status, so the assertion below has something to
    /// catch instead of passing on a mirror whose status happened to read `verified`.
    #[test]
    fn a_default_report_carries_no_verbose_fact() {
        let (report, _) = sample_report_with(
            verify::SignatureStatus::Invalid("altered".into()),
            false,
            "default-facts",
        );
        let text = report.join("\n");

        // Each row's AS number and its same/diff/unknown match with the server.
        assert!(
            !text.contains(" AS "),
            "an AS is in the default report:\n{text}"
        );
        assert!(!text.contains("12929"), "an AS number is in it:\n{text}");
        assert!(
            !text.contains(" same"),
            "an AS match word is in it:\n{text}"
        );
        assert!(
            !text.contains(" diff"),
            "an AS match word is in it:\n{text}"
        );
        assert!(
            !text.contains("unknown"),
            "an AS match word is in it:\n{text}"
        );
        // The server's AS on the context line; the line itself stays.
        assert!(
            !text.contains("server AS"),
            "the server's AS is in it:\n{text}"
        );
        assert!(
            text.contains("mirrors: Launchpad · reference:"),
            "the context line lost its own fields:\n{text}"
        );
        // Each row's country and the address the run resolved for it.
        assert!(
            !text.contains("country: United States · ip:"),
            "a per-row country is in it:\n{text}"
        );
        assert!(
            !text.contains("192.0.2.10"),
            "a resolved address is in it:\n{text}"
        );
        // Both pocket dates in full and the `-security` lag.
        assert!(
            !text.contains("UTC"),
            "a full pocket date is in it:\n{text}"
        );
        assert!(
            !text.contains("-updates:"),
            "a pocket date line is in it:\n{text}"
        );
        assert!(
            !text.contains("-security"),
            "the -security pocket is in it:\n{text}"
        );
        assert!(
            !text.contains("sec:"),
            "the -security lag is in it:\n{text}"
        );
        // The exact signature status as `Detail`'s own line, instead of the table's yes/no/?.
        assert!(
            !text.contains("     signature:"),
            "the detail signature line is in it:\n{text}"
        );
        // And nowhere else either: the recommendation the default run prints names the column and
        // its cell, never the phrase `Detail` and a `--verbose` run use, and this run's chosen
        // mirror is the invalid one.
        assert!(
            !text.contains("unverified"),
            "the exact signature status is in the recommendation:\n{text}"
        );
        assert!(
            text.contains(" — freshest, verified no, United States,"),
            "the recommendation does not speak the table's word:\n{text}"
        );
        // And the row itself carries the short form: `?` for a check that could not run, which is
        // what the unreachable-keyring row is.
        let unchecked = report
            .iter()
            .find(|l| l.starts_with("   2  "))
            .unwrap_or_else(|| panic!("no second row:\n{text}"));
        assert!(
            unchecked.ends_with("  ?"),
            "the unchecked row must read ?: {unchecked}"
        );
        // A redirect host in full, instead of the cut form.
        assert!(
            text.contains("redir:a-very-long-mir~"),
            "the cut redirect is missing:\n{text}"
        );
        assert!(
            !text.contains(LONG_HOST),
            "an uncut redirect host is in it:\n{text}"
        );
        // The full text of every rejection reason, instead of the grouped class and its count.
        assert!(
            text.contains("  404               mirror.lostpacket.org"),
            "the grouped class is missing:\n{text}"
        );
        assert!(
            !text.contains("answered HTTP 404"),
            "a full rejection reason is in it:\n{text}"
        );
        assert!(
            !text.contains("the limit is 24 h"),
            "a full rejection reason is in it:\n{text}"
        );
        // The `Detail` section is the holder of all of the above, so it is not in the default report.
        assert!(
            !text.contains("Detail"),
            "the Detail section is in it:\n{text}"
        );
    }

    /// The other half of the contract: `--verbose` carries every fact the default report dropped, on
    /// its own named line, so deleting one from the verbose path fails here by name rather than
    /// leaving a quietly thinner proof.
    #[test]
    fn the_verbose_report_carries_every_moved_fact() {
        let (report, _) = sample_report(true, "verbose-facts");
        let text = report.join("\n");
        // Each row's AS number and match, and the chosen mirror's in the recommendation.
        assert!(text.contains("12929 same"), "no AS cell:\n{text}");
        assert!(text.contains("3356 diff"), "no AS cell:\n{text}");
        assert!(
            text.contains(", AS 12929 same, 0m behind"),
            "the recommendation names no AS:\n{text}"
        );
        // The server's AS on the context line, which keeps `mirrors:` and `reference:`.
        assert!(
            text.contains("server AS12929 · mirrors: Launchpad · reference:"),
            "no server AS:\n{text}"
        );
        // Each row's country and the address the run resolved.
        assert!(
            text.contains("country: United States · ip: 192.0.2.10 · as: AS12929 (ARIZONA, US)"),
            "no per-row country, address or AS holder:\n{text}"
        );
        // Both pocket dates in full and the `-security` lag.
        assert!(
            text.contains("-updates: 1970-01-01 00:16 UTC, 0m lag"),
            "no -updates date:\n{text}"
        );
        assert!(
            text.contains("-security: 1970-01-01 00:10 UTC, 778m lag"),
            "no -security date and lag:\n{text}"
        );
        // The exact signature status.
        assert!(
            text.contains("signature: unverified (signature unavailable)"),
            "no exact signature status:\n{text}"
        );
        assert!(
            text.contains("signature: verified"),
            "no exact signature status for the verified row:\n{text}"
        );
        // A redirect host in full.
        assert!(
            text.contains(&format!("redirects to {LONG_HOST}")),
            "no uncut redirect host:\n{text}"
        );
        // The full text of every rejection reason.
        assert!(
            text.contains("answered HTTP 404"),
            "no full rejection reason:\n{text}"
        );
        assert!(
            text.contains("stale: 162 h behind the freshest date; the limit is 24 h"),
            "no full stale reason:\n{text}"
        );
    }

    /// `--verbose` adds to the default report instead of replacing any part of it: everything the
    /// default says about this run that is not one of the moved facts is still there, byte for byte,
    /// and each row keeps the same numbers in both.
    #[test]
    fn the_verbose_report_adds_to_the_default_one() {
        let (default, _) = sample_report(false, "adds-default");
        let (verbose, _) = sample_report(true, "adds-verbose");
        for needle in [
            "apt-scout 0.1.0 ·",
            "  sources.list",
            "Planned change",
            "    - http://archive.ubuntu.com/ubuntu",
            "    + https://mirror.example/ubuntu",
        ] {
            let from_default: Vec<&String> =
                default.iter().filter(|l| l.contains(needle)).collect();
            let from_verbose: Vec<&String> =
                verbose.iter().filter(|l| l.contains(needle)).collect();
            assert!(
                !from_default.is_empty(),
                "{needle} is missing from both reports"
            );
            assert_eq!(
                from_default, from_verbose,
                "{needle} differs between the two"
            );
        }
        // The `Sources` heading names the apt root, and each fixture has its own, so it is compared by
        // shape rather than by the directory one test happened to use.
        for report in [&default, &verbose] {
            assert!(
                report.iter().any(|l| l.starts_with("Sources ")),
                "no Sources heading: {report:?}"
            );
        }
        // The same run, so each row carries the same behind, speed, latency and cell word in both:
        // what `--verbose` adds is the AS column, not a different measurement.
        let row_of = |report: &[String], rank: usize| {
            report
                .iter()
                .find(|l| l.starts_with(&format!("   {rank}  ")))
                .cloned()
                .unwrap_or_else(|| panic!("no row {rank}: {report:?}"))
        };
        for rank in [1, 2] {
            let a = row_of(&default, rank);
            let b = row_of(&verbose, rank);
            for fact in ["0m", "360m", "1.0 MB/s", "10 ms", "yes", "?"] {
                assert_eq!(
                    a.contains(fact),
                    b.contains(fact),
                    "{fact} in row {rank}:\n{a}\n{b}"
                );
            }
        }
        // The AS column is the only difference in that row: take the field out of the verbose row and
        // it is the default row again, byte for byte.
        assert_eq!(
            row_of(&verbose, 1).replacen(" 12929 same", "", 1),
            row_of(&default, 1),
            "the AS column is the only difference between the two rows"
        );
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

    /// An apt root, its inventory and the plan a write would make, so a report test can print the
    /// `Planned change` section without going near a real `/etc/apt`.
    fn sample_plan(tag: &str) -> (TempApt, sources::Inventory, plan::Plan) {
        let apt = TempApt::new(tag);
        apt.write(
            "sources.list.d/ubuntu.sources",
            "# noble\nTypes: deb\nURIs: http://archive.ubuntu.com/ubuntu\nSuites: noble noble-updates\nComponents: main\n\nTypes: deb\nURIs: http://security.ubuntu.com/ubuntu\nSuites: noble-security\nComponents: main\n",
        );
        let hosts = listed(&["archive.ubuntu.com", "mirror.example"]);
        let inventory = sources::inventory(&apt.0, &hosts, false);
        let plan = build_plan(
            &apt.0,
            &inventory,
            &hosts,
            &probed(MIRROR, verify::SignatureStatus::verified_for_test()),
            plan::Platform::Ubuntu,
        )
        .expect("the archive stanza is rewritable")
        .expect("one file changes");
        (apt, inventory, plan)
    }

    /// Every styled site in one report — the identity line, the four headings, the `verified` column,
    /// both diff signs, a rejection reason — built with whichever painter it is given, so a colour run
    /// and a plain run can be compared line for line (SPEC.md Output). It is a `--verbose` report, so
    /// the styled `verified` cells and the `Detail` heading are both in it.
    fn painted_report(
        ink: Painter,
        apt: &TempApt,
        inventory: &sources::Inventory,
        plan: &plan::Plan,
    ) -> Vec<String> {
        let verified = verify::SignatureStatus::verified_for_test();
        let invalid = verify::SignatureStatus::Invalid("bad signature".into());
        // All three `verified` cells in one report, so each ink site is here: `yes` green, `no` and
        // `?` yellow.
        let unavailable = verify::SignatureStatus::Unavailable("no gpgv in this image".into());
        let rows = vec![
            row(MIRROR, 0, &verified, Some("12929 same")),
            row("http://b.example/ubuntu", 600, &invalid, None),
            row(
                "http://c.example/ubuntu",
                700,
                &unavailable,
                Some("3356 diff"),
            ),
        ];
        let rejected = vec![Rejected {
            host: "mirror.lostpacket.org".into(),
            kind: Reject::NotFound,
            why: "answered HTTP 404".into(),
        }];
        let mut report = Report::default();
        report.line(header_identity(
            "0.1.0",
            "Ubuntu 24.04 (noble)",
            "United States",
            "--country",
            ink,
        ));
        report.block(sources_section(&apt.0, inventory, ink));
        let mut ranked_lines = vec![ranked_title(rows.len(), rows.len(), rejected.len(), ink)];
        ranked_lines.extend(ranked_rows(&rows, ink));
        report.block(ranked_lines);
        report.block(detail_section(
            &rows,
            &Default::default(),
            &Default::default(),
            ink,
        ));
        report.block(vec![
            ink.bold("Recommendation"),
            recommendation(
                &probed(MIRROR, verify::SignatureStatus::verified_for_test()),
                "United States",
                Some("12929 same"),
                true,
                ink,
            ),
        ]);
        let mut planned = vec![titled(
            "Planned change",
            "written only after the confirmation",
            ink,
        )];
        planned.extend(planned_change_lines(
            &apt.0,
            inventory,
            plan,
            Some(&manifest::BackupId::from_secs(1_700_000_000).expect("a valid backup id")),
            ink,
        ));
        report.block(planned);
        report.block(rejected_lines(&rejected, false, ink));
        report.0
    }

    /// `\x1b[…m` removed. `paint` writes nothing else, so scanning for the closing `m` is exact,
    /// and a colour run must differ from the plain one by nothing else.
    fn strip_escapes(line: &str) -> String {
        let mut out = String::with_capacity(line.len());
        let mut rest = line;
        while let Some(at) = rest.find('\u{1b}') {
            out.push_str(&rest[..at]);
            let after = &rest[at + 1..];
            let end = after.find('m').expect("paint closes every escape with 'm'");
            rest = &after[end + 1..];
        }
        out.push_str(rest);
        out
    }

    /// SPEC.md Output: colour appears only when the three inputs agree. One lone `false` is enough to
    /// turn it off, and no pair of them turns it back on.
    #[test]
    fn colour_needs_all_three_conditions() {
        assert!(colour_enabled(true, false, false));
        assert!(!colour_enabled(false, false, false));
        assert!(!colour_enabled(true, true, false));
        assert!(!colour_enabled(true, false, true));
        assert!(!colour_enabled(true, true, true));
        assert!(!colour_enabled(false, true, false));
        assert!(!colour_enabled(false, false, true));
        assert!(!colour_enabled(false, true, true));
    }

    /// A plain painter returns its input, whatever the ink: this is what makes the plain bytes the
    /// coloured bytes minus the escapes, without a branch in any render site.
    #[test]
    fn a_plain_painter_returns_its_input() {
        let plain = Painter::plain();
        assert_eq!(plain.bold("Ranked"), "Ranked");
        assert_eq!(plain.green("yes"), "yes");
        assert_eq!(plain.yellow("no"), "no");
        assert_eq!(plain.yellow("?"), "?");
        assert_eq!(plain.red("- old"), "- old");
        assert_eq!(plain.dim("behind"), "behind");
        assert_eq!(paint(Style::Bold, "x", false), "x");
    }

    /// The ink table, and that `paint` is the only function that makes an escape.
    #[test]
    fn an_enabled_painter_wraps_the_text_in_one_escape() {
        let ink = Painter::new(true);
        assert_eq!(ink.bold("Section"), "\u{1b}[1mSection\u{1b}[0m");
        assert_eq!(ink.green("yes"), "\u{1b}[32myes\u{1b}[0m");
        assert_eq!(ink.yellow("?"), "\u{1b}[33m?\u{1b}[0m");
        assert_eq!(ink.red("- x"), "\u{1b}[31m- x\u{1b}[0m");
        assert_eq!(ink.dim("behind"), "\u{1b}[2mbehind\u{1b}[0m");
        assert_eq!(paint(Style::Dim, "x", true), "\u{1b}[2mx\u{1b}[0m");
        // A plain painter over the same text is the identity, so the two differ by the escape only.
        assert_eq!(
            strip_escapes(&ink.bold("Section")),
            Painter::plain().bold("Section")
        );
    }

    /// The three ways colour is off (SPEC.md Output): stdout is not a terminal — a pipe, a file, the
    /// container matrix — `--no-color`, and `NO_COLOR` set to any value, the empty one included.
    /// Each one alone is enough, and each one must leave the report byte for byte as the plain one.
    #[test]
    fn a_report_with_colour_off_is_the_plain_report_byte_for_byte() {
        let (apt, inventory, plan) = sample_plan("colour-off");
        let plain = painted_report(Painter::plain(), &apt, &inventory, &plan);
        for (stdout_is_terminal, no_color_flag, no_color_set) in [
            (false, false, false),
            (true, true, false),
            (true, false, true),
        ] {
            let ink = Painter::new(colour_enabled(
                stdout_is_terminal,
                no_color_flag,
                no_color_set,
            ));
            assert_eq!(
                painted_report(ink, &apt, &inventory, &plan),
                plain,
                "stdout a terminal: {stdout_is_terminal}, --no-color: {no_color_flag}, NO_COLOR set: {no_color_set}"
            );
        }
        // What a capture sees: no escape sequence at all, which is what the container matrix greps.
        assert!(
            !plain.iter().any(|line| line.contains('\u{1b}')),
            "{plain:?}"
        );
    }

    /// The same report painted: every other byte identical, and each SPEC.md Output site carries the
    /// ink it names. Column width is part of the comparison, so painting a cell before padding it
    /// would fail here.
    #[test]
    fn a_coloured_report_differs_from_the_plain_one_only_by_escapes() {
        let (apt, inventory, plan) = sample_plan("colour-escapes");
        let plain = painted_report(Painter::plain(), &apt, &inventory, &plan);
        let coloured = painted_report(Painter::new(true), &apt, &inventory, &plan);
        assert_ne!(coloured, plain, "something must be painted");
        let stripped: Vec<String> = coloured.iter().map(|line| strip_escapes(line)).collect();
        assert_eq!(stripped, plain);

        // Site by site (SPEC.md Output): bold for the identity line, section headings and the
        // recommended host; green `yes` and `+` lines; yellow `no` and `?`; red rejection reasons and
        // `-` lines; dim `behind`, `speed` and `latency`.
        let any = |needle: &str| coloured.iter().any(|line| line.contains(needle));
        assert!(
            coloured[0].starts_with("\u{1b}[1mapt-scout 0.1.0"),
            "{}",
            coloured[0]
        );
        assert!(any("\u{1b}[1mSources "));
        assert!(any("\u{1b}[1mRanked — "));
        assert!(any("\u{1b}[1mDetail\u{1b}[0m"));
        assert!(any("\u{1b}[1mRecommendation\u{1b}[0m"));
        assert!(any("\u{1b}[1mRejected — 1\u{1b}[0m"));
        assert!(any("\u{1b}[1mPlanned change "));
        assert!(any("\u{1b}[1mhttps://mirror.example/ubuntu/\u{1b}[0m"));
        assert!(any("\u{1b}[2mbehind\u{1b}[0m"));
        assert!(any("\u{1b}[2m     speed\u{1b}[0m"));
        assert!(any("\u{1b}[2m  latency\u{1b}[0m"));
        assert!(any("\u{1b}[32myes\u{1b}[0m"));
        assert!(any("\u{1b}[33mno\u{1b}[0m"));
        // `?` is a yellow cell too: "could not check" is never painted like a pass.
        assert!(
            coloured
                .iter()
                .any(|line| line.contains("\u{1b}[33m?\u{1b}[0m")),
            "no yellow ? cell: {coloured:?}"
        );
        assert!(any("\u{1b}[31m  404"));
        assert!(any("\u{1b}[31m    - http://archive.ubuntu.com/ubuntu"));
        assert!(any("\u{1b}[32m    + https://mirror.example/ubuntu/"));
    }

    /// `--no-color` is a flag of its own and has no short form, so no one can bundle or guess it
    /// (SPEC.md CLI: the one implemented flag without one).
    #[test]
    fn no_color_parses_and_has_no_short_form() {
        let args =
            parse_args_from(["--no-color"].map(str::to_string)).expect("--no-color is a flag");
        assert!(args.no_color);
        assert!(
            !parse_args_from(std::iter::empty::<String>())
                .expect("no flags")
                .no_color
        );
        // `-N` and every other guess stay an unknown option, and one that exits 2.
        for argv in ["-N", "-C", "-x"] {
            let err = parse_args_from([argv].map(str::to_string))
                .expect_err(&format!("{argv} must be refused"));
            assert_eq!(err, format!("unknown option '{argv}'; try --help"));
        }
    }

    /// `--help` stays in sync with what the run does: the flag is listed, and the help says colour and
    /// the progress line are for a terminal only, because that is the difference a user sees between
    /// a run here and the same run in a pipe.
    #[test]
    fn the_help_lists_no_color_and_says_display_is_terminal_only() {
        assert!(HELP.contains("--no-color"), "{HELP}");
        assert!(
            HELP.contains("colour and the progress line are terminal-only"),
            "{HELP}"
        );
        assert!(HELP.contains("NO_COLOR is unset"), "{HELP}");
        // The flag has no short form, so the help must not invent one beside it.
        assert!(!HELP.contains("-N, --no-color"), "{HELP}");
    }

    /// The test sink: what the run would have shown, in order, with no terminal anywhere. `events`
    /// keeps all three calls in one sequence, because "was that frame drawn after the summary" is a
    /// question about order that three separate vectors cannot answer.
    #[derive(Default)]
    struct RecordingSink {
        lines: Vec<String>,
        clears: usize,
        settled: Vec<String>,
        events: Vec<String>,
    }

    impl Sink for RecordingSink {
        fn rewrite(&mut self, line: &str) {
            self.lines.push(line.to_string());
            self.events.push(format!("rewrite:{line}"));
        }

        fn clear(&mut self) {
            self.clears += 1;
            self.events.push("clear".to_string());
        }

        fn settle(&mut self, line: &str) {
            self.settled.push(line.to_string());
            self.events.push(format!("settle:{line}"));
        }
    }

    /// The distinct lines a run drew, in order, with the frame cell and any repeated frame of the same
    /// text taken out: the animation redraws one line, so "which lines did it show" is the sequence of
    /// distinct texts whatever the frames did or how many of them a slow ticker squeezed in.
    fn drawn(progress: &Progress<RecordingSink>) -> Vec<String> {
        let sink = lock(&progress.sink);
        let mut out: Vec<String> = Vec::new();
        for line in &sink.lines {
            let text = match line.split_once(' ') {
                Some((_, rest)) => rest.to_string(),
                None => line.clone(),
            };
            if out.last() != Some(&text) {
                out.push(text);
            }
        }
        out
    }

    /// The real sink's whole job: rewrite in place and erase what it leaves, so a shorter line never
    /// leaves the tail of a longer one behind and the summary stops being transient.
    #[test]
    fn the_stderr_sink_rewrites_in_place_and_erases_what_it_leaves() {
        let mut sink = StderrSink::with_writer(Vec::new());
        sink.rewrite("ranking");
        sink.clear();
        sink.settle("done — 1 usable, 0 rejected in 0.2 s");
        assert_eq!(
            String::from_utf8(sink.err).expect("ASCII"),
            "\rranking\u{1b}[K\r\u{1b}[K\rdone — 1 usable, 0 rejected in 0.2 s\u{1b}[K\n"
        );
    }

    /// A piped or logged stderr is silent: no line, no frame, no thread — nothing is started to draw
    /// one, so the silence is structural rather than a branch inside the loop (SPEC.md Output).
    #[test]
    fn progress_writes_nothing_when_stderr_is_not_a_terminal() {
        let mut progress = Progress::new(RecordingSink::default(), false);
        assert!(progress.ticker.is_none(), "no terminal, no animation");
        progress.load();
        progress.reference();
        progress.probe(1, 3);
        progress.probe(2, 3);
        progress.probe(3, 3);
        progress.ranked();
        progress.clear();
        progress.finish(4, 0);
        let sink = lock(&progress.sink);
        assert!(sink.events.is_empty(), "{:?}", sink.events);
        assert!(sink.lines.is_empty(), "{:?}", sink.lines);
        assert_eq!(sink.clears, 0);
        assert!(sink.settled.is_empty(), "{:?}", sink.settled);
    }

    /// At a terminal the run names each phase it is in and how far probing got, and it ends with the one
    /// summary line: nothing transient survives into the report (SPEC.md Output).
    #[test]
    fn progress_names_each_phase_and_ends_with_the_summary() {
        let mut progress = Progress::new(RecordingSink::default(), true);
        progress.load();
        progress.reference();
        progress.probe(1, 3);
        progress.probe(2, 3);
        progress.probe(3, 3);
        progress.ranked();
        // A warning that lands mid-run erases the live line instead of sharing it.
        progress.clear();
        progress.finish(4, 0);
        // The animation is joined by `finish`, so every line it drew is already in the sink.
        assert_eq!(
            drawn(&progress),
            [
                "loading the mirror list",
                "verifying the reference",
                "probing 3 mirrors… 1/3",
                "probing 3 mirrors… 2/3",
                "probing 3 mirrors… 3/3",
                "ranking",
            ]
        );
        let sink = lock(&progress.sink);
        assert_eq!(sink.clears, 1);
        assert_eq!(sink.settled.len(), 1, "one summary, and nothing after it");
        let summary = &sink.settled[0];
        let number = summary
            .strip_prefix("done — 4 usable, 0 rejected in ")
            .and_then(|rest| rest.strip_suffix(" s"))
            .unwrap_or_else(|| panic!("not the summary line: {summary}"));
        assert!(number.parse::<f64>().is_ok(), "{summary}");
    }

    /// One candidate is not "1 mirrors": the line counts the way a person says it.
    #[test]
    fn a_single_candidate_is_not_plural() {
        assert_eq!(probing_line(1, 1), "probing 1 mirror… 1/1");
        assert_eq!(probing_line(1, 3), "probing 3 mirrors… 1/3");
    }

    /// "The candidates probed so far" (SPEC.md Output): the count comes from the probes that have
    /// finished, not from the joins, so it starts at the truth and moves while the slow ones run —
    /// with the first probe slow, a join-driven count would sit on `1/2` while `2/2` was the truth.
    #[test]
    fn the_probe_count_follows_the_probes_that_finished() {
        let slow = thread::spawn(|| thread::sleep(Duration::from_millis(60)));
        let quick = thread::spawn(|| ());
        let mut progress = Progress::new(RecordingSink::default(), true);
        assert_eq!(await_probes(vec![slow, quick], &mut progress).len(), 2);
        let frames = drawn(&progress);
        // The first frame is honest: nothing has finished yet, so it does not claim one.
        assert_eq!(
            frames.first().map(String::as_str),
            Some("probing 2 mirrors… 0/2")
        );
        let counts: Vec<usize> = frames
            .iter()
            .map(|frame| {
                frame
                    .split_once('/')
                    .and_then(|(left, _)| left.split_whitespace().next_back())
                    .and_then(|n| n.parse().ok())
                    .unwrap_or_else(|| panic!("not a counting frame: {frame}"))
            })
            .collect();
        assert!(counts.len() >= 2, "{frames:?}");
        // Each frame counts exactly one more than the one before it, so the line never claims a probe
        // that is not done, never stands still while a probe lands, and reaches the whole batch.
        // Strict because `drawn` already dropped a repeated text: a count that repeated after a poll
        // interval cannot survive it, so `<=` would only give up the claim.
        assert!(counts.windows(2).all(|w| w[0] < w[1]), "{frames:?}");
        assert_eq!(counts.last(), Some(&2));
    }

    /// The one widening probes more candidates into the same set, so the line continues the count:
    /// a viewer who saw `2/2` is never shown `0/1` next (SPEC.md Output: `n/m` is the candidates
    /// probed **so far** in the run).
    #[test]
    fn the_probe_count_continues_into_the_widening_pass() {
        let mut progress = Progress::new(RecordingSink::default(), true);
        assert_eq!(
            await_probes(
                vec![thread::spawn(|| ()), thread::spawn(|| ())],
                &mut progress
            )
            .len(),
            2
        );
        // The second pass is the one widening's batch.
        await_probes(vec![thread::spawn(|| ())], &mut progress);
        let counts: Vec<(usize, usize)> = drawn(&progress)
            .iter()
            .map(|frame| {
                let (_, counted) = frame
                    .split_once('…')
                    .unwrap_or_else(|| panic!("not a counting frame: {frame}"));
                let (done, total) = counted
                    .trim()
                    .split_once('/')
                    .unwrap_or_else(|| panic!("not a counting frame: {frame}"));
                (
                    done.parse()
                        .unwrap_or_else(|_| panic!("not a frame: {frame}")),
                    total
                        .parse()
                        .unwrap_or_else(|_| panic!("not a frame: {frame}")),
                )
            })
            .collect();
        assert_eq!(counts.first(), Some(&(0, 2)), "{:?}", drawn(&progress));
        assert_eq!(
            counts.last(),
            Some(&(3, 3)),
            "the union is counted to its end: {:?}",
            drawn(&progress)
        );
        assert!(
            counts
                .windows(2)
                .all(|w| w[0].0 <= w[1].0 && w[0].1 <= w[1].1),
            "the line never goes backwards: {:?}",
            drawn(&progress)
        );
        assert!(
            counts.iter().any(|&(_, total)| total == 3),
            "the widening pass raises the total: {:?}",
            drawn(&progress)
        );
    }

    /// A run whose own country the page does not name probes nothing before it widens, and an empty
    /// candidate set has no count to show: no `0/0` line that a viewer reads as a stuck run.
    #[test]
    fn an_empty_candidate_set_shows_no_probing_line() {
        let mut progress = Progress::new(RecordingSink::default(), true);
        let none: Vec<thread::JoinHandle<()>> = Vec::new();
        assert!(await_probes(none, &mut progress).is_empty());
        assert!(drawn(&progress).is_empty(), "{:?}", drawn(&progress));
        // The widening pass that follows counts from that zero, so its first line is still honest.
        await_probes(vec![thread::spawn(|| ())], &mut progress);
        assert_eq!(
            drawn(&progress).first().map(String::as_str),
            Some("probing 1 mirror… 0/1")
        );
    }

    /// The frame is a pure function of the time a phase has been active: ten Braille cells, one every
    /// `SPIN_INTERVAL`, and it starts over rather than sticking (SPEC.md Output).
    #[test]
    fn the_spinner_frame_is_a_pure_function_of_time() {
        assert_eq!(SPIN, ['⠋', '⠙', '⠹', '⠸', '⠼', '⠴', '⠦', '⠧', '⠇', '⠏']);
        // Every cell is one character inside the Braille block, so a frame never shifts the text it
        // carries and a captured line is one cell plus the text.
        assert!(SPIN.iter().all(|c| c.to_string().chars().count() == 1));
        assert!(SPIN.iter().all(|c| ('\u{2800}'..='\u{28ff}').contains(c)));
        assert_eq!(spinner_frame(Duration::ZERO), '⠋');
        assert_eq!(spinner_frame(Duration::from_millis(99)), '⠋');
        assert_eq!(spinner_frame(Duration::from_millis(100)), '⠙');
        assert_eq!(spinner_frame(Duration::from_millis(300)), '⠸');
        // Ten frames, then it starts over: an eleven-second phase is still moving, not stuck.
        assert_eq!(spinner_frame(Duration::from_millis(1_000)), '⠋');
        assert_eq!(spinner_frame(Duration::from_millis(1_100)), '⠙');
    }

    /// The one line a frame carries: the cell, a space, then the phase's own text, so the probing count
    /// rides inside the same frame instead of becoming a second line (SPEC.md Output).
    #[test]
    fn the_frame_line_carries_the_probing_count() {
        assert_eq!(
            spinner_line(Duration::from_millis(200), &probing_line(2, 3)),
            "⠹ probing 3 mirrors… 2/3"
        );
        // One line, whatever it carries: no newline, and the frame is the first cell.
        let line = spinner_line(Duration::ZERO, "ranking");
        assert_eq!(line, "⠋ ranking");
        assert_eq!(line.lines().count(), 1);
    }

    /// The animation's loop with no thread and no sleep: a synthetic phase of three ticks draws four
    /// frames, each carrying the same text, and the frames advance **because time passed** and not
    /// because a count changed — the difference a still, count-driven line could not show.
    #[test]
    fn the_ticker_advances_a_frame_every_tick() {
        let mut sink = RecordingSink::default();
        let (ack, acked) = mpsc::sync_channel(1);
        let script = [
            Wake::Message(Tick::Text("probing 3 mirrors… 1/3".to_string(), ack)),
            Wake::Timeout,
            Wake::Timeout,
            Wake::Timeout,
            Wake::Message(Tick::Stop),
        ];
        let mut waits = script.into_iter();
        run_ticker(&mut sink, move |_| {
            waits.next().expect("the script ends with Stop")
        });
        assert_eq!(
            sink.lines,
            [
                "⠋ probing 3 mirrors… 1/3",
                "⠙ probing 3 mirrors… 1/3",
                "⠹ probing 3 mirrors… 1/3",
                "⠸ probing 3 mirrors… 1/3",
            ]
        );
        // The text never changed, and the four lines are four different frames: that is the whole fix.
        assert_eq!(
            sink.lines
                .iter()
                .filter_map(|l| l.chars().next())
                .collect::<Vec<char>>(),
            SPIN[..4].to_vec()
        );
        // The draw was acknowledged only after it happened.
        assert_eq!(acked.try_recv(), Ok(()));
    }

    /// The frame is a pure function of the time **this** phase has been active (SPEC.md Output), so a
    /// phase starts on the first cell however long the run has been going, while a count change under
    /// the same phase leaves the clock where it was. Without the reset each phase's first frame would
    /// be a function of the whole run's elapsed time instead.
    #[test]
    fn a_new_phase_starts_the_frame_over_and_a_count_does_not() {
        let mut sink = RecordingSink::default();
        let script = [
            Wake::Message(Tick::Phase(probing_line(1, 3), {
                let (ack, _) = mpsc::sync_channel(1);
                ack
            })),
            Wake::Timeout,
            Wake::Timeout,
            // The same phase, one more probe finished: the clock keeps running.
            Wake::Message(Tick::Text(probing_line(2, 3), {
                let (ack, _) = mpsc::sync_channel(1);
                ack
            })),
            Wake::Timeout,
            // A new phase: the clock starts over, so its first frame is the first cell.
            Wake::Message(Tick::Phase("ranking".to_string(), {
                let (ack, _) = mpsc::sync_channel(1);
                ack
            })),
            Wake::Timeout,
            Wake::Message(Tick::Stop),
        ];
        let mut waits = script.into_iter();
        run_ticker(&mut sink, move |_| {
            waits.next().expect("the script ends with Stop")
        });
        assert_eq!(
            sink.lines,
            [
                "⠋ probing 3 mirrors… 1/3",
                "⠙ probing 3 mirrors… 1/3",
                "⠹ probing 3 mirrors… 1/3",
                // The count changed and the cell did not: the phase's clock kept running.
                "⠹ probing 3 mirrors… 2/3",
                "⠸ probing 3 mirrors… 2/3",
                // A new phase opens on the first cell, whatever the run had already drawn.
                "⠋ ranking",
                "⠙ ranking",
            ]
        );
    }

    /// A warning keeps its own line: `hide` erases the live line and acknowledges the erase, so nothing
    /// the animation drew can sit over the warning, and the next phase starts a fresh line
    /// (SPEC.md Output).
    #[test]
    fn the_ticker_hides_the_line_before_a_warning_and_resumes() {
        let mut sink = RecordingSink::default();
        let (shown, drawn_ack) = mpsc::sync_channel(1);
        let (hidden, hidden_ack) = mpsc::sync_channel(1);
        let script = [
            Wake::Message(Tick::Text("verifying the reference".to_string(), shown)),
            Wake::Message(Tick::Hide(hidden)),
            Wake::Message(Tick::Text("ranking".to_string(), {
                let (ack, _) = mpsc::sync_channel(1);
                ack
            })),
            Wake::Message(Tick::Stop),
        ];
        let mut waits = script.into_iter();
        run_ticker(&mut sink, move |_| {
            waits.next().expect("the script ends with Stop")
        });
        assert_eq!(sink.events.len(), 3, "{:?}", sink.events);
        assert_eq!(sink.events[0], "rewrite:⠋ verifying the reference");
        assert_eq!(sink.events[1], "clear");
        // The line comes back with the next phase, so the animation resumes instead of stopping.
        assert!(sink.events[2].ends_with("ranking"), "{:?}", sink.events);
        // Both acknowledgements were sent, which is what makes "the line is gone" a fact.
        assert_eq!(drawn_ack.try_recv(), Ok(()));
        assert_eq!(hidden_ack.try_recv(), Ok(()));
    }

    /// The run's own `clear` reaches the animation, so a warning mid-run erases the live line instead
    /// of sharing it, and the animation draws again for the phase that follows.
    #[test]
    fn progress_hands_the_live_line_back_before_a_warning() {
        let mut progress = Progress::new(RecordingSink::default(), true);
        progress.load();
        progress.clear();
        progress.ranked();
        progress.finish(1, 0);
        let sink = lock(&progress.sink);
        let at = sink
            .events
            .iter()
            .position(|e| e == "clear")
            .unwrap_or_else(|| panic!("the clear never reached the sink: {:?}", sink.events));
        assert_eq!(sink.clears, 1);
        assert!(
            sink.events[at - 1].ends_with("loading the mirror list"),
            "the cleared line was not the phase on screen: {:?}",
            sink.events
        );
        assert!(
            sink.events[at + 1].ends_with("ranking"),
            "the animation did not resume: {:?}",
            sink.events
        );
    }

    /// The animation never outlives the summary: it is joined before the `done` line is written, so no
    /// frame can land on the summary or on the report's first line (SPEC.md Output).
    #[test]
    fn progress_stops_its_ticker_before_the_done_line() {
        let mut progress = Progress::new(RecordingSink::default(), true);
        progress.load();
        progress.ranked();
        progress.finish(4, 0);
        let sink = lock(&progress.sink);
        let settled = sink
            .events
            .iter()
            .position(|e| e.starts_with("settle:"))
            .unwrap_or_else(|| panic!("no summary: {:?}", sink.events));
        assert_eq!(
            settled,
            sink.events.len() - 1,
            "a frame was drawn after the summary: {:?}",
            sink.events
        );
        assert!(
            sink.events[settled].starts_with("settle:done — 4 usable, 0 rejected in "),
            "{:?}",
            sink.events
        );
    }

    /// `stop` joins: once it returns the animation cannot draw again, which is what makes an early
    /// `return Err` in `run()` safe — the progress owns the animation and its `Drop` stops it.
    #[test]
    fn stopping_the_animation_joins_it() {
        let sink = Arc::new(Mutex::new(RecordingSink::default()));
        let mut ticker = Ticker::start(Arc::clone(&sink));
        ticker.text("ranking".to_string());
        ticker.hide();
        let drawn_before = lock(&sink).events.len();
        assert!(drawn_before >= 2, "{:?}", lock(&sink).events);
        ticker.stop();
        assert!(ticker.handle.is_none(), "the thread was not joined");
        // A message after the join has nobody to draw it: the thread has ended.
        ticker.text("probing 3 mirrors… 3/3".to_string());
        assert_eq!(lock(&sink).events.len(), drawn_before);
    }

    /// An early exit stops and joins the animation **before** it prints: the line is erased, no
    /// `done` line follows a run with no count, and nothing can draw a frame over the report that
    /// prints next (SPEC.md Output). Hiding the line and printing while the thread still lived was the
    /// earlier shape, and it left the report's first line at the mercy of a frame.
    #[test]
    fn an_early_stop_joins_the_animation_before_the_report() {
        let mut progress = Progress::new(RecordingSink::default(), true);
        progress.load();
        progress.stop();
        let stopped = lock(&progress.sink).events.clone();
        assert_eq!(
            stopped.last().map(String::as_str),
            Some("clear"),
            "the live line was not the last thing drawn: {stopped:?}"
        );
        assert_eq!(
            stopped.iter().filter(|e| *e == "clear").count(),
            1,
            "the one erase came from the stop: {stopped:?}"
        );
        assert!(
            progress.ticker.as_ref().is_some_and(|t| t.handle.is_none()),
            "the animation outlived the stop"
        );
        assert!(
            lock(&progress.sink).settled.is_empty(),
            "an early stop has no count, so it prints no summary"
        );
        // A phase named after the join draws nothing: there is no thread left to draw it.
        progress.load();
        progress.stop();
        assert_eq!(lock(&progress.sink).events, stopped);
    }

    /// A piped run has no animation to stop, and stopping one is neither an error nor a line.
    #[test]
    fn stopping_a_piped_run_writes_nothing() {
        let mut progress = Progress::new(RecordingSink::default(), false);
        progress.stop();
        let sink = lock(&progress.sink);
        assert!(sink.events.is_empty(), "{:?}", sink.events);
    }

    /// The one line that survives, with one decimal place, as SPEC.md Output spells it.
    #[test]
    fn the_summary_line_has_one_decimal_place() {
        assert_eq!(
            done_line(4, 0, Duration::from_millis(3_100)),
            "done — 4 usable, 0 rejected in 3.1 s"
        );
        assert_eq!(
            done_line(2, 7, Duration::from_millis(59_940)),
            "done — 2 usable, 7 rejected in 59.9 s"
        );
        // The counts come from the ranked title, which a run with nothing usable still carries.
        assert_eq!(
            done_line(0, 0, Duration::from_millis(40)),
            "done — 0 usable, 0 rejected in 0.0 s"
        );
    }
}
