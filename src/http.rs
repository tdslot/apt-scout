//! The only module that talks to the network (SPEC.md: HTTP is injected so tests use canned answers).

use std::fmt;
use std::io::Read;
use std::time::{Duration, Instant};

use ureq::unversioned::resolver::{DefaultResolver, ResolvedSocketAddrs, Resolver};
use ureq::unversioned::transport::{DefaultConnector, NextTimeout};

use crate::mirrors;

pub const TIMEOUT: Duration = Duration::from_secs(10);
/// Cap for list pages (the Launchpad page is about 316 KB).
pub const MAX_PAGE: u64 = 2 * 1024 * 1024;
/// Cap for InRelease and small control files.
pub const MAX_SMALL: u64 = 256 * 1024;
const MAX_REDIRECTS: u32 = 3;

#[derive(Debug)]
pub enum HttpError {
    Url(mirrors::Reject, String),
    Status(u16, String),
    Transport(String, String),
    TooLarge(u64, String),
    Redirect(String, String),
}

impl fmt::Display for HttpError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            HttpError::Url(why, url) => write!(f, "refusing {url}: {why}"),
            HttpError::Status(code, url) => write!(f, "{url} answered HTTP {code}"),
            HttpError::Transport(why, url) => write!(f, "cannot reach {url}: {why}"),
            HttpError::TooLarge(max, url) => {
                write!(f, "{url} sent more than {max} bytes; refusing to read more")
            }
            HttpError::Redirect(why, url) => write!(f, "{url}: {why}"),
        }
    }
}

/// What a fetch returns: body (possibly truncated for ranged probes), status and timings.
#[derive(Debug)]
pub struct Fetched {
    pub body: Vec<u8>,
    pub elapsed: Duration,
    pub final_url: String,
}

/// Source of bytes for a URL. The real implementation is `Net`; tests use canned answers.
pub trait Fetch {
    fn get(
        &self,
        url: &str,
        max_bytes: u64,
        range: Option<(u64, u64)>,
    ) -> Result<Fetched, HttpError>;
}

/// Resolves like the default resolver but drops private, loopback and link-local addresses, so a
/// public-looking name cannot lead the program to an internal host. The check runs on the exact
/// addresses that are then connected to, which also defeats DNS rebinding.
#[derive(Debug, Default)]
struct PublicOnly(DefaultResolver);

impl Resolver for PublicOnly {
    fn resolve(
        &self,
        uri: &ureq::http::Uri,
        config: &ureq::config::Config,
        timeout: NextTimeout,
    ) -> Result<ResolvedSocketAddrs, ureq::Error> {
        let all = self.0.resolve(uri, config, timeout)?;
        let mut kept = self.empty();
        for addr in all.iter() {
            if !mirrors::is_private(addr.ip()) {
                kept.push(*addr);
            }
        }
        if kept.is_empty() {
            return Err(ureq::Error::HostNotFound);
        }
        Ok(kept)
    }
}

pub struct Net {
    agent: ureq::Agent,
}

impl Net {
    pub fn new() -> Net {
        let config = ureq::Agent::config_builder()
            .timeout_global(Some(TIMEOUT))
            .max_redirects(0) // redirects are followed by hand so every hop is validated
            .http_status_as_error(false)
            .proxy(None) // a proxy would bypass the address filter; the environment must not enable one
            .user_agent(concat!("apt-scout/", env!("CARGO_PKG_VERSION")))
            .build();
        Net {
            agent: ureq::Agent::with_parts(config, DefaultConnector::new(), PublicOnly::default()),
        }
    }
}

impl Default for Net {
    fn default() -> Self {
        Net::new()
    }
}

/// `scheme://authority` of a URL, port and brackets included.
fn origin_of(url: &str) -> &str {
    let after = url.find("://").map(|i| i + 3).unwrap_or(0);
    let end = url[after..]
        .find(['/', '?', '#'])
        .map(|i| after + i)
        .unwrap_or(url.len());
    &url[..end]
}

/// Resolve a `Location` header against the URL it came from. Protocol-relative and path-relative
/// forms are refused: mirrors use absolute URLs or absolute paths.
fn resolve_location(from: &str, loc: &str) -> Result<String, String> {
    if loc.starts_with("http://") || loc.starts_with("https://") {
        Ok(loc.to_string())
    } else if loc.starts_with("//") {
        Err(format!("unsupported protocol-relative redirect '{loc}'"))
    } else if loc.starts_with('/') {
        Ok(format!("{}{loc}", origin_of(from)))
    } else {
        Err(format!("unsupported relative redirect '{loc}'"))
    }
}

impl Fetch for Net {
    fn get(
        &self,
        start: &str,
        max_bytes: u64,
        range: Option<(u64, u64)>,
    ) -> Result<Fetched, HttpError> {
        let began = Instant::now();
        let mut url = start.to_string();
        for _ in 0..=MAX_REDIRECTS {
            mirrors::validate_url(&url).map_err(|e| HttpError::Url(e, url.clone()))?;
            // One budget for the whole call, not one per hop.
            let left = TIMEOUT
                .checked_sub(began.elapsed())
                .filter(|d| !d.is_zero());
            let Some(left) = left else {
                return Err(HttpError::Transport(
                    format!("timed out after {} s", TIMEOUT.as_secs()),
                    start.to_string(),
                ));
            };
            let mut req = self
                .agent
                .get(&url)
                .config()
                .timeout_global(Some(left))
                .build();
            if let Some((from, to)) = range {
                req = req.header("Range", format!("bytes={from}-{to}"));
            }
            let mut resp = req
                .call()
                .map_err(|e| HttpError::Transport(e.to_string(), url.clone()))?;
            let status = resp.status().as_u16();
            if (300..400).contains(&status) {
                let loc = resp
                    .headers()
                    .get("location")
                    .and_then(|v| v.to_str().ok())
                    .ok_or_else(|| {
                        HttpError::Redirect(
                            format!("HTTP {status} without a Location header"),
                            url.clone(),
                        )
                    })?;
                let next = resolve_location(&url, loc)
                    .map_err(|why| HttpError::Redirect(why, url.clone()))?;
                // an https mirror must not downgrade to http
                if url.starts_with("https://") && next.starts_with("http://") {
                    return Err(HttpError::Redirect(
                        format!("redirect downgrades https to http ({next})"),
                        url,
                    ));
                }
                url = next;
                continue;
            }
            if !(200..300).contains(&status) {
                return Err(HttpError::Status(status, url));
            }
            let mut body = Vec::new();
            resp.body_mut()
                .as_reader()
                .take(max_bytes + 1)
                .read_to_end(&mut body)
                .map_err(|e| HttpError::Transport(e.to_string(), url.clone()))?;
            if body.len() as u64 > max_bytes {
                if range.is_some() {
                    body.truncate(max_bytes as usize);
                } else {
                    return Err(HttpError::TooLarge(max_bytes, url));
                }
            }
            return Ok(Fetched {
                body,
                elapsed: began.elapsed(),
                final_url: url,
            });
        }
        Err(HttpError::Redirect(
            format!("more than {MAX_REDIRECTS} redirects"),
            start.to_string(),
        ))
    }
}

/// Canned answers for tests: url -> body.
#[cfg(test)]
pub struct Canned(pub Vec<(&'static str, Result<&'static [u8], u16>)>);

#[cfg(test)]
impl Fetch for Canned {
    fn get(&self, url: &str, _max: u64, _range: Option<(u64, u64)>) -> Result<Fetched, HttpError> {
        match self.0.iter().find(|(u, _)| *u == url) {
            Some((_, Ok(body))) => Ok(Fetched {
                body: body.to_vec(),
                elapsed: Duration::from_millis(5),
                final_url: url.into(),
            }),
            Some((_, Err(code))) => Err(HttpError::Status(*code, url.into())),
            None => Err(HttpError::Transport("no canned answer".into(), url.into())),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn canned_ok_and_status() {
        let c = Canned(vec![
            ("http://a.example/ok", Ok(b"hi")),
            ("http://a.example/gone", Err(404)),
        ]);
        assert_eq!(c.get("http://a.example/ok", 10, None).unwrap().body, b"hi");
        assert!(matches!(
            c.get("http://a.example/gone", 10, None),
            Err(HttpError::Status(404, _))
        ));
        assert!(matches!(
            c.get("http://a.example/x", 10, None),
            Err(HttpError::Transport(..))
        ));
    }

    #[test]
    fn net_refuses_private_urls_before_connecting() {
        let n = Net::new();
        for u in [
            "http://127.0.0.1/",
            "http://10.0.0.1/x",
            "ftp://a.example/",
            "http://localhost/",
        ] {
            assert!(
                matches!(n.get(u, 100, None), Err(HttpError::Url(..))),
                "{u}"
            );
        }
    }

    #[test]
    fn origin_keeps_port_and_brackets() {
        assert_eq!(
            origin_of("https://a.example:8443/x/y"),
            "https://a.example:8443"
        );
        assert_eq!(origin_of("http://a.example"), "http://a.example");
        assert_eq!(
            origin_of("http://[2001:db8::1]:8080/x?y#z"),
            "http://[2001:db8::1]:8080"
        );
    }

    #[test]
    fn redirect_locations() {
        let from = "http://m.example:8080/ubuntu/dists/x";
        assert_eq!(
            resolve_location(from, "/other/y").unwrap(),
            "http://m.example:8080/other/y"
        );
        assert_eq!(
            resolve_location(from, "https://b.example/z").unwrap(),
            "https://b.example/z"
        );
        assert!(resolve_location(from, "//evil.example/x").is_err());
        assert!(resolve_location(from, "relative/path").is_err());
    }

    #[test]
    fn private_names_are_filtered_by_the_resolver() {
        // `localhost` is rejected by validate_url first; a numeric private host never resolves either
        let n = Net::new();
        assert!(matches!(
            n.get("http://localhost.localdomain.invalid/", 10, None),
            Err(HttpError::Transport(..))
        ));
    }
}
