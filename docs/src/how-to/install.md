# Install recto

This guide runs recto for good with Docker Compose, next to its own CUPS server. You need Docker
with Compose, and an OpenID Connect application for recto (see [Set up sign-in](sign-in.md)).

## Get the files

The repository holds the Compose file and the CUPS and SANE configuration it mounts:

```sh
git clone https://github.com/AlixANNERAUD/recto.git
cd recto
```

## Create the scan folder

The **Save to folder** button writes into `./consume`, mounted at `/consume`. recto runs as user
65532, which must be able to write there:

```sh
mkdir -p consume && sudo chown 65532:65532 consume
```

To save into a folder owned by someone else, such as the consume folder of Paperless-ngx, see
[Save scans into Paperless-ngx](paperless.md). To have no Save button at all, remove
`RECTO_SCAN_OUTPUT_DIR` and the `./consume` volume from `compose.yml`.

## Configure

Put the three required settings in a `.env` file next to `compose.yml`:

```sh
RECTO_PUBLIC_URL=https://recto.example.org
RECTO_OIDC_ISSUER=https://id.example.org
RECTO_OIDC_CLIENT_ID=your-client-id
```

`RECTO_PUBLIC_URL` is the address people open, through your reverse proxy (see
[Run behind a reverse proxy](reverse-proxy.md)). Sign-in only works through that address.

Other settings go under `environment` in `compose.yml`; [Configuration](../reference/configuration.md)
lists them all. recto publishes port 8080; change it with `RECTO_PORT` in `.env`.

## Start

```sh
docker compose up -d --wait
```

`--wait` returns once both containers are running and recto reports itself healthy.

Compose uses the image `ghcr.io/alixanneraud/recto:latest`. To pin a version, set
`RECTO_IMAGE=ghcr.io/alixanneraud/recto:0.1` in `.env` (see
[Container image](../reference/container-image.md) for the tags). To build the image from your
checkout instead, for example to run your own changes or before the first release exists:

```sh
docker compose up -d --wait --build
```

## Add printers and scanners

CUPS starts with no printers, and recto finds no scanner until SANE is told where to look:

- [Add a printer](add-printer.md)
- [Add a scanner](add-scanner.md)

## Check

Open your public address and sign in. The **Printers** page lists the printers CUPS knows, and
the **Scan** page the scanners SANE finds. If something is missing, see
[Troubleshoot](troubleshoot.md).
