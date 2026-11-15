# Pages and endpoints

Every address except `/health`, the sign-in addresses and the interface's static files requires a
signed-in user. A request without a session is sent to sign in; a signed-in user without the
required claim is sent to `/no-access`.

## Pages

| Path | Page |
| --- | --- |
| `/` | Print: the print form and the chosen printer's status. |
| `/printers` | Printers: every printer with its state and supplies. |
| `/jobs` | Jobs: the print queue, with Cancel buttons. |
| `/scan` | Scan: scanners, options, preview and results. Needs JavaScript. |
| `/no-access` | Shown to people without the required claim. |
| `/auth/error` | Shown when sign-in fails. |
| `/auth/signed-out` | Shown after signing out; the post-logout redirect URI. |

## Forms and actions

These work without JavaScript and accept only requests from recto's own pages (see
[Security model](../explanation/security.md#cross-site-requests)).

| Method and path | Does |
| --- | --- |
| `POST /print` | Sends the uploaded file to a printer, then redirects to `/jobs`. |
| `POST /jobs/{id}/cancel` | Cancels a job, then redirects to `/jobs`. |
| `GET /scan/results/{id}` | The result of a scan, for the preview image. |
| `GET /scan/results/{id}/download` | The scan as a file download. |
| `POST /scan/results/{id}/save` | Saves the scan into `RECTO_SCAN_OUTPUT_DIR`. |

The Scan page and the live parts of the other pages call server functions under `/api/`. They
are internal to the interface and may change between versions.

## Sign-in

| Method and path | Does |
| --- | --- |
| `GET /auth/login` | Starts a sign-in at the provider. |
| `GET /auth/callback` | Where the provider sends people back; the redirect URI. |
| `POST /auth/logout` | Ends the session and signs out at the provider when it supports it. |

## Health

| Method and path | Does |
| --- | --- |
| `GET /health` | Answers `200 ok` while recto runs. It does not check CUPS, SANE or the provider. |
