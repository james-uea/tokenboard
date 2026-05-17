# Changelog

All notable changes to Tokenboard will be documented in this file.

## 1.0.6 - 2026-05-18

- Fix installer version reporting when installing outside the current `PATH`.
- Gate GitHub release publishing on server tests, frontend build, and client
  tests.
- Document the public GitHub contribution data proxy used by profile pages.
- Remove Rust test-build warning noise from pricing-loader internals.

## 1.0.5 - 2026-05-17

- Harden avatar SVG fallback escaping for public deployments.
- Align CLI and server release metadata for GitHub release publishing.
- Refresh server development dependency hygiene.

## 1.0.3 - 2026-05-15

- First stable public release.
- Includes the Rust `tokenboard` CLI, Express/PostgreSQL server, Vue 3 web UI,
  GitHub OAuth setup flow, autosync, and GitHub Releases based CLI updates.
