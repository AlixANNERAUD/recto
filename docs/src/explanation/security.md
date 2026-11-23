# Security model

recto puts printers and scanners on a web page. This page says whom it trusts, what it defends
against, and how.

## Whom recto trusts

recto is built for a household, or a small team, that trusts itself:

- **Everyone signed in is equal for printing and scanning, by default.** Anyone who passes
  sign-in, and the required claim when one is set, can use every printer and scanner, and see
  and cancel every print job. An administrator can limit a device to some groups, which come
  from the provider's groups claim (see
  [Limit printers and scanners to groups](../how-to/restrict-access.md#limit-printers-and-scanners-to-groups)).
  With `RECTO_JOB_VISIBILITY=own`, jobs are private: people see and cancel only their own, and
  administrators see all (see [Keep jobs private](../how-to/restrict-access.md#keep-jobs-private)).
- **These limits apply inside recto only.** Anyone who reaches CUPS or `saned` directly, from
  your network, bypasses them, print quotas included: keep those services off networks you do
  not trust.
- **Administrators manage accounts and device rules.** They create local accounts, set
  passwords, disable users, revoke their device passwords, appoint other administrators and
  limit devices to groups. They can use every device and see every job. They cannot read passwords, and they cannot act on their
  own account from the web page.
- **Scans are private.** A scan result can only be reached by the person who made it, and only for
  an hour. Scans saved to a folder belong to that folder's readers.
- **Whoever runs the server is above all of them.** The command line and the database give full
  control over accounts, which is how a lost administrator is replaced.

recto defends against people **outside** that group: anyone on the internet, or on your network,
who is not signed in, and other websites trying to act through a signed-in person's browser.

## Nothing works without sign-in

Every page, form and server function requires a session, except the sign-in addresses, `/health`
and the interface's static files (scripts, styles and icons, which hold no data). The printer's
addresses for computers and phones, under `/ipp/`, `/eSCL/` and `/device/`, take no session
either: they require a [device password](#device-passwords) instead, checked on every request,
and only Get-Printer-Attributes, which says what the printer is, answers without one, besides
[scanning from open networks](#scanning-without-a-password). The check runs
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

## Device passwords

Computers and phones cannot follow a sign-in page, so they authenticate with
[device passwords](../reference/accounts.md#device-passwords). A device password grants
exactly what a printer or a scanner needs: sending print jobs as its owner, seeing and
cancelling those still waiting for their document, and scanning on the scanners its owner may use. It never opens a
session, never reaches a web page and never changes the account: the sign-in form refuses it like
any wrong password, and the device routes accept nothing else, not even the account's password.
That keeps the account password out of device keychains, and makes revoking a lost phone a click
that touches nothing else.

Each is 160 random bits from the system's random source, shown once and stored as its SHA-256.
A slow hash such as Argon2id protects passwords people choose, which can be guessed; a random
secret of that size cannot, so a fast hash loses nothing, and lets a device that polls every few
seconds authenticate with one database lookup. The database never holds the secret, and recto
never logs it: log lines name the device password's id and name.

A device password lasts until it is revoked; it does not expire. Access ends with it when:

- its owner or an administrator revokes it, on the Account page or with **Revoke devices** on the
  Users page;
- an administrator sets the user's password, which is what follows a compromised account: a
  device password an intruder made with a stolen session goes with it. Changing your own password
  keeps yours;
- the identity provider refuses the user recto's required claim at a sign-in: recto revokes their
  device passwords before showing **No access to recto**;
- the user is disabled, until they are enabled again.

Removing someone at the identity provider alone does not reach recto: the provider never sees
device passwords, and recto only learns of the change if the person tries to sign in again.
Disable the user in recto, or revoke their devices, as well.

Revoking a device password, or disabling its user, applies to the next request: what grants
access is never cached. Only refusals are: a secret that matches no device password is refused
again from memory for 10 minutes (up to 1024 of them), since a random secret never becomes one
later. A disabled user's secret is not remembered, so enabling them again brings their devices
back at once. Over plain HTTP, recto refuses device passwords unless told otherwise
(`RECTO_IPP_ALLOW_PLAIN`), as the password would cross the network in clear; behind a reverse
proxy recto cannot see the hop to the device, and trusts the proxy as the sign-in form does.
Connections from the server itself are always accepted, so a proxy on the same host makes every
device look local: the check then holds only if that proxy serves devices over HTTPS alone.

Wrong device passwords have **their own limit**, not the password sign-in one. Slowing guesses
would gain nothing against a 160-bit secret, so the limit only keeps floods off the database: 30
at once, then one a second, and right passwords never count. It is kept apart on purpose,
departing from the first accounts design, which shared one limit: behind a proxy without trusted
forwarded headers, or behind a school's NAT, every client has the same address, and one phone
still polling with a revoked password would otherwise make password sign-in fail for everyone
there. Such a phone does not drain the device limit either: its secret, refused once, is refused
again from memory without counting, so only new guesses do. A flood of new guesses through a
shared address still makes every device behind it wait; with `RECTO_TRUST_FORWARDED_HEADERS`
on, the limit counts by the forwarded client address, as the sign-in limit does, and only the
guesser waits. Whether plain HTTP is accepted always depends on the connection's address, never
on a forwarded header, which a device could write itself.

## The LAN listener

Devices that find recto through DNS-SD connect to it directly, by its `.local` name or address,
which the public certificate does not cover. So discovery points at a second listener,
`RECTO_LAN_LISTEN_ADDRESS`, off by default. It speaks TLS only, never plain HTTP, and serves the
printer's and the scanners' addresses alone: no page, no sign-in, no session, no file, every other
address a bare `404`. Device passwords work there as behind the proxy, and the wrong ones count
toward the same limit, by the connection's address: forwarded headers are never read there,
since devices reach it without a proxy and a header would be the device's own claim. A device
has 10 seconds to complete its handshake, each in a task of its own, so one that connects and
says nothing holds nothing up. One address may run 8 handshakes and keep 32 connections at
once; past that, its new connections are closed, so one host on the network cannot take every
place. On both listeners a connection whose next request headers do not arrive within 30
seconds is closed, and a request without a document must arrive whole within 30 seconds, one
with a document within 10 minutes.

Its certificate is the operator's (`RECTO_LAN_TLS_CERTIFICATE`), or one recto makes itself at
the first start and keeps in `RECTO_DATA_DIR/lan-tls/`, readable by recto alone. No device
trusts a self-signed certificate on its own: Apple devices and sane-airscan accept it the first
time and Apple devices then remember it, the usual AirPrint trust model. That protects against
someone listening on the network, not against someone who answers in recto's place on that
first connection. recto logs the certificate's SHA-256 fingerprint at each start (`LAN
listener`), so it can be compared with what a device shows. Since devices remember it, recto
never renews nor changes it; delete `lan-tls` to make a new one, and devices that remembered the
old one must forget recto and add it again.

The DNS-SD responder announces recto under its own name, `recto-<hostname>.local`, never the
host's, which the host's avahi-daemon owns, and only on the interfaces of
`RECTO_DNSSD_INTERFACES`: by default not on Docker's, virtual machines' or VPNs', whose addresses
the LAN cannot reach and which would tell a VPN's peers about the printer. It announces where
recto is, its name and the pages `/jobs` and `/scan`, as any network printer does, and no
secret.

## Scanning without a password

macOS, Windows and Mopria scan apps find scanners over DNS-SD and cannot carry a secret in the
address they find. `RECTO_ESCL_OPEN_NETWORKS`, empty by default, lets the devices of the networks
it lists scan without a device password. That is what a network scanner's own eSCL already
allows anyone on its network, and the limits keep it there:

- it holds on the LAN listener only, where devices connect to recto directly; behind the reverse
  proxy every client would come from the proxy's address;
- the address checked is the TCP connection's, never a forwarded header, which a device writes
  itself, and an IPv4 address seen as IPv6 is compared as IPv4;
- only scanners open to everyone are reachable: one limited to groups does not exist for them;
- such scans are logged without a user, and every device of those networks counts as the same
  anonymous client: while a scan waits for its app, for at most 10 minutes, another anonymous
  device can see it in the scanner's status and fetch it.

## Held documents

Documents sent from computers and phones are kept, which is a deliberate change from the Print
page, where an upload goes straight to CUPS and recto keeps nothing. They wait under
`RECTO_DATA_DIR/held`, in files readable by recto alone, until their owner releases or deletes
them, or until they expire after `RECTO_HELD_JOB_HOURS` (24 by default). Expired documents are
deleted within 10 minutes, even with OS integration off, and a file no held job names, left by a
crash, is deleted at the next start.

They are their owner's only. Every query for them names the user, or for a device the device
password that sent them: an administrator cannot list, read or release another person's
documents, and a device sees only what it sent itself, so a leaked phone password shows nothing
of what the person printed from their laptop. Whoever reads the data volume or its backups reads
the documents waiting there: keep it as private as the database.

Printing one goes through the same printer access rules, quotas and ledger as an upload on the
Print page. The job is marked printed in the database transaction that counts it, and a held job
whose count cannot be written is not printed, quota rules or not: a document from a device never
prints uncounted, nor twice. Its owner's preview is sent with `Cache-Control: no-store` and
`X-Content-Type-Options: nosniff`, so browsers keep no copy and never read it as another type,
but without the sandbox of the brand logo, which would stop the browser's PDF viewer.

Limits keep a device from filling the disk: a document is at most `RECTO_MAX_UPLOAD_SIZE`, and
each person may have at most 20 waiting and 10 times that size in all, checked before anything
is written. A body over the limit is refused without being kept. recto authenticates before
reading a document: a personal address is checked before any byte of the body, and on
`/ipp/print` only the 8-byte header that names the operation is read first. A device may still
have sent part of its document by then, and some proxies read whole bodies anyway: recto only
promises not to read or keep it. Without credentials, recto reads at most 64 KiB, enough for the
printer attributes devices ask before they know they need a password, and the same for any
request that carries no document, credentials or not. Before any attribute is
parsed, one pass over the request refuses collections nested more than 4 deep, more than 1,000
attributes and more than 64 values for one: the IPP parser copies each collection as it closes,
so deep nesting would cost minutes or overflow the stack. A request whose first
bytes or later pieces stall for 30 seconds is dropped, and each person's devices send at most
two documents at once, since a document is held in memory until it is written: a leaked device
password cannot fill the memory with parallel uploads.

Printing directly (**Documents from your devices** on the Account page) changes only when a
document is released, not how: the same access rules, quotas and ledger apply as on the Jobs
page, checked again at each document, so a printer the person may no longer use holds it
instead. It cannot reach a printer the person could not choose on the Print page.

### Rasters

Phones and Windows send rasters, pictures of each page, which recto decodes itself to make a
PDF: with IPP and the eSCL XML, the only parsers of untrusted input recto adds, and written in
safe Rust. Their line coding lets a few bytes stand for a whole page, so the bounds are on
decoded pixels, read from each page's header before a byte of it is decoded: 160 MiB per page,
4 GiB and 500 pages per document, with every size computed so that it cannot overflow. Decoded
lines go straight into a compressor, so memory holds one line and the compressed result, which
stops at `RECTO_MAX_UPLOAD_SIZE`. Two rasters convert at once, each for at most 60 seconds; the
deadline is checked inside the decoder, because a blocking thread cannot be stopped from outside.
A raster cut short is refused, never padded. The bounds and the IPP answers are listed in
[Printing](../reference/printing.md#printing-from-computers-and-phones).

The personal address puts the device password in a URL. recto never logs it and never redirects
with it, and any unknown address under `/device/` is a bare `404`. Your reverse proxy's access
log will record it unless told not to (see
[Run behind a reverse proxy](../how-to/reverse-proxy.md#printing-from-computers-and-phones)), and
on a shared computer anyone who opens the printer's settings reads it. Revoking the device
password is the remedy.

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
passwords offline. It also holds the history of who printed and scanned what and when: the names
of printed documents, and for scans the scanner, the pages and the format, never their content.
Keep it, and its backups, as private as the server itself.

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

## The service worker

The service worker of the installed app caches only the interface's scripts and styles
(`/pkg/`, fetched from the server first and kept only as a fallback), the brand images
(`/brand/`) and the offline page, which it fetches without cookies so it holds no user's data. It
never stores pages, scans, print or account responses: for everything else it leaves the request to the browser, as if no
worker existed. The manifest and the worker are public because browsers fetch them without
cookies, and they hold nothing private: a name, a color, icons and a few lines of script.

## Files

Uploaded files, and documents sent from computers and phones, are recognized by their content,
not their name or declared type, and only PDF, JPEG and PNG are accepted. recto never opens or renders them: CUPS's filters do, in the CUPS container.

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

recto's image runs as user 65532, not root, and holds only recto, the SANE libraries and
`scanimage`: no shell, package manager or other programs an intruder could use. `compose.yml`
adds:

- `read_only: true`: the file system cannot be changed, except a memory-backed `/tmp` and the
  output folder.
- `cap_drop: [ALL]`: no Linux capabilities.
- `no-new-privileges`: nothing inside can gain privileges, even through a setuid binary.

USB devices are passed one way only, with `devices`; never with `privileged`, which would hand the
container the whole machine.

## CUPS

The CUPS image used by `compose.yml` comes with an administrator account, `print`, with a known
password. The configuration in `examples/cups/` turns that off:

- Only root and a `recto` account, both inside the container, administer CUPS, and only through
  its local socket (`SystemGroup root recto`). Root is first because CUPS gives its root
  certificate the group of the first entry. The `print` and `lpadmin` accounts, which have
  passwords, are not in that group, and administration over the network is always refused.
- The web interface is off.
- Only the Compose subnet may print, and port 631 is not published, so printing goes through
  recto and its sign-in.

That `recto` account is what lets recto manage printers. `compose.yml` creates it when the `cups`
container starts, with recto's uid (`RECTO_UID`, 65532 by default), and shares the socket's
directory between the two containers through the `cups-socket` volume, mounted read-only in
recto. Its group is always 65532: CUPS makes an administrator of every account whose main group
is the `recto` group, and the image's `print` account, with its known password, has group 1000. When `RECTO_CUPS_ADMIN_SOCKET` names the socket, recto administers CUPS itself, so **a
recto administrator is a CUPS administrator**: they can add and remove printers, and the address
of a new printer makes CUPS connect to that address from its own container. Grant administration
in recto with that in mind.

recto proves who it is to CUPS with the `PeerCred` method. CUPS accepts it only on its local
socket, from a process whose uid is the one of its `recto` account and whose group is a
`SystemGroup`, so another uid on the volume is refused. Over the network, CUPS refuses every
administration request, so a stolen request or a mistaken `Listen` line gives nothing. recto
checks every printer name and address before it sends them (an address with a user name or
password is refused, since CUPS would store it in clear), and never logs a request body.

**Share the socket only with recto.** CUPS knows recto by its uid alone, so any process running
as that uid that can reach the socket administers CUPS. Distroless images that run as "nonroot"
all use 65532, recto's default: do not mount the `cups-socket` volume, or the host's
`/run/cups`, into another container, and keep other services on the host off that uid.

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
