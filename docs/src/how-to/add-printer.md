# Add a printer

recto prints on the printers of its CUPS server. With the `compose.yml` of this repository,
administrators add, describe and remove printers on the **Printers** page of **Administration**,
with nothing to set up. With another CUPS server, or when you prefer the command line, you add
them with `lpadmin`, as the rest of this guide shows.

To use a CUPS server that already runs on your network instead, see
[Use an existing CUPS server](existing-cups.md).

<picture>
  <source media="(prefers-color-scheme: dark)" srcset="../images/printers-admin-dark.png">
  <img src="../images/printers-admin-light.png" alt="The Printers tab of the Administration page: the Add a printer form with a name, an address, IPP Everywhere as the driver, a description and a location, then the printers, the first with its Manage section open: who may use it, its description and location, Default printer, Pause, Turn jobs away and Remove printer.">
</picture>

## Add a printer from recto

Printer management is on when `RECTO_CUPS_ADMIN_SOCKET` names the CUPS socket. `compose.yml`
sets it, shares the socket between the `cups` and `recto` services in the `cups-socket` volume,
and has the `cups` service create a `recto` account that `examples/cups/cups-files.conf` makes an
administrator over that socket. If **Administration > Printers** has no **Add a printer** section,
it is off: see [Configuration](../reference/configuration.md) and
[Security](../explanation/security.md#cups).

recto and the `recto` account of the `cups` service both take their uid from `RECTO_UID` in `.env`
(65532 by default), so they always match: change recto's uid there, not with `user:` on the
`recto` service. With a CUPS of your own, you need the same three things: recto and CUPS share
the socket, CUPS has a `recto` account with recto's uid, and `SystemGroup` lists `root` first,
then that account (see [Use an existing CUPS server](existing-cups.md#printer-management)).
Never list a group whose users have passwords.

1. Open **Administration > Printers**.
2. Under **Add a printer**, give a short **Name**: letters, digits, dots, underscores and hyphens.
   A name already in use, whatever its case, is refused, since CUPS would replace that printer.
3. Give the **Address** of the printer, such as `ipp://192.168.1.60/ipp/print`. The schemes are
   `ipp`, `ipps`, `http`, `https`, `socket`, `lpd`, `dnssd` and `usb`. Give the printer a fixed
   address in your router, since CUPS keeps this one. An address with a user name or password
   (`ipp://user:pass@host/...`) is refused, because CUPS would store it in clear.
4. Leave the **Driver** on **IPP Everywhere** if the printer speaks it (most printers made since
   about 2015, also sold as AirPrint or Mopria). CUPS then asks the printer for its capabilities,
   so the address must answer: an unreachable printer is reported on the page, after up to a
   minute.
5. For an older printer, type words of its make and model in **Find a driver** and press
   **Search**. The page reloads with up to 50 matching drivers in the **Driver** list. Pick one,
   then fill in the form. Printers usually take the raw socket address, `socket://<address>:9100`.
6. Fill in the optional **Description** and **Location**, which recto shows on the printer, then
   press **Add printer**.

CUPS cannot discover printers from inside the container, because Docker's bridge network does not
carry the multicast that discovery uses: give the address yourself.

Each printer's row has a **Manage** section with who may use it and the other actions: edit the
description and location (up to 1023 characters each, on one line), make it the default printer (the first on the Print page), pause or
resume it, make it accept or turn away new jobs, run its maintenance commands (see
[Run maintenance](#run-maintenance)), and remove it. Removing asks for a tick in a
checkbox, cancels the printer's waiting jobs and forgets its access rule. These actions also work
on printers added with `lpadmin` under names the Add form would not take, such as `Büro`.

A refusal that says CUPS refused the change means CUPS does not accept recto as an
administrator: check that the `recto` account exists in the `cups` container (`docker compose
exec cups grep recto /etc/passwd`), that its uid is recto's, and the `SystemGroup` line of
`examples/cups/cups-files.conf`.

## Add a printer with the command line

This works with or without printer management. The commands go through `docker compose exec
cups`, because the web interface of CUPS is off and the administrators are root and `recto`, only
over the socket inside the container.

### A network printer

Most printers made since about 2015 speak **IPP Everywhere** (also sold as AirPrint or Mopria),
which needs no driver. With the printer's address, add it under a short name:

```sh
docker compose exec cups lpadmin -p office -E \
  -v ipp://192.168.1.60/ipp/print -m everywhere \
  -D "Office laser" -L "Study"
```

- `-p office` is the name recto shows.
- `-v` is the printer's address. `ipp://<address>/ipp/print` works for most printers; give the
  printer a fixed address in your router, since CUPS keeps this one.
- `-m everywhere` asks the printer for its capabilities instead of using a driver. If the
  printer does not answer over IPP, this fails with an error.
- `-D` and `-L`, the description and the location, are optional. recto shows them on the printer's
  card.

CUPS cannot discover printers from inside the container, because Docker's bridge network does not
carry the multicast that discovery uses: give the address yourself.

#### An older printer

A printer without IPP Everywhere needs a driver. Find one for your model, then add the printer
with the driver's name and, usually, the raw socket port 9100:

```sh
docker compose exec cups lpinfo -m | grep -i "laserjet 1020"
docker compose exec cups lpadmin -p old -E \
  -v socket://192.168.1.61:9100 -m "<driver name from the list>"
```

The CUPS image includes the drivers of Gutenprint, HPLIP, Foomatic and others.

## A USB printer

1. Give the `cups` service access to USB devices, in `compose.yml`:

   ```yaml
     cups:
       devices:
         - /dev/bus/usb:/dev/bus/usb
   ```

   Never use `privileged: true`: it gives the container the whole machine.
2. Recreate it: `docker compose up -d cups`.
3. Find the printer's address, which starts with `usb://`:

   ```sh
   docker compose exec cups lpinfo -v
   ```

4. Add it as above, from the page or with `lpadmin`, with that address and IPP Everywhere (for
   IPP-over-USB printers) or a driver. The page takes `usb://` addresses only: for an address
   that starts with `hp:`, from HPLIP, use `lpadmin`.

## Choose the default printer

The default printer comes first on the Print page. Use **Make default** on the Printers page, or:

```sh
docker compose exec cups lpadmin -d office
```

## Pause, resume and remove

The Printers page has a button for each. With the command line:

```sh
docker compose exec cups cupsdisable office   # the printer shows as Stopped, jobs wait
docker compose exec cups cupsenable office
docker compose exec cups cupsreject office    # new jobs are refused: "Not accepting jobs"
docker compose exec cups cupsaccept office
docker compose exec cups lpadmin -x office    # remove it
```

## Run maintenance

When the printer's driver offers maintenance commands, its **Manage** section has a
**Maintenance** part with a button for each:

| Button | CUPS command | Does |
| --- | --- | --- |
| **Clean print heads** | `Clean all` | Cleans every print head, for streaks or missing colors. |
| **Print a test page** | `PrintSelfTestPage` | Prints the printer's own test page. |
| **Refresh ink levels** | `ReportLevels` | Asks the printer for its supply levels, which the Printers page shows once it answers. |
| **Configure automatically** | `AutoConfigure` | Asks the printer for its installed options and sets the queue's defaults from them. |

recto offers only the commands the queue lists in CUPS, as the CUPS web interface does. Many
driverless (IPP Everywhere) queues list none, so their row has no Maintenance part: use the
printer's own controls for them. Queues with a driver, such as Gutenprint or HPLIP ones, often
list some.

A command reaches the printer as a job that CUPS hands to the driver instead of printing it. It
waits behind the printer's other jobs, waits while the printer is paused, and is refused while
the printer turns jobs away. recto never records it, counts it against a quota or holds it.

## Check

Reload the **Printers** page in recto: the printer is there, **Ready**, with its supply levels if
the printer reports them. Print a page from the **Print** page to be sure.

Printers live in the `cups-config` volume, so they survive restarts and upgrades. `docker compose
down -v` deletes them.
