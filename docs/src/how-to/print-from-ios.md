# Print from an iPhone or iPad

> **Experimental.** Printing from iOS and iPadOS through AirPrint has not yet passed its checks on
> real devices, and parts of it are still being built (below). Printing from recto's Print page in
> Safari always works.

## Print from recto in Safari

This works today, with nothing to set up:

1. Open recto in Safari, or as an app on the Home Screen (see
   [Install recto as an app](install-app.md)), and sign in.
2. On **Print**, choose the file, then the printer and options, and tap **Print**. To print from another app, share the document to **Files** first, or save it
   as a PDF with **Print > Share > Save to Files**.

The rest of this guide adds recto to the **Print** menu of every app, through AirPrint. Documents
sent that way wait in recto until you print them from the Jobs page, where you choose the printer
(see [How printing from devices works](../explanation/devices.md)).

## Create a device password

1. Open recto and sign in. Tap the account icon at the top right to open **Account**.
2. Under **Devices**, type a **Device name**, such as `My iPhone`, and tap **Create device
   password**.
3. recto shows **Device password for My iPhone** once, with the **Password**, your **Username**,
   the **Printer address** and the **Personal printer address**. Keep the page open until the
   printer works: the password cannot be shown again.

## Add the printer

> **In progress.** iOS and iPadOS cannot add a printer by its address: they use printers that
> announce themselves on the local network, or a profile. recto is gaining both. This section
> describes them as planned: names, steps and settings may still change.

**With a profile**, coming to the Account page:

1. Next to the new device password, tap **Download for iPhone, iPad and Mac**, and allow the
   download.
2. Open **Settings**, tap **Profile Downloaded**, then **Install**, and confirm with your passcode.

The profile adds recto with your personal address, over HTTPS to your public host name, such as
`recto.example.org`. Away from home that works as is. On your home network, that name must also
lead to recto: your router needs split DNS (the name resolves to recto's local address at home)
or NAT loopback (it accepts its own public address from inside). Without either, printing works
only away from home.

**As a discovered printer**, when your administrator has recto announce itself on the local
network: recto appears in the **Print** menu's printer list while you are on that network. The
first time, iOS asks for a name and a password: give your **Username** and the device password.

## Print

In any app, tap **Share**, then **Print**. Choose **recto** as the **Printer**, set the copies,
sides and color you want, and tap **Print**. The iPhone sends the document to recto and treats
the job as done: nothing has printed yet.

## Release the job

1. Open **Jobs** in recto. The document is under **Waiting for you**, with the name of the device
   it came from.
2. Tap **Preview** to check it, if you like.
3. Choose the **Printer** and the options, which start as the iPhone asked, and tap **Print**. Or
   tap **Delete** to drop it.

Nothing is counted until you tap **Print**. A document you leave waits 24 hours, or what your
administrator set, then is deleted.

To print documents from your devices at once, without this step, choose **Print it at once on**
a printer under **Documents from your devices** on the Account page (see
[Printing directly](../reference/printing.md#printing-directly)).

## Remove the printer

1. If you installed the profile, open **Settings > General > VPN & Device Management**, tap the
   recto profile, then **Remove Profile**. A discovered printer needs nothing removed on the
   device.
2. In recto, on the Account page, tap **Revoke** next to the device password. The iPhone can no
   longer print as you.

## Scanning

iOS and iPadOS have no built-in app that scans from a network scanner. Scan from the **Scan**
page of recto in Safari, then download the result to your device.
