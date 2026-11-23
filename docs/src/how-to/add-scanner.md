# Add a scanner

recto scans through libsane, inside its own container. The image includes the SANE backends of
Debian and [sane-airscan](https://github.com/alexpevzner/sane-airscan). This guide makes a
scanner visible to it.

Check what recto sees at any time with:

```sh
docker compose exec recto scanimage -L
```

## A network scanner (eSCL or WSD)

Most network scanners and multifunction printers made since about 2015 (Brother, Canon, Epson,
HP and others) speak eSCL, also called AirScan, or WSD. sane-airscan drives them without a
driver.

sane-airscan normally discovers scanners with multicast, which Docker's bridge network does not
carry, so list yours by address:

1. Find the scanner's eSCL address. It is usually `http://<address>/eSCL`; some HP models use
   `http://<address>:8080/eSCL`. Give the scanner a fixed address in your router.
2. In `examples/sane/airscan.conf`, add a line under `[devices]`:

   ```ini
   [devices]
   "Office scanner" = http://192.168.1.50/eSCL, eSCL
   ```

   For a WSD scanner, use its WSD address and `WSD`.
3. In `compose.yml`, uncomment the two volume lines of the `recto` service:

   ```yaml
         - ./examples/sane/airscan.conf:/etc/sane.d/airscan.conf:ro
         - ./examples/sane/dll.conf:/etc/sane.d/dll.conf:ro
   ```

   The `dll.conf` enables only the `airscan`, `escl` and `net` backends. Debian's default enables
   all of them, and each one probes for its hardware, which makes listing scanners slow.
4. Recreate recto: `docker compose up -d recto`.

### Or let it discover

With `network_mode: host` on the `recto` service, discovery works and no address is needed. It
costs the network isolation of the container, and recto then leaves the Compose network: it
listens on the host's port 8080 directly (remove `ports`), and reaches CUPS only through a port
published on the host, which `cupsd.conf` must then allow. Listing the scanner by address is
simpler and keeps the isolation.

recto's own scanners, once advertised on the same network, are never listed back: the image's
`/etc/sane.d/airscan.d/recto.conf` blacklists them, and recto leaves out any device named like
them, so a scan never goes from recto to recto.

## A scanner shared by another machine (saned)

When a USB scanner hangs off another computer that runs `saned`:

1. List that machine in `examples/sane/net.conf`, one host per line.
2. Mount it, and the `dll.conf` above, by uncommenting their lines in `compose.yml`.
3. Recreate recto: `docker compose up -d recto`.

## A USB scanner on the same machine

1. Give the `recto` service the USB devices, in `compose.yml`:

   ```yaml
     recto:
       devices:
         - /dev/bus/usb:/dev/bus/usb
   ```

2. recto runs as user 65532, which must be able to open the scanner's device node. Add a udev
   rule on the host, for example in `/etc/udev/rules.d/60-recto-scanner.rules`, with your
   scanner's vendor and product ids from `lsusb`:

   ```text
   SUBSYSTEM=="usb", ATTRS{idVendor}=="04a9", ATTRS{idProduct}=="1909", MODE="0660", GROUP="65532"
   ```

   then reload the rules with `sudo udevadm control --reload` and replug the scanner.
3. If you mounted the trimmed `dll.conf`, add the scanner's backend to it (for example `pixma`
   or `epson2`), or do not mount it.
4. Recreate recto: `docker compose up -d recto`.

Never use `privileged: true` to get around permissions: it gives the container the whole machine.

## Check

Run `docker compose exec recto scanimage -L`, then open **Scan** in recto and press the round
arrow to look for scanners again. The scanner appears in the list. If not, see
[Troubleshoot](troubleshoot.md#scanning).
