# Security model

recto puts printers and scanners on a web page. This page says whom it trusts, what it defends
against, and how.

## Whom recto trusts

recto is built for a household, or a small team, that trusts itself:

- **Everyone signed in is equal for printing and scanning.** Anyone who passes sign-in, and the
  required claim when one is set, can use every printer and scanner, and see and cancel every
  print job.
- **Administrators manage accounts, nothing more.** They create local accounts, set passwords,
  disable users and appoint other administrators. They cannot read passwords, and they cannot
  act on their own account from the web page.
- **Scans are private.** A scan result can only be reached by the person who made it, and only for
  an hour. Scans saved to a folder belong to that folder's readers.
- **Whoever runs the server is above all of them.** The command line and the database give full
  control over accounts, which is how a lost administrator is replaced.

recto defends against people **outside** that group: anyone on the internet, or on your network,
who is not signed in, and other websites trying to act through a signed-in person's browser.

## Nothing works without sign-in

Every page, form and server function requires a session, except the sign-in addresses, `/health`
and the interface's static files (scripts, styles and icons, which hold no data). The check runs
for every request at the router, and again inside each action that touches a printer, a scanner or
a result. The administration pages and forms also require an administrator, checked before the
form is even read; to anyone else, they do not exist.

Sign-in details are in [Sign-in and sessions](sign-in.md): PKCE, `state` and `nonce` against
forged or replayed sign-ins, signature and audience checks on the ID token, and a new session id
at every sign-in.

## Passwords

Local passwords are hashed with Argon2id, a slow hash built to make guessing from a stolen
database expensive, with a random salt per password. Hashing runs off the threads that serve
requests, so a burst of sign-ins does not stall the pages. recto never stores or logs a password
in clear, and never shows one again.

A failed password sign-in answers the same, in the same time, whether the username is unknown,
the password wrong, or the account disabled or without a password: a dummy hash is checked when
there is no real one. Nobody can learn from the form which accounts exist.

To slow down guessing online, password attempts are limited per client address: 5 at once, then
one every 4 seconds, about 900 an hour. Sign-in and the current password of a password change
share that budget. With passwords of 12 characters or more, that is far too slow to matter. An
IPv6 client counts by its `/64` network, which it can otherwise roam freely. The limit is kept
in memory, and counts the address of the connection unless `RECTO_TRUST_FORWARDED_HEADERS` is
on. Behind a proxy without it, all clients share the proxy's budget, so one person's typos make
everyone wait; with it on and a proxy that does not replace `X-Forwarded-For`, a client can claim
a new address at each attempt and escape the limit (see
[Run behind a reverse proxy](../how-to/reverse-proxy.md#count-sign-in-attempts-per-person)).

Setting a user's password, or disabling them, ends all their sessions. Changing your own password
on the Account page asks for the current one, and ends your other sessions.

## Administrators

The web page refuses an administrator's actions on their own account: disabling it, changing its
administration or setting its password there. An administrator cannot lock everyone out by
mistake, and someone who borrows an administrator's open session cannot lock the owner out.

Administration granted by the identity provider's claim is only changed in the provider. It
takes back only what it granted, so a claim that stops matching cannot remove the administrators
appointed in recto.

## Sessions and the database

The session cookie holds only a random identifier. Sessions are stored in the database and
checked against the user at every request, so a disabled user or a reset password takes effect
at the next request.

The database holds password hashes and the identifiers of open sessions. Whoever reads it, or a
backup of it, can sign in as anyone with a session open at the time, and try to guess the
passwords offline. Keep it, and its backups, as private as the server itself.

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

The operator's logo may be an SVG, which is a document that can hold scripts. Pages show it
through `<img>`, and browsers never run scripts or load anything from an image shown that way.
Opened at its own address, `/brand/logo`, it comes with
`Content-Security-Policy: default-src 'none'; style-src 'unsafe-inline'; sandbox;
frame-ancestors 'none'`, so it can draw itself and nothing else: no script, no request, no access
to recto's cookies or pages. The brand color goes into the page's style only after recto has
checked it is a `#rrggbb` value, and the name is escaped like any other text.

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
- **Only password attempts are rate limited.** Single sign-on relies on the provider's own limits,
  and the rest of recto requires a session. Put a limit in the reverse proxy if recto is reachable
  from the internet.
- **There is no two-factor authentication for local accounts.** For that, use single sign-on with
  a provider that offers it.
