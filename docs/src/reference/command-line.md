# Command line

The image runs `recto serve`. The binary is `/app/recto` in the image.

```text
recto <COMMAND>

Commands:
  serve        Run the web server
  healthcheck  Exit 0 when a running server answers `/health`; for container health checks
  help         Print this message or the help of the given subcommand(s)

Options:
  -h, --help     Print help
  -V, --version  Print version
```

## recto serve

Runs the web server until it receives `SIGINT` or `SIGTERM`, then finishes the requests in
progress and exits.

Every option has an environment variable, which [Configuration](configuration.md) describes. An
option given on the command line wins over its variable.

| Option | Variable |
| --- | --- |
| `--public-url <URL>` | `RECTO_PUBLIC_URL` |
| `--oidc-issuer <URL>` | `RECTO_OIDC_ISSUER` |
| `--oidc-client-id <ID>` | `RECTO_OIDC_CLIENT_ID` |
| `--oidc-scopes <SCOPES>` | `RECTO_OIDC_SCOPES` |
| `--oidc-required-claim <CLAIM>` | `RECTO_OIDC_REQUIRED_CLAIM` |
| `--oidc-required-value <VALUE>` | `RECTO_OIDC_REQUIRED_VALUE` |
| `--cups-url <URL>` | `RECTO_CUPS_URL` |
| `--scan-output-dir <DIR>` | `RECTO_SCAN_OUTPUT_DIR` |
| `--max-upload-size <BYTES>` | `RECTO_MAX_UPLOAD_SIZE` |
| `--listen-address <ADDRESS>` | `RECTO_LISTEN_ADDRESS` |
| `--site-root <DIR>` | `RECTO_SITE_ROOT` |
| `--log-format <json\|text>` | `RECTO_LOG_FORMAT` |

Exit status: `0` after a clean shutdown, `1` when it cannot start or stops on an error, `2` for
invalid settings (with a message naming the setting).

## recto healthcheck

Asks the server on this machine for `/health` and exits `0` if it answers with success within 5
seconds, `1` otherwise. The image's `HEALTHCHECK` runs it.

| Option | Variable | Default |
| --- | --- | --- |
| `--listen-address <ADDRESS>` | `RECTO_LISTEN_ADDRESS` | `0.0.0.0:8080` |

A server listening on every interface (`0.0.0.0` or `::`) is reached over loopback.
