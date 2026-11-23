# How printing from devices works

Computers and phones add recto as an ordinary network printer and print to it from any
application's Print dialog. This page explains the three choices behind that: documents are held
until you release them, devices use their own passwords, and some devices carry that password in
their address. The steps for each system are in the how-to guides, for
[macOS](../how-to/print-from-macos.md), [Windows](../how-to/print-from-windows.md),
[Linux](../how-to/print-from-linux.md), [iPhone and iPad](../how-to/print-from-ios.md) and
[Android](../how-to/print-from-android.md); the details are in
[Printing from computers and phones](../reference/printing.md#printing-from-computers-and-phones).

## Hold, then release

recto shows devices one printer, whatever printers CUPS has. A device does not know who may use
which printer, what each one costs, or how much of your quota is left, and it should not choose
for you: recto does. So a document from a device is **held**: recto keeps it, and you print it
from the Jobs page, where you pick the printer and the options, which start as the device asked.
That release goes through the same path as the Print page: the printer's access rules, the page
count, quotas and the ledger. Nothing is counted while a document is held, and one held document
prints at most once.

The device is told the job is complete as soon as recto has the document. Its part is done, and a
job left waiting would block the next ones on a Mac and keep a phone's print screen spinning. The
job's message, **Held in recto until you release it**, says that nothing has printed yet.

A held document waits 24 hours by default (`RECTO_HELD_JOB_HOURS`), then is deleted. Only its
owner can see, print or delete it; administrators cannot. People who always print on the same
printer can skip the release: **Print it at once on** a printer, on the Account page, releases
each document as it arrives, with the same checks.

## Device passwords

Devices never use your account's password. A computer keeps the password of a printer in its
keychain, and a phone in its settings: with the account's password, every device would hold the
key to the web pages and to your account, and stopping one lost phone would mean changing the
password everywhere. People who sign in through single sign-on have no account password at all.

So each device gets its own **device password**, made on the Account page: random, shown once,
named after the device, and revoked with one button when the device is lost, sold or replaced.
It prints and scans as you, and nothing else: it cannot sign in to the web pages or change the
account. The [Security model](security.md#device-passwords) says what it grants and how it is
stored and limited.

## The personal address

Most computers answer a printer that asks for a password: macOS and Linux prompt, then remember
it. Those add the plain printer address, `ipps://recto.example.org:443/ipp/print`, and send the
device password when asked.

Others never answer that question. Windows' built-in IPP driver ignores it, an AirPrint profile
on an iPhone has nowhere to put a password, and Android's built-in print service sends none. For
them, recto also accepts the device password inside the address, in your **personal address**,
`ipps://recto.example.org:443/device/<password>/ipp/print`. Every address recto hands back to the
device, such as the one of a job, keeps that path, since a device that switched to an address
without it would lose the password.

A password in an address is easier to leak than one in a prompt: a reverse proxy writes it in its
access log unless told not to (see
[Run behind a reverse proxy](../how-to/reverse-proxy.md#printing-from-computers-and-phones)), and
anyone who can open the printer's settings on the device can read it. recto never logs it, and a
wrong one answers as an address that does not exist. Use the personal address only on a device
that only you use, and revoke its device password when that changes.

## The Apple profile

iPhones and iPads cannot add a printer by its address. Besides printers that announce themselves
on the network, they take a **configuration profile**: a small file, installed in Settings, that
can add an AirPrint printer at a given address. The Account page offers one for a new device
password, **Download for iPhone, iPad and Mac**, holding the personal address. Macs install it
too.

The profile is not signed. A signature only helps when the device already trusts the certificate
that made it, and a server of your own has no such certificate, so iOS and macOS show it as
unsigned; that changes nothing about what it does. Since it holds the password, recto serves it
only to the person who owns the password, writes it to no file or log, and tells browsers not to
keep it (`Cache-Control: no-store`).

## Finding recto on the network

Printers on a local network announce themselves over multicast DNS, and Print dialogs list what
they hear. recto can do the same (`RECTO_DNSSD`), and for Android it is the only way in: Android
adds printers it found, never one typed with a path.

A discovered printer is reached directly, by a `.local` name, not through the reverse proxy and
its public name, and the proxy's certificate does not cover that name. So discovery points at a
listener of its own, the LAN listener, with a certificate recto makes for its `.local` name and
keeps, since devices remember the certificate they first see. It serves the printer and the
scanners, never a page. recto announces itself under its own name, `recto-<hostname>.local`,
leaving the host's name to the host.

Multicast stays on its network and does not cross Docker's bridge, so recto runs on the host's
network to announce itself, or leaves the announcing to the host's avahi-daemon with the files
`recto dnssd-services` writes. Only scanners open to everyone are announced: apps that find a
scanner this way send no password, and only those scanners can work for them.

## What each system supports

| System | How it adds recto | Status |
| --- | --- | --- |
| macOS | Printer address and a password prompt, or the personal address, by hand or with a profile | Supported |
| Linux (CUPS, GNOME, KDE) | Printer address and a password prompt, or the personal address | Supported |
| Windows 10 and 11 | Personal address only | Experimental |
| iOS and iPadOS | A profile with the personal address, or a discovered printer and a password | Experimental |
| Android | A discovered printer and a password, through Mopria Print Service | Experimental |

A system is experimental until printing from it has passed its checks on real devices. On every
system, printing from recto's Print page in the browser works.

recto converts the picture formats that phones and Windows send, PWG raster and Apple raster, to
a PDF as they arrive, so the preview, the page count and page ranges work the same whatever sent
the document.
