# Run on PostgreSQL

recto keeps its database in a SQLite file by default, which needs nothing more. It can use a
PostgreSQL database instead, for example a server you already run and back up (see
[SQLite or PostgreSQL](../explanation/architecture.md#sqlite-or-postgresql)). Decide before the
first start: recto has no tool to move its data from one database to the other.

recto is tested on PostgreSQL 17, and needs a database and a role that owns it.

## With Docker Compose

[`examples/compose-postgres.yml`](https://github.com/AlixANNERAUD/recto/blob/main/examples/compose-postgres.yml)
adds a `postgres` container to `compose.yml`. Set the database password, and name both files in
`COMPOSE_FILE` so that every `docker compose` command uses them, in the `.env` file:

```sh
RECTO_PUBLIC_URL=https://recto.example.org
RECTO_POSTGRES_PASSWORD=a-long-random-password
COMPOSE_FILE=compose.yml:examples/compose-postgres.yml
```

The password is part of a URL: keep it to letters, digits and `-._~`. Then start recto and create
the first administrator as usual:

```sh
docker compose up -d --wait
docker compose exec recto /app/recto create-administrator --username admin
```

PostgreSQL has a health check, and recto starts once it passes, since it applies its migrations
at startup. The data lives in the `postgres-data` volume.

## With an existing PostgreSQL server

1. Create a role and a database it owns, so that recto can create its tables:

   ```sql
   CREATE ROLE recto LOGIN PASSWORD 'a-long-random-password';
   CREATE DATABASE recto OWNER recto;
   ```

2. Point recto at it, under `environment` in `compose.yml`:

   ```yaml
   RECTO_DATABASE_URL: postgres://recto:a-long-random-password@db.example.org:5432/recto?sslmode=verify-full
   ```

   `sslmode=verify-full` encrypts the connection and checks the server's certificate and name.
   Add `sslrootcert=/path/to/ca.pem`, mounted into the container, when the certificate comes from
   a private authority. On a private network you trust, leave `sslmode` out. Characters of the
   password other than letters, digits and `-._~` must be
   [percent-encoded](../reference/configuration.md#database).

3. Start recto: it creates the schema at the first start, and applies new migrations on later
   ones. Then [create the first administrator](install.md#create-the-first-administrator).

recto opens up to 8 connections to the database.

## Back up

`recto backup` copies SQLite only. Use PostgreSQL's tools: see
[Back up and restore PostgreSQL](back-up.md#back-up-and-restore-postgresql).
