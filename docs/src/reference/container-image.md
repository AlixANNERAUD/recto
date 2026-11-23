# Container image

## Name and tags

The image is published to the GitHub Container Registry as `ghcr.io/alixanneraud/recto`, for
`linux/amd64` and `linux/arm64`, when a version tag `vX.Y.Z` is pushed.

| Tag | Points to |
| --- | --- |
| `X.Y.Z` | That release. |
| `X.Y` | The latest release of `X.Y`. |
| `X` | The latest release of `X`. |
| `latest` | The latest release. |

## Contents

| What | Value |
| --- | --- |
| Base | `gcr.io/distroless/cc-debian13:nonroot` (Debian 13: glibc, OpenSSL, CA certificates), pinned by digest |
| SANE | Debian 13's `libsane1` with every backend, `sane-airscan`, `scanimage` from `sane-utils`, and the libraries and plugins they load |
| Binary | `/app/recto` |
| Interface files | `/app/site` |
| Database folder | `/data`, owned by `65532:65532` |
| Entry point | `/app/recto`, command `serve` |
| User | `65532:65532`, not root |
| Ports | `8080`, and the LAN listener's when `RECTO_LAN_LISTEN_ADDRESS` is set, such as `8631` |
| Health check | `/app/recto healthcheck` every 30 seconds, after a 20-second start period |
| Size | About 57 MB to download and 150 MB unpacked on `arm64`, less on `amd64` |

The SANE packages are installed in a Debian 13 build stage, and only the files they load at run
time are copied into the image, so their packages' other dependencies (Perl, systemd) stay out.
The build fails unless every backend loads in the result.

## No shell

The image has no shell, package manager or other tools: only `/app/recto` and `scanimage` run in
it, for example `docker compose exec recto scanimage -L`. For anything else, start a throwaway
container next to recto's: with `--network container:<recto>` it shares recto's network, with
`--volumes-from <recto>` its volumes. [Troubleshoot](../how-to/troubleshoot.md) and
[Back up and restore](../how-to/back-up.md#restore-a-sqlite-backup) have examples.

## Environment set by the image

| Variable | Value |
| --- | --- |
| `RECTO_LISTEN_ADDRESS` | `0.0.0.0:8080` |
| `RECTO_SITE_ROOT` | `/app/site` |

## Paths recto writes to

| Path | What |
| --- | --- |
| `/data` | The SQLite database, `recto.db` with its `-wal` and `-shm` files, unless `RECTO_DATABASE_URL` points elsewhere. Mount a volume here: `compose.yml` mounts `recto-data`. |
| `/data/held` | Documents sent from computers and phones, until they are released, deleted or expire (`RECTO_HELD_JOB_HOURS`). Readable by recto alone. `RECTO_DATA_DIR` moves it with `/data`. |
| `/data/lan-tls` | The LAN listener's self-signed certificate and key, made at its first start and kept, since devices remember the certificate. Readable by recto alone. Not written with `RECTO_LAN_TLS_CERTIFICATE` and `RECTO_LAN_TLS_KEY`. |
| `/tmp/recto-scans-*` | Scan results, deleted after an hour, in a directory named at random when recto starts and readable by recto alone. In `compose.yml`, `/tmp` is a memory-backed `tmpfs`. |
| `RECTO_SCAN_OUTPUT_DIR` | Scans saved with **Save to folder**. |

Everything else can be read-only: `compose.yml` runs the container with `read_only: true`. A new
named volume mounted on `/data` takes the folder's owner, so recto can write to it; a folder of the
host mounted there must be writable by user 65532.

## Host networking

DNS-SD (`RECTO_DNSSD`) needs the container on the host's network, `network_mode: host`, since
multicast does not cross Docker's bridge: `examples/compose-dnssd.yml` does that. The image needs
nothing more: the LAN listener's port and mDNS's 5353 are above 1024, so it still runs as
`65532` with every capability dropped and a read-only root. recto's responder shares port 5353
with the host's avahi-daemon. Docker Desktop on macOS and Windows has no host networking on the
LAN; see [Let devices find recto](../how-to/discovery.md).

## SANE configuration

libsane reads its configuration from `/etc/sane.d`. Mount files over it to configure backends:

| File | Purpose |
| --- | --- |
| `/etc/sane.d/dll.conf` | The backends libsane loads. |
| `/etc/sane.d/airscan.conf` | eSCL and WSD scanners for sane-airscan. |
| `/etc/sane.d/net.conf` | Hosts running `saned`. |

The repository has examples of all three in `examples/sane/`. The image also ships
`/etc/sane.d/airscan.d/recto.conf`, which keeps recto's own eSCL scanners out of sane-airscan's
discovery; sane-airscan reads it after `airscan.conf`, so mounting your own `airscan.conf` keeps
it.

## Labels

The image carries the OCI labels `org.opencontainers.image.title`, `description`, `source` and
`licenses`.
