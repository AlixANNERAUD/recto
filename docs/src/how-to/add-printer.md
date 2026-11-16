# Add a printer

recto prints on the printers of its CUPS server. With printer management on, administrators add,
describe and remove printers on the **Printers** page of **Administration**. Without it, you add
them with `lpadmin` in the `cups` service of `compose.yml`, as the rest of this guide shows.

To use a CUPS server that already runs on your network instead, see
[Use an existing CUPS server](existing-cups.md).

## Add a printer from recto

Printer management is on when `RECTO_CUPS_ADMIN_SOCKET` names the CUPS socket and the `cups`
service lets recto administer it. If **Administration > Printers** has no **Add a printer**
section, it is off: see [Configuration](../reference/configuration.md) and
[Security](../explanation/security.md#cups).

1. Open **Administration > Printers**.
2. Under **Add a printer**, give a short **Name**: letters, digits, dots, underscores and hyphens.
3. Give the **Address** of the printer, such as `ipp://192.168.1.60/ipp/print`. The schemes are
   `ipp`, `ipps`, `http`, `https`, `socket`, `lpd`, `dnssd` and `usb`. Give the printer a fixed
   address in your router, since CUPS keeps this one.
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

Each printer's row has a **Change who may use it** section with the other actions: edit the
description and location, make it the default printer (the first on the Print page), pause or
resume it, make it accept or turn away new jobs, and remove it. Removing asks for a tick in a
checkbox, cancels the printer's waiting jobs and forgets its access rule.

A refusal that says CUPS refused the change means the `cups` service does not let recto
administer it: check the `RECTO_CUPS_ADMIN_SOCKET` setting and the CUPS configuration in
`examples/cups/`.

## Add a printer with the command line

This works with or without printer management. The commands go through `docker compose exec
cups`, because the web interface of CUPS is off and the only other administrator is root inside
the container.

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
   IPP-over-USB printers) or a driver.

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

## Check

Reload the **Printers** page in recto: the printer is there, **Ready**, with its supply levels if
the printer reports them. Print a page from the **Print** page to be sure.

Printers live in the `cups-config` volume, so they survive restarts and upgrades. `docker compose
down -v` deletes them.
