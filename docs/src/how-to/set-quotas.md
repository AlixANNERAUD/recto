# Set print quotas

Quotas limit what people print: pages, sheets, color pages, black-and-white pages, jobs or
money, per day, week, month, quarter or year, over the last few days, or in total. They are
optional: without a rule, recto prints as before and shows nothing about quotas.

This guide covers common setups. Every field and how rules combine are in the
[quotas reference](../reference/quotas.md), and the reasons behind them in
[How quotas work](../explanation/quotas.md).

## Before you start

- Set `RECTO_TIMEZONE` to your time zone, such as `Europe/Paris` (see
  [Configuration](../reference/configuration.md)). Days, weeks and months start at midnight
  there. With the default, `UTC`, a month may start in the middle of your night.
- To limit money, set the printer prices first (see [Set printer prices](set-prices.md)). A
  printer without prices is free, so a limit on cost never stops its jobs.
- Rules for a group, and pools, need groups, which come from the identity provider (see
  [Show the groups from the provider](sign-in.md#show-the-groups-from-the-provider)). Local
  accounts belong to no group: only rules for everyone, or for them by name, apply to them.

Rules apply to administrators too. They can grant themselves extra like anyone.

## Add a rule

1. Open **Administration**, then **Quotas**.
2. Fill in **New rule**:
   - **Name**, which people read on the Print and Jobs pages, such as `Monthly pages`;
   - **Applies to**: **Everyone**, **Each member of** a group, one user, or a group
     **together** (a pool);
   - **Counts**: **Pages**, **Sheets**, **Color pages**, **Black-and-white pages**, **Jobs** or
     **Cost**;
   - **Limit**: a whole number, or for cost an amount in `RECTO_CURRENCY`, such as `10.00`;
   - **Period**: **Day**, **Week, from Monday**, **Month**, **Quarter**, **Year**, **Rolling
     number of days**, or **Total, never renews**. For a rolling period, also fill in **Days of
     a rolling period**, from 1 to 366;
   - **Printers**: tick the printers it covers, none for every printer;
   - **Over the limit**: **Hard: refuse the job**, or **Soft: print and warn**;
   - **Enabled**, ticked unless you want to prepare the rule for later.
3. Press **Create rule**. The next job is checked against it.

The page lists the rules under a heading for whom they limit, such as **Everyone**, **Each member
of design**, a username, or **design together** for a pool. Each rule shows its name and a line
such as "50 color pages a month, on all printers, hard", where a rolling period reads "in any 7
days". Open **Change** followed by the rule's name to edit it and press **Save rule**, to press
**Disable** or **Enable**, or to delete it: tick **Delete this rule and its grants and resets**,
then press **Delete rule**. A disabled rule is kept, marked **Disabled**, but checks nothing and
shows on no other page.

A change applies at once, to what people already printed in the current window too: recto
counts from its record of every job, not from a counter. Raise a monthly limit on the 20th and
people have more left right away. Once a rule has grants or resets, its metric and period stay:
to count something else, create a new rule and disable or delete the old one.

## What people see

The Print page shows **What you have left**: each rule that applies to the person on a printer
they may use, with its name and what is left in the current window, such as "70 of 100 pages
left this month". A rolling period reads "in the last 7 days", a total "in total", a cost "7.50
EUR of 10.00 EUR left this month". A rule limited to some printers names them. Extra granted is
included, and what is left never shows below 0. When recto cannot read the quotas, the section
is hidden, and printing is refused until it can.

A job over a hard rule is refused before it reaches the printer, and the Print page says which
rule and what is left: "This job needs more than you have left of “Monthly pages”: 1 of 100
pages left this month." Nothing is counted. When the rule no longer applies by the time the page
shows, it says "This job goes over a print quota." instead.

A job over a soft rule prints, and the Jobs page warns: "This job went over “Monthly pages”: 0
of 100 pages left this month."

A job that uses exactly what is left passes. A job that counts nothing for a rule never trips
it: a black-and-white job prints when the color pages are used up, and so does a job on a free
printer under a limit on cost.

The job's owner and administrators see, on the Jobs page, the pages recto counted for each job
in place of the count CUPS reports, and its cost when it is above 0.

When recto cannot count the pages of a PDF, and a rule on that printer counts anything but jobs,
the job is refused: "recto could not count the pages of this PDF, and your quotas count pages.
Save it again as PDF and retry." Printing the document to PDF again from a viewer usually gives
a file recto can read. When recto is busy counting other documents, it says "recto is busy
counting the pages of other documents, and your quotas count pages. Try again in a moment."

## Recipes

### Cap a household

Keep the family's printing in check, and the children's color printing small.

| Name | Whom | Counts | Period | Limit | Enforcement |
| --- | --- | --- | --- | --- | --- |
| Monthly pages | Everyone | Pages | Month | 300 | Soft |
| Color | Everyone | Color pages | Month | 100 | Hard |
| Color for Sam | User `sam` | Color pages | Month | 10 | Hard |

Everyone is warned past 300 pages a month and can still print. Everyone has 100 color pages a
month, except Sam, who has 10: a rule for one user replaces the rule for everyone that counts the
same thing over the same period on the same printers, even when it is lower. With a limit of 0, Sam
could print in black and white only.

### Give each class a monthly allowance

A school whose identity provider puts students in groups such as `class-6a` and `class-6b`, and
teachers in `teachers`.

| Name | Whom | Counts | Period | Limit | Enforcement |
| --- | --- | --- | --- | --- | --- |
| Monthly pages | Everyone | Pages | Month | 50 | Hard |
| Class 6A | Group `class-6a` | Pages | Month | 100 | Hard |
| Class 6B | Group `class-6b` | Pages | Month | 150 | Hard |
| Teachers | Group `teachers` | Pages | Month | 1000 | Hard |

A group rule is an allowance **for each member**: every student of 6A has 100 pages a month of
their own. Someone in no group gets the 50 for everyone. Someone in several groups gets the most
generous: a teacher who is also in `class-6a` has 1000. A student who needs more for a project
gets it with [Grant extra](#give-extra).

Groups are read at sign-in, so a student who changes class counts with the new one from their
next sign-in.

### Share a color budget across a department

A company that gives each department one color budget, on its color printer `lab`.

| Name | Whom | Counts | Printers | Period | Limit | Enforcement |
| --- | --- | --- | --- | --- | --- | --- |
| Marketing color | Pool `marketing` | Color pages | `lab` | Month | 2000 | Hard |
| Personal color | Everyone | Color pages | `lab` | Month | 200 | Soft |

A pool is **one budget the members share**: marketing prints 2000 color pages a month on `lab`
together, and once they are used, nobody in marketing prints color there until next month. A job
must pass the pool and the person's own rules, so each marketer is also warned past 200. The pool
counts the jobs of the people who are members now: someone who leaves the group stops counting
toward it.

Black-and-white jobs on `lab`, and every job on other printers, are not limited by these rules.

### Limit spending

With [prices](set-prices.md) on the printers:

| Name | Whom | Counts | Period | Limit | Enforcement |
| --- | --- | --- | --- | --- | --- |
| Monthly budget | Everyone | Cost | Month | 10.00 | Hard |

Everyone may print 10.00 in `RECTO_CURRENCY` a month. The cost includes every price that
applies: per page, per color page and per sheet. A job's cost is fixed when it is sent, so a
later price change does not change what people have already used.

## Choose soft or hard

A **hard** rule refuses a job that would go over: nothing prints and nothing is counted. Use it
for budgets that matter.

A **soft** rule prints the job and warns, so usage can go past the limit. Use it to make people
aware of what they print, or to try a limit first: start soft, watch the usage on the Users page
for a month, then switch the rule to hard.

A job may meet several rules: any hard rule it goes over refuses it, and every soft one warns.

## Give extra

1. Open **Administration**, then **Users**. The **Quotas** column lists, on each person's row,
   the enabled rules for everyone, their groups or them that apply to them, with their usage,
   such as "Monthly pages: 30 of 100 pages used this month, plus 20 pages granted".
2. Under the rule, open **Grant extra or reset**.
3. Type the **Extra allowance**, a whole number or for cost an amount in the currency, and a
   **Reason**, then press **Grant extra**. The reason is required, up to 200 characters, and kept
   with your name.

The extra is for that person and that rule only, even when the rule is for everyone or a group.
It lasts until the end of the current window: the end of the month for a monthly rule, as many
days as the period for a rolling one, forever for a total.

For a pool, grant from the pool rule on the **Quotas** page instead: the extra goes to the whole
group.

## Reset someone's usage

**Reset** starts a person's usage of a rule over from now: what they printed before no longer
counts toward it. Open **Grant extra or reset** under the rule on the person's row, type a
**Reason** next to **Reset**, and press **Reset**. The next window starts afresh as usual. Extra
granted earlier stays.

A pool is reset from its rule on the **Quotas** page, for the whole group.

Grants and resets cannot be removed from the pages. Deleting a rule deletes them with it.
