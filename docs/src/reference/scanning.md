# Scanning

## Scanners

The Scan page lists the scanners libsane finds. recto lists them once at startup, in the
background, and again when someone presses the round arrow next to the list. Each scanner is
shown by its vendor and model.

## Options

The options offered come from the chosen scanner.

| Field | Values |
| --- | --- |
| Resolution | The resolutions the scanner lists. A scanner that gives a range instead offers the usual steps within it: 75, 100, 150, 200, 300, 600, 1200 and 2400 dpi. 300 dpi, or the highest below it, is chosen first. |
| Colors | Color, Grayscale, Black and white: the SANE modes `Color`, `Gray` and `Lineart` the scanner supports. |
| Source | The scanner's sources, such as Flatbed or ADF. A source whose name contains `adf`, `feeder` or `duplex` is treated as a feeder. |
| Format | PDF, PNG image, JPEG image. |

From a feeder, a scan reads every sheet in the feeder. PDF keeps every page; PNG and JPEG keep
only the first, and choosing a feeder switches the format to PDF.

## Preview

**Preview** scans the whole glass once, in color when the scanner can, at its lowest resolution
of at least 75 dpi, and shows it. It works from the flatbed only: from a feeder, it would pull the
first sheet through.

Dragging on the preview selects the part to scan, kept in millimetres on the glass. Scanners that
measure their scan area in pixels instead of millimetres scan their whole area, and the page says
so.

## Output

| Format | Content |
| --- | --- |
| PDF | One page per sheet, each page a JPEG image at the scan's resolution, sized to the scanned area. |
| PNG | The first page, lossless. |
| JPEG | The first page, quality 85. |

Grayscale and black and white scans are stored as gray images.

The file is named after the local time of the scan in the container: `scan-YYYYMMDD-HHMMSS`,
with the format's extension, such as `scan-20261115-143000.pdf`.

## Results

A finished scan shows **Scan ready** with its file name and number of pages, a **Download**
button, and a **Save to folder** button when `RECTO_SCAN_OUTPUT_DIR` is set.

- Only the person who scanned can download or save the result.
- A result is deleted one hour after the scan, and when recto restarts.
- **Save to folder** writes into `RECTO_SCAN_OUTPUT_DIR` under the scan's file name, or with a
  suffix `-1`, `-2` and so on when the name is taken. It never replaces a file. After saving, the
  button is disabled; the result can still be downloaded until it expires.

## Scanning from computers and phones

With `RECTO_OS_INTEGRATION` on, recto offers each scanner over eSCL, the protocol of AirScan and
Mopria scan apps, to the [device passwords](accounts.md#device-passwords) of the people who may
use it. Each scanner has two addresses (see [Pages and endpoints](endpoints.md#scanners)):

- `/eSCL/<scanner>/`, with the device password as the password the app asks for;
- `/device/<password>/eSCL/<scanner>/`, the personal address, for apps that cannot ask for one,
  such as sane-airscan on Linux.

`<scanner>` is a key of 8 hex digits, the same across restarts as long as the scanner's SANE id
stays. A scanner a person may not use does not exist for their devices. The Account page shows the
personal address of each scanner the person may use, under the public URL, next to a new device
password.

With `RECTO_DNSSD`, recto also announces the scanners open to everyone on the local network, as
`_uscans._tcp` services pointing at the LAN listener, under the name `<vendor> <model> (<name>)`
(see [Let devices find recto](../how-to/discovery.md)). Scan apps that find them that way send
no password: they scan from the networks of `RECTO_ESCL_OPEN_NETWORKS` (below).

The app's request maps onto the scanner as follows.

| eSCL | recto |
| --- | --- |
| `Platen` | The scanner's first source that is not a feeder. |
| `Feeder` | Its feeder; the duplex one when the app asks for both sides. |
| `RGB24`, `Grayscale8`, `BlackAndWhite1` | Color, Gray, Lineart. 16-bit modes scan in 8 bits. |
| Resolution | The nearest one the scanner offers. Without one, 300 dpi or the nearest. |
| Scan region, in 1/300 inch | Millimetres on the glass, within the scanner's area. |
| `application/pdf`, `image/jpeg`, `image/png` | PDF, JPEG, PNG. Without a format, PDF for a document and JPEG otherwise; another format is refused. |

A PDF is one document. JPEG and PNG give one document per page, so every sheet of a feeder
reaches the app, unlike on the Scan page. The scan goes back to the app only: recto keeps
nothing, and the documents wait for the app at most 10 minutes. One eSCL scan runs on a scanner
at a time; another app is asked to try again in 5 seconds, while scans from the Scan page wait
their turn as usual. An empty feeder ends the scan without a document, and the scanner's status
says so until its next scan.

### Scanning without a password

macOS Image Capture and Windows find scanners on the network and cannot carry a password. For
them, `RECTO_ESCL_OPEN_NETWORKS` lists networks whose devices scan without one, on the LAN
listener only, and only on the scanners open to everyone. Their scans are logged without a user.
The address checked is the connection's, so a device cannot claim to be inside by sending a
forwarded header, and the main listener, behind the reverse proxy, never scans without a
password.

## Scan log

Every finished scan is logged in the database, for the usage reports: who scanned, on which
scanner (its SANE device id), how many pages, in which format (`pdf`, `png` or `jpg`), when,
whether it came from the Scan page (`web`) or from a computer or phone (`escl`), and whether the
file was then saved with **Save to folder**. The file itself is never kept. A scan without a
device password is logged without a user.

- **Preview** scans are not logged.
- Downloads are not tracked: a scan downloaded and never saved is logged as not saved.
- Logging is best effort: if the database cannot write the row, the scan is still handed over
  and the log says `recording the scan failed`. A scan or a save waits at most 2 seconds for
  its row, for example while another write holds the database, then goes on and the log says
  `recording the scan timed out` or `marking the scan saved timed out`.
- The log is kept for as long as the database, like the print ledger.

## Limits

| Limit | Value |
| --- | --- |
| One page, before compression | 300 MB, about an A4 page in color at 1000 dpi |
| Scans at once per scanner | 1; others wait their turn, and another eSCL scan is answered `503` |

A page over the limit is refused with **This scan would be too large for recto.**
