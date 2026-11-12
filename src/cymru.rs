//! Team Cymru IP-to-ASN lookup over plain DNS TXT (SPEC.md, Pipeline step 2 and 5). std has no TXT
//! API and an async resolver crate contradicts the no-async rule, so this is a small UDP client.
//! The packet builder and the answer parser are pure and tested; only `lookup` touches the network.

use std::net::{IpAddr, UdpSocket};
use std::time::{Duration, Instant};

use crate::geo::normalize_asn;

const TIMEOUT: Duration = Duration::from_secs(4);
pub const TOTAL_BUDGET: Duration = Duration::from_secs(12);

#[derive(Debug, PartialEq)]
pub enum DnsError {
    /// The name does not exist (private or unrouted addresses): AS is simply unknown.
    NxDomain,
    /// Answer had the truncated bit set; the caller falls back to another source (no TCP).
    Truncated,
    Malformed(&'static str),
    Rcode(u8),
}

impl std::fmt::Display for DnsError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            DnsError::NxDomain => write!(f, "no AS data for this address"),
            DnsError::Truncated => write!(f, "DNS answer was truncated"),
            DnsError::Malformed(why) => write!(f, "malformed DNS answer: {why}"),
            DnsError::Rcode(c) => write!(f, "DNS server answered error code {c}"),
        }
    }
}

/// `1.2.3.4` -> `4.3.2.1.origin.asn.cymru.com.`; IPv6 uses `origin6` with reversed nibbles.
pub fn query_name(ip: IpAddr) -> String {
    match ip {
        IpAddr::V4(v4) => {
            let o = v4.octets();
            format!("{}.{}.{}.{}.origin.asn.cymru.com.", o[3], o[2], o[1], o[0])
        }
        IpAddr::V6(v6) => {
            let mut s = String::new();
            for byte in v6.octets().iter().rev() {
                s.push_str(&format!("{:x}.{:x}.", byte & 0xf, byte >> 4));
            }
            format!("{s}origin6.asn.cymru.com.")
        }
    }
}

/// A DNS TXT query packet (RFC 1035) for `name` with the given transaction id.
pub fn build_query(name: &str, id: u16) -> Vec<u8> {
    let mut p = Vec::with_capacity(32 + name.len());
    p.extend_from_slice(&id.to_be_bytes());
    p.extend_from_slice(&[0x01, 0x00, 0, 1, 0, 0, 0, 0, 0, 0]); // RD, 1 question
    for label in name.trim_end_matches('.').split('.') {
        p.push(label.len() as u8);
        p.extend_from_slice(label.as_bytes());
    }
    p.push(0);
    p.extend_from_slice(&[0, 16, 0, 1]); // TXT, IN
    p
}

/// Decode a (possibly compressed) name starting at `i`; returns its normalized form and the
/// index after the encoded name. Pointers are bounded and must point within the packet.
fn read_name(r: &[u8], mut i: usize) -> Result<(String, usize), DnsError> {
    let mut end = None;
    let mut labels = Vec::new();
    for _ in 0..64 {
        let len = *r
            .get(i)
            .ok_or(DnsError::Malformed("name runs past the packet"))?;
        if len & 0xC0 == 0xC0 {
            let second = *r
                .get(i + 1)
                .ok_or(DnsError::Malformed("compression pointer cut off"))?;
            let pointer = (((len & 0x3f) as usize) << 8) | second as usize;
            if pointer >= r.len() {
                return Err(DnsError::Malformed("compression pointer outside packet"));
            }
            end.get_or_insert(i + 2);
            i = pointer;
            continue;
        }
        if len & 0xC0 != 0 {
            return Err(DnsError::Malformed("reserved label encoding"));
        }
        i += 1;
        if len == 0 {
            return Ok((labels.join(".").to_ascii_lowercase(), end.unwrap_or(i)));
        }
        let label = r
            .get(i..i + len as usize)
            .ok_or(DnsError::Malformed("label runs past packet"))?;
        labels.push(
            std::str::from_utf8(label)
                .map_err(|_| DnsError::Malformed("non-ASCII DNS label"))?
                .to_string(),
        );
        i += len as usize;
    }
    Err(DnsError::Malformed("name too long or compression loop"))
}

/// Parse the matching TXT answer, validating transaction ID, response/question names and type.
pub fn parse_answer(r: &[u8], id: u16, expected_name: &str) -> Result<String, DnsError> {
    if r.len() < 12 || r[..2] != id.to_be_bytes() {
        return Err(DnsError::Malformed("wrong transaction id or too short"));
    }
    let flags = u16::from_be_bytes([r[2], r[3]]);
    if flags & 0x8000 == 0 {
        return Err(DnsError::Malformed("not a response"));
    }
    if flags & 0x0200 != 0 {
        return Err(DnsError::Truncated);
    }
    match (flags & 0xF) as u8 {
        0 => {}
        3 => return Err(DnsError::NxDomain),
        c => return Err(DnsError::Rcode(c)),
    }
    let (qd, an) = (
        u16::from_be_bytes([r[4], r[5]]),
        u16::from_be_bytes([r[6], r[7]]),
    );
    if qd != 1 {
        return Err(DnsError::Malformed("expected exactly one question"));
    }
    let (question, after_name) = read_name(r, 12)?;
    let q = r
        .get(after_name..after_name + 4)
        .ok_or(DnsError::Malformed("question cut off"))?;
    if question != expected_name.trim_end_matches('.').to_ascii_lowercase()
        || u16::from_be_bytes([q[0], q[1]]) != 16
        || u16::from_be_bytes([q[2], q[3]]) != 1
    {
        return Err(DnsError::Malformed("question does not match TXT IN query"));
    }
    let mut i = after_name + 4;
    for _ in 0..an {
        let (owner, after_owner) = read_name(r, i)?;
        i = after_owner;
        let head = r
            .get(i..i + 10)
            .ok_or(DnsError::Malformed("record header cut off"))?;
        let (ty, class, rdlen) = (
            u16::from_be_bytes([head[0], head[1]]),
            u16::from_be_bytes([head[2], head[3]]),
            u16::from_be_bytes([head[8], head[9]]) as usize,
        );
        i += 10;
        let rdata = r
            .get(i..i + rdlen)
            .ok_or(DnsError::Malformed("record data cut off"))?;
        if ty == 16
            && class == 1
            && owner == expected_name.trim_end_matches('.').to_ascii_lowercase()
            && !rdata.is_empty()
        {
            let n = rdata[0] as usize;
            let txt = rdata
                .get(1..1 + n)
                .ok_or(DnsError::Malformed("TXT string cut off"))?;
            return Ok(String::from_utf8_lossy(txt).into_owned());
        }
        i += rdlen;
    }
    Err(DnsError::Malformed("no TXT record in the answer"))
}

/// The AS number from a Cymru answer: `64496 | 203.0.113.0/24 | NO | ripencc | 2010-01-01`.
pub fn asn_from_txt(txt: &str) -> Option<String> {
    normalize_asn(txt.split('|').next()?.trim())
}

/// Nameservers from `/etc/resolv.conf`, at most three, IPv4 and IPv6 literals only.
pub fn nameservers(resolv_conf: &str) -> Vec<IpAddr> {
    resolv_conf
        .lines()
        .filter_map(|l| l.trim().strip_prefix("nameserver"))
        .filter_map(|v| v.split_whitespace().next()?.split('%').next()?.parse().ok())
        .take(3)
        .collect()
}

fn nameserver_timeout(remaining: Duration) -> Option<Duration> {
    (!remaining.is_zero()).then(|| TIMEOUT.min(remaining))
}

fn unpredictable_id() -> Result<u16, DnsError> {
    use std::io::Read;
    let mut bytes = [0u8; 2];
    std::fs::File::open("/dev/urandom")
        .and_then(|mut f| f.read_exact(&mut bytes))
        .map_err(|_| DnsError::Malformed("cannot read /dev/urandom for DNS transaction ID"))?;
    Ok(u16::from_be_bytes(bytes))
}

/// AS for `ip` via Cymru DNS through the system resolver. A missing `resolv.conf` or no answer
/// means unknown (the caller falls back to the next source). Total UDP wait is bounded.
pub fn lookup(ip: IpAddr) -> Result<String, DnsError> {
    let conf = std::fs::read_to_string("/etc/resolv.conf")
        .map_err(|_| DnsError::Malformed("cannot read /etc/resolv.conf"))?;
    let servers = nameservers(&conf);
    if servers.is_empty() {
        return Err(DnsError::Malformed("no nameserver in /etc/resolv.conf"));
    }
    let id = unpredictable_id()?;
    let name = query_name(ip);
    let query = build_query(&name, id);
    let deadline = Instant::now() + TOTAL_BUDGET;
    let mut last = DnsError::Malformed("no nameserver answered");
    for server in servers {
        let timeout = match nameserver_timeout(deadline.saturating_duration_since(Instant::now())) {
            Some(timeout) => timeout,
            None => break,
        };
        let bind = if server.is_ipv4() {
            "0.0.0.0:0"
        } else {
            "[::]:0"
        };
        let Ok(sock) = UdpSocket::bind(bind) else {
            continue;
        };
        let _ = sock.set_read_timeout(Some(timeout));
        if sock.connect((server, 53)).is_err() || sock.send(&query).is_err() {
            continue;
        }
        let mut buf = [0u8; 512];
        let Ok(n) = sock.recv(&mut buf) else { continue };
        match parse_answer(&buf[..n], id, &name) {
            Ok(txt) => {
                return asn_from_txt(&txt).ok_or(DnsError::Malformed("answer has no AS number"));
            }
            Err(DnsError::NxDomain) => return Err(DnsError::NxDomain),
            Err(e) => last = e,
        }
    }
    Err(last)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Real answer captured for `7.113.0.203.origin.asn.cymru.com` (TXT), id 0x1234.
    fn answer(id: u16, flags: u16, txt: Option<&str>) -> Vec<u8> {
        let mut p = id.to_be_bytes().to_vec();
        p.extend_from_slice(&flags.to_be_bytes());
        p.extend_from_slice(&[0, 1, 0, if txt.is_some() { 1 } else { 0 }, 0, 0, 0, 0]);
        let q = build_query("7.113.0.203.origin.asn.cymru.com", id);
        p.extend_from_slice(&q[12..]); // the question section
        if let Some(t) = txt {
            p.extend_from_slice(&[0xC0, 0x0C, 0, 16, 0, 1, 0, 0, 0, 60]);
            p.extend_from_slice(&((t.len() + 1) as u16).to_be_bytes());
            p.push(t.len() as u8);
            p.extend_from_slice(t.as_bytes());
        }
        p
    }

    #[test]
    fn query_names() {
        assert_eq!(
            query_name("203.0.113.7".parse().unwrap()),
            "7.113.0.203.origin.asn.cymru.com."
        );
        assert_eq!(
            query_name("2001:4860:4860::8888".parse().unwrap()),
            "8.8.8.8.0.0.0.0.0.0.0.0.0.0.0.0.0.0.0.0.0.6.8.4.0.6.8.4.1.0.0.2.origin6.asn.cymru.com."
        );
    }

    #[test]
    fn query_packet_layout() {
        let p = build_query("a.bc.", 0xABCD);
        assert_eq!(&p[..2], &[0xAB, 0xCD]);
        assert_eq!(&p[4..6], &[0, 1]); // one question
        assert_eq!(&p[12..], &[1, b'a', 2, b'b', b'c', 0, 0, 16, 0, 1]);
    }

    #[test]
    fn parses_a_txt_answer() {
        let txt = "64496 | 203.0.113.0/24 | NO | ripencc | 2010-01-01";
        let r = answer(0x1234, 0x8180, Some(txt));
        assert_eq!(
            parse_answer(&r, 0x1234, "7.113.0.203.origin.asn.cymru.com.").unwrap(),
            txt
        );
        assert_eq!(asn_from_txt(txt), Some("AS64496".into()));
    }

    #[test]
    fn rejects_bad_answers() {
        let ok = answer(1, 0x8180, Some("15169 | x"));
        assert!(matches!(
            parse_answer(&ok, 2, "7.113.0.203.origin.asn.cymru.com."),
            Err(DnsError::Malformed(_))
        )); // wrong id
        assert!(matches!(
            parse_answer(
                &answer(1, 0x0100, None),
                1,
                "7.113.0.203.origin.asn.cymru.com."
            ),
            Err(DnsError::Malformed(_))
        )); // not a response
        assert_eq!(
            parse_answer(
                &answer(1, 0x8183, None),
                1,
                "7.113.0.203.origin.asn.cymru.com."
            ),
            Err(DnsError::NxDomain)
        );
        assert_eq!(
            parse_answer(
                &answer(1, 0x8182, None),
                1,
                "7.113.0.203.origin.asn.cymru.com."
            ),
            Err(DnsError::Rcode(2))
        );
        assert_eq!(
            parse_answer(
                &answer(1, 0x8380, Some("x")),
                1,
                "7.113.0.203.origin.asn.cymru.com."
            ),
            Err(DnsError::Truncated)
        );
        assert!(matches!(
            parse_answer(
                &answer(1, 0x8180, None),
                1,
                "7.113.0.203.origin.asn.cymru.com."
            ),
            Err(DnsError::Malformed(_))
        )); // no answer
        assert!(matches!(
            parse_answer(&ok[..20], 1, "7.113.0.203.origin.asn.cymru.com."),
            Err(DnsError::Malformed(_))
        )); // cut off
        assert!(matches!(
            parse_answer(&[], 1, "7.113.0.203.origin.asn.cymru.com."),
            Err(DnsError::Malformed(_))
        ));
    }

    #[test]
    fn rejects_unrelated_question_and_owner() {
        let wrong_question = answer(1, 0x8180, Some("15169 | x"));
        assert!(parse_answer(&wrong_question, 1, "other.asn.cymru.com.").is_err());

        let mut wrong_owner = answer(1, 0x8180, Some("15169 | x"));
        let rr_owner = build_query("7.113.0.203.origin.asn.cymru.com", 1).len();
        let other_owner = wrong_owner.len();
        wrong_owner.extend_from_slice(&[1, b'x', 0]);
        wrong_owner[rr_owner] = 0xC0;
        wrong_owner[rr_owner + 1] = other_owner as u8;
        assert!(parse_answer(&wrong_owner, 1, "7.113.0.203.origin.asn.cymru.com.").is_err());

        let mut loop_packet = vec![0, 1, 0x81, 0x80, 0, 1, 0, 0, 0, 0, 0, 0];
        loop_packet.extend_from_slice(&[0xC0, 12, 0, 16, 0, 1]);
        assert!(matches!(
            read_name(&loop_packet, 12),
            Err(DnsError::Malformed(_))
        ));
    }

    #[test]
    fn per_nameserver_timeout_never_exceeds_shared_budget() {
        assert_eq!(nameserver_timeout(Duration::from_secs(20)), Some(TIMEOUT));
        assert_eq!(
            nameserver_timeout(Duration::from_secs(2)),
            Some(Duration::from_secs(2))
        );
        assert_eq!(nameserver_timeout(Duration::ZERO), None);
    }

    #[test]
    fn resolv_conf_parsing() {
        let c = "# x\nsearch example.org\nnameserver 10.0.0.2\nnameserver fe80::1%eth0\nnameserver bad\nnameserver 1.1.1.1\nnameserver 8.8.8.8\n";
        let n = nameservers(c);
        assert_eq!(n.len(), 3);
        assert_eq!(n[0].to_string(), "10.0.0.2");
        assert_eq!(n[1].to_string(), "fe80::1");
        assert_eq!(n[2].to_string(), "1.1.1.1");
        assert!(nameservers("").is_empty());
    }
}
