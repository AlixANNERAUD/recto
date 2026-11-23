# Print from Android

> **Experimental.** Printing from Android's Print menu has not yet passed its checks on real
> devices, and needs discovery, which is still being built (below). Printing from recto's Print
> page in the browser always works, and is the supported way on Android.

## Print from recto in the browser

This works today, with nothing to set up:

1. Open recto in Chrome, or as an app on the home screen (see
   [Install recto as an app](install-app.md)), and sign in.
2. On **Print**, choose the file, then the printer and options, and tap **Print**. To print from
   another app, save the document as a PDF first: in the app's **Print** menu, choose **Save as
   PDF** as the printer.

The rest of this guide adds recto to the **Print** menu of every app, through the Mopria Print
Service. Documents sent that way wait in recto until you print them from the Jobs page, where you
choose the printer (see [How printing from devices works](../explanation/devices.md)).

## Create a device password

1. Open recto and sign in. Tap the account icon at the top right to open **Account**.
2. Under **Devices**, type a **Device name**, such as `My phone`, and tap **Create device
   password**.
3. recto shows **Device password for My phone** once, with the **Password**, your **Username**,
   the **Printer address** and the **Personal printer address**. Keep the page open until the
   printer works: the password cannot be shown again.

## Add the printer

> **In progress.** Android adds a printer by a bare address, never one with a path, so it cannot
> use your personal address: it needs recto to announce itself on the local network, which recto
> is gaining. This section describes it as planned: names, steps and settings may still change.

Android's built-in **Default Print Service** never sends a password, so it cannot print to
recto. The **Mopria Print Service** app can:

1. Install **Mopria Print Service** from the Play Store, and turn it on under **Printing** in
   **Settings** (search Settings for `Printing`: its place differs between phones).
2. On the same network as recto, once your administrator has recto announce itself there, recto
   appears in the printer list of the **Print** menu.
3. The first time, Mopria asks for a login and a password: give your **Username** and the device
   password.

## Print

In any app, open **Print** (often under **Share** or the app's menu), choose **recto** as the
printer, set the copies, sides and color you want, and tap the print button. The phone sends the
document to recto and treats the job as done: nothing has printed yet.

## Release the job

1. Open **Jobs** in recto. The document is under **Waiting for you**, with the name of the device
   it came from.
2. Tap **Preview** to check it, if you like.
3. Choose the **Printer** and the options, which start as the phone asked, and tap **Print**. Or
   tap **Delete** to drop it.

Nothing is counted until you tap **Print**. A document you leave waits 24 hours, or what your
administrator set, then is deleted.

To print documents from your devices at once, without this step, choose **Print it at once on**
a printer under **Documents from your devices** on the Account page (see
[Printing directly](../reference/printing.md#printing-directly)).

## Remove the printer

1. Under **Printing** in **Settings**, turn **Mopria Print Service** off, or uninstall it, if you
   print to no other printer with it.
2. In recto, on the Account page, tap **Revoke** next to the device password. The phone can no
   longer print as you.

## Planned: scanning

> **In progress.** This section describes scanning from Android as planned: names, steps and
> settings may still change.

The **Mopria Scan** app will find recto's scanners when recto announces them on the local
network. It sends them no password, so this needs your administrator to let your network scan
without one; only scanners open to everyone are offered that way. Until then, scan from the
**Scan** page of recto in the browser.
