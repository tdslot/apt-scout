//! Server country and AS from third-party sources (SPEC.md, Pipeline step 2). Everything here is
//! optional: any failure leaves the answer unknown and the run continues (SPEC.md, Resilience).
//! Parsers are pure and tested on saved (anonymised) fixtures; answers are untrusted and validated.

use crate::http::{Fetch, MAX_SMALL};

#[derive(Debug, Default, PartialEq, Clone)]
pub struct Geo {
    pub country: Option<String>,
    /// Canonical `AS<number>`.
    pub asn: Option<String>,
    /// The server's public IP, needed to ask Team Cymru for the AS. Never stored on disk.
    pub ip: Option<String>,
}

/// `^[A-Z]{2}$`, and not one of the placeholder codes services use for "unknown" or "Tor".
pub fn valid_country(raw: &str) -> Option<String> {
    let cc = raw.trim().to_ascii_uppercase();
    let ok = cc.len() == 2 && cc.chars().all(|c| c.is_ascii_uppercase());
    (ok && !matches!(cc.as_str(), "XX" | "T1" | "ZZ")).then_some(cc)
}

/// Canonical `AS<number>` from `AS64496`, `AS64496 Some Name`, or a bare `64496`.
pub fn normalize_asn(raw: &str) -> Option<String> {
    let token = raw.split_whitespace().next()?;
    let digits = token
        .strip_prefix("AS")
        .or_else(|| token.strip_prefix("as"))
        .unwrap_or(token);
    if digits.is_empty() || digits.len() > 10 || !digits.chars().all(|c| c.is_ascii_digit()) {
        return None;
    }
    let n: u64 = digits.parse().ok()?;
    (n > 0 && n <= u64::from(u32::MAX)).then(|| format!("AS{n}"))
}

fn valid_ip(raw: &str) -> Option<String> {
    raw.trim()
        .parse::<std::net::IpAddr>()
        .ok()
        .map(|ip| ip.to_string())
}

/// Text of the first `<tag>...</tag>`.
fn xml_field<'a>(xml: &'a str, tag: &str) -> Option<&'a str> {
    let open = format!("<{tag}>");
    let start = xml.find(&open)? + open.len();
    let end = start + xml[start..].find(&format!("</{tag}>"))?;
    Some(&xml[start..end])
}

/// Value of `"key": "string"` in a flat JSON object. Good enough for the three services we read;
/// a value with escapes or nesting is rejected rather than guessed.
fn json_string(json: &str, key: &str) -> Option<String> {
    let pat = format!("\"{key}\"");
    let after = &json[json.find(&pat)? + pat.len()..];
    let after = after
        .trim_start()
        .strip_prefix(':')?
        .trim_start()
        .strip_prefix('"')?;
    let end = after.find('"')?;
    let v = &after[..end];
    (!v.contains('\\')).then(|| v.to_string())
}

pub fn parse_ubuntu_geoip(xml: &str) -> Geo {
    Geo {
        country: xml_field(xml, "CountryCode").and_then(valid_country),
        ip: xml_field(xml, "Ip").and_then(valid_ip),
        asn: None,
    }
}

pub fn parse_ifconfig_co(json: &str) -> Geo {
    Geo {
        country: json_string(json, "country_iso")
            .as_deref()
            .and_then(valid_country),
        asn: json_string(json, "asn").as_deref().and_then(normalize_asn),
        ip: json_string(json, "ip").as_deref().and_then(valid_ip),
    }
}

pub fn parse_ipinfo(json: &str) -> Geo {
    Geo {
        country: json_string(json, "country")
            .as_deref()
            .and_then(valid_country),
        asn: json_string(json, "org").as_deref().and_then(normalize_asn),
        ip: json_string(json, "ip").as_deref().and_then(valid_ip),
    }
}

pub fn parse_cloudflare_trace(text: &str) -> Geo {
    let field = |k: &str| {
        text.lines()
            .find_map(|l| l.strip_prefix(k))
            .map(str::to_string)
    };
    Geo {
        country: field("loc=").as_deref().and_then(valid_country),
        ip: field("ip=").as_deref().and_then(valid_ip),
        asn: None,
    }
}

type Parser = fn(&str) -> Geo;

const SOURCES: [(&str, &str, Parser); 4] = [
    (
        "Canonical geoip",
        "http://geoip.ubuntu.com/lookup",
        parse_ubuntu_geoip,
    ),
    ("ifconfig.co", "https://ifconfig.co/json", parse_ifconfig_co),
    ("ipinfo.io", "https://ipinfo.io/json", parse_ipinfo),
    (
        "Cloudflare trace",
        "https://www.cloudflare.com/cdn-cgi/trace",
        parse_cloudflare_trace,
    ),
];

/// First source that yields a valid country wins; later sources may still fill in a missing AS or IP.
/// Returns what is known and one line per source that failed or gave nothing usable.
pub fn detect(net: &dyn Fetch) -> (Geo, Vec<String>) {
    let mut geo = Geo::default();
    let mut notes = Vec::new();
    for (name, url, parse) in SOURCES {
        if geo.country.is_some() && geo.asn.is_some() && geo.ip.is_some() {
            break;
        }
        match net.get(url, MAX_SMALL, None) {
            Ok(got) => {
                let found = parse(&String::from_utf8_lossy(&got.body));
                if found.country.is_none() && found.asn.is_none() && found.ip.is_none() {
                    notes.push(format!("{name} ({url}) answered but had no usable country, AS or IP; its format may have changed"));
                    continue;
                }
                geo.country = geo.country.take().or(found.country);
                geo.asn = geo.asn.take().or(found.asn);
                geo.ip = geo.ip.take().or(found.ip);
            }
            Err(e) => notes.push(format!("{name}: {e}")),
        }
    }
    (geo, notes)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::http::Canned;

    const GEOIP: &str = include_str!("../tests/fixtures/geoip-ubuntu.xml");
    const IFCONFIG: &str = include_str!("../tests/fixtures/ifconfig-co.json");
    const IPINFO: &str = include_str!("../tests/fixtures/ipinfo.json");
    const CLOUDFLARE: &str = include_str!("../tests/fixtures/cloudflare-trace.txt");

    #[test]
    fn real_shaped_answers_parse() {
        let g = parse_ubuntu_geoip(GEOIP);
        assert_eq!(
            (g.country.as_deref(), g.ip.as_deref(), g.asn),
            (Some("NO"), Some("203.0.113.7"), None)
        );
        let g = parse_ifconfig_co(IFCONFIG);
        assert_eq!(
            (g.country.as_deref(), g.asn.as_deref(), g.ip.as_deref()),
            (Some("NO"), Some("AS64496"), Some("203.0.113.7"))
        );
        let g = parse_ipinfo(IPINFO);
        assert_eq!(
            (g.country.as_deref(), g.asn.as_deref()),
            (Some("NO"), Some("AS64496"))
        );
        let g = parse_cloudflare_trace(CLOUDFLARE);
        assert_eq!(
            (g.country.as_deref(), g.ip.as_deref(), g.asn),
            (Some("NO"), Some("203.0.113.7"), None)
        );
    }

    #[test]
    fn country_validation() {
        assert_eq!(valid_country(" lt "), Some("LT".into()));
        for bad in [
            "", "L", "LTU", "XX", "T1", "ZZ", "1A", "Unknown", "L T", "Ł",
        ] {
            assert_eq!(valid_country(bad), None, "{bad}");
        }
    }

    #[test]
    fn asn_normalization() {
        assert_eq!(normalize_asn("AS64496"), Some("AS64496".into()));
        assert_eq!(
            normalize_asn("AS64496 Example Network AS"),
            Some("AS64496".into())
        );
        assert_eq!(normalize_asn("64496"), Some("AS64496".into()));
        assert_eq!(normalize_asn("as15169 Google"), Some("AS15169".into()));
        for bad in [
            "",
            "AS",
            "ASabc",
            "AS12abc",
            "AS0",
            "AS99999999999",
            "-5",
            "AS-1",
        ] {
            assert_eq!(normalize_asn(bad), None, "{bad}");
        }
    }

    #[test]
    fn changed_format_gives_unknown_not_garbage() {
        assert_eq!(parse_ifconfig_co("<html>blocked</html>"), Geo::default());
        assert_eq!(parse_ubuntu_geoip("{\"country\":\"NO\"}"), Geo::default());
        assert_eq!(
            parse_cloudflare_trace("loc=XX\nip=not-an-ip\n"),
            Geo::default()
        );
        // an escaped value is rejected rather than guessed
        assert_eq!(parse_ipinfo("{\"country\": \"N\\u004f\"}").country, None);
    }

    #[test]
    fn first_source_wins_and_later_ones_fill_gaps() {
        let net = Canned(vec![
            ("http://geoip.ubuntu.com/lookup", Ok(GEOIP.as_bytes())),
            ("https://ifconfig.co/json", Ok(IFCONFIG.as_bytes())),
        ]);
        let (g, notes) = detect(&net);
        assert_eq!(g.country.as_deref(), Some("NO"));
        assert_eq!(g.asn.as_deref(), Some("AS64496")); // geoip has no AS; ifconfig.co supplied it
        assert!(notes.is_empty(), "{notes:?}");
    }

    #[test]
    fn failures_are_reported_and_next_source_is_used() {
        let net = Canned(vec![
            ("http://geoip.ubuntu.com/lookup", Err(503)),
            ("https://ifconfig.co/json", Err(429)),
            ("https://ipinfo.io/json", Ok(IPINFO.as_bytes())),
        ]);
        let (g, notes) = detect(&net);
        assert_eq!(g.country.as_deref(), Some("NO"));
        assert_eq!(notes.len(), 2);
        assert!(
            notes[0].contains("Canonical geoip") && notes[0].contains("503"),
            "{notes:?}"
        );
        assert!(
            notes[1].contains("ifconfig.co") && notes[1].contains("429"),
            "{notes:?}"
        );
    }

    #[test]
    fn every_source_failing_is_not_fatal() {
        let net = Canned(vec![]);
        let (g, notes) = detect(&net);
        assert_eq!(g, Geo::default());
        assert_eq!(notes.len(), 4);
    }

    #[test]
    fn a_reachable_but_useless_source_is_called_out() {
        let net = Canned(vec![(
            "http://geoip.ubuntu.com/lookup",
            Ok(b"<html>captive portal</html>".as_slice()),
        )]);
        let (_, notes) = detect(&net);
        assert!(notes[0].contains("format may have changed"), "{notes:?}");
    }
}
