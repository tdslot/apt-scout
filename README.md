# apt-scout

Standalone Rust CLI for finding fresh, fast Ubuntu archive mirrors. Current release is read-only: it never changes apt sources.

## Usage

```sh
apt-scout --country LT
apt-scout --country LT --no-geo --top 5 --https-only
```

Without `--country`, apt-scout attempts optional country detection. If every source fails, it asks for `--country`. `--no-geo` disables country and AS lookups and requires an explicit country. `--list` prints ranked mirrors; `--top N` limits results.

## Network and privacy

Country detection contacts Canonical geoip, ifconfig.co, ipinfo.io, and Cloudflare trace in order until country, public IP, and AS are found. Team Cymru DNS provides AS lookups for the server and mirror hosts. These services or the configured DNS resolver can observe the server's public IP; DNS queries also include mirror IPs during AS ranking. Use `--no-geo --country CC` to skip geolocation and AS queries.

See [SPEC.md](SPEC.md) for implemented behavior and planned milestones. See [AGENTS.md](AGENTS.md) for contributor checks.

## License

MIT OR Apache-2.0
