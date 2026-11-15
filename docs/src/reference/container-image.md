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
| Base | `debian:trixie-slim`, pinned by digest |
| Packages | `libsane1`, `sane-airscan`, `sane-utils` (for `scanimage`), `ca-certificates` |
| Binary | `/app/recto` |
| Interface files | `/app/site` |
| Database folder | `/data`, owned by `65532:65532` |
| Entry point | `/app/recto`, command `serve` |
| User | `65532:65532`, not root |
| Port | `8080` |
| Health check | `/app/recto healthcheck` every 30 seconds, after a 20-second start period |

## Environment set by the image

| Variable | Value |
| --- | --- |
| `RECTO_LISTEN_ADDRESS` | `0.0.0.0:8080` |
| `RECTO_SITE_ROOT` | `/app/site` |

## Paths recto writes to

| Path | What |
| --- | --- |
| `/data` | The SQLite database, `recto.db` with its `-wal` and `-shm` files, unless `RECTO_DATABASE_URL` points elsewhere. Mount a volume here: `compose.yml` mounts `recto-data`. |
| `/tmp/recto-scans` | Scan results, deleted after an hour. In `compose.yml`, `/tmp` is a memory-backed `tmpfs`. |
| `RECTO_SCAN_OUTPUT_DIR` | Scans saved with **Save to folder**. |

Everything else can be read-only: `compose.yml` runs the container with `read_only: true`. A new
named volume mounted on `/data` takes the folder's owner, so recto can write to it; a folder of the
host mounted there must be writable by user 65532.

## SANE configuration

libsane reads its configuration from `/etc/sane.d`. Mount files over it to configure backends:

| File | Purpose |
| --- | --- |
| `/etc/sane.d/dll.conf` | The backends libsane loads. |
| `/etc/sane.d/airscan.conf` | eSCL and WSD scanners for sane-airscan. |
| `/etc/sane.d/net.conf` | Hosts running `saned`. |

The repository has examples of all three in `examples/sane/`.

## Labels

The image carries the OCI labels `org.opencontainers.image.title`, `description`, `source` and
`licenses`.
