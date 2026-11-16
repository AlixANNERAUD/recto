# Configuration

recto reads its settings from environment variables only; there is no configuration file. Each
variable can also be given as a command-line option of `recto serve`: `RECTO_PUBLIC_URL` is
`--public-url`, and so on (see [Command line](command-line.md)).

An optional variable without a default, set to an empty value, counts as unset. A variable with
a default, such as `RECTO_LOCAL_ACCOUNTS`, must be left out rather than empty.

## Settings

| Variable | Default | Meaning |
| --- | --- | --- |
| `RECTO_PUBLIC_URL` | required | The address people open, such as `https://recto.example.org`. Sign-in redirects, the allowed origin of forms and the `Secure` flag of the session cookie come from it. It must be the root of a site: a path such as `/recto` is refused. |
| `RECTO_DATABASE_URL` | `sqlite:///data/recto.db` | The database. See [Database](#database). |
| `RECTO_LOCAL_ACCOUNTS` | `true` | `true` lets local accounts sign in with a password at `/sign-in`; `false` leaves single sign-on only, and needs it configured. |
| `RECTO_PASSWORD_MINIMUM_LENGTH` | `12` | The fewest characters a new password may have, from 1 to 1,024; another value is refused at startup and by `recto create-administrator`. A password has at most 1,024 characters. |
| `RECTO_TRUST_FORWARDED_HEADERS` | `false` | `true` counts password sign-in attempts by the address in `X-Forwarded-For`, `X-Real-IP` or `Forwarded`, rather than the connection's. Only behind a proxy that replaces these headers: see [Run behind a reverse proxy](../how-to/reverse-proxy.md#count-sign-in-attempts-per-person). |
| `RECTO_OIDC_ISSUER` | none | The issuer URL of the OpenID Connect provider. Set it with `RECTO_OIDC_CLIENT_ID`, or neither: one without the other is refused at startup. |
| `RECTO_OIDC_CLIENT_ID` | none | The client id of recto's public client in that provider. |
| `RECTO_OIDC_SCOPES` | none | Extra scopes, separated by spaces. recto always asks for `openid`, `profile` and `email`. |
| `RECTO_OIDC_REQUIRED_CLAIM` | none | Let in only people whose sign-in token has this claim. See [Required claim](#required-claim). |
| `RECTO_OIDC_REQUIRED_VALUE` | none | The value the required claim must hold. Setting it without `RECTO_OIDC_REQUIRED_CLAIM` is refused at startup. |
| `RECTO_OIDC_ADMINISTRATOR_CLAIM` | none | The claim that makes a single sign-on user an administrator. See [Administrator claim](accounts.md#administrator-claim). |
| `RECTO_OIDC_ADMINISTRATOR_VALUE` | none | The value the administrator claim must hold. Setting it without `RECTO_OIDC_ADMINISTRATOR_CLAIM` is refused at startup. |
| `RECTO_OIDC_GROUPS_CLAIM` | none | The claim holding a single sign-on user's groups. See [Groups](accounts.md#groups). |
| `RECTO_CUPS_URL` | `http://localhost:631` | The CUPS server. `ipp://` means `http://` and `ipps://` means `https://`. Without a port, the scheme's default (80 or 443) is used, not 631. |
| `RECTO_CUPS_ADMIN_SOCKET` | none | The Unix socket of CUPS, such as `/run/cups/cups.sock`. Lets administrators manage printers from recto. Without it, printer administration is off. recto authenticates to CUPS as `recto` over this socket, so CUPS needs a `recto` account whose uid is recto's, listed in its `SystemGroup`. `compose.yml` and `examples/cups/` set that up; see [Add a printer](../how-to/add-printer.md#add-a-printer-from-recto). |
| `RECTO_SCAN_OUTPUT_DIR` | none | The folder **Save to folder** writes into. Without it, scans can only be downloaded and the button is hidden. |
| `RECTO_MAX_UPLOAD_SIZE` | `52428800` (50 MiB) | The largest file accepted for printing, in bytes. |
| `RECTO_LISTEN_ADDRESS` | `0.0.0.0:8080` | The address and port recto listens on. |
| `RECTO_SITE_ROOT` | `/app/site` | The folder of the built interface. The image sets it. |
| `RECTO_JOB_VISIBILITY` | `all` | `all` shows every print job to everyone. `own` shows people only the jobs they submitted through recto, and administrators every job: see [Keep jobs private](../how-to/restrict-access.md#keep-jobs-private). |
| `RECTO_CURRENCY` | `EUR` | The ISO 4217 code of the currency costs are shown in, three letters. It only changes how amounts read: recto converts nothing. |
| `RECTO_TIMEZONE` | `UTC` | The IANA time zone name, such as `Europe/Paris`, whose calendar day, week and month the print periods follow. |
| `RECTO_LOG_FORMAT` | `json` | `json` for one JSON object per line, or `text`. |
| `RECTO_BRAND_NAME` | `recto` | The name in the header, the page titles, messages and the installed app. 1 to 64 characters once spaces around it are removed. See [Brand recto](../how-to/brand.md). |
| `RECTO_BRAND_SHORT_NAME` | the name | The installed app's label where space is short, such as under a home screen icon. 1 to 32 characters. |
| `RECTO_BRAND_COLOR` | `#0f6cbd` | The accent color, `#rrggbb` in either case. White text on it needs a contrast of at least 4.5:1, or recto refuses to start. |
| `RECTO_BRAND_LOGO` | none | A file shown before the name in the header and on the sign-in page instead of the printer glyph. PNG, JPEG or SVG, recognized by content, at most 1 MiB. |
| `RECTO_BRAND_ICON` | none | A file the app icons are made from. A square PNG or JPEG, from 512 by 512 to 4096 by 4096 pixels, at most 5 MiB. Without it, recto draws its icon in the accent color. |

At least one way to sign in is needed: `RECTO_LOCAL_ACCOUNTS=false` without an OpenID Connect
provider is refused at startup.

The `RECTO_BRAND_*` settings and files are read once, at startup, before the database is opened.
A value or file that breaks a rule above stops recto with a message naming the variable.

`RUST_LOG` sets which log lines are written; see [Logs and health](logs.md).

## Database

`RECTO_DATABASE_URL` takes one of two forms:

| Form | Database |
| --- | --- |
| `sqlite://<path>` | A SQLite file, created if missing. Three slashes for an absolute path: `sqlite:///data/recto.db`. |
| `postgres://<user>:<password>@<host>:<port>/<database>` | A PostgreSQL database, which must exist. `postgresql://` works too. |

recto applies the pending migrations of the schema at startup, before it accepts requests.

In a PostgreSQL URL, characters of the user or the password other than letters, digits and
`-._~` must be percent-encoded: `@` is `%40`, `:` is `%3A`, `/` is `%2F`, `%` is `%25`. Options
follow a `?`, such as `?sslmode=verify-full`, or `&sslrootcert=/path/to/ca.pem` after another
option.

## In compose.yml

`compose.yml` reads `RECTO_PUBLIC_URL`, and `RECTO_OIDC_ISSUER` and `RECTO_OIDC_CLIENT_ID` when
they are set, from `.env`. It sets `RECTO_CUPS_URL` and `RECTO_SCAN_OUTPUT_DIR` for its own `cups`
service and `./consume` folder, and mounts the `recto-data` volume at `/data` for the database.
More variables of `.env` are read by Compose itself, not by recto:

| Variable | Default | Meaning |
| --- | --- | --- |
| `RECTO_IMAGE` | `ghcr.io/alixanneraud/recto:latest` | The image to run. |
| `RECTO_PORT` | `8080` | The port published on the host. |
| `RECTO_UID` | `65532` | The uid and gid recto runs as, which must be able to write to the scan folder. The `cups` service gives its `recto` account the same uid, so printer management keeps working. See [Save scans into Paperless-ngx](../how-to/paperless.md#2-let-recto-write-there). |
| `RECTO_POSTGRES_PASSWORD` | required by `examples/compose-postgres.yml` | The password of its PostgreSQL database. |
| `COMPOSE_FILE` | `compose.yml` and `compose.override.yml` | The Compose files to use, separated by `:`, such as `compose.yml:examples/compose-postgres.yml`. Once it is set, `compose.override.yml` is only read if listed. |

## Required claim

With `RECTO_OIDC_REQUIRED_CLAIM` set, recto checks the claims of the ID token, completed by the
UserInfo endpoint when the claim is missing from the token, at each single sign-on. If the
UserInfo request fails, whatever the reason, the sign-in stops with **Sign-in provider
unavailable. Try again in a moment.** and a **Try again** button, rather than refusing access.
The administrator claim is matched the same way.

The claim is looked up by its whole name first, then as a path of names separated by dots into
nested objects: `realm_access.roles` reads `roles` inside `realm_access`.

| Claim holds | Without a required value | With a required value |
| --- | --- | --- |
| A string | allowed | allowed if equal to the value |
| A list | allowed | allowed if it contains the value |
| An object | allowed | allowed if it has the value as a key |
| `null` or `false` | refused | refused |
| Anything else | allowed | refused |
| Nothing (missing) | refused | refused |

## Fixed values

These are not configurable:

| What | Value |
| --- | --- |
| Session lifetime without a request | 12 hours |
| Time to finish a sign-in at the provider | 10 minutes |
| Password attempts per client address (IPv6: per `/64`) | 5 at once, then one every 4 seconds |
| Longest password | 1,024 characters |
| Connections to the database | at most 8 |
| Lifetime of a scan result | 1 hour |
| Largest scanned page, before compression | 300 MB |
| Timeout of a request to CUPS | 10 seconds; longer for sending a file |
| Timeout of a request to the identity provider | 10 seconds |
