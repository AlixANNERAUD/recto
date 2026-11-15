# Command line

The image runs `recto serve`. The binary is `/app/recto` in the image; with Compose, run the
other commands in the running container, such as
`docker compose exec recto /app/recto migrate`.

```text
recto <COMMAND>

Commands:
  serve                 Run the web server
  healthcheck           Exit 0 when a running server answers `/health`; for container health checks
  migrate               Apply the pending database migrations and exit
  backup                Copy the SQLite database to a new file (use pg_dump for PostgreSQL)
  create-administrator  Create a local administrator; the password is read from standard input
  administrator         Grant or remove administration of an existing user
  help                  Print this message or the help of the given subcommand(s)

Options:
  -h, --help     Print help
  -V, --version  Print version
```

Every command but `healthcheck` works on the database of `RECTO_DATABASE_URL`, or of its
`--database-url` option (default `sqlite:///data/recto.db`). The commands other than `serve` exit
`0` on success and `1` with a message on standard error otherwise.

## recto serve

Runs the web server until it receives `SIGINT` or `SIGTERM`, then finishes the requests in
progress and exits. It applies the pending migrations before it starts listening.

Every option has an environment variable, which [Configuration](configuration.md) describes. An
option given on the command line wins over its variable.

| Option | Variable |
| --- | --- |
| `--public-url <URL>` | `RECTO_PUBLIC_URL` |
| `--database-url <URL>` | `RECTO_DATABASE_URL` |
| `--local-accounts <true\|false>` | `RECTO_LOCAL_ACCOUNTS` |
| `--password-minimum-length <LENGTH>` | `RECTO_PASSWORD_MINIMUM_LENGTH` |
| `--trust-forwarded-headers <true\|false>` | `RECTO_TRUST_FORWARDED_HEADERS` |
| `--oidc-issuer <URL>` | `RECTO_OIDC_ISSUER` |
| `--oidc-client-id <ID>` | `RECTO_OIDC_CLIENT_ID` |
| `--oidc-scopes <SCOPES>` | `RECTO_OIDC_SCOPES` |
| `--oidc-required-claim <CLAIM>` | `RECTO_OIDC_REQUIRED_CLAIM` |
| `--oidc-required-value <VALUE>` | `RECTO_OIDC_REQUIRED_VALUE` |
| `--oidc-administrator-claim <CLAIM>` | `RECTO_OIDC_ADMINISTRATOR_CLAIM` |
| `--oidc-administrator-value <VALUE>` | `RECTO_OIDC_ADMINISTRATOR_VALUE` |
| `--oidc-groups-claim <CLAIM>` | `RECTO_OIDC_GROUPS_CLAIM` |
| `--cups-url <URL>` | `RECTO_CUPS_URL` |
| `--scan-output-dir <DIR>` | `RECTO_SCAN_OUTPUT_DIR` |
| `--max-upload-size <BYTES>` | `RECTO_MAX_UPLOAD_SIZE` |
| `--listen-address <ADDRESS>` | `RECTO_LISTEN_ADDRESS` |
| `--site-root <DIR>` | `RECTO_SITE_ROOT` |
| `--log-format <json\|text>` | `RECTO_LOG_FORMAT` |

Exit status: `0` after a clean shutdown, `1` when it cannot start (the database cannot be opened,
for example) or stops on an error, `2` for invalid settings (with a message naming the setting).

## recto healthcheck

Asks the server on this machine for `/health` and exits `0` if it answers with success within 5
seconds, `1` otherwise. The image's `HEALTHCHECK` runs it.

| Option | Variable | Default |
| --- | --- | --- |
| `--listen-address <ADDRESS>` | `RECTO_LISTEN_ADDRESS` | `0.0.0.0:8080` |

A server listening on every interface (`0.0.0.0` or `::`) is reached over loopback.

## recto migrate

Applies the pending migrations to the database, creating a SQLite file if missing, then exits.
`recto serve` does the same at startup; this command is for deployments that want migrations as
a separate step.

## recto backup

```text
recto backup [--database-url <URL>] <FILE>
```

Writes a consistent copy of the SQLite database to `FILE`, while recto runs. It refuses a `FILE`
that already exists, a SQLite database that does not exist, and a PostgreSQL database, which
`pg_dump` backs up instead. The folder of `FILE` must exist and be writable by the user recto runs
as. See [Back up and restore](../how-to/back-up.md).

## recto create-administrator

```text
recto create-administrator --username <USERNAME> [--display-name <NAME>] [--email <EMAIL>]
```

Creates a local account that is an administrator. The username follows the rules of
[local accounts](accounts.md#local-accounts); the display name defaults to it.

On a terminal, the command asks for the password twice, without showing it. Otherwise it reads
one line of standard input, so a script can pipe the password in:

```sh
printf '%s\n' "$PASSWORD" | recto create-administrator --username admin
```

| Option | Variable | Default |
| --- | --- | --- |
| `--password-minimum-length <LENGTH>` | `RECTO_PASSWORD_MINIMUM_LENGTH` | `12` |

It creates the database if it does not exist yet, so it can run before the first start of the
server.

## recto administrator

```text
recto administrator [--remove] <USERNAME>
```

Makes an existing user an administrator, or with `--remove` takes administration away. Unlike the
Users page, it refuses nothing: it can remove the last administrator, and administration that
came from the identity provider (which the provider's claim may grant again at the next sign-in).
It refuses a SQLite database that does not exist.
