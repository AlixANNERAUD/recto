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

## Scan log

Every finished scan is logged in the database, for the usage reports: who scanned, on which
scanner (its SANE device id), how many pages, in which format (`pdf`, `png` or `jpg`), when, and
whether the file was then saved with **Save to folder**. The file itself is never kept.

- **Preview** scans are not logged.
- Downloads are not tracked: a scan downloaded and never saved is logged as not saved.
- Logging is best effort: if the database cannot write the row, the scan is still handed over
  and the log says `recording the scan failed`.
- The log is kept for as long as the database, like the print ledger.

## Limits

| Limit | Value |
| --- | --- |
| One page, before compression | 300 MB, about an A4 page in color at 1000 dpi |
| Scans at once per scanner | 1; others wait their turn |

A page over the limit is refused with **This scan would be too large for recto.**
