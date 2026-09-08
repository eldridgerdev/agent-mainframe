# Dependency advisory follow-up

Local cargo-audit 0.22.2 scan on 2026-09-08, against the current RustSec database
(1,242 advisories, 265 locked dependencies). The initial scan failed on four
rustls-webpki 0.103.9 advisories:

- [RUSTSEC-2026-0049](https://rustsec.org/advisories/RUSTSEC-2026-0049): CRL distribution-point matching.
- [RUSTSEC-2026-0098](https://rustsec.org/advisories/RUSTSEC-2026-0098): URI name constraints.
- [RUSTSEC-2026-0099](https://rustsec.org/advisories/RUSTSEC-2026-0099): wildcard name constraints.
- [RUSTSEC-2026-0104](https://rustsec.org/advisories/RUSTSEC-2026-0104): panic while parsing CRLs.

The targeted lockfile update to 0.103.13 clears all four. The dependency is
reachable through rustls/rustls-platform-verifier and ureq. No other package
version or manifest requirement changed. The second audit exited successfully
with zero blocking vulnerabilities and the five default nonfatal warnings below.
No advisory is ignored or suppressed by configuration.

| Locked dependency | Advisory / warning | Dependency path and follow-up |
| --- | --- | --- |
| paste 1.0.15 | [RUSTSEC-2024-0436](https://rustsec.org/advisories/RUSTSEC-2024-0436), unmaintained | ratatui; evaluate the upstream replacement during a focused UI dependency update |
| anyhow 1.0.102 | [RUSTSEC-2026-0190](https://rustsec.org/advisories/RUSTSEC-2026-0190), unsound downcast_mut | direct dependency; assess affected API usage and patched release separately |
| lru 0.12.5 | [RUSTSEC-2026-0253](https://rustsec.org/advisories/RUSTSEC-2026-0253), unsound pop panic handling | ratatui; assess cache usage and upstream patch path |
| lru 0.12.5 | [RUSTSEC-2026-0002](https://rustsec.org/advisories/RUSTSEC-2026-0002), unsound IterMut | ratatui; evaluate together with the other lru finding |
| rand 0.8.5 | [RUSTSEC-2026-0097](https://rustsec.org/advisories/RUSTSEC-2026-0097), unsound custom logger interaction | throbber-widgets-tui; assess exposure and compatible upstream update |

The repository maintainer owns these follow-ups. Exposure has not been ruled
out; a successful audit exit does not resolve nonfatal warnings. Keep them
visible on PR and scheduled runs, revisit on the next dependency update, and
track separate fixes rather than bundling unrelated upgrades into this refactor.
See [checks.md](checks.md) for audit commands and exception policy. Hosted CI
validation is still pending publication of this branch.
