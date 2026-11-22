# Quotas

Quota rules limit what people print, counted from the ledger recto keeps of every job (see
[What recto counts](printing.md#what-recto-counts)). Administrators manage them on the
**Quotas** administration page. To set them up, see [Set print quotas](../how-to/set-quotas.md).

Without an enabled rule, nothing is checked and nothing about quotas is shown.

## Rule fields

| Field | Values |
| --- | --- |
| Name | Shown to people on the Print and Jobs pages and in refusals. Required; spaces around it are removed. |
| Enabled | A disabled rule is kept, but checks nothing and is shown to nobody but administrators. |
| Subject | Whom the rule limits: `everyone` (each person), `group:<id>` (each member of the group), `user:<id>` (one user) or `pool:<id>` (the members of the group together). The group or user must exist. |
| Printers | CUPS printer names, compared without case, each kept once. None for every printer. |
| Metric | What the rule counts: `pages`, `sheets`, `color_pages`, `mono_pages`, `jobs` or `cost`. |
| Period | `day`, `week`, `month`, `quarter`, `year`, `rolling` with a number of days from 1 to 366, or `total`. See [Periods](#periods). |
| Limit | A whole number, 0 to 1,000,000,000, in the metric's unit. A cost is typed in `RECTO_CURRENCY`, such as `10.00`, and kept in the currency's smallest unit. |
| Enforcement | `hard` refuses a job that would go over; `soft` prints it with a warning. |

A rule with no name, a limit that is not a whole number (for cost, not an amount in
`RECTO_CURRENCY`), a negative limit, a limit over 1,000,000,000, a rolling period outside 1 to 366 days, a group or user that
does not exist, or an empty printer name is refused.

## Metrics

| Metric | A job counts |
| --- | --- |
| `pages` | Its pages: the selected pages times the copies. An image is one page. |
| `sheets` | Its sheets of paper: the selected pages, halved and rounded up when two-sided, times the copies. |
| `color_pages` | Its pages when printed in color, otherwise 0. |
| `mono_pages` | Its pages when printed in black and white, otherwise 0. |
| `jobs` | 1. |
| `cost` | Its cost, from the printer's prices when it was sent (see [Set printer prices](../how-to/set-prices.md)). |

Color is the color mode chosen on the Print page, not what the pages contain: a color job of
black text counts as color pages.

## Which rules apply to a job

1. Only enabled rules count.
2. A rule covers the job's printer when it has no printers, or lists that printer.
3. A rule matches the person when its subject is `everyone`, a group they are in, them, or the
   pool of a group they are in.
4. `everyone`, `group` and `user` rules are personal allowances. Those with the same metric, the
   same period (and number of days) and the same printers form a **family**, and one rule per
   family remains, the most specific: a `user` rule over `group` rules over an `everyone` rule.
   Among `group` rules, the highest limit wins; between equal limits, the oldest rule. The most
   specific rule wins even when its limit is lower.
5. `pool` rules never join a family: every matching pool applies, next to the personal rules.

The job must pass every rule that remains.

For example, with "everyone: 50 color pages a month", "group design: 500 color pages a month" and
"everyone: 1000 pages a month", designers have 500 color pages a month and the others 50, and
everybody has 1000 pages a month in all. A rule on the printer `lab` and a rule on every printer
are in different families, so a job on `lab` must pass both.

Groups are those recorded at the person's last sign-in (see [Groups](accounts.md#groups)). Local
accounts belong to no group. Rules apply to administrators too.

## Pools

A pool rule is one budget for a group: its usage is the sum of the jobs of every current member
of the group, on the rule's printers. Membership is read when the usage is: a person who left the
group no longer counts toward it, and one who joined brings their jobs of the current window
with them. Grants and resets of a pool are for the whole group.

## Periods

Calendar periods follow `RECTO_TIMEZONE` (see [Configuration](configuration.md)), `UTC` by
default.

| Period | The current window |
| --- | --- |
| `day` | From midnight today to midnight tomorrow. |
| `week` | From midnight on Monday, for seven days. |
| `month` | From midnight on the first of the month to the first of the next. |
| `quarter` | From midnight on January 1, April 1, July 1 or October 1, for three months. |
| `year` | From midnight on January 1 to the next. |
| `rolling` | The last N times 24 hours before now. A job stops counting exactly N days after it was sent. |
| `total` | Every job, ever. It never renews. |

A job belongs to the window of the moment recto sent it. Across a change of clocks a day lasts 23
or 25 hours; where the change skips midnight, the day starts at the first local time that exists.
Changing `RECTO_TIMEZONE` moves every calendar window at once.

The Quotas page words a rule's limit with "a day", "a week", "a month", "a quarter", "a year",
"in any 7 days" or "in total", such as "50 color pages a month, on all printers, hard". What is
left and what was used are worded with "today", "this week", "this month", "this quarter", "this
year", "in the last 7 days" or "in total", such as "70 of 100 pages left this month".

## Usage

What a person (or a pool) **used** of a rule is the sum of the metric over their jobs that:

- were not refunded (see [Refunds](#refunds));
- were sent on one of the rule's printers, or any printer when it has none;
- were sent in the current window, after the latest reset (see [Adjustments](#adjustments)).

A job whose pages recto could not count adds nothing, except 1 to `jobs`.

What is **left** is the limit, plus the grants that have not expired, minus what was used. It
can be below 0 after a soft rule let jobs through; the pages then show 0.

## The check

When someone prints, for each rule that applies:

- A job that counts 0 for the rule passes it, even when nothing is left: a black-and-white job
  under a `color_pages` rule, a job on a free printer under a `cost` rule.
- A job that counts no more than what is left passes.
- Otherwise, a `hard` rule refuses the job, and a `soft` rule lets it through with a warning.

A refused job is not sent to CUPS and is not counted. The Print page names the first refusing
rule, by creation order, and what is left of it. A job over soft rules prints, and the Jobs page
names each of them.

The check and the entry of the job in the ledger happen together, so two jobs sent at the same
moment cannot both use the last pages (see [How quotas work](../explanation/quotas.md)).

## Adjustments

Administrators adjust a rule for one person, or for a pool's group, without changing the rule.
Each adjustment keeps a reason (1 to 200 characters), who made it and when.

| Kind | Effect |
| --- | --- |
| Grant extra | Adds an amount, more than 0 and at most 1,000,000,000, to the limit until it expires: at the end of the window it was granted in for a calendar period, N days after it was granted for a `rolling` period, never for `total`. |
| Reset | Only jobs sent after it count toward the rule. The latest reset counts. Grants stay. |

An adjustment of an `everyone` or `group` rule is for one person, not for everyone or the group.
An adjustment of a pool rule is for the group. An adjustment belongs to its rule: when another
rule replaces it for the person, such as a new rule for them by name, the adjustment no longer
matters.

A reset is a moment, not an amount, so it stays exact on rolling periods, where earlier jobs age
out one by one. Changing a rule keeps its adjustments, and a grant keeps the expiry it got when it
was made. So a rule with adjustments keeps its metric and its kind of period, since a grant of
pages must not turn into an amount of money: such a change is refused with "This rule has grants
or resets, so its metric and period cannot change. Create a new rule instead." Its other fields,
the days of a rolling period included, still change. Deleting a rule deletes its adjustments.

Personal adjustments are made on the **Users** page, pool adjustments on the **Quotas** page.

## Refunds

A refunded job stops counting toward every rule, which gives back what it used. A job is
refunded when CUPS refuses it, when CUPS never received it, or when it ends canceled or aborted
before it started printing (see [Refunds](printing.md#refunds)). A job canceled after it started
keeps its full counts.

## Counting

Counts are taken when the job is sent, before CUPS prints it, and never change afterwards.

The pages of a PDF come from reading the file, at most two PDFs at a time and for at most 10
seconds each, waiting included. recto walks the page tree as CUPS does: a page the tree lists
several times counts each time, since it prints each time. The page count the file states is
not used.

A PDF is uncountable when it cannot be read (damaged, or encrypted), or when its page tree loops
back on itself, is more than 64 levels deep, holds a node that is neither a page nor a list of
pages, or has no pages or more than 100,000. When the counts matter, because a rule that applies
to the job on that printer counts anything but `jobs`:

| Case | The job is |
| --- | --- |
| The PDF is uncountable, or reading it takes too long | refused: "recto could not count the pages of this PDF, and your quotas count pages. Save it again as PDF and retry." |
| Other PDFs held both places for the whole 10 seconds | refused: "recto is busy counting the pages of other documents, and your quotas count pages. Try again in a moment." |

Otherwise such a job prints, with unknown counts.

## When the database fails

Quotas fail closed. When recto cannot read the rules, it refuses the job, since any of them might
apply. With at least one enabled rule, it also refuses the job when it cannot read the person's
groups or the printer's prices, or cannot check and record the job. The person reads "The
printing service is unavailable. Try again later.", and the log says why (see
[Logs](logs.md#events)). Printing past a limit because the database was down would make every
limit easy to bypass. The Print page then shows no **What you have left** section.

Without an enabled rule, a ledger entry that cannot be written does not stop printing; that job
is not counted.
