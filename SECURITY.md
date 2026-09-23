# Security policy

## Report a vulnerability privately

Please do not publish a suspected vulnerability, exploit, credentials, or sensitive scan data in a public issue or pull request.

Use this repository's **Security → Advisories → Report a vulnerability** option when private vulnerability reporting is available. If that option is unavailable, contact [Smet Software Solutions](https://smetsoftwaresolutions.be) through its website to arrange a private reporting channel before sending sensitive details.

Include the affected version or commit, operating system, source type (local, network share, or SSH), a description of the impact, and minimal reproduction steps. A synthetic fixture is preferable to a real scan report. Remove private paths, hostnames, usernames, and other confidential information; never send private keys or passwords.

Security fixes are developed against the current codebase. This project does not currently maintain a separate long-term support branch for older releases. Reports about older versions are still useful; state the exact version and whether the issue also occurs on the latest available version.

## Security boundaries

SpaceTrace scans filesystem metadata using the permissions of the local user or selected SSH account. Remote scanning runs bundled metadata helpers on the selected device over SSH. Verify a new host's fingerprint through a trusted channel before accepting it.

Local scan history and exported reports contain filesystem paths and scan metadata. Treat them as private data and review them before sharing. See the [README](README.md) for scan limitations and [the report format](docs/report-format.md) for exported fields.
