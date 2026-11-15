# Logs and health

## Logs

recto writes its log to standard error, one line per event. `docker compose logs recto` shows it.

| `RECTO_LOG_FORMAT` | Lines |
| --- | --- |
| `json` (default) | One JSON object per line, with `timestamp`, `level`, `target`, `message` and the event's fields. |
| `text` | Human-readable lines. |

`RUST_LOG` filters the lines, with the syntax of
[tracing's `EnvFilter`](https://docs.rs/tracing-subscriber/latest/tracing_subscriber/filter/struct.EnvFilter.html).
The default is `info`. `RUST_LOG=debug` adds detail; `RUST_LOG=info,recto=debug` adds it for recto
only.

### Events

| Level | Message | Fields |
| --- | --- | --- |
| info | `listening` | `address` |
| info | `scanners listed` | `count` |
| info | `signed in` | `user` |
| info | `document sent to the printer` | `user`, `job` |
| info | `job canceled` | `user`, `job` |
| info | `scan saved` | `user`, `path` |
| info | `shutting down` | |
| warn | `listing the scanners failed` | `error` |
| warn | `the printing service failed`, `the printing service refused` | `error` |
| warn | `scanning failed` | `error`, `user` |
| warn | `saving a scan failed` | `error` |
| warn | `sign-in refused`, `starting a sign-in failed` | `error` |
| warn | `reading the user information failed` | `error` |
| warn | `SANE could not start; scanning is off` | `error` |
| error | `starting the session failed` | `error` |

Tokens, cookies and file contents are never logged.

## Health

`GET /health` answers `200` with the body `ok` while the server runs. It does not check CUPS,
SANE or the identity provider, which recto reaches only when someone uses them, so a printer or
scanner being off never marks the container unhealthy.

`recto healthcheck` calls it; the image uses that as its `HEALTHCHECK`, every 30 seconds with a
6-second timeout, after a 20-second start period, failing after 3 misses.
