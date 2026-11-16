# Upgrade

recto keeps its users, groups and sessions in its database, and applies the database's
migrations when it starts. Upgrading is replacing the container; sessions survive it, so nobody
is signed out.

Migrations only go forward. Take a [backup](back-up.md) first, so that you can go back to the
older version by restoring it.

## With the published image

```sh
git pull                  # newer compose.yml and example configuration
docker compose exec -T recto /app/recto backup "/backups/recto-$(date +%F).db"
docker compose pull
docker compose up -d --wait
```

The backup line needs the `/backups` folder of [Back up and restore](back-up.md); on PostgreSQL,
use `pg_dump` instead. With `RECTO_IMAGE` pinned to a version, change it in `.env` first. Read the
[releases](https://github.com/AlixANNERAUD/recto/releases) for changes that need you to act.

To run the migrations as a separate step, before starting the new version, use
`recto migrate` (see [Command line](../reference/command-line.md#recto-migrate)).

## With an image built from the checkout

```sh
git pull
docker compose up -d --wait --build
```

## From a version without accounts

Versions of recto before local accounts and administrators kept nothing on disk. recto now keeps
a database, which its first start after the upgrade creates. Three things change:

- **The database needs a volume.** The `compose.yml` of the repository mounts the `recto-data`
  volume at `/data`, where the SQLite database lives. If you run recto from your own Compose file
  or another tool, add a volume at `/data` before upgrading: with a read-only file system, recto
  cannot create its database and stops; without one, the database is lost with the container.
- **Everyone is signed out once.** The old sessions were in memory and do not survive the
  upgrade. Single sign-on users sign in again, and get their user then, with the same username as
  before, so their print jobs keep their owner. From then on, restarts and upgrades keep sessions.
- **The sign-in page can get a password form.** Local accounts are on by default, but as long as
  no account has a password, recto still sends everyone straight to your provider. Once an
  account has one (a local administrator, or a password set on the Users page), recto shows its
  sign-in page instead, with a **Sign in with single sign-on** button. Set
  `RECTO_LOCAL_ACCOUNTS: "false"` to keep going straight to the provider whatever happens (see
  [Set up single sign-on](sign-in.md#use-single-sign-on-only)).

Nobody is an administrator after the upgrade. Appoint one with `recto administrator <USERNAME>`
once they have signed in, or with the administrator claim (see
[Manage users and administrators](manage-users.md#create-the-first-administrator)).

## The CUPS server

The `cups` service is pinned to an image digest in `compose.yml`, which `git pull` updates. Your
printers live in the `cups-config` volume and survive the upgrade. Do not run
`docker compose down -v`: it deletes that volume, with your printers, and the `recto-data` volume,
with recto's database.

The `cups` and `recto` services share CUPS's socket through the `cups-socket` volume, and the
`cups` service creates the `recto` account that printer management uses. After an upgrade that
changes `compose.yml`, run `docker compose up -d`: it recreates both services, so they share the
volume again. Printer management stays off until then.
