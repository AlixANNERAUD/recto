# Pages and endpoints

Every address except `/health`, the sign-in addresses and the interface's static files requires a
signed-in user. A page requested without a session redirects to `/sign-in?next=<path>`, which
brings the person back after signing in; forms, downloads and server functions answer `401`
instead. A single sign-on user without the required claim, or disabled, is sent to `/no-access`.

## Pages

| Path | Page |
| --- | --- |
| `/` | Print: the print form and the chosen printer's status. |
| `/printers` | Printers: every printer with its state and supplies. |
| `/jobs` | Jobs: the print queue, with Cancel buttons. |
| `/scan` | Scan: scanners, options, preview and results. Needs JavaScript. |
| `/account` | Account: change your password. Only for users with a password, and with local accounts on. |
| `/administration` | Redirects to `/administration/users`. Administrators only. |
| `/administration/users` | Users: create local accounts, set passwords, disable users, grant administration. Administrators only. |
| `/administration/groups` | Groups: the groups from the identity provider and their members. Administrators only. |
| `/sign-in` | The password form, and the single sign-on button when a provider is set. It redirects to `/auth/login` instead when a provider is set and local accounts are off, or no account has a password. |
| `/no-access` | Shown to single sign-on users without the required claim, or disabled. |
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
| `POST /account/password` | Changes your password, then redirects to `/account?changed=1`. Answers `400` with the page for a refused change, and `429` after too many password attempts (see [Password sign-in limit](accounts.md#password-sign-in-limit)). |
| `POST /administration/users` | Creates a local account. |
| `POST /administration/users/{id}/administrator` | Grants or removes administration. |
| `POST /administration/users/{id}/disabled` | Disables or enables a user. |
| `POST /administration/users/{id}/password` | Sets a user's password. |

The administration pages and their forms answer `404`, with the not-found page, to anyone but an
administrator. The account page and its form answer `404` to users without a password.

The Scan page and the live parts of the other pages call server functions under `/api/`. They
are internal to the interface and may change between versions.

## Theme

| Method and path | Does |
| --- | --- |
| `POST /theme` | Public. Takes the form fields `theme` (`system`, `light` or `dark`) and `next` (a path on recto, else `/`). Stores the choice in the `recto_theme` cookie (one year, `HttpOnly`, `SameSite=Lax`, and `Secure` when the public URL is HTTPS), then redirects to `next`. `system` removes the cookie. Answers `400` for another value. |

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
