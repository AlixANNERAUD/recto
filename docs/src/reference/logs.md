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
| info | `LAN listener`: the LAN listener's address, and its certificate's SHA-256 fingerprint, as `openssl x509 -fingerprint -sha256` writes it | `address`, `fingerprint` |
| info | `DNS-SD responder` | `host` (the name announced), `interfaces` |
| info | `announced over DNS-SD` | `services` (their full names) |
| info | `scanners listed` | `count` |
| info | `signed in` | `user` |
| info | `signed in with a password` | `user` |
| info | `user created at first sign-in` | `user` |
| info | `local user created`, `administrator created` | `user` |
| info | `password changed` | `user` |
| info | `password set`, `disabled changed`, `administration changed` | `by`, `user_id`, `device_passwords` (how many were revoked) for the first, and `value` for the last two |
| info | `device passwords revoked` | `by`, `user_id`, `count` |
| info | `device passwords revoked: no access`: single sign-on refused the user the required claim | `subject`, `count` |
| info | `session ended: the user is gone, disabled or signed out` | `user_id` |
| info | `document sent to the printer` | `user`, `job` |
| info | `document sent to the printer over a soft quota` | `user`, `job`, `rules` (the rule ids, comma-separated) |
| info | `print refused by a quota` | `user`, `rule` |
| info | `job canceled` | `user`, `job` |
| info | `device access changed`, `device access forgotten` | `by`, `device` |
| info | `device password created` | `user`, `device_password_id`, `device_name` |
| info | `no scan addresses: the scanners could not be listed`: the new device password is shown with its printer addresses only | `error` |
| info | `device password revoked` | `user`, `device_password_id` |
| info | `device profile downloaded`: the Apple profile of a device password | `user`, `device_password_id`, `device_name` |
| info | `printer added` | `by`, `printer`, `address` |
| info | `printer prices set` | `by`, `printer`, `mono_page`, `color_page`, `sheet` (minor units) |
| info | `quota rule created`, `quota rule changed`, `quota rule enabled changed`, `quota rule deleted` | `by`, `rule` (its id), and `enabled` for the third |
| info | `quota granted` | `by`, `rule`, `user` or `group`, `amount` (minor units for cost) |
| info | `quota reset` | `by`, `rule`, `user` or `group` |
| info | `printer described`, `default printer set`, `printer paused`, `printer resumed`, `printer accepting jobs`, `printer rejecting jobs`, `printer removed` | `by`, `printer` |
| info | `the CUPS socket does not exist yet`: harmless while CUPS is still starting, since recto connects at each request | `socket` |
| info | `scan saved` | `user`, `path` |
| info | `escl scan started`: a computer or phone started a scan over eSCL | `user` (`anonymous` without a device password), `device_password_id`, `peer`, `scanner` (the scanner's 8-digit key), `format` |
| info | `escl scan finished` | `user`, `scanner`, `pages`, `format` |
| info | `eSCL scan settings refused`: the app answered `400` or `409` | `error`, `user` |
| info | `usage exported` | `by` (who downloaded it), `export` (the file name), `from`, `to` |
| info | `job held`: a computer or phone sent a document, kept until its owner releases it | `user`, `device_password_id`, `job`, `pages` (absent when the PDF could not be counted), `format` |
| info | `held job canceled by its device`: before its document arrived | `user`, `device_password_id`, `job` |
| info | `held job released` | `user`, `held_job`, `job` (the CUPS job), `printer` |
| info | `held job refused by a quota` | `user`, `rule` |
| info | `held job deleted` | `user`, `held_job` |
| info | `held jobs expired`: documents sent from computers and phones that nobody released in time, deleted | `count` |
| info | `held jobs whose document never came were removed`: a device started a job and never sent its document | `count` |
| info | `orphan held files removed`: documents no held job names, left by a crash or by deleting their owner | `count` |
| info | `shutting down` | |
| warn | `listing the scanners failed` | `error` |
| warn | `RECTO_DNSSD found no interface to announce on` | |
| warn | `listing the scanners to announce failed`, `a scanner is not announced`, `the scanners did not answer in time and are not announced`: the printer is announced without them | `error`, `scanner` |
| warn | `withdrawing the DNS-SD services failed` | `error` |
| warn | `the LAN listener could not accept a connection`: out of file descriptors, most likely; it tries again after a second | `error` |
| warn | `the printing service failed`, `the printing service refused` | `error` |
| warn | `scanning failed` | `error`, `user` |
| warn | `eSCL scanning failed`, `escl scan failed` | `error`, `user`, `scanner` |
| warn | `listing the scanners for eSCL failed`: the app is answered `503` | `error` |
| warn | `recording the eSCL scan timed out` | `scanner` |
| warn | `saving a scan failed` | `error` |
| warn | `marking the scan saved failed` | `error` |
| warn | `sign-in refused`, `starting a sign-in failed` | `error` |
| warn | `password sign-in refused` | `client`, `refusal` |
| warn | `cross-site request refused` | `method`, `path`, with the secret of a personal device address shown as `<secret>` |
| warn | `disabled user refused` | `subject` |
| warn | `unknown device password`: counts toward the [device password limit](accounts.md#device-password-limit) | `peer`, `client`, `in_path` |
| warn | `device password of a disabled user` | `peer`, `in_path` |
| warn | `device password refused: too many failures` | `peer`, `client` |
| warn | `device credentials refused over plain HTTP` | `peer` |
| warn | `recording a device's last use failed`: the device is still let in | `error`, `device_password_id` |
| warn | `reading the user information failed` | `error` |
| warn | `SANE could not start; scanning is off`, `SANE crashed while starting; scanning is off` | `error` |
| warn | `the scan results directory is not usable` | `error`, `dir` |
| warn | `the held documents directory is not usable` | `error`, `dir` |
| warn | `held jobs whose release never finished are held again`: recto stopped during a release, before the job was counted | `count` |
| warn | `removing a held document failed`, `removing held documents failed`, `listing the held documents failed`: the next sweep tries again | `error`, and `file` or `dir` |
| warn | `removing a held document that was not recorded failed`: the orphan sweep removes it within the hour | |
| warn | `removing a released document failed`, `removing a deleted held document failed`: the sweep removes it | `job` |
| warn | `a held job's release lost its claim`: a release took longer than 15 minutes and another took over; nothing was printed or counted by this one | |
| warn | `printer administration is off` | `error` |
| warn | `reading a job state failed` | `error`, `job` |
| warn | `refunded ledger entries CUPS never received` | `count` |
| warn | `the printing service did not confirm the job`: CUPS may have queued it, so its ledger entry stays counted | `user`, `entry`, `reason` |
| warn | `listing the devices for the administration page failed` | `error` |
| warn | `printer administration failed` | `error`, and `by` and `printer` for a printer form |
| warn | `listing the printers for the quotas page failed`: the page offers the printers rules already name | `error` |
| error | `starting the session failed` | `error` |
| error | `announcing recto over DNS-SD failed`: recto runs on without announcing | `error` |
| error | `reading the scanner access rules to announce failed`: no scanner is announced | `error` |
| error | `creating a device password failed`, `revoking a device password failed`, `checking a device password failed`: the last answers the device, or the profile download, `503` | `error` |
| error | `password sign-in failed`, `changing a password failed`, `changing a user failed` | `error` |
| error | `writing the ledger entry failed` | `error` |
| error | `recording the scan failed` | `error`, `user` |
| error | `recording the eSCL scan failed` | `error`, `scanner` |
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
| error | `counting the device passwords failed`: the Users page shows the users without them | `error` |
| error | `revoking the device passwords failed`: at a sign-in refused the required claim | `error`, `subject` |
| error | `loading the usage failed`: the Usage page shows the error instead of the tables | `error` |
| error | `exporting the usage failed`: the download answers `503` | `error` |
| error | `an IPP request failed`: the device is answered `server-error-internal-error` | `error`, `user`, `device_password_id` |
| error | `writing a held document failed`: the device is answered `server-error-internal-error` | `error` |
| error | `listing the held jobs failed`: the Jobs page shows the error instead of the documents waiting | `error` |
| error | `reading a held job failed`, `claiming a held job failed`, `reading a held document failed`, `deleting a held job failed` | `error`, `job` |
| error | `writing the ledger entry of a held job failed`: the document is not printed and stays waiting | `error` |
| error | `returning a refused held job failed`, `returning a held job failed`: the sweep returns it within 15 minutes, unless it has a ledger entry | `error` |
| error | `sweeping the held jobs failed`, `reading the held documents' names failed`: the next sweep tries again | `error` |

Commands such as `recto create-administrator` write their messages as text.

Tokens, cookies, passwords, device passwords and file contents are never logged, nor the path of
a request to a personal address, which holds a device password: log lines name the device
password's id. A device
password refused again from memory is logged at `debug` only (`device password refused again`),
so a phone polling with a revoked password does not fill the log, and so are TLS handshakes on the
LAN listener that fail or time out (`LAN TLS handshake failed`, `LAN TLS handshake timed out`). The password of a PostgreSQL URL
is hidden in messages.

## Health

`GET /health` answers `200` with the body `ok` while the server runs. It does not check CUPS,
SANE or the identity provider, which recto reaches only when someone uses them, so a printer or
scanner being off never marks the container unhealthy.

`recto healthcheck` calls it; the image uses that as its `HEALTHCHECK`, every 30 seconds with a
6-second timeout, after a 20-second start period, failing after 3 misses.
