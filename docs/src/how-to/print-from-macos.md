# Print from a Mac

Add recto as a printer on a Mac, so that any application's Print dialog can send documents to it.
They wait in recto until you print them from the Jobs page, where you choose the printer (see
[How printing from devices works](../explanation/devices.md)).

You need recto served over HTTPS, such as `https://recto.example.org` (see
[Run behind a reverse proxy](reverse-proxy.md)), and an administrator account on the Mac.

## Create a device password

1. Open recto and sign in. Select your name, or the account icon, at the top right to open
   **Account**.
2. Under **Devices**, type a **Device name**, such as `MacBook`, and press **Create device
   password**.
3. recto shows **Device password for MacBook** once, with the **Password**, your **Username**, the
   **Printer address** and the **Personal printer address**. Keep the page open until the printer
   works: the password cannot be shown again.

## Add the printer

Open **Terminal** and add the printer address, with your Mac's administrator password when
`sudo` asks:

```sh
sudo lpadmin -p recto -D "recto" -E -v ipps://recto.example.org:443/ipp/print -m everywhere
```

Copy the address from the Account page: it carries the port, which `ipps://` needs, since
without one it means port 631. `-m everywhere` asks recto for its capabilities, so no driver is
needed. `-p` is the name of the queue on the Mac, `-D` the name the Print dialog shows.

The first time you print, the Mac asks for a name and a password:

1. Print a document (below). The job waits in the printer's queue with **Hold for
   Authentication**.
2. Open the printer's queue: its icon is in the Dock while a job waits, or select **recto** in
   **System Settings > Printers & Scanners** and open its queue. Press the refresh button next to
   the job.
3. Give your **Username** and the device password, tick **Remember this password in my keychain**,
   and press **OK**. The job goes, and later ones go without asking.

To skip the prompt, add the **Personal printer address** instead, which holds the password:

```sh
sudo lpadmin -p recto -D "recto" -E -v ipps://recto.example.org:443/device/<password>/ipp/print -m everywhere
```

Any user of the Mac can then read the address, with `lpstat -v`, and print as you. Use it only on
a Mac that only you use.

### Or install a profile

A profile adds the printer with the personal address too, without Terminal:

1. On the page showing the new device password, press **Download for iPhone, iPad and Mac**. The
   Mac saves a `.mobileconfig` file in **Downloads**.
2. Open the file. macOS says the profile is ready to review in **System Settings**.
3. In **System Settings**, open the downloaded profile, under **General > Device Management** or
   **Privacy & Security > Profiles** depending on the version of macOS, press **Install**, and
   confirm with your Mac's password.

macOS shows the profile as unsigned: recto does not sign it, since that needs a certificate your
Mac already trusts, which a server of your own does not have. As with the personal address in
Terminal, any user of the Mac can read the address from the printer's settings.

## Print

Choose **File > Print** in any application, pick **recto** as the printer, set the copies, sides
and color you want, and press **Print**. The Mac sends the document to recto and shows the job as
done, with **Held in recto until you release it**: nothing has printed yet.

## Release the job

1. Open **Jobs** in recto. The document is under **Waiting for you**, with the name of the device
   it came from.
2. Press **Preview** to check it, if you like.
3. Choose the **Printer** and the options, which start as the Mac asked, and press **Print**. Or
   press **Delete** to drop it.

Nothing is counted until you press **Print**. A document you leave waits 24 hours, or what your
administrator set, then is deleted.

To print documents from your devices at once, without this step, choose **Print it at once on**
a printer under **Documents from your devices** on the Account page (see
[Printing directly](../reference/printing.md#printing-directly)).

## Remove the printer

1. In **System Settings > Printers & Scanners**, Control-click **recto** and choose **Remove
   Printer**, or run `sudo lpadmin -x recto`. If you installed the profile, remove the profile in
   **System Settings** instead, where you installed it.
2. In recto, on the Account page, press **Revoke** next to the device password. The Mac can no
   longer print as you, even with a copy of the address.

If you ticked **Remember this password in my keychain**, delete the entry in **Keychain Access**
too.

## Discovery and scanning

When your administrator has recto announce itself on the local network (see
[Let devices find recto](discovery.md)), the Mac finds it by itself.

> **Not yet checked on a Mac.** recto announces its printer and scanners as AirPrint and AirScan
> devices do, but adding them this way has not yet passed its checks on real Macs.

- **Discovered printer.** On the same network as recto, it appears under **System Settings >
  Printers & Scanners > Add Printer, Scanner or Fax**, on the default tab, under the name your
  administrator chose. Select it and press **Add**. The first time you print, the Mac asks for a
  name and a password: give your **Username** and the device password, and keep them in the
  keychain. The first connection also has the Mac accept recto's own certificate on the local
  network, which it remembers.
- **Scanning.** recto's scanners open to everyone appear in **Image Capture** and in **Printers
  & Scanners**. The Mac does not send a password to a scanner it discovered, so scanning needs
  your administrator to let your network scan without one; scanners limited to some groups are
  not announced.
