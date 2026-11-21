# CLI

Single command, no subcommands: `apt-scout [flags]`

No flag is required.
The uid decides whether the run writes: it discovers, ranks and recommends, and a non-root run
prints the one line that repeats the run under `sudo` and exits 3, while the same command under
`sudo` writes the best mirror.
Every flag below is an override, never a step in the workflow.

A short form exists for every flag that is implemented today: `-a`, `-c`, `-n`, `-s`, `-t`, `-v`,
`-l`, `-y`, `-r`, `-h` and `-V`. `--no-color` is the one implemented flag without a short form. `-R`
stays reserved for `--refresh` and is rejected until that flag exists.
Short flags are never bundled: `-lyc NO` is an unknown option.
The line a non-root run prints always spells the long forms, so a copied command is unambiguous.

| ID | Flag | Meaning |
| --- | --- | --- |
| `CLI-1` | `-c`, `--country <CC>` | ISO 3166 alpha-2 override. Always wins over detection. |
| `CLI-2` | `-a`, `--any` | Ignore the detected and the given country: consider the mirrors the list names for **every** country and rank them together. The country is then display-only. `--any` with `--country` is not an error — the country simply stops constraining the candidate set. Needs no country at all, so it also satisfies `--no-geo`. |
| `CLI-3` (planned) | `--refresh` | Not implemented: a run given it fails as an unknown option (exit 2), as a run given the `-R` reserved for it does. Its meaning when it exists: ignore the geo, mirror list and AS caches for this run and rewrite them; v1 keeps no cache (Errors). |
| `CLI-4` | `-n`, `--no-geo` | No geolocation lookups at all. Requires `--country`, or `--any`, which needs no country; AS preference is skipped. Offline-friendly for the server lookup. |
| `CLI-5` | `-t`, `--top <N>` | Show N best mirrors (default 5). |
| `CLI-6` | `-s`, `--https-only` | Only consider https mirrors. |
| `CLI-7` (planned) | `--tui` | Not implemented: a run given it fails as an unknown option (exit 2), so no TUI mode exists, and the table below carries one row for it saying the same thing. Its meaning when it exists: interactive ranked table, selection, detail panel and confirmation, opt-in, with the default run not interactive. The TUI section is that milestone's design. |
| `CLI-8` | `-l`, `--list` | Table and planned change only; never writes, whatever the uid. |
| `CLI-9` | `-y`, `--yes` | Skip the one confirmation before a write in a root run. |
| `CLI-10` | `-r`, `--restore` | Restore the newest backup instead of applying a mirror. Root is needed for the read of the backup and for the write. |
| `CLI-11` | `-v`, `--verbose` | The proof behind the default report (Output): each row's AS number and same/`diff`/`unknown` match, the server's AS, each row's country and the address the run resolved, both pocket dates in full, the `-security` lag, the exact signature status, a redirect host in full, and the full text of every rejection reason. Display only; it changes no decision. |
| `CLI-12` | `--no-color` | No colour in the report, whatever the terminal is. Display only; Output states the rules. No short form. |
| `CLI-13` | `-V`, `--version` | Print the one line `apt-scout <version>` and exit 0. The version is the crate's own, so it cannot drift from `Cargo.toml`. |

`--version` and `--help` are answered while the arguments are read, before distro detection and
before any file is opened or any request made, so both work whatever the uid, on any distribution
and with no network.
Whichever of the two a run meets first answers it and exits 0; a usage error before them is still
exit 2.

Command, uid, and whether apt sources change:

| ID | Command | uid | Writes apt sources |
| --- | --- | --- | --- |
| `CLI-14` | `apt-scout` | non-root | No: discovery, ranking, the recommendation with its reason, the one line `sudo apt-scout`, exit 3. |
| `CLI-15` | `sudo apt-scout` | root | Yes: the same discovery and recommendation, one confirmation (`--yes` skips it), the write. |
| `CLI-16` | `apt-scout --list` | any | No: the ranked table and the change a write would make, exit 0. |
| `CLI-17` (planned) | `apt-scout --tui` | any | Not implemented: a usage error (exit 2), exactly as `--refresh` and `-R` are, so a `--tui` run has no write, no backup and no `sudo apt-scout --tui` line to report. The TUI section is the design that milestone delivers. |
| `CLI-18` | `apt-scout --restore` | non-root | No: the one line `sudo apt-scout --restore`, exit 3. |
| `CLI-19` | `sudo apt-scout --restore` | root | Yes: the newest backup, after one confirmation (`--yes` skips it). |

Behavior by mode:
- `CLI-20` Every run of the mirror pipeline prints the ranked table, then the recommendation and its
  reason: host, the mirror's own country, how far `behind` it is, speed, `latency` and signature
  status.
  The country in that line is the selected mirror's, not the run's, because the two differ once a
  run is not narrowed to its own country.
  The chosen mirror's AS, and whether it matches the server's, are `--verbose`'s and the default
  recommendation does not name them (Output).
  Rejected mirrors are listed with the reason they were rejected.
  A run whose candidate set was widened (`pipeline.md` step 4) prints one `note:` line under the report
  header, before the table, saying why; a run given `--any` says in the header's country field that
  the country does not constrain it.
  Neither statement is opt-in and neither can be turned off.
- `CLI-21` A write confirms once: the files it will change, the old URI and the new one, and the backup
  path it will create.
  It does not repeat what `Sources` already reported as skipped. `--yes` skips that prompt.
  The prompt is a safety rail, not a security control: the guarantee is the validation described in
  Apply, and no answer to the prompt can bypass it.
- `CLI-22` A non-root run without `--list` prints the table, the recommendation and one line, then exits 3:
  `sudo apt-scout` with the flags of that run repeated.
  It opens no apt file for writing.
- `CLI-23` (planned) `--tui` is opt-in and is never chosen for the user (not implemented yet: a run given it exits 2).
  The default run is non-interactive, so no flag turns the TUI off.
- `CLI-24` `--restore` replaces the source of the rewrite: the newest backup instead of the mirror pipeline.
  Root is needed for that read as well as for the write, so the non-root run prints only
  `sudo apt-scout --restore` and exits 3 before it opens the backup.
  Its confirmation names the backup it reads, the files it writes and the backup it creates, instead
  of the old and new URI an apply's confirmation shows, because a restore's plan carries bytes and
  no URI pair. `--list` with `--restore` is a usage error (exit 2), refused before the backup is
  opened: a non-root run could not read the root-only manifest to show anything, and a root run's
  own confirmation names the same files and backups.
  A restore runs nothing of the mirror pipeline, so it is refused in the same way with `--country`,
  `--any`, `--top`, `--https-only` and `--no-geo`, each a usage error (exit 2) naming the offending
  flag; `--yes` is the unattended form, and `--verbose` and `--no-color` are display only.
  The block a finished restore prints is its own, under a `Restore` heading rather than the mirror
  report's sections: the backup it put back, each file it wrote and each file it removed, and the
  backup it created for what it replaced, and `--verbose` adds each file's digest, mode and owner.
  A restore the confirmation cancelled (nothing was written) exits 0: that answer is the user's
  decision, not a failure, and only an apply refuses a no, because there the write is the point of
  the run.
  The block names the backup that was put back, taken from the plan the confirmation printed,
  because the plan a run prints is the plan it writes: no step re-derives it after the answer.
- `CLI-25` No step of the write is a mode the user types.
  Building the plan, validating it against the closed target enumeration and the current on-disk
  digests, verifying the mirror's `InRelease`, and writing are one code path inside the program (see
  Apply); the plan never crosses a process boundary and is never a file.
