# Troubleshoot

Start with recto's log, which says what failed and why:

```sh
docker compose logs recto
```

Set `RECTO_LOG_FORMAT: text` to read it more easily, and `RUST_LOG: debug` for more detail (see
[Logs and health](../reference/logs.md)).

## Startup

**recto exits with a message about a setting.** recto checks its settings before it starts. A
required setting is missing, a URL is malformed, `RECTO_PUBLIC_URL` has a path, or
`RECTO_OIDC_REQUIRED_VALUE` is set without `RECTO_OIDC_REQUIRED_CLAIM`. The message names the
setting; see [Configuration](../reference/configuration.md).

**Compose says `set RECTO_PUBLIC_URL to the address users open`.** The `.env` file is missing or
not next to `compose.yml`.

**Compose cannot pull `ghcr.io/alixanneraud/recto:latest`.** No release is published yet: build
the image with `docker compose up -d --wait --build`.

## Sign-in

**Your provider shows an error about the redirect URI.** The redirect URI registered in the
provider must be exactly `{RECTO_PUBLIC_URL}/auth/callback`, scheme and port included.

**Sign-in failed: Sign-in provider unavailable.** recto could not reach the issuer, or its
discovery document. Check `RECTO_OIDC_ISSUER` from inside the container:

```sh
docker compose exec recto bash -c 'exec 3<>/dev/tcp/id.example.org/443' && echo reachable
```

recto keeps trying at each sign-in; nothing needs restarting once the provider is back.

**Sign-in failed: This sign-in expired or was started in another window.** The sign-in took more
than 10 minutes, or the browser lost the session cookie between leaving and coming back. The
second happens when you open recto at another address than `RECTO_PUBLIC_URL`: use that one.

**No access to recto.** You signed in, but the token lacks the required claim or value. See
[Let only some people in](restrict-access.md#check).

**Everyone is signed out.** recto restarted: sessions are kept in memory. A session also ends
after 12 hours without a request.

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

**No printers yet.** CUPS has no printer: [Add a printer](add-printer.md).

**A printer shows Offline or "The printer cannot be reached".** CUPS cannot reach it. Check its
address with `docker compose exec cups lpstat -v`, and that the printer is on.

**Cancelling a job fails.** The CUPS server hides job owners from recto. Set `JobPrivateValues
none` and `JobPrivateAccess all` (see [Use an existing CUPS server](existing-cups.md)).

**The file is too large to print.** It is over `RECTO_MAX_UPLOAD_SIZE`. Without JavaScript, some
browsers show a connection reset instead of this message. Behind nginx, a `413` error means
`client_max_body_size` is lower than recto's limit.

**Only PDF, JPEG and PNG files can be printed.** Export the document to PDF first.

## Scanning

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
