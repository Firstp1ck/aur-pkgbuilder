# Security Policy

## Supported Versions

aur-pkgbuilder is pre-1.0 software. Security fixes are applied to the
current minor release line only.


| Version | Supported          |
| ------- | ------------------ |
| 0.1.x   | :white_check_mark: |
| < 0.1.0 | :x:                |


## Reporting a Vulnerability

If you believe you've found a security issue in aur-pkgbuilder, please
report it responsibly.

- Preferred: Email **[firstpick1992@proton.me](mailto:firstpick1992@proton.me)** with the subject
`[aur-pkgbuilder Security]`.
- Alternative: If email isn't possible, open a GitHub issue with minimal
detail and the word "Security" in the title. We'll triage and, if
appropriate, coordinate privately.

Please include, when possible:

- aur-pkgbuilder version (commit hash or release tag) and install
method (cargo build, AUR package if/when published).
- Arch-based distribution and version.
- Relevant external tool versions (`makepkg`, `ssh`, `git`,
`shellcheck`, `namcap`).
- Reproduction steps and expected vs. actual behavior.
- Impact assessment and a proof-of-concept if available.
- Any relevant logs or screenshots (redacted — see **What not to send**
below).

What to expect:

- Acknowledgement within 3 business days.
- Status updates at least weekly until resolution.
- Coordinated disclosure: we'll work with you on timing and credit, or
anonymity if you prefer.

## Threat model

aur-pkgbuilder interacts with the user's environment in several
sensitive ways. Vulnerabilities in any of the following areas are
in-scope:

- **Shell injection** through package names, commit messages, PKGBUILD
URLs, or any other string fed to an external command. The codebase
exclusively uses `Command::new().arg()` — reports of any path that
hands unsanitized input to `sh -c` are taken seriously.
- **Filesystem safety**: symlink-follow races on writes under
`~/.config/aur-pkgbuilder/`, `~/.ssh/`, or the build working
directory; permission regressions on `~/.ssh/aur` (must stay `0600`),
`~/.ssh` (must stay `0700`), or `~/.ssh/config` (must stay `0600`).
- **SSH trust**: anything that silently trusts an AUR host key without
surfacing the fingerprint for the user to verify.
- **Credential leakage**: private key contents, session output
containing passphrases, or registered usernames written into toasts,
log files, or third-party services.
- **Network**: any code path that disables TLS verification, follows
redirects to a non-HTTPS destination, or fetches PKGBUILDs from a
URL it did not validate.
- **Privilege escalation**: any path that bypasses the `nix_is_root()`
guard and lets `makepkg` run as root.
- **Registry tampering**: if `packages.jsonc` parsing is ever exploitable
(malformed JSON leading to panics / OOM / RCE via deserialization of
attacker-controlled data, etc.).

## Out of scope

- Issues in external tools (`makepkg`, `git`, `openssh`, `pacman-contrib`,
`shellcheck`, `namcap`, `fakeroot`) — report those upstream.
- Vulnerabilities in third-party AUR helpers.
- Issues in the AUR server itself (`aur@aur.archlinux.org`) — report
those to the [aurweb](https://gitlab.archlinux.org/archlinux/aurweb)
maintainers.
- Non-security bugs — please use regular GitHub issues.

## What not to send

When reporting, please **do not** attach:

- The private halves of SSH keys.
- Credentials for your AUR account (which are kept server-side anyway —
the app only uses SSH auth and the public RPC).
- Full host-key fingerprints from other people's systems.

Thank you for helping keep aur-pkgbuilder and its users safe.