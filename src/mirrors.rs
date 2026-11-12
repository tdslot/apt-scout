//! Mirror lists (see SPEC.md, Pipeline step 3): parsers are pure and tested on saved fixtures.
// ponytail: not called by main yet; remove this allow when the pipeline uses the module.
#![allow(dead_code)]

use std::fmt;

#[derive(Debug, Clone, PartialEq)]
pub enum Status {
    Up,
    Unknown,
    Other(String),
}

#[derive(Debug, Clone, PartialEq)]
pub struct Mirror {
    pub country: String,
    pub name: String,
    pub url: String,
    pub https: bool,
    pub speed: Option<String>,
    pub status: Status,
}

#[derive(Debug, PartialEq)]
pub enum ParseError {
    /// The page has no country section: the format probably changed.
    NoCountries,
    /// No usable http(s) mirror was found in the whole page.
    NoMirrors,
}

impl fmt::Display for ParseError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ParseError::NoCountries => write!(
                f,
                "the Launchpad mirror page has no country sections (<th colspan=\"2\">); its format has probably changed"
            ),
            ParseError::NoMirrors => {
                write!(f, "the Launchpad mirror page lists no http(s) mirrors")
            }
        }
    }
}

/// Text between the first `open` and the next `close`, plus the rest after `close`.
fn between<'a>(s: &'a str, open: &str, close: &str) -> Option<(&'a str, &'a str)> {
    let start = s.find(open)? + open.len();
    let end = start + s[start..].find(close)?;
    Some((&s[start..end], &s[end + close.len()..]))
}

fn strip_tags(s: &str) -> String {
    let mut out = String::new();
    let mut in_tag = false;
    for c in s.chars() {
        match c {
            '<' => in_tag = true,
            '>' => in_tag = false,
            _ if !in_tag => out.push(c),
            _ => {}
        }
    }
    out.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// `<a href="URL">http(s)</a>` links of one row, in page order: (url, is_https).
fn web_links(row: &str) -> Vec<(String, bool)> {
    let mut links = Vec::new();
    let mut rest = row;
    while let Some((href, after)) = between(rest, "<a href=\"", "\"") {
        rest = after;
        if let Some(url) = href.strip_prefix("https://") {
            links.push((format!("https://{url}"), true));
        } else if let Some(url) = href.strip_prefix("http://") {
            links.push((format!("http://{url}"), false));
        }
    }
    links
}

/// Parse the Launchpad `+archivemirrors` page. Rows without http(s) links (ftp/rsync only)
/// are skipped; unknown statuses are kept as `Status::Other`.
pub fn parse_launchpad(html: &str) -> Result<Vec<Mirror>, ParseError> {
    let mut mirrors = Vec::new();
    let mut country: Option<String> = None;
    let mut countries = 0;

    for chunk in html.split("<tr").skip(1) {
        if let Some((name, _)) = between(chunk, "<th colspan=\"2\">", "</th>") {
            country = Some(strip_tags(name));
            countries += 1;
            continue;
        }
        let Some(country) = country.as_ref() else {
            continue;
        };
        let Some((_, after_id)) = between(chunk, "href=\"/ubuntu/+mirror/", "\"") else {
            continue;
        };
        let name = between(after_id, ">", "</a>")
            .map(|(n, _)| strip_tags(n))
            .unwrap_or_default();
        let links = web_links(chunk);
        // Prefer https; both are listed per mirror.
        let Some((url, https)) = links
            .iter()
            .find(|l| l.1)
            .or_else(|| links.first())
            .cloned()
        else {
            continue;
        };
        let speed = chunk
            .split("<td>")
            .map(strip_tags)
            .find(|t| t.ends_with("bps"));
        let status = match between(chunk, "distromirrorstatus", "\"") {
            Some((code, _)) => match code {
                "UP" => Status::Up,
                "UNKNOWN" => Status::Unknown,
                other => Status::Other(other.to_string()),
            },
            None => Status::Unknown,
        };
        mirrors.push(Mirror {
            country: country.clone(),
            name,
            url,
            https,
            speed,
            status,
        });
    }

    if countries == 0 {
        return Err(ParseError::NoCountries);
    }
    if mirrors.is_empty() {
        return Err(ParseError::NoMirrors);
    }
    Ok(mirrors)
}

/// Parse a `mirrors.ubuntu.com/<CC>.txt` list: one URL per line, in proximity order.
/// Lines that are not http(s) URLs are ignored.
pub fn parse_text_list(text: &str) -> Vec<String> {
    text.lines()
        .map(str::trim)
        .filter(|l| l.starts_with("http://") || l.starts_with("https://"))
        .map(String::from)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    const LAUNCHPAD: &str = include_str!("../tests/fixtures/launchpad-archivemirrors.html");

    fn by_country<'a>(m: &'a [Mirror], c: &str) -> Vec<&'a Mirror> {
        m.iter().filter(|x| x.country == c).collect()
    }

    #[test]
    fn launchpad_lithuania() {
        let all = parse_launchpad(LAUNCHPAD).expect("fixture parses");
        let lt = by_country(&all, "Lithuania");
        assert_eq!(lt.len(), 5);
        let bacloud = lt
            .iter()
            .find(|m| m.url.contains("bacloud"))
            .expect("bacloud");
        assert_eq!(
            bacloud.url,
            "https://mirror.bacloud.com/ubuntu-mirror/archive/"
        );
        assert!(bacloud.https);
        assert_eq!(bacloud.speed.as_deref(), Some("10 Gbps"));
        assert_eq!(bacloud.status, Status::Up);
        let vu = lt
            .iter()
            .find(|m| m.url.contains("mirror.vu.lt"))
            .expect("vu");
        assert_eq!(vu.url, "http://ubuntu.mirror.vu.lt/ubuntu/");
        assert!(!vu.https);
        assert_eq!(vu.status, Status::Unknown);
    }

    #[test]
    fn launchpad_skips_rows_without_http() {
        let all = parse_launchpad(LAUNCHPAD).expect("fixture parses");
        assert!(all.iter().all(|m| m.url.starts_with("http")));
        // fixture was built from a page that had rows with only ftp/rsync links
        let html_rows = LAUNCHPAD.matches("href=\"/ubuntu/+mirror/").count();
        assert!(
            all.len() < html_rows,
            "{} parsed of {html_rows} rows",
            all.len()
        );
    }

    #[test]
    fn launchpad_all_countries_present() {
        let all = parse_launchpad(LAUNCHPAD).expect("fixture parses");
        for c in ["Algeria", "Lithuania", "Norway", "Germany", "Netherlands"] {
            assert!(!by_country(&all, c).is_empty(), "{c}");
        }
    }

    #[test]
    fn launchpad_changed_format_is_an_error() {
        assert_eq!(
            parse_launchpad("<html><body>maintenance</body></html>"),
            Err(ParseError::NoCountries)
        );
        let only_head = "<tr class=\"head\"><th colspan=\"2\">Lithuania</th></tr>";
        assert_eq!(parse_launchpad(only_head), Err(ParseError::NoMirrors));
    }

    #[test]
    fn launchpad_unknown_status_is_kept() {
        let html = "<tr class=\"head\"><th colspan=\"2\">X</th></tr><tr><td><a href=\"/ubuntu/+mirror/m\">M</a></td><td><a href=\"https://m.example/ubuntu/\">https</a></td><td>1 Gbps</td><td><span class=\"distromirrorstatusONEDAYBEHIND\">x</span></td></tr>";
        let all = parse_launchpad(html).expect("parses");
        assert_eq!(all[0].status, Status::Other("ONEDAYBEHIND".into()));
    }

    #[test]
    fn text_list() {
        let lt = parse_text_list(include_str!("../tests/fixtures/mirrors-LT.txt"));
        // The order depends on the requester's location, so assert content, not position.
        assert_eq!(lt.len(), 6);
        assert!(lt.contains(&"http://ubuntu.mirror.vu.lt/ubuntu/".to_string()));
        assert!(lt.contains(&"http://archive.ubuntu.com/ubuntu/".to_string()));
        assert_eq!(
            parse_text_list("junk\nftp://x\n  http://a/  \n\n"),
            vec!["http://a/"]
        );
    }
}

/// Why a candidate URL was rejected (SPEC.md, Apply: candidate hygiene).
#[derive(Debug, PartialEq)]
pub enum Reject {
    Scheme,
    Userinfo,
    Whitespace,
    NoHost,
    PrivateAddress(String),
}

impl fmt::Display for Reject {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Reject::Scheme => write!(f, "scheme is not http or https"),
            Reject::Userinfo => write!(f, "URL contains credentials (user@host)"),
            Reject::Whitespace => write!(f, "URL contains whitespace"),
            Reject::NoHost => write!(f, "URL has no host"),
            Reject::PrivateAddress(a) => {
                write!(f, "host {a} is a private, loopback or link-local address")
            }
        }
    }
}

fn is_private(ip: std::net::IpAddr) -> bool {
    match ip {
        std::net::IpAddr::V4(v4) => {
            v4.is_private() || v4.is_loopback() || v4.is_link_local() || v4.is_unspecified()
        }
        std::net::IpAddr::V6(v6) => {
            let seg = v6.segments();
            v6.is_loopback()
                || v6.is_unspecified()
                || (seg[0] & 0xfe00) == 0xfc00 // unique local fc00::/7
                || (seg[0] & 0xffc0) == 0xfe80 // link-local fe80::/10
        }
    }
}

/// Check a candidate mirror URL before it is probed or written anywhere. Names are not resolved
/// here: a hostname that later resolves to a private address must be re-checked by the prober.
pub fn validate_url(url: &str) -> Result<(), Reject> {
    if url.chars().any(char::is_whitespace) {
        return Err(Reject::Whitespace);
    }
    let rest = url
        .strip_prefix("https://")
        .or_else(|| url.strip_prefix("http://"))
        .ok_or(Reject::Scheme)?;
    let authority = rest.split(['/', '?', '#']).next().unwrap_or_default();
    if authority.contains('@') {
        return Err(Reject::Userinfo);
    }
    let host = if let Some(v6) = authority.strip_prefix('[') {
        v6.split(']').next().unwrap_or_default()
    } else {
        authority.split(':').next().unwrap_or_default()
    };
    if host.is_empty() {
        return Err(Reject::NoHost);
    }
    match host.parse::<std::net::IpAddr>() {
        Ok(ip) if is_private(ip) => Err(Reject::PrivateAddress(host.to_string())),
        _ => Ok(()),
    }
}

#[cfg(test)]
mod hygiene_tests {
    use super::*;

    #[test]
    fn accepts_normal_mirrors() {
        for u in [
            "http://archive.ubuntu.com/ubuntu/",
            "https://mirror.bacloud.com/ubuntu-mirror/archive/",
            "http://mirror.example.org:8080/ubuntu",
            "https://[2001:4860:4860::8888]/ubuntu/",
            "http://8.8.8.8/ubuntu",
        ] {
            assert_eq!(validate_url(u), Ok(()), "{u}");
        }
    }

    #[test]
    fn rejects_bad_urls() {
        assert_eq!(validate_url("ftp://x.example/ubuntu"), Err(Reject::Scheme));
        assert_eq!(validate_url("file:///etc/passwd"), Err(Reject::Scheme));
        assert_eq!(
            validate_url("http://user:pw@host.example/"),
            Err(Reject::Userinfo)
        );
        assert_eq!(
            validate_url("http://host.example/a b"),
            Err(Reject::Whitespace)
        );
        assert_eq!(validate_url("http:///ubuntu"), Err(Reject::NoHost));
    }

    #[test]
    fn rejects_private_addresses() {
        for u in [
            "http://127.0.0.1/ubuntu",
            "http://10.1.2.3/ubuntu",
            "http://192.168.0.1/ubuntu",
            "http://172.16.5.5/ubuntu",
            "http://169.254.169.254/latest",
            "http://0.0.0.0/",
            "http://[::1]/ubuntu",
            "http://[fe80::1]/ubuntu",
            "http://[fd00::1]/ubuntu",
        ] {
            assert!(
                matches!(validate_url(u), Err(Reject::PrivateAddress(_))),
                "{u}"
            );
        }
    }
}
