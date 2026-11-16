# Use an existing CUPS server

If a CUPS server already runs on your network, for example on the host itself, recto can print
through it instead of the `cups` service of `compose.yml`.

## Point recto at it

1. In `compose.yml`, remove the `cups` service, its volumes and the `depends_on` of `recto`.
2. Set `RECTO_CUPS_URL` to the server, with its port:

   ```yaml
   RECTO_CUPS_URL: http://192.168.1.10:631
   ```

   For the host itself, use `http://host.docker.internal:631` and add to the `recto` service:

   ```yaml
       extra_hosts:
         - host.docker.internal:host-gateway
   ```

   A URL without a port uses 80 or 443, not 631. `ipp://` and `ipps://` are accepted and mean
   `http://` and `https://`.

## Let recto in

Edit the server's `cupsd.conf` (usually `/etc/cups/cupsd.conf`):

1. Accept recto's `Host` header, or CUPS answers every request with `400 Bad Request`:

   ```text
   ServerAlias *
   ```

   Instead of `*`, you can name the host recto uses, such as `host.docker.internal`.
2. Let recto's address print, in the `<Location />` block:

   ```text
   <Location />
     Order allow,deny
     Allow from 172.17.0.0/16
   </Location>
   ```

   Use the address or subnet recto connects from. Keep it as narrow as you can: everyone allowed
   there can print.
3. Let recto read job owners, so it can list and cancel everyone's jobs. Without this, cancelling
   fails with CUPS's defaults:

   ```text
   JobPrivateValues none
   SubscriptionPrivateValues none
   ```

   and, in the `<Policy default>` block:

   ```text
   JobPrivateAccess all
   SubscriptionPrivateAccess all
   ```

4. Restart CUPS: `sudo systemctl restart cups`.

## Printer management

The **Printers** page can add and manage printers only through CUPS's local socket, so only with
a CUPS on the same host as recto. With a CUPS on another machine, remove
`RECTO_CUPS_ADMIN_SOCKET` and the `cups-socket` volume from the `recto` service: the page then
only lists printers, and you manage them with CUPS's own tools.

On the same host:

1. Share the host's socket with recto, in the `recto` service of `compose.yml`, in place of the
   `cups-socket` volume:

   ```yaml
       environment:
         RECTO_CUPS_ADMIN_SOCKET: /run/cups/cups.sock
       volumes:
         - /run/cups:/run/cups:ro
   ```

2. Create a system account `recto` on the host, with recto's uid (65532, or `RECTO_UID`) and a
   new group of its own. CUPS knows recto by that uid, and makes an administrator of every
   account whose main group is that group, so never reuse an existing group:

   ```sh
   sudo groupadd --system recto
   sudo useradd --system --uid 65532 --gid recto --no-create-home --shell /usr/sbin/nologin recto
   ```

3. In the server's `cups-files.conf` (usually `/etc/cups/cups-files.conf`), make root and that
   account the administrators, root first:

   ```text
   SystemGroup root recto
   ```

   Root comes first because CUPS gives its root certificate the group of the first entry. Replace
   Debian's default, `SystemGroup lpadmin`, do not add `recto` to it: the `lpadmin` group holds
   people with passwords, who could then administer CUPS over the network. `sudo lpadmin` keeps
   working, as root.

4. Restart CUPS (`sudo systemctl restart cups`) and recto (`docker compose up -d recto`).

Every process with recto's uid that can reach the socket administers CUPS: see
[Security](../explanation/security.md#cups).

## Check

Reload the **Printers** page in recto: it lists the server's printers. If it says the printing
service is unavailable, see [Troubleshoot](troubleshoot.md#printing).
