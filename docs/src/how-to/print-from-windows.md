# Print from Windows

> **Experimental.** Printing from Windows has not yet passed its checks on real computers. It may
> not work with your version of Windows; printing from recto's Print page in the browser always
> does.

Add recto as a printer on Windows 10 or 11, so that any application's Print dialog can send
documents to it. They wait in recto until you print them from the Jobs page, where you choose the
printer (see [How printing from devices works](../explanation/devices.md)).

Windows never answers a printer's password prompt, so it uses your **personal printer address**,
which holds the device password. You need:

- recto served over HTTPS with a certificate Windows trusts, such as one from Let's Encrypt on
  your reverse proxy (see [Run behind a reverse proxy](reverse-proxy.md)): Windows refuses a
  self-signed one;
- an administrator account on the computer.

> **Warning.** Windows adds the printer for the whole computer, not for your Windows account.
> Everyone who signs in to it can print as you, and can read your personal address in the
> printer's port settings. Add recto only on a computer that only you use, and revoke the device
> password when you stop using it.

## Create a device password

1. Open recto and sign in. Select your name, or the account icon, at the top right to open
   **Account**.
2. Under **Devices**, type a **Device name**, such as `Desktop PC`, and press **Create device
   password**.
3. recto shows **Device password for Desktop PC** once, with the **Password**, your **Username**,
   the **Printer address** and the **Personal printer address**. Keep the page open until the
   printer works: the password cannot be shown again.

## Add the printer

1. Open **PowerShell** as administrator: right-click **Start**, then **Terminal (Admin)** or
   **Windows PowerShell (Admin)**.
2. Check that your Windows can add a printer by its address. This answers `True`; `False` means
   your Windows is too old for it:

   ```powershell
   (Get-Command Add-Printer).Parameters.ContainsKey("IppURL")
   ```

3. Add the printer with the **Personal printer address** from the Account page:

   ```powershell
   Add-Printer -Name "recto" -IppURL "ipps://recto.example.org:443/device/<password>/ipp/print"
   ```

Copy the address whole: it carries the port, which `ipps://` needs, since without one it means
port 631. Windows uses its built-in IPP driver, so no driver is needed. The printer then appears
in **Settings > Bluetooth & devices > Printers & scanners**.

## Print

Print from any application, pick **recto** as the printer, set the copies, sides and color you
want, and print. Windows sends the document to recto and shows the job as done: nothing has
printed yet. recto tells it the job is **Held in recto until you release it**.

## Release the job

1. Open **Jobs** in recto. The document is under **Waiting for you**, with the name of the device
   it came from.
2. Press **Preview** to check it, if you like.
3. Choose the **Printer** and the options, which start as Windows asked, and press **Print**. Or
   press **Delete** to drop it.

Nothing is counted until you press **Print**. A document you leave waits 24 hours, or what your
administrator set, then is deleted.

To print documents from your devices at once, without this step, choose **Print it at once on**
a printer under **Documents from your devices** on the Account page (see
[Printing directly](../reference/printing.md#printing-directly)).

## Remove the printer

1. In **Settings > Bluetooth & devices > Printers & scanners**, select **recto** and press
   **Remove**, or run `Remove-Printer -Name "recto"` in PowerShell as administrator.
2. In recto, on the Account page, press **Revoke** next to the device password. The address stops
   working, even where it was copied.

## Planned: discovery and scanning

> **In progress.** recto is gaining automatic discovery on the local network and scanning from
> Windows. This section describes them as planned: names, steps and settings may still change.

- **Discovered printer.** Windows lists printers that announce themselves on the local network,
  but cannot send them a password, so add recto with your personal address as above, even when
  your administrator has recto announce itself.
- **Scanning.** recto's scanners will appear under **Settings > Bluetooth & devices > Printers &
  scanners > Add device** when recto announces them on the local network, and then in the
  **Windows Scan** app. Windows finds scanners only this way and sends them no password, so this
  needs your administrator to let your network scan without one; only scanners open to everyone
  are offered that way.
