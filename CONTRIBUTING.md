# Contributing

Thanks for taking the time to improve Tokenboard.

## Development Setup

Use the local development flow in `README.md` to install Node, frontend, Rust,
and PostgreSQL dependencies. Keep server, frontend, and client changes focused
unless the feature crosses those boundaries.

## Checks

Run the relevant checks before opening a pull request:

```bash
npm --prefix server test
npm --prefix server run frontend:build
cd client && cargo test
```

For scanner changes, include representative fixture coverage where possible and
preserve existing aggregation, model normalization, and cache-token semantics.

## Pull Requests

- Explain the user-visible behavior change.
- Mention any configuration or deployment impact.
- Include screenshots for visible frontend changes.
- Do not commit local secrets, generated build outputs, or private workspace
  notes.
