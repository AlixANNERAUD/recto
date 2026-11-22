# See who printed and scanned what

The **Usage** tab of the **Administration** page totals what was printed and scanned over a range
of days, for everyone and by user, group, printer or scanner. It reads the print ledger and the
scan log, which recto keeps on its own: there is nothing to turn on. What each column counts is
in the [reports reference](../reference/reports.md).

<picture>
  <source media="(prefers-color-scheme: dark)" srcset="../images/usage-dark.png">
  <img src="../images/usage-light.png" alt="The Usage tab of the Administration page: links for this month and last month and a form with two dates, links to the tables by user, group, printer and scanner and to the CSV downloads, the totals of the range, then a table with one row per user and their jobs, pages, sheets, color and black-and-white pages, cost, unknown pages, scans, scanned pages and saved scans.">
</picture>

## Before you start

Set `RECTO_TIMEZONE` to your time zone, such as `Europe/Paris` (see
[Configuration](../reference/configuration.md)). Days start at midnight there, so a job printed
at 23:30 on the last evening of the month counts in that month. With the default, `UTC`, a day
may end in the middle of your evening.

## Pick a range

1. Open **Administration**, then **Usage**. The page shows this month.
2. Press **Last month** for the month before, or fill in **From** and **To** and press **Show**.
   Both days are included.

The address keeps the range, so you can bookmark it or send it to another administrator.

## Switch tables

Under the range, **By user**, **By group**, **By printer** and **By scanner** pick the table
below the totals. The range stays the same.

- **By user** has a row for everyone who printed or scanned in the range, heaviest first, and a
  row **Anonymous (network)** for scans that came from the network without an account.
- **By group** counts each group's current members. Someone in two groups counts in both, and
  someone in no group in none, so the group rows need not add up to the total.
- **By printer** counts only printing, and **By scanner** only scanning.

## Export to a spreadsheet

Next to the tables, **Download** offers three CSV files for the range shown:

- **Print jobs (CSV)**: every print job, one per line, with its document, options, counts,
  cost and state. Refunded jobs are included and marked, so it is the whole record. To bill
  people, keep only the lines where `refunded` is `false` before you add up the cost; the
  Usage page and **This table (CSV)** already leave refunded jobs out.
- **Scans (CSV)**: every scan, one per line.
- **This table (CSV)**: the table on the page, by user, group, printer or scanner, without the
  total line.

LibreOffice Calc asks how to read the file: choose comma as the separator. Excel opens them
as they are, accents included, when Windows uses a comma as its list separator, as in English.
Where it uses a semicolon, as in French or German, Excel puts each line in one cell: open the
file with **Data**, **From Text/CSV** instead, which detects the commas. Costs use a decimal
point, so set the column's locale to English there if they don't read as numbers.

In a script, read them as UTF-8 with a byte order mark, such as `encoding="utf-8-sig"` in
Python. The columns are in the [reports reference](../reference/reports.md#csv-files).

The files hold personal data: who printed which document, and when. Each download is logged
with the administrator who made it.

## When the numbers look wrong

- **A job is missing.** Jobs that printed nothing (refused by CUPS, or cancelled before
  printing) are refunded and left out. The ledger export still lists them.
- **Pages are 0 but there are jobs.** recto could not count the pages of some documents. The
  **Unknown pages** column counts those jobs (jobs, not pages), and they add nothing to the
  page columns, the cost or the totals at the top.
- **The page says the dates are wrong.** Both dates must be between 2000 and 2999, and **To**
  cannot be before **From**.
