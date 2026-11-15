#!/usr/bin/env bash
# Contract check for live data sources apt-scout depends on.
# Verifies each source still answers and still has the fields the parser needs.
# Usage: check-sources.sh   (exit 0 = all ok, 1 = at least one source changed or is down)
set -uo pipefail

fail=0
ok()  { printf 'OK   %s\n' "$1"; }
bad() { printf 'FAIL %s: %s\n' "$1" "$2"; fail=1; }
get() { curl -fsSL --max-time 20 --retry 2 --retry-delay 5 --retry-all-errors "$@"; }
skip() { printf 'SKIP %s: %s\n' "$1" "$2"; }
# HTTP status of a URL ("000" when the request itself failed).
code() { curl -s -o /dev/null -w '%{http_code}' --max-time 20 "$1"; }
# Public geo services may block or rate limit shared CI runner addresses (403/429): that is
# not a contract break, so it is a SKIP. A 200 with a wrong body is a FAIL.
geo() { # <name> <url> <extract-command>
  local name=$1 url=$2 status cc
  status=$(code "$url")
  case "$status" in
    403|429) skip "$name" "HTTP $status (blocked or rate limited)"; return ;;
    200) ;;
    *) bad "$name" "HTTP $status"; return ;;
  esac
  cc=$(get "$url" | eval "$3" | tr -d '[:space:]')
  [[ "$cc" =~ ^[A-Z]{2}$ ]] && ok "$name ($cc)" || bad "$name" "unexpected answer '$cc'"
}

# Launchpad mirror list: country head rows, mirror rows, http(s) links, status spans.
if page=$(get https://launchpad.net/ubuntu/+archivemirrors); then
  heads=$(grep -c '<th colspan="2">' <<<"$page")
  rows=$(grep -c 'href="/ubuntu/+mirror/' <<<"$page")
  https=$(grep -c '>https</a>' <<<"$page")
  status=$(grep -c 'distromirrorstatus' <<<"$page")
  if [ "$heads" -ge 50 ] && [ "$rows" -ge 300 ] && [ "$https" -ge 100 ] && [ "$status" -ge 300 ]; then
    ok "launchpad (countries=$heads mirrors=$rows https=$https status=$status)"
  else
    bad launchpad "structure changed (countries=$heads mirrors=$rows https=$https status=$status)"
  fi
else bad launchpad "request failed"; fi

# mirrors.ubuntu.com country list: plain URLs.
if list=$(get http://mirrors.ubuntu.com/NO.txt) && grep -Eq '^https?://' <<<"$list"; then
  ok "mirrors.ubuntu.com/NO.txt"
else bad mirrors.ubuntu.com "no URLs in NO.txt"; fi

# Reference archive: -updates InRelease must carry a Date header.
if d=$(get http://archive.ubuntu.com/ubuntu/dists/noble-updates/InRelease | sed -n 's/^Date: //p' | head -1) && [ -n "$d" ]; then
  ok "archive.ubuntu.com InRelease Date ($d)"
else bad archive.ubuntu.com "no Date in noble-updates InRelease"; fi

# Geo sources: country must be two letters.
cc=$(get http://geoip.ubuntu.com/lookup | sed -n 's:.*<CountryCode>\([A-Z][A-Z]\)</CountryCode>.*:\1:p')
[ -n "$cc" ] && ok "geoip.ubuntu.com ($cc)" || bad geoip.ubuntu.com "no CountryCode"
geo ifconfig.co https://ifconfig.co/country-iso cat
geo ipinfo.io https://ipinfo.io/country cat
geo "cloudflare trace" https://www.cloudflare.com/cdn-cgi/trace "sed -n 's/^loc=//p'"

# AS sources: Team Cymru DNS and RIPEstat for a known address.
if command -v dig >/dev/null; then
  as=$(dig +short TXT 8.8.8.8.origin.asn.cymru.com 2>/dev/null | tr -d '"' | cut -d' ' -f1)
  [ "$as" = "15169" ] && ok "team cymru DNS (AS$as)" || bad "team cymru" "unexpected answer '$as'"
else
  echo "SKIP team cymru (dig not installed)"
fi
asn=$(get "https://stat.ripe.net/data/prefix-overview/data.json?resource=8.8.8.8" | python3 -c 'import json,sys;print(json.load(sys.stdin)["data"]["asns"][0]["asn"])' 2>/dev/null)
[ "$asn" = "15169" ] && ok "ripestat (AS$asn)" || bad ripestat "unexpected answer '$asn'"

exit $fail
