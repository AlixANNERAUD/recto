<h1 align="center">recto</h1>

<p align="center">
  <strong>Print and scan from a browser, through your home server.</strong><br>
  A small self-hosted web app for CUPS and SANE, behind your OpenID Connect provider.
</p>

<p align="center">
  <a href="https://github.com/AlixANNERAUD/recto/actions/workflows/ci.yml"><img alt="CI" src="https://github.com/AlixANNERAUD/recto/actions/workflows/ci.yml/badge.svg"></a>
  <a href="LICENSE"><img alt="License: Apache-2.0" src="https://img.shields.io/badge/license-Apache--2.0-blue.svg"></a>
</p>

---

recto gives everyone in the house one page to **print** a file on any CUPS printer (over IPP) and to
**scan** from any SANE scanner (through libsane). Scans can be downloaded or saved straight into a
folder such as Paperless's consume folder. Sign-in goes through your own **OpenID Connect**
provider (Zitadel, Keycloak, Authentik and others), so there are no passwords to manage in recto.

It is one container plus a CUPS server. Printing works without JavaScript; scanning needs it.

## Quick start

You need an OpenID Connect provider with a public client (see [Sign-in](#sign-in-with-openid-connect)).

```sh
git clone https://github.com/AlixANNERAUD/recto && cd recto
mkdir -p consume && sudo chown 65532:65532 consume
cat > .env <<'ENV'
RECTO_PUBLIC_URL=https://recto.example.org
RECTO_OIDC_ISSUER=https://id.example.org
RECTO_OIDC_CLIENT_ID=your-client-id
ENV
docker compose up -d --wait
```

Open <http://localhost:8080> (change the port with `RECTO_PORT`). Sign-in only works through the
address in `RECTO_PUBLIC_URL` (the session cookie and the redirects depend on it), so if that is
another address, open that one. `compose.yml` starts recto and a
CUPS server on a private network. Add a printer to it:

```sh
docker compose exec cups lpadmin -p office -E -v ipp://192.168.1.60/ipp/print -m everywhere
```

Scanners need a little more setup, see [SANE](#scanners-sane).

Images are published as `ghcr.io/alixanneraud/recto` for linux/amd64 and linux/arm64, with the
tags `X.Y.Z`, `X.Y`, `X` and `latest`. Set `RECTO_IMAGE` to pin one. `latest` exists only once the first `vX.Y.Z` release is published;
before that, run `docker compose up -d --build` to build it from the checkout. `docker compose up --build`
builds the image from your checkout instead.

recto listens in plain HTTP. Put a reverse proxy with TLS in front of it and set
`RECTO_PUBLIC_URL` to the address people open: sign-in redirects and the `Secure` cookie flag
depend on it.

## Configuration

Every setting is an environment variable. In `compose.yml`, put them under `environment`. A
optional variable set to an empty value counts as unset.

| Variable | Default | Meaning |
| --- | --- | --- |
| `RECTO_PUBLIC_URL` | required | The address users open, such as `https://recto.example.org`. It must be the root of a site: recto cannot run under a path such as `/recto`. |
| `RECTO_OIDC_ISSUER` | required | The issuer URL of your identity provider. |
| `RECTO_OIDC_CLIENT_ID` | required | The client id of the public client. |
| `RECTO_OIDC_SCOPES` | none | Extra scopes, separated by spaces, requested next to `openid`. |
| `RECTO_OIDC_REQUIRED_CLAIM` | none | Only let in users whose token has this claim. |
| `RECTO_OIDC_REQUIRED_VALUE` | none | The value the claim must equal or contain. recto refuses to start with a value and no claim. |
| `RECTO_CUPS_URL` | `http://localhost:631` | The CUPS server. `ipp://` and `ipps://` are accepted. |
| `RECTO_SCAN_OUTPUT_DIR` | none | The folder behind "Save" on the scan page. Without it, scans can only be downloaded. |
| `RECTO_MAX_UPLOAD_SIZE` | `52428800` (50 MiB) | The largest file accepted for printing, in bytes. |
| `RECTO_LISTEN_ADDRESS` | `0.0.0.0:8080` | Where recto listens inside its container. |
| `RECTO_SITE_ROOT` | `/app/site` | Where the built interface lives; the image sets it. |
| `RECTO_LOG_FORMAT` | `json` | `json` or `text`, on standard error. |

A `RECTO_CUPS_URL` without a port uses the default of its scheme (80 or 443), so write `:631`.

Sessions are kept in memory: restarting recto signs everyone out, and a session ends after 12 hours
without a request.

## Sign-in with OpenID Connect

Create a **public client** (no secret) that uses the authorization code flow with **PKCE**, and
register two URIs, where `{RECTO_PUBLIC_URL}` is your public address:

- redirect URI: `{RECTO_PUBLIC_URL}/auth/callback`
- post-logout redirect URI: `{RECTO_PUBLIC_URL}/auth/signed-out`

Set `RECTO_OIDC_ISSUER` and `RECTO_OIDC_CLIENT_ID`. By default every user your provider signs in can
use recto. To restrict it, set `RECTO_OIDC_REQUIRED_CLAIM`, and optionally `RECTO_OIDC_REQUIRED_VALUE`.
The claim can be a string (must equal the value), an array (must contain it), an object (must have
it as a key) or a dotted path into nested objects, such as `realm_access.roles` for Keycloak.
Without a value, the claim only has to be present.

### Zitadel

1. Create a project, and in it a role with the key `recto`.
2. In the project settings, enable **Assert roles on authentication**. (Alternatively, enable
   **User roles inside ID token** on the application.)
3. Create an application of type **User Agent** with authentication method **PKCE**, and add the
   two URIs above. Copy its client id.
4. Grant the `recto` role to the users who may use it.
5. Optionally, enable **User Info inside ID Token** in the application's token settings. Zitadel
   otherwise leaves the username and name out of the ID token, and recto reads them from the
   UserInfo endpoint at each sign-in (one more request).
6. Configure recto:

```yaml
RECTO_OIDC_ISSUER: https://id.example.org
RECTO_OIDC_CLIENT_ID: "<client id>"
RECTO_OIDC_SCOPES: urn:zitadel:iam:org:projects:roles
RECTO_OIDC_REQUIRED_CLAIM: urn:zitadel:iam:org:project:roles
RECTO_OIDC_REQUIRED_VALUE: recto
```

## Printers (CUPS)

recto is a client of a CUPS server. The compose file runs one (`cups`), configured by
`examples/cups/cupsd.conf` and `examples/cups/cups-files.conf`:

- The only administrator is root over the local socket (`SystemGroup root`). The default `print`
  user of the base image is disabled.
- Only the compose network, 172.30.80.0/24, may print. If that subnet clashes with yours, change it
  in both `compose.yml` and `cupsd.conf`.
- Port 631 is not published, on purpose: it would expose printing to whoever can reach it. If you
  must, bind it to a trusted address, such as `127.0.0.1:631:631`.
- The CUPS web interface is off. Add printers with `lpadmin`, as in the quick start. The printers
  live in the `cups-config` volume.

A printer attached by USB needs its device node on the `cups` service (never `privileged`):

```yaml
    devices:
      - /dev/bus/usb:/dev/bus/usb
```

### Using a CUPS server on the host

Set `RECTO_CUPS_URL` to it (for example `http://host.docker.internal:631`, with an `extra_hosts`
entry, or the host's address) and remove the `cups` service. In its `cupsd.conf`:

- `ServerAlias *` (or the name recto uses), or CUPS answers recto with 400 because of the Host
  header.
- `JobPrivateValues none` (also `SubscriptionPrivateValues none` and `JobPrivateAccess all` in the
  policy), because recto lists and cancels the jobs of every user and needs to read their owners.
  Cancelling fails with CUPS's default.
- `Allow from` the address recto connects from, in the `<Location />` block.

Anyone signed in to recto can see and cancel any job. recto assumes a household that trusts itself.

## Scanners (SANE)

recto uses libsane inside its container. Open the Scan page and press **Refresh** to list the
scanners. To check from the shell:

```sh
docker compose exec recto scanimage -L
```

- **Network scanners with eSCL (AirScan)**, which most recent Brother, Canon, Epson and HP models
  speak. Discovery uses mDNS, which does not cross Docker's bridge network, so list the scanner in
  `examples/sane/airscan.conf` under `[devices]` and mount that file (and `dll.conf`, which enables
  only the backends recto needs) as the commented lines of `compose.yml` show. The alternative is
  `network_mode: host` on the recto service, with its trade-offs: no network isolation, and the
  `ports` and `RECTO_CUPS_URL: http://cups:631` settings must change.
- **Scanners behind `saned`** on another machine: list the host in `examples/sane/net.conf`, and
  mount it.
- **USB scanners**: pass the device with `devices: [/dev/bus/usb:/dev/bus/usb]` on the recto
  service. The container runs as uid 65532, which must be able to open the device (a udev rule or
  a group on the host). Never use `privileged`.

If the feeder jams in the middle of a batch, the pages read so far are dropped and the error is
shown: re-feed the stack and scan again.

Scans are kept for 1 hour for download or save, then deleted. Only the user who scanned can reach a
result.

## Save into Paperless

Mount the folder as `RECTO_SCAN_OUTPUT_DIR`, as `compose.yml` does with `./consume:/consume`. recto
runs as uid 65532, so the folder must be writable by it:

```sh
sudo chown 65532:65532 consume
```

To write into a folder that someone else owns, such as the consume folder of Paperless, set
`user: "<uid>:<gid>"` on the recto service to that owner instead.

Files never overwrite an existing file. Normally they are written to a hidden file, then moved in
atomically by a rename that refuses to replace, which every folder watcher notices. Where the
filesystem does not support that, recto uses a hard link, and on SMB and NFS shares that refuse
both, it writes the file directly, so it may be briefly partial; Paperless copes with that.

## Limits

Uploads over `RECTO_MAX_UPLOAD_SIZE` are refused in the browser. Without JavaScript, some browsers
show a connection reset instead of the error message.

Each scanned page is compressed as soon as it is read, so a feeder batch takes little memory. A
single page may take at most 300 MB before compression (an A4 page in colour at about 1000 dpi);
a larger scan is refused with a message to lower the resolution or select a smaller area.

## Security

The image runs as a non-root user (65532), and `compose.yml` sets a read-only root filesystem,
drops all capabilities and sets `no-new-privileges`. CUPS and SANE access are kept as narrow as
shown above. recto does not convert Office documents in this version. If that is added, it will run
in a separate unprivileged container with no network, never `privileged`.

## Development

You need [mise](https://mise.jdx.dev), plus `libsane-dev` and libclang for the libsane bindings
(`apt install libsane-dev libclang-dev pkg-config`).

```sh
mise install
mise run dev         # http://127.0.0.1:3000, signed in as a dev user, with fake printers and scanners
mise run check       # lint, rustdoc, tests, cargo-deny: what CI runs
mise run test-sane   # scanner tests against libsane's virtual test backend
```

`mise run dev` sets `RECTO_DEV_MOCKS=1`: the mocks sign every request in and exist in debug builds
only, so never expose that server. If a build complains about
the `wasm-bindgen` version, a stale `wasm-bindgen` in `~/.cargo/bin` may be shadowing the one mise
pins: remove it.

Releases are cut by pushing a `vX.Y.Z` tag, which publishes the image. See `AGENTS.md` for the
layout of the code.

## Licence

[Apache-2.0](LICENSE).
