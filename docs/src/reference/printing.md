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

## What recto counts

Recto writes every job it sends in a ledger, before it asks CUPS to print, so that a job cannot
print without being counted. The ledger keeps the username as it was at that time, the document
type, the printer, the copies, the sides and the color mode, and these counts:

| Count | Value |
| --- | --- |
| Pages | The pages of the document that the page ranges select, times the copies. An image is one page. |
| Sheets | The selected pages, halved and rounded up when printing two-sided, times the copies. |
| Color pages | All the pages when the color mode is color, otherwise none. |
| Black and white pages | The other pages. |

The pages of a PDF come from reading the file. When recto cannot read a PDF, it still prints it
and records the counts as unknown, which is not the same as zero, unless a quota rule needs them:
then it refuses the job (see [Counting](quotas.md#counting)). A page range that goes past the
end of the document selects only the pages that exist.

## Refunds

A refunded job stops counting. A job is refunded when:

- CUPS refuses it.
- CUPS never received it: the entry still has no CUPS job id after ten minutes, for example
  because CUPS timed out while recto sent the job.
- It ends canceled or aborted, never reached the processing state and printed no impression.

Every 30 seconds, while some jobs are not final, recto asks CUPS about them and records the final
state (completed, canceled or aborted) and the impressions printed. A job that was canceled after
it started, or that printed some pages before it was aborted, keeps its full counts. A job that
CUPS no longer knows is finished without a refund, because nothing says it did not print.

A job's cost is the printer's price per black-and-white page, color page and sheet times its
counts, taken from the prices when the job is sent and kept with it: a later price change does
not change it. A printer without prices is free, and a job with unknown counts costs 0. See
[Set printer prices](../how-to/set-prices.md).

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

A job shows the pages CUPS reports for it, when it reports any. For a job sent through recto, its
owner and administrators see instead the pages recto counted for it, and its cost when it is
above 0. Others see the count CUPS reports, and no cost.
