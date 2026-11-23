# Print from Linux

Add recto as a printer on a Linux computer, so that any application's Print dialog can send
documents to it. They wait in recto until you print them from the Jobs page, where you choose the
printer (see [How printing from devices works](../explanation/devices.md)). This works with CUPS,
which GNOME, KDE and most desktops print through.

You need recto served over HTTPS, such as `https://recto.example.org` (see
[Run behind a reverse proxy](reverse-proxy.md)), CUPS installed on the computer, and `sudo`.

## Create a device password

1. Open recto and sign in. Select your name, or the account icon, at the top right to open
   **Account**.
2. Under **Devices**, type a **Device name**, such as `Laptop`, and press **Create device
   password**.
3. recto shows **Device password for Laptop** once, with the **Password**, your **Username**, the
   **Printer address** and the **Personal printer address**. Keep the page open until the printer
   works: the password cannot be shown again.

## Add the printer

Pick the address that suits the computer.

**On a computer that only you use**, add the **Personal printer address**, which holds the
password, so that every application and `lp` print without asking:

```sh
sudo lpadmin -p recto -D "recto" -E -v ipps://recto.example.org:443/device/<password>/ipp/print -m everywhere
```

Every user of the computer can read that address, with `lpstat -v`, and print as you.

**On a shared computer**, add the **Printer address**, and have CUPS ask for a password:

```sh
sudo lpadmin -p recto -D "recto" -E -v ipps://recto.example.org:443/ipp/print -m everywhere \
  -o auth-info-required=username,password
```

Print dialogs that handle passwords, such as those of GNOME and GTK applications like Firefox,
then ask for a name and a password when you print: give your **Username** and the device
password. If yours does not ask, the job waits in CUPS, held for authentication: use the personal
address instead.

Copy the addresses from the Account page: they carry the port, which `ipps://` needs, since
without one it means port 631. `-m everywhere` asks recto for its capabilities, so no driver is
needed. `-p` is the name of the queue, `-D` the name print dialogs show.

GNOME's **Settings > Printers** and KDE's **System Settings > Printers** list the printer once it
is added. KDE can also add it by hand: **Add Printer**, then **Manual URI** under **Other**, with
the same address and **IPP Everywhere** as the driver.

## Print

Print from any application, pick **recto** as the printer, set the copies, sides and color you
want, and print. From the command line:

```sh
lp -d recto document.pdf
```

CUPS sends the document to recto and shows the job as completed: nothing has printed yet. recto
tells it the job is **Held in recto until you release it**.

## Release the job

1. Open **Jobs** in recto. The document is under **Waiting for you**, with the name of the device
   it came from.
2. Press **Preview** to check it, if you like.
3. Choose the **Printer** and the options, which start as the computer asked, and press
   **Print**. Or press **Delete** to drop it.

Nothing is counted until you press **Print**. A document you leave waits 24 hours, or what your
administrator set, then is deleted.

To print documents from your devices at once, without this step, choose **Print it at once on**
a printer under **Documents from your devices** on the Account page (see
[Printing directly](../reference/printing.md#printing-directly)).

## Remove the printer

1. Run `sudo lpadmin -x recto`, or remove **recto** in your desktop's printer settings.
2. In recto, on the Account page, press **Revoke** next to the device password. The computer can
   no longer print as you, even with a copy of the address.

## Planned: discovery and scanning

> **In progress.** recto is gaining automatic discovery on the local network and scanning from
> Linux applications. This section describes them as planned: names, steps and settings may still
> change.

- **Discovered printer.** When your administrator has recto announce itself on the local
  network, desktops that list network printers, such as GNOME's **Settings > Printers > Add
  Printer**, show recto. Add it, then give your **Username** and the device password when asked.
- **Scanning.** Linux scanning applications, such as **Document Scanner** (Simple Scan),
  **Skanlite** and `scanimage`, reach network scanners through sane-airscan. Point it at a
  recto scanner with your personal address, in `/etc/sane.d/airscan.conf`:

  ```ini
  [devices]
  "recto office" = https://recto.example.org:443/device/<password>/eSCL/<scanner>/, eSCL
  ```

  The Account page will show the scan address of each scanner you may use, with its
  `<scanner>` part, next to a new device password. Check with `scanimage -L`, which lists
  `recto office`. Scanners recto announces on the local network will also appear without this
  line when your administrator lets your network scan without a password.
- **Removing a scanner.** Delete its line from `airscan.conf` and revoke the device password.
