# Pages and endpoints

Every address requires a signed-in user except `/health`, the sign-in addresses (`/sign-in`,
`/auth/*`, `/no-access`), `POST /theme`, `/brand/*`, `/manifest.webmanifest`, `/sw.js`,
`/pwa.js`, `/offline`, `/device/*` and the interface's static files. A page requested without a session redirects to `/sign-in?next=<path>`, which
brings the person back after signing in; forms, downloads and server functions answer `401`
instead. A single sign-on user without the required claim, or disabled, is sent to `/no-access`.

## Pages

| Path | Page |
| --- | --- |
| `/` | Print: the print form, the chosen printer's status, and what the user has left of each quota rule that applies to them. `?error=<code>` shows why the last job was refused, with `&rule=<id>` for a quota. |
| `/printers` | Printers: every printer the user may use, with its state and supplies. |
| `/jobs` | Jobs: the documents waiting for you from your computers and phones (see [Held jobs](printing.md#held-jobs)), then the print queue, with Cancel buttons, and the pages and cost of each job to its owner and to administrators. `?submitted=<id>` confirms a job, with `&over=<id>,<id>` naming the soft quota rules it went over; `?error=<code>` shows why a cancel or a release was refused, with `&rule=<id>` for a quota; `?done=deleted` confirms a deleted document. |
| `/scan` | Scan: scanners, options, preview and results. Needs JavaScript. |
| `/account` | Account: change your password, for users with a password while local accounts are on, and keep your [device passwords](accounts.md#device-passwords), for everyone while `RECTO_OS_INTEGRATION` is on. `?changed=1` confirms a password change and `?done=revoked` a revoked device password. |
| `/administration` | Redirects to `/administration/users`. Administrators only. |
| `/administration/users` | Users: create local accounts, set passwords, disable users, grant administration, count and revoke each user's device passwords, see each user's quota usage, grant extra and reset it. Administrators only. |
| `/administration/groups` | Groups: the groups from the identity provider and their members. Administrators only. |
| `/administration/printers` | Printers: who may use each printer, its prices, and, with printer management on, adding printers and managing each one. `?drivers=<words>` fills the Add form's driver list with up to 50 matching drivers. Administrators only. |
| `/administration/scanners` | Scanners: who may use each scanner. Administrators only. |
| `/administration/quotas` | Quotas: create, change, disable and delete quota rules, and grant extra to or reset a pool. Administrators only. |
| `/administration/usage` | Usage: what was printed and scanned over a range, in total and by user, group, printer or scanner. `?from=YYYY-MM-DD&to=YYYY-MM-DD` sets the range, both days included, the current month without them; `&by=user`, `group`, `printer` or `scanner` the table. Answers `400` with the page and a message when the dates cannot be read (see [Reports](reports.md#range)), and `503` when the database fails. Administrators only. |
| `/sign-in` | The password form, and the single sign-on button when a provider is set. It redirects to `/auth/login` instead when a provider is set and local accounts are off, or no account has a password. |
| `/no-access` | Shown to single sign-on users without the required claim, or disabled. |
| `/auth/error` | Shown when sign-in fails. |
| `/auth/signed-out` | Shown after signing out; the post-logout redirect URI. |
| `/offline` | Shown by the installed app's service worker when recto cannot be reached. Open to anyone. |

## Forms and actions

These work without JavaScript and accept only requests from recto's own pages (see
[Security model](../explanation/security.md#cross-site-requests)).

| Method and path | Does |
| --- | --- |
| `POST /print` | Sends the uploaded file to a printer, then redirects to `/jobs?submitted=<id>`, with `&over=<rule id>,<rule id>` when the job went over soft quota rules. A refused job redirects to `/?error=<code>`: `quota` with `&rule=<id>` for a hard quota rule, `uncountable` or `counting_busy` when a quota needs the pages of a PDF recto could not count, `unavailable` when the quota rules cannot be read, or the database fails while a rule is enabled, `unconfirmed` when CUPS may have received the job but did not confirm it (the job stays counted and may print). See [Quotas](quotas.md). |
| `GET /jobs/held/{id}/document` | A document waiting for you, shown in the browser (`Content-Disposition: inline`), with its type, `Cache-Control: no-store` and `X-Content-Type-Options: nosniff`. |
| `POST /jobs/held/{id}/release` | Prints a document waiting for you. Fields: `printer`, and `copies`, `sides`, `color_mode`, `page_ranges` as for `POST /print`, empty for the printer's default. Redirects to `/jobs?submitted=<id>`, or `/jobs?error=<code>` with the codes of `POST /print` (`&rule=<id>` for a quota), and `held_gone` when the document is no longer waiting: printed, being printed, deleted or expired. |
| `POST /jobs/held/{id}/delete` | Deletes a document waiting for you, then redirects to `/jobs?done=deleted`; one being printed stays, with `/jobs?error=held_gone`. |
| `POST /jobs/{id}/cancel` | Cancels a job, then redirects to `/jobs`. With `RECTO_JOB_VISIBILITY=own`, a job the user may not see redirects to `/jobs?error=job_not_found`, as a missing one does. |
| `GET /scan/results/{id}` | The result of a scan, for the preview image. |
| `GET /scan/results/{id}/download` | The scan as a file download. |
| `POST /scan/results/{id}/save` | Saves the scan into `RECTO_SCAN_OUTPUT_DIR`. |
| `GET /administration/usage/print-jobs.csv` | Every ledger row of the range as CSV, refunded ones included. Same query as `/administration/usage`. Administrators only. See [Reports](reports.md#csv-files). |
| `GET /administration/usage/scans.csv` | Every logged scan of the range as CSV. Same query. Administrators only. |
| `GET /administration/usage/totals.csv` | The table the page shows (`by`) as CSV, without the total line. Same query. Administrators only. The three answer `400` with the message as plain text when the dates cannot be read, and `503` when the database fails. |
| `POST /account/password` | Changes your password, then redirects to `/account?changed=1`. Answers `400` with the page for a refused change, and `429` after too many password attempts (see [Password sign-in limit](accounts.md#password-sign-in-limit)). |
| `POST /account/devices` | Creates a device password. Field: `name`, 1 to 64 characters once trimmed, without control characters such as line breaks or tabs. Answers `200` with the Account page showing the new password once, with `Cache-Control: no-store`, never a redirect, so the password stays out of addresses and the history. Answers `400` with the page and the typed name for a name too short or too long or with a control character, or a user who already has 20, and `503` when the database fails. |
| `POST /account/devices/{id}/revoke` | Revokes one of your device passwords, then redirects to `/account?done=revoked`. Another user's, or one already revoked, gets `404`. |
| `POST /administration/users` | Creates a local account. |
| `POST /administration/users/{id}/administrator` | Grants or removes administration. |
| `POST /administration/users/{id}/disabled` | Disables or enables a user. |
| `POST /administration/users/{id}/password` | Sets a user's password, and revokes all their device passwords. |
| `POST /administration/users/{id}/devices/revoke` | Revokes all of a user's device passwords, then redirects with `?done=devices_revoked&user=<id>`. Answers `404` for a user that does not exist, and while `RECTO_OS_INTEGRATION` is off. |
| `POST /administration/users/{id}/grant` | Grants the user extra of a quota rule until its current window ends. Fields: `rule` (the id of an `everyone` or `group` rule, or of a `user` rule for that user), `amount` (a whole number above 0, at most 1000000000; for a cost rule, an amount in `RECTO_CURRENCY` above 0) and `reason` (1 to 200 characters once trimmed). Redirects with `?done=quota_granted&user=<id>`. |
| `POST /administration/users/{id}/reset` | Resets the user's usage of a quota rule: only later jobs count. Fields: `rule` and `reason`, as for a grant. Redirects with `?done=quota_reset&user=<id>`. |
| `POST /administration/printers/access` and `/administration/scanners/access` | Sets who may use a device. Fields: `device`, `access` (`everyone` or `groups`) and one `groups` field per group id; `groups` with none means administrators only. Redirects to the page with `?done=access`. Answers `400` with the page when `device` or `access` is missing or a group is unknown, and `503` when the database fails. |
| `POST /administration/printers/forget` and `/administration/scanners/forget` | Drops the rule of a device that is gone, and for a printer its prices. Field: `device`. Redirects with `?done=forgotten`. |
| `POST /administration/printers/prices` | Sets the prices of a printer. Fields: `printer`, `mono_page`, `color_page` and `sheet`, each an amount in `RECTO_CURRENCY` such as `0.05` or `0,05`, empty for 0, with at most the currency's decimals and at most 1,000,000. All 0 removes the prices. Redirects with `?done=prices`. Answers `400` with the page and the typed amounts when one is not such an amount or `printer` is missing, and `503` when the database fails. Works with printer management off. |
| `POST /administration/printers/add` | Adds a printer to CUPS. Fields: `name` (1 to 127 of `A-Za-z0-9_.-`), `address` (a URL with the scheme `ipp`, `ipps`, `http`, `https`, `socket`, `lpd`, `dnssd` or `usb`), `driver` (`everywhere` or a driver name from the list), and optional `description` and `location` (up to 1023 characters each, without control characters). A name CUPS already has, whatever its case, is refused. Redirects with `?done=added`. |
| `POST /administration/printers/describe` | Sets the description and location, an empty one clearing it. Fields: `printer`, `description`, `location`, the last two up to 1023 characters each, without control characters. Redirects with `?done=described`. |
| `POST /administration/printers/default`, `/pause`, `/resume`, `/accept` and `/reject` | Makes the printer the default, pauses or resumes it, makes it accept or turn away jobs. Field: `printer`. Redirects with `?done=default`, `paused`, `resumed`, `accepting` or `rejecting`. |
| `POST /administration/printers/remove` | Removes a printer from CUPS and forgets its access rule and prices. Fields: `printer` and `confirm=yes`. Redirects with `?done=removed`. |
| `POST /administration/quotas` | Creates a quota rule. Fields: `name`; `enabled=true` for an enabled rule, absent for a disabled one; `subject` (`everyone`, `group:<id>`, `user:<id>` or `pool:<id>`); one `printers` field per printer name, none for every printer; `metric` (`pages`, `sheets`, `color_pages`, `mono_pages`, `jobs` or `cost`); `period` (`day`, `week`, `month`, `quarter`, `year`, `rolling` or `total`); `rolling_days` (1 to 366, for `rolling` only); `limit` (a whole number, 0 to 1000000000; for `cost`, an amount in `RECTO_CURRENCY`); `enforcement` (`hard` or `soft`). Redirects with `?done=created`. |
| `POST /administration/quotas/{id}` | Changes a rule. The same fields. Its grants and resets stay, so a rule that has some keeps its `metric` and `period`, apart from the days of a rolling one. Redirects with `?done=saved`. |
| `POST /administration/quotas/{id}/enabled` | Enables or disables a rule. Field: `enabled` (`true` or `false`). Redirects with `?done=enabled` or `disabled`. |
| `POST /administration/quotas/{id}/delete` | Deletes a rule, with its grants and resets. Field: `confirm=yes`. Redirects with `?done=deleted`. |
| `POST /administration/quotas/{id}/grant` and `/reset` | Grants extra to, or resets, the group of a pool rule. Fields as for a user's grant and reset, without `rule`. Redirects with `?done=granted` or `reset`. |

The printer forms answer `404` when `RECTO_CUPS_ADMIN_SOCKET` is not set. The `printer` field of
the forms other than add takes any name CUPS accepts: up to 127 bytes, without spaces, control
characters or any of `/ \ ? ' " #`. A refused form renders the Printers page again with the
reason: `400` for an invalid name, address, description or location, a missing printer or
confirmation, or a change CUPS declined; `409` when adding a name CUPS already has; `502` when
CUPS refused recto or could not reach the printer; `503` when CUPS did not answer.

A refused quota form, grant or reset renders its page again with the reason: `400` for an empty
name, a limit that is not a number or not an amount, a negative limit or one over 1000000000, a
change of metric or period of a rule with grants or resets, rolling days outside 1 to 366, a group or user that does not exist, a deletion without `confirm=yes`, an extra allowance
not above 0 or over 1000000000, a reason empty or over 200 characters, or a field the page never offers, such as a
grant of another user's rule, of a pool rule through a user's address, or of a rule that is not
a pool through the Quotas page; `404` for a rule that does not exist; `503` when the database
fails. A refused rule form keeps what was typed, and the form of the rule it changes is open.

The three addresses of a held document answer `404`, with the not-found page, to anyone but its
owner, administrators included, as for one that does not exist. They work whether or not
`RECTO_OS_INTEGRATION` is on, so documents sent before it was turned off can still be printed or
deleted.

The administration pages and their forms answer `404`, with the not-found page, to anyone but an
administrator. The account page answers `404` to users it has nothing for: without a password,
or with local accounts off, while `RECTO_OS_INTEGRATION` is off. The password form answers `404`
to users without a password, and the device forms answer `404` while `RECTO_OS_INTEGRATION` is
off.

## Devices

recto's printer, for computers and phones. These addresses take no session and no cross-site
check: devices authenticate with a [device password](accounts.md#device-passwords). Bodies are
IPP requests (`Content-Type: application/ipp`) of at most `RECTO_MAX_UPLOAD_SIZE` plus 64 KiB,
and answers carry `Cache-Control: no-cache`. See
[Printing from computers and phones](printing.md#printing-from-computers-and-phones).

| Method and path | Does |
| --- | --- |
| `POST /ipp/print` | The printer, with the device password as the password of HTTP Basic. recto reads the 8-byte IPP header first: Get-Printer-Attributes needs no credentials, every other operation answers `401` with `WWW-Authenticate: Basic` before the rest of the body is read when they are missing or wrong. |
| `POST /ipp/print/{job}` | The same: the address of a job that recto hands out as its `job-uri`. |
| `POST /device/{password}/ipp/print` and `/device/{password}/ipp/print/{job}` | The personal address. The password is checked before any of the body is read; a wrong one answers `404`. |

Other answers: `403` for device credentials over plain HTTP (see `RECTO_IPP_ALLOW_PLAIN`), `413`
for a body over the limit, `429` with `Retry-After: 1` after too many wrong passwords from one
address (see [Device password limit](accounts.md#device-password-limit)), `503` when the
database cannot check the password.

Any other method or path under `/ipp/`, `/device/` and `/eSCL/` answers a bare `404` with no
body, signed in or not, and so do all of them while `RECTO_OS_INTEGRATION` is off: never the
sign-in redirect, which would copy a password into a `Location` header and the browser's history.

The Scan page and the live parts of the other pages call server functions under `/api/`. They
are internal to the interface and may change between versions.

## Theme

| Method and path | Does |
| --- | --- |
| `POST /theme` | Public. Takes the form fields `theme` (`system`, `light` or `dark`) and `next` (a path on recto, else `/`). Stores the choice in the `recto_theme` cookie (one year, `HttpOnly`, `SameSite=Lax`, and `Secure` when the public URL is HTTPS), then redirects to `next`. `system` removes the cookie. Answers `400` for another value. |

## Brand

Public, so the sign-in page and the installed app can show them. Each answers with
`Cache-Control: public, max-age=3600`. See [Brand recto](../how-to/brand.md).

| Method and path | Does |
| --- | --- |
| `GET /brand/icon-192.png` | The app icon, 192 by 192 pixels, PNG. Also the browser tab's icon. |
| `GET /brand/icon-512.png` | The app icon, 512 by 512 pixels, PNG. |
| `GET /brand/icon-maskable-512.png` | The app icon for devices that crop it to their own shape, 512 by 512 pixels, PNG: the artwork keeps to the middle 80 % on a full background of the accent color. |
| `GET /brand/apple-touch-icon.png` | The home screen icon of iPhones and iPads, 180 by 180 pixels, PNG. |
| `GET /brand/logo` | The logo of `RECTO_BRAND_LOGO`, with the type read from its content. `404` without one. An SVG logo is sent with a Content Security Policy that sandboxes it. |

## The installed app

Open to anyone, because browsers fetch them without cookies. They hold nothing private. See
[Install recto as an app](../how-to/install-app.md).

| Method and path | Does |
| --- | --- |
| `GET /manifest.webmanifest` | The web app manifest: the brand's name, color and icons, `display: standalone`. `Cache-Control: no-cache`. |
| `GET /sw.js` | The service worker. Its cache is named after the recto version. `Cache-Control: no-cache`. |
| `GET /pwa.js` | The script that registers the service worker. One of the interface's static files. |

## Sign-in

| Method and path | Does |
| --- | --- |
| `POST /sign-in` | Signs in with a username or email address and a password, then redirects to the `next` path. Answers `401` with the form for a wrong password, and `429` after too many attempts (see [Password sign-in limit](accounts.md#password-sign-in-limit)). `404` with local accounts off. |
| `GET /auth/login` | Starts a sign-in at the provider. |
| `GET /auth/callback` | Where the provider sends people back; the redirect URI. |
| `POST /auth/logout` | Ends the session. A session signed in with a password then lands on `/sign-in`. One signed in through single sign-on goes on to the provider's end-session endpoint, when it has one, so the provider's session ends too, then to `/auth/signed-out`. |

## Health

| Method and path | Does |
| --- | --- |
| `GET /health` | Answers `200 ok` while recto runs. It does not check CUPS, SANE or the provider. |
