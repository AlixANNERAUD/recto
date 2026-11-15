# Add a printer

recto prints on the printers of its CUPS server. This guide adds printers to the `cups` service
of `compose.yml`. Its web interface is off, and the only administrator is root inside the
container, so every command goes through `docker compose exec cups`.

To use a CUPS server that already runs on your network instead, see
[Use an existing CUPS server](existing-cups.md).

## A network printer

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

### An older printer

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

4. Add it as above, with that address and `-m everywhere` (for IPP-over-USB printers) or a driver.

## Choose the default printer

The default printer comes first on the Print page:

```sh
docker compose exec cups lpadmin -d office
```

## Pause, resume and remove

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
