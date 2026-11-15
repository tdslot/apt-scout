//! Detect the distribution from os-release (see SPEC.md, Pipeline step 1).

use std::fs;

const PATHS: [&str; 2] = ["/etc/os-release", "/usr/lib/os-release"];

#[derive(Debug, PartialEq)]
pub struct Distro {
    pub id: String,
    pub id_like: Vec<String>,
    /// `UBUNTU_CODENAME` when present, else `VERSION_CODENAME` (Mint reports wilma, apt uses noble).
    pub codename: Option<String>,
    /// `VERSION_ID` (for example `24.04`), shown in the run's header and nothing else.
    pub version_id: Option<String>,
}

impl Distro {
    pub fn is_ubuntu_family(&self) -> bool {
        self.id == "ubuntu" || self.id_like.iter().any(|l| l == "ubuntu")
    }
}

// ponytail: no shell escapes (\" \$) or `export`; fine for ID and codename values. Add if a distro needs it.
pub fn parse(text: &str) -> Distro {
    let get = |key: &str| {
        text.lines()
            .filter_map(|l| l.split_once('='))
            .rfind(|(k, _)| k.trim() == key) // last assignment wins
            .map(|(_, v)| v.trim().trim_matches(|c| c == '"' || c == '\'').to_string())
            .filter(|v| !v.is_empty())
    };
    Distro {
        id: get("ID").unwrap_or_default(),
        id_like: get("ID_LIKE")
            .map(|v| v.split_whitespace().map(String::from).collect())
            .unwrap_or_default(),
        codename: get("UBUNTU_CODENAME").or_else(|| get("VERSION_CODENAME")),
        version_id: get("VERSION_ID"),
    }
}

/// Returns the parsed distro and the file it was read from.
pub fn detect() -> Result<(Distro, &'static str), String> {
    let mut tried = Vec::new();
    for path in PATHS {
        match fs::read_to_string(path) {
            Ok(text) => return Ok((parse(&text), path)),
            Err(e) => tried.push(format!("{path}: {e}")),
        }
    }
    Err(format!(
        "Cannot read os-release ({}). apt-scout reads /etc/os-release (or /usr/lib/os-release) to find the Ubuntu codename; run it on an Ubuntu-based Linux system.",
        tried.join("; ")
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ubuntu() {
        let d = parse(
            "NAME=\"Ubuntu\"\nVERSION_ID=\"24.04\"\nID=ubuntu\nID_LIKE=debian\nVERSION_CODENAME=noble\nUBUNTU_CODENAME=noble\n",
        );
        assert!(d.is_ubuntu_family());
        assert_eq!(d.codename.as_deref(), Some("noble"));
        assert_eq!(d.version_id.as_deref(), Some("24.04"));
    }

    #[test]
    fn mint_uses_ubuntu_codename() {
        let d = parse(
            "ID=linuxmint\nID_LIKE=\"ubuntu debian\"\nVERSION_CODENAME=wilma\nUBUNTU_CODENAME=noble\n",
        );
        assert!(d.is_ubuntu_family());
        assert_eq!(d.codename.as_deref(), Some("noble"));
    }

    #[test]
    fn pop() {
        let d = parse(
            "ID=pop\nID_LIKE=\"ubuntu debian\"\nVERSION_CODENAME=jammy\nUBUNTU_CODENAME=jammy\n",
        );
        assert!(d.is_ubuntu_family());
        assert_eq!(d.id, "pop");
    }

    #[test]
    fn debian_is_not_ubuntu_family() {
        let d = parse("ID=debian\nVERSION_CODENAME=bookworm\n");
        assert!(!d.is_ubuntu_family());
        assert_eq!(d.codename.as_deref(), Some("bookworm"));
    }

    #[test]
    fn last_assignment_wins() {
        assert_eq!(parse("ID=a\nID=ubuntu\n").id, "ubuntu");
        assert_eq!(
            parse("UBUNTU_CODENAME=noble\nUBUNTU_CODENAME=\nVERSION_CODENAME=jammy\n")
                .codename
                .as_deref(),
            Some("jammy")
        );
    }

    #[test]
    fn missing_codename() {
        assert_eq!(parse("ID=ubuntu\n").codename, None);
        // A release file without VERSION_ID is shown by codename alone, never guessed.
        assert_eq!(parse("ID=ubuntu\n").version_id, None);
    }
}
