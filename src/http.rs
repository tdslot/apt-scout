//! The only module that talks to the network (SPEC.md: HTTP is injected so tests use canned answers).
#![allow(dead_code)] // ponytail: not called by main yet; remove this allow when the pipeline uses it.

use std::fmt;
use std::io::Read;
use std::time::{Duration, Instant};

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
    pub status: u16,
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

pub struct Net {
    agent: ureq::Agent,
}

impl Net {
    pub fn new() -> Net {
        let config = ureq::Agent::config_builder()
            .timeout_global(Some(TIMEOUT))
            .max_redirects(0) // redirects are followed by hand so every hop is validated
            .http_status_as_error(false)
            .user_agent(concat!("apt-scout/", env!("CARGO_PKG_VERSION")))
            .build();
        Net {
            agent: config.into(),
        }
    }
}

impl Default for Net {
    fn default() -> Self {
        Net::new()
    }
}

fn host_of(url: &str) -> &str {
    let rest = url.split_once("://").map(|(_, r)| r).unwrap_or(url);
    rest.split(['/', ':']).next().unwrap_or_default()
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
            let mut req = self.agent.get(&url);
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
                let next = if loc.starts_with("http://") || loc.starts_with("https://") {
                    loc.to_string()
                } else if let Some(path) = loc.strip_prefix('/') {
                    let scheme = url.split("://").next().unwrap_or("http");
                    format!("{scheme}://{}/{path}", host_of(&url))
                } else {
                    return Err(HttpError::Redirect(
                        format!("unsupported relative redirect '{loc}'"),
                        url,
                    ));
                };
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
                status,
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
pub struct Canned(pub Vec<(&'static str, Result<&'static str, u16>)>);

#[cfg(test)]
impl Fetch for Canned {
    fn get(&self, url: &str, _max: u64, _range: Option<(u64, u64)>) -> Result<Fetched, HttpError> {
        match self.0.iter().find(|(u, _)| *u == url) {
            Some((_, Ok(body))) => Ok(Fetched {
                body: body.as_bytes().to_vec(),
                status: 200,
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
            ("http://a.example/ok", Ok("hi")),
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
    fn host_of_works() {
        assert_eq!(host_of("https://a.example:8443/x/y"), "a.example");
        assert_eq!(host_of("http://a.example"), "a.example");
    }
}
