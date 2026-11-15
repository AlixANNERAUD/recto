# Security model

recto puts printers and scanners on a web page. This page says whom it trusts, what it defends
against, and how.

## Whom recto trusts

recto is built for a household, or a small team, that trusts itself:

- **Everyone signed in is equal.** Anyone who passes sign-in, and the required claim when one is
  set, can use every printer and scanner, and see and cancel every print job. There are no
  roles inside recto.
- **Scans are private.** A scan result can only be reached by the person who made it, and only for
  an hour. Scans saved to a folder belong to that folder's readers.
- **The identity provider decides who gets in.** recto keeps no accounts or passwords.

recto defends against people **outside** that group: anyone on the internet, or on your network,
who is not signed in, and other websites trying to act through a signed-in person's browser.

## Nothing works without sign-in

Every page, form and server function requires a session, except the sign-in addresses, `/health`
and the interface's static files (scripts, styles and icons, which hold no data). The check runs
for every request at the router, and again inside each action that touches a printer, a scanner or
a result.

Sign-in details are in [Sign-in and sessions](sign-in.md): PKCE, `state` and `nonce` against
forged or replayed sign-ins, signature and audience checks on the ID token, and a new session id
at sign-in.

## Cross-site requests

A signed-in browser could be tricked by another site into posting a form to recto, printing or
cancelling on the person's behalf. recto refuses any request that changes something (anything but
`GET`, `HEAD` and `OPTIONS`) when the browser says it comes from elsewhere: a `Sec-Fetch-Site`
header other than `same-origin`, or an `Origin` header other than `RECTO_PUBLIC_URL`. Browsers
send these headers themselves and pages cannot forge them. Combined with the `SameSite=Lax`
session cookie, that closes cross-site forms without a token in every form.

## What the browser may run

Pages carry a Content Security Policy that lets only recto's own scripts run, using a fresh
random nonce for each response, so injected markup cannot run script. Every response forbids
being framed by another site (`frame-ancestors 'none'`), stops the browser from guessing content
types (`X-Content-Type-Options: nosniff`), and sends no referrer to other sites.

## Files

Uploaded files are recognized by their content, not their name, and only PDF, JPEG and PNG are
accepted. recto never opens or renders them: CUPS's filters do, in the CUPS container.

recto does not convert Office documents. Converters such as LibreOffice are large, parse complex
formats, and have a history of vulnerabilities; running one inside recto would expose the web
container to every uploaded file. If conversion is added, it will run in a separate container
with no network and no privileges, never `privileged`.

Saved scans get names recto chooses. A name that would leave the output folder is refused, and an
existing file is never replaced.

## Scanners

A scanner id coming from the browser is only used if it is one of the scanners libsane listed.
Without that check, a crafted id could make a network backend connect to an arbitrary address.

## The containers

recto's image runs as user 65532, not root. `compose.yml` adds:

- `read_only: true`: the file system cannot be changed, except a memory-backed `/tmp` and the
  output folder.
- `cap_drop: [ALL]`: no Linux capabilities.
- `no-new-privileges`: nothing inside can gain privileges, even through a setuid binary.

USB devices are passed one way only, with `devices`; never with `privileged`, which would hand the
container the whole machine.

## CUPS

The CUPS image used by `compose.yml` comes with an administrator account, `print`, with a known
password. The configuration in `examples/cups/` turns that off:

- Only root inside the container administers CUPS, through its local socket
  (`SystemGroup root`), which is why printers are added with `docker compose exec cups lpadmin`.
- The web interface is off.
- Only the Compose subnet may print, and port 631 is not published, so printing goes through
  recto and its sign-in.

`JobPrivateValues none` lets recto read every job's owner, which it needs to list and cancel
jobs. Within the trusted group, that is the point; outside it, CUPS is unreachable.

## What is not covered

- **The network between the proxy and recto** is plain HTTP. Keep it on the same machine or a
  private network.
- **A restart signs everyone out**, by design: sessions are not written to disk.
- **There is no rate limiting** in recto. Put it in the reverse proxy if recto is reachable from
  the internet.
