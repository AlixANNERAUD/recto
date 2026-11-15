# Configuration

recto reads its settings from environment variables only; there is no configuration file. Each
variable can also be given as a command-line option of `recto serve`: `RECTO_PUBLIC_URL` is
`--public-url`, and so on (see [Command line](command-line.md)).

An optional variable set to an empty value counts as unset.

## Settings

| Variable | Default | Meaning |
| --- | --- | --- |
| `RECTO_PUBLIC_URL` | required | The address people open, such as `https://recto.example.org`. Sign-in redirects, the allowed origin of forms and the `Secure` flag of the session cookie come from it. It must be the root of a site: a path such as `/recto` is refused. |
| `RECTO_OIDC_ISSUER` | required | The issuer URL of the OpenID Connect provider. |
| `RECTO_OIDC_CLIENT_ID` | required | The client id of recto's public client in that provider. |
| `RECTO_OIDC_SCOPES` | none | Extra scopes, separated by spaces. recto always asks for `openid`, `profile` and `email`. |
| `RECTO_OIDC_REQUIRED_CLAIM` | none | Let in only people whose sign-in token has this claim. See [Required claim](#required-claim). |
| `RECTO_OIDC_REQUIRED_VALUE` | none | The value the required claim must hold. Setting it without `RECTO_OIDC_REQUIRED_CLAIM` is refused at startup. |
| `RECTO_CUPS_URL` | `http://localhost:631` | The CUPS server. `ipp://` means `http://` and `ipps://` means `https://`. Without a port, the scheme's default (80 or 443) is used, not 631. |
| `RECTO_SCAN_OUTPUT_DIR` | none | The folder **Save to folder** writes into. Without it, scans can only be downloaded and the button is hidden. |
| `RECTO_MAX_UPLOAD_SIZE` | `52428800` (50 MiB) | The largest file accepted for printing, in bytes. |
| `RECTO_LISTEN_ADDRESS` | `0.0.0.0:8080` | The address and port recto listens on. |
| `RECTO_SITE_ROOT` | `/app/site` | The folder of the built interface. The image sets it. |
| `RECTO_LOG_FORMAT` | `json` | `json` for one JSON object per line, or `text`. |

`RUST_LOG` sets which log lines are written; see [Logs and health](logs.md).

## In compose.yml

`compose.yml` reads `RECTO_PUBLIC_URL`, `RECTO_OIDC_ISSUER` and `RECTO_OIDC_CLIENT_ID` from `.env`
and sets `RECTO_CUPS_URL` and `RECTO_SCAN_OUTPUT_DIR` for its own `cups` service and `./consume`
folder. Two more variables of `.env` are read by Compose itself, not by recto:

| Variable | Default | Meaning |
| --- | --- | --- |
| `RECTO_IMAGE` | `ghcr.io/alixanneraud/recto:latest` | The image to run. |
| `RECTO_PORT` | `8080` | The port published on the host. |

## Required claim

With `RECTO_OIDC_REQUIRED_CLAIM` set, recto checks the claims of the ID token, completed by the
UserInfo endpoint when the claim is missing from the token, at each sign-in.

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
| Lifetime of a scan result | 1 hour |
| Largest scanned page, before compression | 300 MB |
| Timeout of a request to CUPS | 10 seconds; longer for sending a file |
| Timeout of a request to the identity provider | 10 seconds |
