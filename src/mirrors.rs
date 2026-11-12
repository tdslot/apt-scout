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
        let status = match between(chunk, "distromirrorstatus", "") {
            Some((_, rest)) => {
                let code: String = rest
                    .chars()
                    .take_while(char::is_ascii_alphanumeric)
                    .collect();
                match code.as_str() {
                    "UP" => Status::Up,
                    "UNKNOWN" => Status::Unknown,
                    _ => Status::Other(code),
                }
            }
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

/// Why a candidate URL was rejected (SPEC.md, Apply: candidate hygiene).
#[derive(Debug, PartialEq)]
pub enum Reject {
    Scheme(String),
    Userinfo,
    TooLong(usize),
    Whitespace,
    NoHost,
    /// Characters outside letters, digits, `.` and `-` (backslash, `%` escapes, zone ids).
    BadHost(String),
    /// Number-like hosts that libc would read as an IPv4 address (`2130706433`, `0x7f.1`, `127.1`).
    NumericHost(String),
    BadPort(String),
    Localhost(String),
    PrivateAddress(String),
}

impl fmt::Display for Reject {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Reject::Scheme(s) => write!(
                f,
                "scheme '{s}' is not http or https; use an http:// or https:// mirror URL"
            ),
            Reject::Userinfo => write!(
                f,
                "URL contains '@' or a backslash (credentials or a parser-confusing character); remove it"
            ),
            Reject::TooLong(n) => write!(f, "URL is {n} bytes long, over the 2048 byte limit"),
            Reject::Whitespace => write!(f, "URL contains whitespace; remove it"),
            Reject::NoHost => write!(f, "URL has no host; use http://host/path"),
            Reject::BadHost(h) => write!(
                f,
                "host '{h}' contains characters other than letters, digits, '.' and '-'"
            ),
            Reject::NumericHost(h) => write!(
                f,
                "host '{h}' looks like a non-standard IPv4 form; write it as a.b.c.d or use a name"
            ),
            Reject::BadPort(p) => write!(f, "port '{p}' is not a number from 1 to 65535"),
            Reject::Localhost(h) => write!(f, "host '{h}' is a local name, not a public mirror"),
            Reject::PrivateAddress(a) => write!(
                f,
                "host {a} is a private, loopback, link-local, shared or multicast address, not a public mirror"
            ),
        }
    }
}

fn is_private(ip: std::net::IpAddr) -> bool {
    match ip {
        std::net::IpAddr::V4(v4) => {
            let o = v4.octets();
            v4.is_private()
                || v4.is_loopback()
                || v4.is_link_local()
                || v4.is_unspecified()
                || v4.is_multicast()
                || v4.is_broadcast()
                || o[0] == 0 // 0.0.0.0/8
                || o[0] >= 240 // 240.0.0.0/4 reserved
                || (o[0] == 192 && o[1] == 0 && o[2] == 0) // 192.0.0.0/24
                || (o[0] == 198 && (o[1] & 0xfe) == 18) // 198.18.0.0/15 benchmarking
                || (o[0] == 100 && (o[1] & 0xc0) == 64) // 100.64.0.0/10 shared address space
        }
        std::net::IpAddr::V6(v6) => {
            if let Some(v4) = v6.to_ipv4_mapped() {
                return is_private(v4.into());
            }
            let seg = v6.segments();
            seg[..6].iter().all(|&x| x == 0) // ::a.b.c.d IPv4-compatible
                || (seg[0] == 0x64 && seg[1] == 0xff9b) // 64:ff9b::/96 NAT64
                || seg[0] == 0x2002 // 2002::/16 6to4
                || (seg[0] & 0xffc0) == 0xfec0 // fec0::/10 site-local
                || v6.is_loopback()
                || v6.is_unspecified()
                || v6.is_multicast()
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
    // Parsers disagree on `\` and on `@` after an early authority end, so reject both anywhere.
    if url.contains('\\') || url.contains('@') {
        return Err(Reject::Userinfo);
    }
    if url.len() > 2048 {
        return Err(Reject::TooLong(url.len()));
    }
    let rest = url
        .strip_prefix("https://")
        .or_else(|| url.strip_prefix("http://"))
        .ok_or_else(|| Reject::Scheme(url.split(':').next().unwrap_or_default().to_string()))?;
    // `\` is treated like `/` by many URL parsers, so it also ends the authority.
    let authority = rest.split(['/', '\\', '?', '#']).next().unwrap_or_default();
    if authority.contains('@') {
        return Err(Reject::Userinfo);
    }
    let (host, port) = if let Some(v6) = authority.strip_prefix('[') {
        let (h, after) = v6
            .split_once(']')
            .ok_or_else(|| Reject::BadHost(authority.to_string()))?;
        let port = match after {
            "" => "",
            _ => after
                .strip_prefix(':')
                .ok_or_else(|| Reject::BadHost(authority.to_string()))?,
        };
        (h, port)
    } else {
        authority.split_once(':').unwrap_or((authority, ""))
    };
    if host.is_empty() {
        return Err(Reject::NoHost);
    }
    if !port.is_empty()
        && !(port.chars().all(|c| c.is_ascii_digit())
            && matches!(port.parse::<u16>(), Ok(p) if p > 0))
    {
        return Err(Reject::BadPort(port.to_string()));
    }
    if authority.starts_with('[') {
        return match host.parse::<std::net::Ipv6Addr>() {
            Ok(ip) if is_private(ip.into()) => Err(Reject::PrivateAddress(host.to_string())),
            Ok(_) => Ok(()),
            Err(_) => Err(Reject::BadHost(host.to_string())),
        };
    }
    let name = host.trim_end_matches('.');
    if name.is_empty() || name.len() > 253 {
        return Err(Reject::BadHost(host.to_string()));
    }
    if !name
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '.' || c == '-')
    {
        return Err(Reject::BadHost(host.to_string()));
    }
    if name.eq_ignore_ascii_case("localhost") || name.to_ascii_lowercase().ends_with(".localhost") {
        return Err(Reject::Localhost(host.to_string()));
    }
    let last = name.rsplit('.').next().unwrap_or_default();
    if !last.is_empty()
        && (last.chars().all(|c| c.is_ascii_digit())
            || last.starts_with("0x")
            || last.starts_with("0X"))
    {
        // Looks like an IPv4 address: only strict dotted quads are accepted, and checked.
        return match name.parse::<std::net::Ipv4Addr>() {
            Ok(ip) if is_private(ip.into()) => Err(Reject::PrivateAddress(host.to_string())),
            Ok(_) => Ok(()),
            Err(_) => Err(Reject::NumericHost(host.to_string())),
        };
    }
    Ok(())
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
    fn https_is_preferred_even_when_listed_second() {
        let html = "<tr class=\"head\"><th colspan=\"2\">X</th></tr><tr><td><a href=\"/ubuntu/+mirror/m\">M</a></td><td><a href=\"http://m.example/ubuntu/\">http</a><a href=\"https://m.example/ubuntu/\">https</a></td><td>1 Gbps</td><td><span class=\"distromirrorstatusUP\">x</span></td></tr>";
        let all = parse_launchpad(html).expect("parses");
        assert_eq!(all[0].url, "https://m.example/ubuntu/");
        assert!(all[0].https);
    }

    #[test]
    fn status_code_stops_at_non_alphanumeric() {
        let html = "<tr class=\"head\"><th colspan=\"2\">X</th></tr><tr><td><a href=\"/ubuntu/+mirror/m\">M</a></td><td><a href=\"https://m.example/\">https</a></td><td>1 Gbps</td><td><span class=\"distromirrorstatusUP extra\">x</span></td></tr>";
        assert_eq!(parse_launchpad(html).expect("parses")[0].status, Status::Up);
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

#[cfg(test)]
mod hygiene_tests {
    use super::*;

    #[test]
    fn accepts_normal_mirrors() {
        for u in [
            "http://archive.ubuntu.com/ubuntu/",
            "https://mirror.bacloud.com/ubuntu-mirror/archive/",
            "http://mirror.example.org:8080/ubuntu",
            "http://mirror.example.org./ubuntu",
            "https://[2001:4860:4860::8888]/ubuntu/",
            "https://[2001:4860:4860::8888]:8443/ubuntu/",
            "http://8.8.8.8/ubuntu",
            "http://ftp.uk.debian.org:80/",
        ] {
            assert_eq!(validate_url(u), Ok(()), "{u}");
        }
    }

    #[test]
    fn rejects_bad_urls() {
        assert!(
            matches!(validate_url("ftp://x.example/ubuntu"), Err(Reject::Scheme(s)) if s == "ftp")
        );
        assert!(matches!(
            validate_url("file:///etc/passwd"),
            Err(Reject::Scheme(_))
        ));
        assert!(matches!(
            validate_url("HTTP://x.example/"),
            Err(Reject::Scheme(_))
        ));
        assert_eq!(
            validate_url("http://user:pw@host.example/"),
            Err(Reject::Userinfo)
        );
        assert_eq!(
            validate_url("http://host.example/a b"),
            Err(Reject::Whitespace)
        );
        assert_eq!(validate_url("http:///ubuntu"), Err(Reject::NoHost));
        assert!(matches!(
            validate_url("http://host.example:abc/"),
            Err(Reject::BadPort(_))
        ));
        assert!(matches!(
            validate_url("http://host.example:99999/"),
            Err(Reject::BadPort(_))
        ));
        assert!(matches!(
            validate_url("http://host.example:0/"),
            Err(Reject::BadPort(_))
        ));
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
            "http://0.1.2.3/",
            "http://100.64.0.1/",
            "http://224.0.0.1/",
            "http://[::1]/ubuntu",
            "http://[fe80::1]/ubuntu",
            "http://[fd00::1]/ubuntu",
            "http://[ff02::1]/ubuntu",
            "http://[::ffff:10.0.0.1]/",
            "http://[::ffff:127.0.0.1]/",
        ] {
            assert!(
                matches!(validate_url(u), Err(Reject::PrivateAddress(_))),
                "{u}"
            );
        }
    }

    #[test]
    fn rejects_review_round_two_forms() {
        assert_eq!(
            validate_url("http://evil\\@127.0.0.1/"),
            Err(Reject::Userinfo)
        );
        assert_eq!(validate_url("http://a.example/x@y"), Err(Reject::Userinfo));
        assert!(matches!(validate_url("http://./"), Err(Reject::BadHost(_))));
        assert!(matches!(
            validate_url("https://[2001:db8::1]junk/"),
            Err(Reject::BadHost(_))
        ));
        assert!(matches!(
            validate_url("http://host.example:+80/"),
            Err(Reject::BadPort(_))
        ));
        assert!(matches!(
            validate_url(&format!("http://{}.example/", "a".repeat(300))),
            Err(Reject::BadHost(_))
        ));
        assert!(matches!(
            validate_url(&format!("http://a.example/{}", "x".repeat(3000))),
            Err(Reject::TooLong(_))
        ));
        for u in [
            "http://[::127.0.0.1]/",
            "http://[64:ff9b::7f00:1]/",
            "http://[2002:7f00:1::]/",
            "http://[fec0::1]/",
            "http://240.0.0.1/",
            "http://192.0.0.1/",
            "http://198.18.0.1/",
        ] {
            assert!(
                matches!(validate_url(u), Err(Reject::PrivateAddress(_))),
                "{u}"
            );
        }
    }

    #[test]
    fn rejects_ssrf_bypass_forms() {
        // libc getaddrinfo reads these as 127.0.0.1
        for u in [
            "http://2130706433/",
            "http://0x7f.1/",
            "http://0177.0.0.1/",
            "http://127.1/",
            "http://0x7f000001/",
        ] {
            assert!(
                matches!(validate_url(u), Err(Reject::NumericHost(_))),
                "{u}"
            );
        }
        // percent escapes, zone ids, backslash tricks
        assert!(matches!(
            validate_url("http://%31%32%37.0.0.1/"),
            Err(Reject::BadHost(_))
        ));
        assert!(matches!(
            validate_url("http://[fe80::1%25eth0]/"),
            Err(Reject::BadHost(_))
        ));
        assert_eq!(validate_url("http://127.0.0.1\\x/"), Err(Reject::Userinfo));
        assert!(matches!(
            validate_url("http://[::1/ubuntu"),
            Err(Reject::BadHost(_))
        ));
        // local names
        for u in [
            "http://localhost/",
            "http://localhost./",
            "http://LOCALHOST/",
            "http://x.localhost/",
        ] {
            assert!(matches!(validate_url(u), Err(Reject::Localhost(_))), "{u}");
        }
    }
}
