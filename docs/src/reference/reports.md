# Reports

The **Usage** administration page (`/administration/usage`) totals the print ledger and the scan
log over a range of days. Only administrators see it; anyone else gets the not-found page.

## Range

| Query parameter | Values |
| --- | --- |
| `from`, `to` | Dates as `YYYY-MM-DD`, both included. Without both, the current month. |
| `by` | `user` (default), `group`, `printer` or `scanner`. Anything else is `user`. |

The range runs from midnight of `from` to midnight after `to`, in `RECTO_TIMEZONE`. A day the
clocks change is 23 or 25 hours long, and counts as one day.

**This month** and **Last month** fill in the calendar month that contains today and the one
before it, in `RECTO_TIMEZONE`.

The page answers `400`, with a message and without tables, when only one date is given, a date
does not read as `YYYY-MM-DD`, `to` is before `from`, a year is outside 2000 to 2999, or a
parameter is given twice. The CSV files answer `400` in the same cases.

## What counts

- **Printing** comes from the ledger: one row per job recto sent. Refunded jobs, which printed
  nothing, are left out. Jobs still pending or printing count.
- **Scanning** comes from the scan log: one row per finished scan, previews excluded (see
  [Scan log](scanning.md#scan-log)).
- **Users** are named by their current display name and username. Scans from the network without
  an account are one **Anonymous (network)** row.
- **Groups** count their current members, as the identity provider listed them at each member's
  last sign-in, not the members they had when they printed. A person in two groups counts in
  both, and a person in no group in none, so group rows need not add up to the total.
- **Printers and scanners** are named as CUPS and SANE name them: the printer's queue name, the
  scanner's device id such as `airscan:e0:Office`.

Rows are sorted by printed pages, then scanned pages (scanned pages alone for scanners), most
first, then by name.

## Columns

| Column | Meaning |
| --- | --- |
| Jobs | Print jobs, refunded ones excluded. |
| Pages | Printed pages, copies included. |
| Sheets | Sheets of paper, two-sided jobs counting half. |
| Color pages, Black-and-white pages | Pages by color mode. |
| Cost | The sum of the jobs' cost in `RECTO_CURRENCY`, from the printer prices when each job was sent. |
| Uncounted jobs | Jobs whose pages recto could not count, such as a PDF it could not read. They add nothing to the page columns or the cost. |
| Scans | Finished scans. |
| Scanned pages | Their pages. |
| Saved | Scans saved with **Save to folder**. Downloads are not tracked. |

The **Total** line above the table counts the whole range: printed pages, jobs, cost and
uncounted jobs, scanned pages and scans, anonymous scans included.

The user and group tables have every column, the printer table the print columns, and the
scanner table the scan columns. A user who only scanned shows 0 for printing, and the other way
round.

## CSV files

The **Download** links export the range shown, with the same query. Each file is RFC 4180:
comma separated, lines ending with CRLF, cells holding a comma, a quote or a line break between
double quotes, quotes doubled. It starts with a UTF-8 byte order mark, so that Excel reads
accents.

- Headers are fixed snake_case English, whatever the interface language: they are for
  scripts.
- Costs are amounts in `RECTO_CURRENCY` with a dot, such as `0.66`, next to a `currency` column.
- Times are RFC 3339 in `RECTO_TIMEZONE`, with their offset, such as
  `2026-11-17T11:00:00+01:00`.
- Unknown counts are empty cells; yes and no are `true` and `false`.
- A text cell starting with `=`, `+`, `-`, `@`, a tab or a carriage return gets a leading `'`,
  so that a spreadsheet shows it rather than running it as a formula: a document named
  `=HYPERLINK("http://example.com")` stays text.

Each download is logged as `usage exported`, with who downloaded which file (see
[Logs](logs.md)).

### Print jobs

`print-jobs-<from>-<to>.csv`: one line per ledger row of the range, oldest first, refunded ones
included.

| Column | Content |
| --- | --- |
| `id` | The row's number in the ledger. |
| `submitted_at` | When recto sent the job. |
| `username` | The username when the job was sent. |
| `printer` | The CUPS queue. |
| `document_name` | The uploaded file's name. |
| `document_format` | Its MIME type, such as `application/pdf`. |
| `document_pages` | The pages of the document itself; empty when unknown. |
| `copies` | Copies asked for. |
| `sides`, `color_mode` | The IPP keywords, such as `two-sided-long-edge` and `monochrome`. |
| `pages`, `sheets`, `color_pages`, `mono_pages` | What the job counts for, copies and page range included; empty when unknown. |
| `cost`, `currency` | Its cost. |
| `state` | The last state recto saw: `pending`, `processing`, `completed`, `canceled` and so on. |
| `refunded` | `true` for a job that printed nothing, which the totals leave out. |
| `cups_job_id` | The CUPS job number; empty if CUPS never confirmed it. |

### Scans

`scans-<from>-<to>.csv`: one line per logged scan, oldest first.

| Column | Content |
| --- | --- |
| `id` | The row's number in the scan log. |
| `scanned_at` | When the scan finished. |
| `source` | `web` for the Scan page. |
| `username` | The username at the time; empty for an anonymous network scan. |
| `scanner` | The SANE device id. |
| `pages` | Pages in the file. |
| `format` | `pdf`, `png` or `jpg`. |
| `saved` | Whether it was saved with **Save to folder**. |

### Totals

`usage-by-<table>-<from>-<to>.csv`: the rows of the table on the page, in its order, without
the total line, which would get in the way of a pivot table.

| Table | Columns |
| --- | --- |
| `user` | `username`, `display_name`, then the print and scan columns. The anonymous line has an empty `username` and the `display_name` `anonymous`. |
| `group` | `group`, then the print and scan columns. |
| `printer` | `printer`, then the print columns. |
| `scanner` | `scanner`, then the scan columns. |

The print columns are `jobs`, `pages`, `sheets`, `color_pages`, `mono_pages`, `cost`,
`currency` and `uncounted_jobs`; the scan columns `scans`, `scanned_pages` and `saved_scans`.
They count as the page does (see [Columns](#columns)).

## Retention

The ledger and the scan log are kept as long as the database. recto has no way to delete a
range.
