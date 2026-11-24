# Pipeline

1. `PIPE-1` The run MUST detect the distro from `/etc/os-release`.
   It MUST read the codename: `UBUNTU_CODENAME` when present, otherwise `VERSION_CODENAME` (Linux Mint
   reports `VERSION_CODENAME=wilma`, but apt uses `noble`) (ADR-0008).
2. `PIPE-2` The run MUST detect the location of the server (country, then AS), skipped with `--no-geo`.
   Every source is optional: if all fail, the program asks for `--country`, or for `--any` where no
   country is needed; it does not guess a worldwide list.
   The mirror-list sources themselves are Launchpad for the selected country, then the
   `mirrors.ubuntu.com` country list.
   Sources are tried in order and the first valid answer wins; web sources use HTTPS except
   Canonical's endpoint noted below; AS comes from DNS.
   1. Country: Canonical `http://geoip.ubuntu.com/lookup` (XML, field `CountryCode`; the same
      endpoint the Ubuntu installer uses; plain HTTP, so treat the answer as untrusted and validate
      it). Fallbacks: `https://ifconfig.co/json` (`country_iso`, operator asks for at most 1
      automated request per minute), `https://ipinfo.io/json` (`country`),
      `https://www.cloudflare.com/cdn-cgi/trace` (`loc=<CC>`).
   2. AS of the server: Team Cymru DNS, TXT query for `<reversed-ip>.origin.asn.cymru.com` (IPv6:
      `<nibble-reversed-ip>.origin6.asn.cymru.com`; answer `AS | prefix | CC | registry | date`; the
      `CC` is the registry country, not a location, so it is never used as the server country). No
      web API and no HTTP: plain DNS through the system resolver. Fallback in the current
      implementation: the already-fetched `ifconfig.co` or `ipinfo.io` `asn`/`org` fields. RIPEstat
      is planned, not yet implemented.
   - To query Cymru the server's public IP is needed: take it from whichever HTTP source
     answered (geoip, ifconfig.co, ipinfo, Cloudflare `ip=`).
     If none answered, AS is unknown.
   - No retries: any failure (timeout, non-200 including 429, malformed body, DNS error) falls
     through to the next source.
     HTTP requests use a 10 s whole-request timeout, including redirects.
     Geolocation responses are capped at 256 KiB; the mirror-list page has a 2 MiB cap.
     The maximum chain duration depends on which sources fail and whether later AS queries are
     needed; AS DNS lookup has a 12 s total per-query budget and mirror-AS ranking has a 20 s
     overall budget.
     Print which sources failed and why.
   - Validation: country is uppercased and MUST match `^[A-Z]{2}$`; `XX` (unknown), `T1` (Tor)
     and `Unknown` are treated as unknown.
     The same rule applies to `--country`.
   - AS normalization: every source's value becomes canonical `AS<number>`.
     Take the first whitespace-delimited token of `org`/`as`, prefix bare integers with `AS`,
     validate `^AS[0-9]{1,10}$`, compare numerically.
     Empty or malformed means unknown.
     For a prefix with several origin ASNs the first listed is used; this is approximate.
   - `--country` overrides the country only; AS is still looked up unless `--no-geo`.
   - Cache is planned, not yet implemented.
   - Privacy: live geolocation web services learn the server's public IP; system DNS and Team
     Cymru receive the IP inside a DNS query.
     Candidate mirror host IPs are sent to DNS and Team Cymru for AS lookup. `--no-geo` skips all
     these lookups and requires `--country`.
   - `--country` replaces only the country answer; the chain still runs to learn the public IP
     for the AS lookup unless `--no-geo`.
**Preflight before any network access:** validate CLI country values against the supported country
table and validate the local architecture.
Invalid country/unsupported architecture MUST fail before geolocation or any HTTP/DNS request.
Resolve this before invoking geo or mirror-list sources.

3. `PIPE-3` The run MUST load the mirror list, the first success wins, and the source it used MUST be
   shown in the output. The sources are tried in this order:
   1. Launchpad `https://launchpad.net/ubuntu/+archivemirrors` (HTML). Each mirror row carries a
      `/ubuntu/+mirror/<id>` link, one `<a href>` per protocol, a declared speed and a status; only
      `http(s)` URLs are used, and a row with neither is skipped: it cannot be probed.
      Launchpad's own status (UP = up to date) is a hint, never proof of freshness.
      The parser MUST tolerate a missing protocol and an unknown status, and MUST be tested against a
      saved fixture of this page. The page's measured shape, counts and date are recorded in
      [ADR-0005](../decisions/0005-mirror-list-and-freshness-measurements.md), which is what the fixture
      and the tolerance rules follow from.
   2. `http://mirrors.ubuntu.com/<CC>.txt` (plain text, one URL per line, already ordered by
      geographic proximity, no speed and no status field).
      Used when Launchpad fails or returns no mirror for the country.
      One country's file per run, and no run fetches another country's file: the file
      cannot widen a country's set, which is why it is a fallback and not a source of its own
      ([ADR-0005](../decisions/0005-mirror-list-and-freshness-measurements.md)).
   3. Planned, not implemented (D28): disk cache `~/.cache/apt-scout/mirrors.json` (valid 24 h;
      stale cache used only if all else fails, with a warning)
   4. Planned, not implemented: snapshot compiled into the binary (dated; warn when older than 90 days)

   Each network source: one 10 s time budget per call covering redirects (no retries in v1: a
failure falls through to the next source), response validated (non-empty, at least one well-formed
`http(s)://` URL; for Launchpad, at least one country section).
Invalid response counts as failure.
4. `PIPE-4` The run MUST filter to the mirrors in the detected country only, unless `--any` is given,
   which removes the
   country from the candidate set; plus the one automatic widening described below, which adds every
   other country's mirrors once and only when the run's own country came back with nothing usable.
   Candidates are deduplicated (ignoring a trailing slash and case), capped at 30, and
   `--https-only` drops http.
   This filter narrows candidates only: every mirror host the Launchpad page names, in every
   country, stays eligible for replacement under Apply's rewrite rule, and `--https-only` does not
   narrow that set.
   An unknown country code (not one of Launchpad's 82 countries) is a usage error (exit 2) before
   any network request; it does not fall back to `mirrors.ubuntu.com/<CC>.txt`.
   - `--any` (`-a`): every mirror the loaded list names enters the candidate set, whatever
     country the list puts it in, and they are ranked together as one set.
     The country is display-only: the run still detects it or takes it from `--country`, prints it,
     and says it is not a constraint, and it decides nothing else. `--any` with `--country` is
     therefore not an error. `--any` needs no country at all, so with `--no-geo` it satisfies the
     country requirement, the `<CC>.txt` fallback then has no file to read, and only Launchpad can
     supply the list; a list that names one country is reported as one country, never presented as
     every country.
     The match and tie-break of step 9 are unchanged, so AS preference still applies when the server
     AS is known.
   - **Automatic widening, once, from an empty set only.** When every candidate in the run's
     own country is rejected — stale, unreachable, non-200 including 404, no usable `InRelease` or
     `Date`, no speed sample, or a crashed probe — the run adds the mirrors the same list names for
     every other country, probes those in one extra pass, ranks them together with the country's own
     results, and prints one `note:` line under the report header saying why it widened.
     It does not exit with an error.
     It is never a preference and never a top-up of a set that has a usable mirror: a run that found
     one usable mirror in its own country never widens, and no candidate is probed twice.
     A widened run that finds a mirror exits as any run does — 0, or 3 for a non-root uid that would
     have written; a widened run that finds nothing fails exactly as it did before, with the
     no-fresh-mirror message and exit 1. `--any` never widens: its set is already every country, so
     an empty one means nothing usable anywhere.
   - Widening reuses the list the run already loaded and never fetches a second one.
     Launchpad names every country, so it can widen. `mirrors.ubuntu.com/<CC>.txt` names one country
     per run and no run fetches another country's file (step 3), so a run whose only list is that
     file cannot widen: it prints a line saying so and fails exactly as before.
     A widened set is therefore always drawn from a list fetched over HTTPS.
   - The 30-candidate cap applies to a set that is not narrowed to one country, and there it is
     applied by Launchpad's own declared link speed, descending, then by URL: the page names about
     560 mirrors, and probing them all is not a burst any run makes.
     That prefilter decides what is probed and never what ranks — a mirror whose declared speed is
     unknown sorts last among the candidates and still ranks wherever its probe puts it.
     A run narrowed to one country keeps the cap in list order.
5. `PIPE-5` The run MUST resolve the AS of the candidates (skipped with `--no-geo`; informational,
   never a reason to fail the run): resolve each hostname to an IP, then Team Cymru DNS TXT per IP
   (parallel, cap 8 at a time), normalized as in step 2.
   No per-IP fallback is implemented yet; failures leave AS unknown.
   Cymru client: hand-rolled raw UDP query (about 80 lines; std has no TXT API and an async resolver
   crate contradicts the no-async rule).
   Read nameservers from `/etc/resolv.conf` (up to 3; the systemd-resolved stub `127.0.0.53` and
   Docker's `127.0.0.11` handle TXT), use an absolute name with a trailing dot (no search-domain
   expansion), connect the UDP socket, use a transaction ID, validate the reply ID and `QR` bit, cap
   compression-pointer traversal, treat NXDOMAIN as AS unknown, and treat the `TC` (truncated) bit
   as failure (no TCP fallback).
   Missing `resolv.conf` means AS unknown.
   No HTTP batch API and no rate-limited free tier is required.
   AS caching is planned, not implemented.
   CDN-backed mirrors can resolve to different AS per query; AS is approximate.
   Current implementation resolves the first address returned by the system resolver; IPv6-specific
   candidate selection is not implemented yet.
6. `PIPE-6` The run MUST fetch the reference once, before probing: from
   `http://archive.ubuntu.com/ubuntu/dists/<codename>-updates/InRelease` and
   `<codename>-security/InRelease` get the `Date:` of each pocket.
   The `<codename>-updates` reference is downloaded in full under the 512 KiB download/verification
   cap and its OpenPGP signature is verified, and both its `Date` and its signature status are kept:
   it is the freshness baseline a write may trust (decision 0003).
   The `<codename>-security` reference is a ranged 4 KiB head and is never verified: nothing may
   consume its signature (decision 0003).
   This is the freshness baseline.
   The `<codename>` release pocket is not compared: its `Date` is the release date (for example
   `noble` shows April 2024) and never changes, so it says nothing about freshness.
7. `PIPE-7` The run MUST probe each candidate in parallel (`std::thread`): fetch
   `<mirror>/dists/<codename>-updates/InRelease` in full under the 512 KiB download/verification cap
   so its OpenPGP signature can be checked, and fetch only the head (`Range: bytes=0-4095`) of
   `<mirror>/dists/<codename>-security/InRelease` for the informational `Date`.
   Measure time to first byte (DNS, TCP, TLS and one round trip) on the `-updates` request, never
   the body download time.
   Measure throughput on a larger sample (`Range: bytes=0-1048575` of
   `main/binary-<arch>/Packages.xz`): the `InRelease` is too small and too variable to measure
   throughput ([ADR-0005](../decisions/0005-mirror-list-and-freshness-measurements.md)).
   Redirects: follow at most 3, re-validate the final host and scheme; report mirrors that redirect
   to another host.
8. `PIPE-8` The run MUST reject unreachable mirrors, non-200 responses, and stale mirrors.
   `<codename>-updates`
   decides: a mirror is stale if its `Date` is older than the reference by more than `MAX_LAG` (24
   hours). `<codename>-security` lag is shown for information only: `security.ubuntu.com` is
   never rewritten (`apply.md`, the refusal list), so its lag decides nothing.
   A missing `-updates` pocket or unparsable `Date` rejects.
   A `Date:` is trusted only when it is inside the cleartext-signed region: nothing but
   whitespace-only lines may precede `-----BEGIN PGP SIGNED MESSAGE-----`, which MUST itself start
   its line (`gpgv` refuses a space-prefixed armour header), the armour headers that follow it are
   skipped up to the first blank line, and the search stops at `-----BEGIN PGP SIGNATURE-----`.
   A body with any other content before the armour header, or with its only `Date:` after the
   signature header, still has a good signature: `gpgv` accepts such a prefix, so the status stays
   `verified` and is displayed as such.
   What such a body cannot supply is a trusted `Date`: a `Date` read outside the region is never a
   verified date, never a write baseline and never authorizes a write: a write takes only a date
   parsed inside the region, so the parser, not the verifier, separates a signed `Date` from a
   displayed one.
   Read-only ranking keeps its tolerance: a `Date` read outside the signed region is still used for
   lag, ranking and the table, so a mirror that serves one is ranked, not rejected.
   An `InRelease` larger than the 512 KiB download/verification cap is rejected at download: the
   verifier would refuse it anyway.
   That oversized rejection applies in read-only mode too: the promise that read-only ranking still
   works covers a missing or unusable `gpgv`, not a mirror that sends more than the cap. `Date` is
   the signing time, not the sync time.
   If the reference host is unreachable (network error, 5xx), use the newest `Date` among candidates
   as baseline and print a warning; a `Date` more than `MAX_LAG` ahead of the median is treated as
   bogus and ignored, so one wrong clock cannot make every other mirror stale.
   Only the `<codename>-updates` reference decides whether the release left the archive; an
   unreadable `-security` reference is ignored (it is informational).
   If the reference answers 404 for the codename, the release is no longer on `archive.ubuntu.com`
   (it moved to `old-releases.ubuntu.com`, which has no mirrors): exit 4 with a message that names
   the codename and explains this; never use the newest-candidate baseline in that case.
   `MAX_LAG` is 24 hours because of a measurement, not a preference: it is recorded with its
   date in [ADR-0005](../decisions/0005-mirror-list-and-freshness-measurements.md), and the comparison
   against the reference stays exact (decision 0003).
   Freshness also guards against replay of old indexes; apt still verifies signatures.
9. `PIPE-9` The run MUST rank by comparing the `-updates` `Date` exactly (no tolerance); lag is the
   reference `Date` minus
   the mirror `Date`, clamped to 0: the reference is fetched first, and a mirror may be newer.
   Primary key: lag ascending (mirrors with equal `Date` form one group).
   Within a group: throughput descending, then latency ascending; a mirror in the same AS as the
   server wins when its throughput is within 20% of the group's fastest mirror; the band is
   computed against that top speed and is not chained. The 20% is a chosen tolerance, not a
   measurement: it decides only which mirrors count as equally fast, never what counts as fresh.
   A lagging but not stale mirror stays selectable and shows a warning.
   Accepted trade-off: a slow mirror with the exact reference `Date` ranks above a much faster one
   that is one signing generation (about 6 hours) behind. `--verbose` MUST show the actual mirror AS
   (or `unknown`) and same-AS status (`same`, `diff`, or `unknown`) so a reader can understand and
   audit the AS tie-break; the default report shows neither (Output), and the output MUST NOT imply AS
   preference when the lookup failed.
10. `PIPE-10` The run MUST build the plan described in Apply, validate it against the closed target
    enumeration and the current on-disk digests, and only then write.
    A run that may not write stops after the recommendation.
    No step of this is user-visible or separately invocable.
