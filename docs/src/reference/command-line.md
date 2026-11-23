# Command line

The image runs `recto serve`. The binary is `/app/recto` in the image; with Compose, run the
other commands in the running container, such as
`docker compose exec recto /app/recto migrate`.

```text
Self-hosted web app to print and scan, with local accounts or OpenID Connect sign-in.

Usage: recto [OPTIONS] <COMMAND>

Commands:
  serve                 Run the web server
  healthcheck           Exit 0 when a running server answers `/health`; for container health checks
  migrate               Apply the pending database migrations and exit
  backup                Copy the SQLite database to a new file (use pg_dump for PostgreSQL)
  create-administrator  Create a local administrator; the password is asked twice on a terminal, otherwise read from one line of standard input
  administrator         Grant or remove administration of an existing user
  dnssd-services        Write the DNS-SD records of the LAN listener as Avahi service files, for a host whose avahi-daemon announces recto while it stays on a bridge network

Options:
  -h, --help     Print help
  -V, --version  Print version
```

Every command but `healthcheck` works on the database of `RECTO_DATABASE_URL`, or of its
`--database-url` option (default `sqlite:///data/recto.db`). The commands other than `serve` exit
`0` on success and `1` with a message on standard error otherwise. `recto <COMMAND> --help` lists
the options of a command; a wrong command line exits `2`.

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
| `--os-integration <true\|false>` | `RECTO_OS_INTEGRATION` |
| `--ipp-allow-plain <true\|false>` | `RECTO_IPP_ALLOW_PLAIN` |
| `--data-dir <DIR>` | `RECTO_DATA_DIR` |
| `--held-job-hours <HOURS>` | `RECTO_HELD_JOB_HOURS` |
| `--default-media <MEDIA>` | `RECTO_DEFAULT_MEDIA` |
| `--escl-open-networks <CIDRS>` | `RECTO_ESCL_OPEN_NETWORKS` |
| `--oidc-issuer <URL>` | `RECTO_OIDC_ISSUER` |
| `--oidc-client-id <ID>` | `RECTO_OIDC_CLIENT_ID` |
| `--oidc-scopes <SCOPES>` | `RECTO_OIDC_SCOPES` |
| `--oidc-required-claim <CLAIM>` | `RECTO_OIDC_REQUIRED_CLAIM` |
| `--oidc-required-value <VALUE>` | `RECTO_OIDC_REQUIRED_VALUE` |
| `--oidc-administrator-claim <CLAIM>` | `RECTO_OIDC_ADMINISTRATOR_CLAIM` |
| `--oidc-administrator-value <VALUE>` | `RECTO_OIDC_ADMINISTRATOR_VALUE` |
| `--oidc-groups-claim <CLAIM>` | `RECTO_OIDC_GROUPS_CLAIM` |
| `--lan-listen-address <ADDRESS>` | `RECTO_LAN_LISTEN_ADDRESS` |
| `--lan-tls-certificate <FILE>` | `RECTO_LAN_TLS_CERTIFICATE` |
| `--lan-tls-key <FILE>` | `RECTO_LAN_TLS_KEY` |
| `--dnssd <true\|false>` | `RECTO_DNSSD` |
| `--dnssd-name <NAME>` | `RECTO_DNSSD_NAME` |
| `--dnssd-interfaces <NAMES>` | `RECTO_DNSSD_INTERFACES` |
| `--cups-url <URL>` | `RECTO_CUPS_URL` |
| `--cups-admin-socket <FILE>` | `RECTO_CUPS_ADMIN_SOCKET` |
| `--scan-output-dir <DIR>` | `RECTO_SCAN_OUTPUT_DIR` |
| `--max-upload-size <BYTES>` | `RECTO_MAX_UPLOAD_SIZE` |
| `--listen-address <ADDRESS>` | `RECTO_LISTEN_ADDRESS` |
| `--site-root <DIR>` | `RECTO_SITE_ROOT` |
| `--job-visibility <all\|own>` | `RECTO_JOB_VISIBILITY` |
| `--currency <CODE>` | `RECTO_CURRENCY` |
| `--timezone <ZONE>` | `RECTO_TIMEZONE` |
| `--log-format <json\|text>` | `RECTO_LOG_FORMAT` |
| `--brand-name <NAME>` | `RECTO_BRAND_NAME` |
| `--brand-short-name <NAME>` | `RECTO_BRAND_SHORT_NAME` |
| `--brand-color <COLOR>` | `RECTO_BRAND_COLOR` |
| `--brand-logo <FILE>` | `RECTO_BRAND_LOGO` |
| `--brand-icon <FILE>` | `RECTO_BRAND_ICON` |

Exit status: `0` after a clean shutdown, `1` when it cannot start (a brand setting or file is
refused, the database cannot be opened, the LAN listener's certificate cannot be read or made, or
its address or a DNS-SD interface cannot be used) or stops on an error, `2` for invalid settings (with
a message naming the setting).

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
one line of standard input, so a script can pipe the password in; an empty standard input is
refused:

```sh
printf '%s\n' "$PASSWORD" | recto create-administrator --username admin
```

| Option | Variable | Default |
| --- | --- | --- |
| `--password-minimum-length <LENGTH>` | `RECTO_PASSWORD_MINIMUM_LENGTH` | `12`, from 1 to 1,024 |

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

## recto dnssd-services

```text
recto dnssd-services [--port <PORT>] <DIR>
```

Writes what `RECTO_DNSSD` would announce as Avahi service files into `DIR`, created if missing:
`recto-printer.service`, and `recto-scanner-<scanner>.service` for each scanner open to everyone,
whose capabilities it reads through SANE as the server does. It removes the
`recto-scanner-*.service` files of scanners no longer announced, and leaves other files alone.
The host's avahi-daemon then announces recto from `/etc/avahi/services`, while recto stays on a
bridge network: see [Let devices find recto](../how-to/discovery.md#keep-the-bridge-network).
It prints the path of each file written.

The records name no host, so they point at the host's own name, and say the LAN listener is on
`PORT`: the port published on the host, `8631` by default. Run it again after adding a scanner or
changing who may use one.

| Option | Variable | Default |
| --- | --- | --- |
| `--port <PORT>` | | `8631` |
| `--public-url <URL>` | `RECTO_PUBLIC_URL` | required |
| `--dnssd-name <NAME>` | `RECTO_DNSSD_NAME` | the brand name |
| `--brand-name <NAME>` | `RECTO_BRAND_NAME` | `recto` |

It refuses a SQLite database that does not exist. Without SANE, it writes the printer's file
alone.
