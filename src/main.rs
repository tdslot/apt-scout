mod distro;
mod mirrors;

use std::process::ExitCode;

fn main() -> ExitCode {
    let (d, source) = match distro::detect() {
        Ok(found) => found,
        Err(e) => {
            eprintln!("error: {e}");
            return ExitCode::from(1);
        }
    };
    if !d.is_ubuntu_family() {
        eprintln!(
            "error: unsupported distribution (ID='{}', ID_LIKE='{}' in {source}). apt-scout supports Ubuntu and derivatives: ID or ID_LIKE must contain 'ubuntu'.",
            d.id,
            d.id_like.join(" ")
        );
        return ExitCode::from(4);
    }
    match d.codename {
        Some(c) => println!("{} {c}", d.id),
        None => {
            eprintln!(
                "error: {source} contains neither UBUNTU_CODENAME nor VERSION_CODENAME, so the Ubuntu release cannot be determined. Check that the distribution's os-release file is intact; report this error with its ID and codename fields at https://github.com/tdslot/apt-scout/issues."
            );
            return ExitCode::from(1);
        }
    }
    ExitCode::SUCCESS
}
