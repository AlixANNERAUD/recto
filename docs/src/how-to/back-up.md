# Back up and restore

recto's database holds its users, password hashes, groups and sessions: back it up. It is the
SQLite file `/data/recto.db` in the `recto-data` volume, or your PostgreSQL database. Treat the
backups as secrets: whoever reads one can try the password hashes offline, and use the sessions
still valid when it was taken.

Nothing else in recto needs a backup. Scans are kept for an hour only, and saved scans belong to
the folder they were saved to. Your printers live in CUPS, in the `cups-config` volume; adding
them again with `lpadmin` is usually quicker than backing it up.

## Take a backup of SQLite

Do not copy `recto.db` while recto runs: SQLite keeps recent changes in `recto.db-wal` next to it,
so the copy can be inconsistent. `recto backup <FILE>` writes a consistent copy of the live
database, without stopping recto. It refuses to overwrite an existing file.

The container's file system is read-only, so give it a folder for the backups, owned by its
user (65532). Add a volume to the `recto` service in `compose.yml`:

```yaml
    volumes:
      - recto-data:/data
      - ./backups:/backups
```

```sh
mkdir -p backups && sudo chown 65532:65532 backups
docker compose up -d --wait
```

Then, by hand or daily from cron on the host (where `$(date +%F)` is expanded):

```sh
docker compose exec -T recto /app/recto backup "/backups/recto-$(date +%F).db"
```

Rotate the files in `backups/` and copy them off the machine with your usual tools. A backup is an
ordinary SQLite database: `sqlite3 backups/recto-2026-11-15.db 'PRAGMA integrity_check'` checks
it.

## Restore a SQLite backup

1. Stop recto.
2. Replace the database with the backup, and delete the `-wal` and `-shm` files next to it, which
   belong to the old database.
3. Start recto. If the backup comes from an older version, its migrations run at startup.

With Compose, a one-off container of the recto image does the copy, as recto's user:

```sh
docker compose stop recto
docker compose run --rm --no-deps --entrypoint sh recto -c '
  cp /backups/recto-2026-11-15.db /data/recto.db &&
  rm -f /data/recto.db-wal /data/recto.db-shm'
docker compose start recto
```

People who signed in after the backup was taken sign in again. Accounts created or changed since
then are lost.

## Back up and restore PostgreSQL

`recto backup` copies SQLite only; on PostgreSQL it stops with a message pointing to `pg_dump`.
`pg_dump` writes a consistent snapshot of the live database, without stopping recto, and
`--format=custom` makes it compressed and ready for `pg_restore`. With
[`examples/compose-postgres.yml`](postgresql.md#with-docker-compose) in `COMPOSE_FILE`:

```sh
mkdir -p backups
docker compose exec -T postgres \
  pg_dump --username recto --format=custom recto > "backups/recto-$(date +%F).dump"
```

Against another server, `pg_dump --format=custom --file recto.dump "$RECTO_DATABASE_URL"` does the
same, or use the snapshots of a managed database.

To restore:

1. Stop recto:

   ```sh
   docker compose stop recto
   ```

2. Empty the database. `pg_restore --clean` only drops what is in the dump, which would leave the
   tables of newer migrations behind after an upgrade, so drop the whole schema:

   ```sh
   docker compose exec -T postgres psql --username recto --dbname recto \
     --command 'DROP SCHEMA public CASCADE; CREATE SCHEMA public;'
   ```

3. Restore the dump:

   ```sh
   docker compose exec -T postgres pg_restore --username recto --dbname recto --no-owner \
     < backups/recto-2026-11-15.dump
   ```

4. Start recto. If the dump comes from an older version, its migrations run at startup.

   ```sh
   docker compose start recto
   ```
