# Printing

## Files

| Format | Recognized by |
| --- | --- |
| PDF | content starting with `%PDF-` |
| JPEG | content starting with the JPEG signature |
| PNG | content starting with the PNG signature |

The file name and the type the browser claims are not trusted: recto reads the start of the file.
Any other file is refused with **Only PDF, JPEG and PNG files can be printed.** The largest file
is `RECTO_MAX_UPLOAD_SIZE`, 50 MiB by default.

## Options

The Print page offers an option only when the chosen printer supports it, as CUPS reports it.

| Field | Values | Sent to CUPS as |
| --- | --- | --- |
| Printer | the printers of CUPS; the default one first | the printer's URI |
| Copies | from 1 to the printer's maximum | `copies` |
| Sides | One-sided; Two-sided, flip on long edge; Two-sided, flip on short edge | `sides`: `one-sided`, `two-sided-long-edge`, `two-sided-short-edge` |
| Color | Color; Black and white | `print-color-mode`: `color`, `monochrome` |
| Pages (PDF only) | empty for all, or ranges such as `1-3, 5` | `page-ranges` |

Each job is sent with the file name as `job-name` and the signed-in user's username as
`requesting-user-name`, so CUPS logs and quotas see the person, not recto.

## Printer states

| Shown | Meaning |
| --- | --- |
| Ready | The printer is idle. |
| Printing | The printer is processing a job. |
| Stopped | The printer is paused in CUPS, or stopped on an error. |
| Default | The CUPS default printer. |
| Offline | CUPS cannot reach the printer. |
| Not accepting jobs | CUPS refuses new jobs for this printer. |

Below the state, recto lists what the printer reports: out of paper, paper jam, toner or ink low
or empty, door or cover open, paused, or any other CUPS reason by its keyword. Supplies show
their level as a percentage when the printer reports one.

The status is read when the page loads.

## Jobs

The Jobs page lists the jobs CUPS knows, of every user, newest first, and refreshes every 5
seconds while the page is visible.

| Shown | IPP job state |
| --- | --- |
| Queued | `pending` |
| Held | `pending-held` |
| Printing | `processing` |
| Stopped | `processing-stopped` |
| Canceled | `canceled` |
| Failed | `aborted` |
| Printed | `completed` |

Queued, held, printing and stopped jobs have a **Cancel** button. A job whose owner CUPS hides is
shown as **another user**. Jobs sent through recto show the sender's display name, from the record
recto keeps of each job it submits; other jobs show the owner CUPS reports.
