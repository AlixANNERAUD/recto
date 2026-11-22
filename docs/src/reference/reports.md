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
does not read as `YYYY-MM-DD`, `to` is before `from`, or a year is outside 2000 to 2999.

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
| Unknown pages | Jobs whose pages recto could not count, such as a PDF it could not read. They add nothing to the page columns or the cost. |
| Scans | Finished scans. |
| Scanned pages | Their pages. |
| Saved | Scans saved with **Save to folder**. Downloads are not tracked. |

The **Total** line above the table counts the whole range: printed pages, jobs and cost, scanned
pages and scans, anonymous scans included.

The user and group tables have every column, the printer table the print columns, and the
scanner table the scan columns. A user who only scanned shows 0 for printing, and the other way
round.

## Retention

The ledger and the scan log are kept as long as the database. recto has no way to delete a
range.
