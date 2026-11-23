# Troubleshoot

Start with recto's log, which says what failed and why:

```sh
docker compose logs recto
```

Set `RECTO_LOG_FORMAT: text` to read it more easily, and `RUST_LOG: debug` for more detail (see
[Logs and health](../reference/logs.md)).

## Startup

**recto exits with a message about a setting.** recto checks its settings before it starts. A
required setting is missing, a URL is malformed, `RECTO_PUBLIC_URL` has a path, only one of
`RECTO_OIDC_ISSUER` and `RECTO_OIDC_CLIENT_ID` is set, local accounts are off without a provider,
a `_VALUE` setting is set without its `_CLAIM`, or `RECTO_PASSWORD_MINIMUM_LENGTH` is not between
1 and 1,024. The message names the setting; see [Configuration](../reference/configuration.md).

**recto: cannot open the database sqlite:///data/recto.db (RECTO_DATABASE_URL): … unable to
open database file.** No volume is mounted on `/data`, so the read-only file system has no room
for the database, or the folder mounted there is not writable by user 65532. Mount a volume, as
`compose.yml` does (see [Upgrade](upgrade.md#from-a-version-without-accounts)).

**recto: cannot open the database postgres://… (RECTO_DATABASE_URL): …** PostgreSQL is not
reachable, the database does not exist, or the password is wrong. The message hides the
password. Check `RECTO_DATABASE_URL`, and percent-encode special characters in the password (see
[Database](../reference/configuration.md#database)).

**Compose says `set RECTO_PUBLIC_URL to the address users open`.** The `.env` file is missing or
not next to `compose.yml`.

**Compose cannot pull `ghcr.io/alixanneraud/recto:latest`.** No release is published yet: build
the image with `docker compose up -d --wait --build`.

## Sign-in

**The username or password is wrong.** recto gives the same answer for an unknown username, a
wrong password, a disabled account and a single sign-on user without a password, so as not to tell
which accounts exist. An administrator can check the account on the Users page, and set a new
password (see [Manage users and administrators](manage-users.md#reset-a-password)).

**Too many attempts. Wait a few seconds, then try again.** More than 5 password attempts came from
the same address in a row; one more is allowed every 4 seconds. If it happens to everyone at once
behind a reverse proxy, every client shares the proxy's address: see
[Run behind a reverse proxy](reverse-proxy.md#count-sign-in-attempts-per-person).

**Nobody can sign in, or no administrator is left.** Create an administrator, or make an
existing user one, on the server (see
[Manage users and administrators](manage-users.md#regain-access-without-an-administrator)).

**Your provider shows an error about the redirect URI.** The redirect URI registered in the
provider must be exactly `{RECTO_PUBLIC_URL}/auth/callback`, scheme and port included.

**Sign-in failed: Sign-in provider unavailable.** recto could not reach the issuer, or its
discovery document. The recto image has no shell, so fetch the document from a throwaway curl
container that shares recto's network, with `RECTO_OIDC_ISSUER` in place of
`https://id.example.org`:

```sh
docker run --rm --network container:"$(docker compose ps --quiet recto)" \
  curlimages/curl:8.23.0@sha256:3fb7e13e5d179a845c97d78204f3f1064d1907037f26a3ebf91710571151fd93 \
  --fail --silent --show-error https://id.example.org/.well-known/openid-configuration
```

It prints the provider's settings when recto can reach it, and the reason otherwise.

recto keeps trying at each sign-in; nothing needs restarting once the provider is back.

**Sign-in failed: This sign-in expired or was started in another window.** The sign-in took more
than 10 minutes, or the browser lost the session cookie between leaving and coming back. The
second happens when you open recto at another address than `RECTO_PUBLIC_URL`: use that one.

**No access to recto.** You signed in through the provider, but the token lacks the required
claim or value (see [Let only some people in](restrict-access.md#check)), or an administrator
disabled your account.

**Someone is signed out.** A session ends after 12 hours without a request, and when the user's
password is set or they are disabled. Sessions are kept in the database, so restarts do not end
them; upgrading from a version without accounts signs everyone out once (see
[Upgrade](upgrade.md#from-a-version-without-accounts)).

**The Administration entry is missing.** Only administrators have it. With the administrator
claim, check its name and value, and sign out and in again: the claim is read at sign-in.

**Single sign-on users have the wrong username.** The username is taken at the first sign-in,
from `preferred_username`, else `email`, else `sub`, and never changes. Have the provider send
`preferred_username` before people first sign in.

**This request did not come from recto's own pages, so it was refused.** The browser sent a form
from another origin than `RECTO_PUBLIC_URL`, which recto refuses to prevent cross-site requests.
Open recto at its public address.

## Printing

**Printing service unavailable** on the Print page. recto cannot reach CUPS. With the bundled
`cups` service, check that it runs: `docker compose ps`. With your own server, see below.

**CUPS answers `400 Bad Request`** (in recto's log). CUPS refuses the `Host` header recto sends:
add `ServerAlias *` to its `cupsd.conf` (see [Use an existing CUPS server](existing-cups.md)).

**CUPS answers `403 Forbidden`**, or refuses every job. recto's address is not allowed in the
`<Location />` block of `cupsd.conf`. The bundled configuration allows the Compose subnet
172.30.80.0/24; if you changed the subnet in `compose.yml`, change it in
`examples/cups/cupsd.conf` too.

**No printers yet.** CUPS has no printer: [Add a printer](add-printer.md). People who are not
administrators read **No printer is available to you right now** instead, which also means that
access rules leave them none (see [Restrict access](restrict-access.md)).

**A printer shows Offline or "The printer cannot be reached".** CUPS cannot reach it. Check its
address with `docker compose exec cups lpstat -v`, and that the printer is on.

**Cancelling a job fails.** The CUPS server hides job owners from recto. Set `JobPrivateValues
none` and `JobPrivateAccess all` (see [Use an existing CUPS server](existing-cups.md)).

**The file is too large to print.** It is over `RECTO_MAX_UPLOAD_SIZE`. Without JavaScript, some
browsers show a connection reset instead of this message. Behind nginx, a `413` error means
`client_max_body_size` is lower than recto's limit.

**Only PDF, JPEG and PNG files can be printed.** Export the document to PDF first.

## Scanning

**No scanner is available to you right now**, for someone who is not an administrator: SANE finds
nothing, or access rules leave them none (see [Restrict access](restrict-access.md)).

**No scanners found.** SANE finds nothing. Run `docker compose exec recto scanimage -L`:

- An empty list for a network scanner: list it in `airscan.conf`, and check that the file is
  mounted (see [Add a scanner](add-scanner.md)). Discovery does not work through Docker's bridge
  network.
- An empty list for a USB scanner: the container lacks the device, or user 65532 cannot open it.
- Listing takes a long time: mount the trimmed `dll.conf`, which skips backends you do not use.

**The scanner is busy.** Another program, or another person in recto, is using it. Scans queue
inside recto, but the scanner itself can only serve one client at a time.

**Scanner not found.** The scanner went away since the page loaded. Press the round arrow to look
again.

**This scan would be too large for recto.** One page would take more than 300 MB before
compression. Lower the resolution, or select a smaller area on the preview.

**The connection to recto was cut before the scan finished.** A reverse proxy gave up waiting.
Raise its timeout (see [Run behind a reverse proxy](reverse-proxy.md#let-long-scans-finish)).

**Cannot write to /consume.** recto's user cannot write to the output folder. See
[Save scans into Paperless-ngx](paperless.md#2-let-recto-write-there).

**Save to folder is missing.** `RECTO_SCAN_OUTPUT_DIR` is not set.
