mod distro;

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
                "error: no UBUNTU_CODENAME or VERSION_CODENAME in os-release; pass --codename."
            );
            return ExitCode::from(1);
        }
    }
    ExitCode::SUCCESS
}
