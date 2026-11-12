mod distro;

use std::process::ExitCode;

fn main() -> ExitCode {
    let d = match distro::detect() {
        Ok(d) => d,
        Err(e) => {
            eprintln!("error: {e}");
            return ExitCode::from(1);
        }
    };
    if !d.is_ubuntu_family() {
        eprintln!(
            "error: unsupported distribution '{}'. apt-scout supports Ubuntu and derivatives (ID or ID_LIKE contains 'ubuntu').",
            d.id
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
