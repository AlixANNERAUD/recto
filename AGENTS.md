# AGENTS.md

recto is a self-hosted web app to print (CUPS/IPP) and scan (SANE) behind OpenID Connect, written
in Rust.

## Layout

| Crate | Role |
| --- | --- |
| `crates/printing` | Print models and the CUPS/IPP client |
| `crates/scanning` | Scan models and the SANE client |
| `crates/authentication` | OpenID Connect sign-in and sessions |
| `crates/app` | Leptos interface (server-rendered, islands), English |
| `crates/frontend` | WebAssembly entry point of the interface |
| `crates/server` | The `recto` binary and library: configuration, routes, serving |

## Commands

Everything goes through [mise](https://mise.jdx.dev), which pins the toolchain.

| Command | Does |
| --- | --- |
| `mise install` | Installs the pinned tools |
| `mise run format` | Formats the code |
| `mise run lint` | rustfmt check, clippy with `-D warnings` for the server and the WebAssembly frontend |
| `mise run test` | The test suite (nextest) |
| `mise run doc` | Rustdoc with warnings as errors |
| `mise run check` | What CI runs: lint, rustdoc, tests, cargo-deny |
| `mise run dev` | Development server on http://127.0.0.1:3000 with mocked printers and scanners and a signed-in dev user (debug builds only) |
| `mise run build` | Release binary and site |

Builds are heavy: run one cargo command at a time.

## Rules

- Every interface string goes through `t!`/`tf!`/`tp!` with a key in `crates/app/locales/en.json`.
- Print and cancel work without JavaScript (plain forms). Only the scan page and live refresh are
  islands.
- Tests never touch a real printer, scanner or identity provider: use mocks and `wiremock`.
- Comments explain why, never restate the code. Docs are plain, concise English with no em dashes.
- Work and push directly on `main`; there are no pull requests. Commits follow Conventional
  Commits (`feat:`, `fix:`, `test:`, `build:`, `ci:`, `docs:`), imperative, with a body. Do not
  mention AI tools and add no `Co-Authored-By` or "Generated with" lines.
