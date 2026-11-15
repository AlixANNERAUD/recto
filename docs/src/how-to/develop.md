# Develop and contribute

recto is written in Rust, with [axum](https://github.com/tokio-rs/axum) for the server and
[Leptos](https://leptos.dev) for the interface. Every tool is pinned and run through
[mise](https://mise.jdx.dev).

## Set up

You need mise, plus libsane and libclang for the libsane bindings. On Debian or Ubuntu:

```sh
sudo apt install libsane-dev libclang-dev pkg-config
git clone https://github.com/AlixANNERAUD/recto.git
cd recto
mise install
```

## Run it

```sh
mise run dev
```

It serves <http://127.0.0.1:3000> and reloads on every change. Every request is signed in as a
development user, and the printers and scanners are fakes: three printers in various states, a
few print jobs, and a scanner that scans a sample letter. This mode exists in debug builds only;
never expose that server.

If the build complains about the `wasm-bindgen` version, an older `wasm-bindgen` in
`~/.cargo/bin` is probably shadowing the one mise pins: remove it.

## Check your changes

```sh
mise run format      # format the code
mise run check       # what CI runs: rustfmt, clippy, rustdoc, tests, cargo-deny
mise run test-sane   # scanner tests against SANE's virtual test backend
mise run docs        # build this book into docs/book
```

Tests never touch a real printer, scanner or identity provider: the CUPS client and sign-in are
tested against fake servers built with `wiremock`, and scanning against a mock and SANE's `test`
backend.

## Where things are

| Crate | Role |
| --- | --- |
| `crates/printing` | Print models and the CUPS/IPP client |
| `crates/scanning` | Scan models and the SANE client |
| `crates/authentication` | OpenID Connect sign-in |
| `crates/app` | The interface, rendered on the server, with islands for the interactive parts |
| `crates/frontend` | The WebAssembly entry point of the interface |
| `crates/server` | The `recto` binary: configuration, routes, serving |

`AGENTS.md` holds the rules the code follows. Every interface string has a key in
`crates/app/locales/en.json`, which is also where translations start.

## Send a change

Open a pull request against `main`. Commit messages follow
[Conventional Commits](https://www.conventionalcommits.org): `feat:`, `fix:`, `docs:` and so on.

## Release

Pushing a tag `vX.Y.Z` builds the image for amd64 and arm64 and publishes it to
`ghcr.io/alixanneraud/recto`.
