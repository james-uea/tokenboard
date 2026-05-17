# Security Policy

## Supported Versions

Security fixes target the latest public release of Tokenboard.

## Reporting a Vulnerability

Please do not open a public issue for suspected vulnerabilities. Report security
issues privately to the repository owner through GitHub's private vulnerability
reporting flow when available, or by contacting the maintainer directly.

Include enough detail to reproduce the issue, affected versions, and any known
workarounds. Tokenboard deployments should keep `ALLOW_LEGACY_API_KEY=false`
for public or team instances and should rotate any exposed API tokens or OAuth
secrets immediately.
