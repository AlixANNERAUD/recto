# Set printer prices

recto can put a price on what each printer prints, so every job records what it cost. Prices
are optional: a printer without them is free, and its jobs cost 0.

Each printer has three prices, and a job pays all that apply:

- **Black-and-white page**: for each page printed in black and white, copies included.
- **Color page**: for each page printed in color, copies included.
- **Sheet**: for each sheet of paper, copies included. A two-sided job uses half as many sheets.

For example, with 0.05 a black-and-white page and 0.01 a sheet, 3 copies of a 10-page document
printed two-sided cost 30 × 0.05 + 15 × 0.01 = 1.65.

## Set the prices of a printer

1. Choose the currency with `RECTO_CURRENCY` (see
   [Configuration](../reference/configuration.md)), `EUR` by default. It only decides how amounts
   read: recto converts nothing.
2. Open **Administration**, then **Printers**.
3. Open the printer's row (**Change access and prices**, or **Manage** with printer management on)
   and fill in the prices under **Prices**, such as `0.05`. A comma works as the decimal mark
   too, and an empty field means 0.
4. Press **Save prices**.

The row then shows the prices, or **Free** when they are all 0.

Amounts are kept in the currency's smallest unit, so they take at most its decimals: two for
`EUR` or `USD`, none for `JPY`, three for `KWD`. A price is at most 1,000,000.

## What changes

- A job's cost is computed when it is sent, from the prices at that moment, and kept with the
  job. A new price applies to later jobs only.
- A PDF whose pages recto cannot count costs 0. With a [quota](set-quotas.md) on pages or cost,
  such a PDF is refused instead.
- A job that printed nothing (refused by CUPS, or canceled before it started) is refunded: it
  stops counting.

Prices are kept under the printer's name in CUPS. When a printer is removed outside recto, its
row stays, marked **Not found**: press **Forget** to drop its prices and access rule. Removing a
printer from recto's Printers page forgets them at once.
