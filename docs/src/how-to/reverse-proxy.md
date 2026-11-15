# Run behind a reverse proxy

recto serves plain HTTP. Put a reverse proxy with TLS in front of it, and set `RECTO_PUBLIC_URL`
to the address people open through the proxy, such as `https://recto.example.org`. Sign-in
redirects and the `Secure` flag of the session cookie depend on it.

recto must be at the root of its address: `https://recto.example.org` works,
`https://example.org/recto` does not, and recto refuses to start with such a URL.

## Let long scans finish

A scan answers only once the whole batch is read, which can take minutes for a stack in the
feeder. Many proxies give up on a response after 60 seconds. Raise that limit for recto:

- **nginx:** `proxy_read_timeout 600s;` (the default is 60 seconds).
- **Caddy:** no limit by default.
- **Traefik:** no limit by default, unless `forwardingTimeouts.responseHeaderTimeout` is set on
  the transport.
- **Cloudflare Tunnel:** cuts requests after 100 seconds, and no setting raises it on the free
  plans. Scan long batches in smaller stacks.

When the proxy gives up, the Scan page says **The connection to recto was cut before the scan
finished.** The scanner may still finish, but the result is lost.

## Let uploads through

Files up to `RECTO_MAX_UPLOAD_SIZE` (50 MiB by default) are sent to recto in one request. nginx
refuses bodies over 1 MiB by default: set `client_max_body_size` to at least that size.

## Examples

### Caddy

```text
recto.example.org {
	reverse_proxy recto:8080
}
```

### nginx

```nginx
server {
    listen 443 ssl;
    server_name recto.example.org;
    # ssl_certificate and ssl_certificate_key here.

    client_max_body_size 50m;

    location / {
        proxy_pass http://127.0.0.1:8080;
        proxy_set_header Host $host;
        proxy_read_timeout 600s;
    }
}
```

### Traefik

```yaml
  recto:
    labels:
      traefik.enable: "true"
      traefik.http.routers.recto.rule: Host(`recto.example.org`)
      traefik.http.routers.recto.tls.certresolver: letsencrypt
      traefik.http.services.recto.loadbalancer.server.port: "8080"
```

When the proxy reaches recto over the Compose network, you can remove the `ports` of the `recto`
service, so recto is reachable only through the proxy.

## Check

Open `RECTO_PUBLIC_URL`, sign in, and check that your browser shows a valid certificate. If
sign-in loops or fails, see [Troubleshoot](troubleshoot.md#sign-in).
