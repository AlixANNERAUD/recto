# Upgrade

recto keeps nothing on disk besides the scans you saved, so upgrading is replacing the container.
Everyone is signed out, because sessions live in memory.

## With the published image

```sh
git pull                  # newer compose.yml and example configuration
docker compose pull
docker compose up -d --wait
```

With `RECTO_IMAGE` pinned to a version, change it in `.env` first. Read the
[releases](https://github.com/AlixANNERAUD/recto/releases) for changes that need you to act.

## With an image built from the checkout

```sh
git pull
docker compose up -d --wait --build
```

## The CUPS server

The `cups` service is pinned to an image digest in `compose.yml`, which `git pull` updates. Your
printers live in the `cups-config` volume and survive the upgrade. Do not run
`docker compose down -v`: it deletes that volume, and with it your printers.
