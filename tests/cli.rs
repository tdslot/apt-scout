//! The binary itself, as a subprocess: the answers that exit before a run starts. Those are the
//! only ones that can be checked this way on any machine — `--version` and `--help` read no file,
//! open no socket and never look at the uid, so they answer the same for every user. The child
//! inherits the uid of whoever runs the tests, which is non-root on a laptop and in CI and root in
//! a container: the flag answering in all three is the "any uid" property, and no case here needs
//! the write matrix.

use std::process::{Command, Output};

/// The package version, read the way the crate reads it, so this test states what `Cargo.toml`
/// states and carries no second copy of the number.
const VERSION: &str = env!("CARGO_PKG_VERSION");

fn run(args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_apt-scout"))
        .args(args)
        .output()
        .expect("the apt-scout binary starts")
}

/// The one line `--version` prints, and the whole of what it prints.
fn expected() -> String {
    format!("apt-scout {VERSION}\n")
}

fn assert_version(output: &Output, argv: &str) {
    assert_eq!(
        output.status.code(),
        Some(0),
        "{argv} exits 0: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8(output.stdout.clone()).expect("stdout is UTF-8");
    assert_eq!(stdout, expected(), "{argv}");
    assert_eq!(
        stdout.lines().count(),
        1,
        "{argv} prints one line: {stdout}"
    );
    assert!(output.stderr.is_empty(), "{argv} prints nothing on stderr");
}

#[test]
fn version_prints_the_crate_version_as_one_line_and_exits_zero() {
    assert_version(&run(&["--version"]), "--version");
}

#[test]
fn the_short_version_is_the_same_line() {
    assert_version(&run(&["-V"]), "-V");
}

/// The interaction rule is `--help`'s, unchanged: the argument that answers exits the process where
/// the loop meets it, so the first of `--help` and `--version` a run was given wins, and a usage
/// error before either still wins over both (SPEC.md CLI).
#[test]
fn the_first_of_help_and_version_wins_and_an_earlier_error_beats_both() {
    // Version first: the version, not the help text, whatever follows it.
    let version_first = run(&["--version", "--help"]);
    assert_version(&version_first, "--version --help");

    // Help first: the help text, and no version line in it.
    let help_first = run(&["--help", "--version"]);
    assert_eq!(help_first.status.code(), Some(0), "--help exits 0");
    let help = String::from_utf8(help_first.stdout).expect("stdout is UTF-8");
    assert!(help.contains("USAGE"), "--help prints the text:\n{help}");
    assert!(
        !help.contains(&expected()),
        "--help never prints the version line"
    );

    // A usage error before either is still an error, exit 2, and neither answer is printed.
    let refused = run(&["--bogus", "--version"]);
    assert_eq!(refused.status.code(), Some(2), "--bogus exits 2");
    assert_eq!(
        String::from_utf8_lossy(&refused.stderr),
        "error: unknown option '--bogus'; try --help\n"
    );
    assert!(refused.stdout.is_empty(), "nothing is printed on stdout");
}
