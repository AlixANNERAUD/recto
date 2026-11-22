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
| info | `signed in with a password` | `user` |
| info | `user created at first sign-in` | `user` |
| info | `local user created`, `administrator created` | `user` |
| info | `password changed` | `user` |
| info | `password set`, `disabled changed`, `administration changed` | `by`, `user_id`, and `value` for the last two |
| info | `session ended: the user is gone, disabled or signed out` | `user_id` |
| info | `document sent to the printer` | `user`, `job` |
| info | `document sent to the printer over a soft quota` | `user`, `job`, `rules` (the rule ids, comma-separated) |
| info | `print refused by a quota` | `user`, `rule` |
| info | `job canceled` | `user`, `job` |
| info | `device access changed`, `device access forgotten` | `by`, `device` |
| info | `printer added` | `by`, `printer`, `address` |
| info | `printer prices set` | `by`, `printer`, `mono_page`, `color_page`, `sheet` (minor units) |
| info | `quota rule created`, `quota rule changed`, `quota rule enabled changed`, `quota rule deleted` | `by`, `rule` (its id), and `enabled` for the third |
| info | `quota granted` | `by`, `rule`, `user` or `group`, `amount` (minor units for cost) |
| info | `quota reset` | `by`, `rule`, `user` or `group` |
| info | `printer described`, `default printer set`, `printer paused`, `printer resumed`, `printer accepting jobs`, `printer rejecting jobs`, `printer removed` | `by`, `printer` |
| info | `the CUPS socket does not exist yet`: harmless while CUPS is still starting, since recto connects at each request | `socket` |
| info | `scan saved` | `user`, `path` |
| info | `shutting down` | |
| warn | `listing the scanners failed` | `error` |
| warn | `the printing service failed`, `the printing service refused` | `error` |
| warn | `scanning failed` | `error`, `user` |
| warn | `saving a scan failed` | `error` |
| warn | `marking the scan saved failed` | `error` |
| warn | `sign-in refused`, `starting a sign-in failed` | `error` |
| warn | `password sign-in refused` | `client`, `refusal` |
| warn | `disabled user refused` | `subject` |
| warn | `reading the user information failed` | `error` |
| warn | `SANE could not start; scanning is off`, `SANE crashed while starting; scanning is off` | `error` |
| warn | `the scan results directory is not usable` | `error`, `dir` |
| warn | `printer administration is off` | `error` |
| warn | `reading a job state failed` | `error`, `job` |
| warn | `refunded ledger entries CUPS never received` | `count` |
| warn | `the printing service did not confirm the job`: CUPS may have queued it, so its ledger entry stays counted | `user`, `entry`, `reason` |
| warn | `listing the devices for the administration page failed` | `error` |
| warn | `printer administration failed` | `error`, and `by` and `printer` for a printer form |
| warn | `listing the printers for the quotas page failed`: the page offers the printers rules already name | `error` |
| error | `starting the session failed` | `error` |
| error | `password sign-in failed`, `changing a password failed`, `changing a user failed` | `error` |
| error | `writing the ledger entry failed` | `error` |
| error | `recording the scan failed` | `error`, `user` |
| error | `reading the quota rules failed`, `reading the groups of the user failed`, `checking the print quotas failed`: the job is refused | `error` |
| error | `reading the print quotas failed`: the Print page hides what the user has left | `error` |
| error | `loading the quota rules failed`, `creating a quota rule failed`, `changing a quota rule failed`, `deleting a quota rule failed` | `error` |
| error | `adjusting a quota failed`, `granting extra failed`, `resetting a quota failed` | `error` |
| error | `reading the quota usage failed`: the Users page shows the users without their usage | `error` |
| error | `reading the printer prices failed`, `loading the printer prices failed`, `saving the printer prices failed`, `forgetting the printer prices failed`: with a quota rule enabled, the first refuses the job | `error` |
| error | `refunding the ledger entry failed` | `error` |
| error | `linking the CUPS job to its ledger entry failed` | `error`, `job` |
| error | `reading the job records failed` | `error`, and `job` when canceling |
| error | `recording a job state failed` | `error`, `job` |
| error | `refunding the unsent ledger entries failed`, `reading the unfinished ledger entries failed` | `error` |
| error | `reading the printer access rules failed`, `reading the scanner access rules failed` | `error` |
| error | `saving the device access failed`, `forgetting the device access failed`, `loading the device rules failed` | `error` |
| error | `listing the users failed`, `listing the groups failed` | `error` |

Commands such as `recto create-administrator` write their messages as text.

Tokens, cookies, passwords and file contents are never logged. The password of a PostgreSQL URL
is hidden in messages.

## Health

`GET /health` answers `200` with the body `ok` while the server runs. It does not check CUPS,
SANE or the identity provider, which recto reaches only when someone uses them, so a printer or
scanner being off never marks the container unhealthy.

`recto healthcheck` calls it; the image uses that as its `HEALTHCHECK`, every 30 seconds with a
6-second timeout, after a 20-second start period, failing after 3 misses.
