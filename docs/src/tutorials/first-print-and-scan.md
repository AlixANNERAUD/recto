# Your first print and scan

In this tutorial you run recto on your own machine, create its first administrator, print a
document on a virtual printer that writes PDF files, and scan a test pattern from SANE's virtual
scanner into a folder. You need no printer, no scanner and no identity provider. It takes about
twenty minutes.

You need:

- Docker with Compose, and git.
- Port 8080 free on your machine.

## 1. Get recto

Clone the repository, which contains a ready `compose.yml`, and create the folder where scans
will be saved. recto runs as user 65532, so that user must own it:

```sh
git clone https://github.com/AlixANNERAUD/recto.git
cd recto
mkdir -p consume && sudo chown 65532:65532 consume
```

Write the address you will open recto at into a `.env` file next to `compose.yml`:

```sh
echo 'RECTO_PUBLIC_URL=http://localhost:8080' > .env
```

That is the only setting recto needs. Without an identity provider, people sign in with local
accounts, which you will create.

## 2. Add a virtual scanner

SANE ships a `test` backend: a scanner that only exists in software and returns test patterns.
Enable it with two configuration files:

```sh
mkdir -p tutorial/sane
printf 'test\n' > tutorial/sane/dll.conf
cat > tutorial/sane/test.conf <<'CONF'
number_of_devices 1
test-picture "Color pattern"
br_x 200.0
br_y 200.0
CONF
```

Then mount them into recto with a `compose.override.yml`, which Compose reads next to
`compose.yml`:

```sh
cat > compose.override.yml <<'YAML'
services:
  recto:
    volumes:
      - ./tutorial/sane/dll.conf:/etc/sane.d/dll.conf:ro
      - ./tutorial/sane/test.conf:/etc/sane.d/test.conf:ro
YAML
```

## 3. Start recto

```sh
docker compose up -d --wait
```

The command returns once recto reports itself healthy. Before the first release is published,
the image may not exist yet; if Compose says so, run `docker compose up -d --wait --build` to
build it from the checkout (it takes several minutes).

recto created its database, a SQLite file with no users yet, in the `recto-data` volume.

Check that recto sees the virtual scanner:

```sh
docker compose exec recto scanimage -L
```

```text
device `test:0' is a Noname frontend-tester virtual device
```

## 4. Create your administrator

Nobody can sign in yet: the database has no users. Create the first one, an administrator named
`admin`:

```sh
docker compose exec recto /app/recto create-administrator --username admin
```

recto asks for a password twice. Choose one of at least 12 characters; it is not shown as you
type. The last line the command prints says `administrator created`.

## 5. Add a virtual printer

The CUPS server next to recto includes CUPS-PDF, a printer that writes each job to a PDF file.
Add it under the name `pdf`:

```sh
docker compose exec cups lpadmin -p pdf -E -v cups-pdf:/ \
  -m lsb/usr/cups-pdf/CUPS-PDF_opt.ppd -D "Virtual PDF printer"
```

`lpadmin` warns that printer drivers are deprecated. The warning is about future versions of
CUPS; the printer works.

## 6. Sign in

Open <http://localhost:8080>. recto shows its sign-in page. Enter `admin` and your password, then
press **Sign in**. You land on the **Print** page, with **admin** at the top and an
**Administration** entry in the menu, which only administrators have.

The three small buttons at the top choose the theme: follow the device, light or dark. The
sign-in page has them too. recto remembers the choice in this browser.

## 7. Print a document

1. Under **File**, choose any PDF on your computer.
2. Under **Printer**, choose **pdf**. The card next to the form shows its state: **Ready**.
3. Leave **Copies** at 1, and **Pages** empty to print them all.
4. Press **Print**.

You land on the **Jobs** page, which says **Sent to the printer as job 1.** The job shows as
**Printing**, then **Printed**: the list refreshes itself every few seconds.

The virtual printer wrote the job to a file in the CUPS container. Copy it out and open it:

```sh
docker compose cp cups:/var/spool/cups-pdf/ANONYMOUS ./printed
ls printed
```

It is the document you printed, named after it.

## 8. Scan a page

1. Open **Scan**. Under **Scanner**, recto shows **Noname frontend-tester**. If the list is
   empty, press the round arrow next to it to look again.
2. Set **Resolution** to 150 dpi, **Colors** to **Color**, **Source** to **Flatbed** and
   **Format** to **PDF**.
3. Press **Preview**. A quick, low-resolution image of the whole glass appears: stripes of
   color, the test pattern.
4. Drag on the preview to draw a rectangle over its top half. recto says **Only the selected part
   will be scanned.**
5. Press **Scan**.

When it is done, **Scan ready** shows a file named after the time, such as
`scan-20261115-143000.pdf`, with **1 page**.

Press **Download** to get it in your browser. Then press **Save to folder**: recto answers
**Saved as /consume/scan-….pdf**, and the file is in the `consume` folder you created:

```sh
ls consume
```

That folder is where, at home, you would point Paperless-ngx (see
[Save scans into Paperless-ngx](../how-to/paperless.md)).

## 9. Print from your computer

Computers and phones can also print to recto from any application's Print dialog. Their
documents wait on the **Jobs** page until you choose a printer for them. A device signs in with
a device password of its own, never with your account's password:

1. Select your name, or the account icon, at the top right to open **Account**.
2. Under **Devices**, set **Device name** to `Laptop` and press **Create device password**.
3. recto shows **Device password for Laptop** once, with the **Password**, the **Printer
   address** and the **Personal printer address**.

recto takes device passwords over HTTPS only, which this tutorial leaves out, so stop here:
press **Revoke** next to **Laptop** in the list. Once recto runs behind a reverse proxy,
follow the guide for your system: [macOS](../how-to/print-from-macos.md),
[Windows](../how-to/print-from-windows.md), [Linux](../how-to/print-from-linux.md),
[iPhone and iPad](../how-to/print-from-ios.md) or [Android](../how-to/print-from-android.md).

## 10. Create an account for someone else

1. Open **Administration**. The **Users** tab lists one user, **admin**, marked **You**.
2. Under **New local user**, set **Username** to `alex`, **Display name** to `Alex`, and a
   **Password** of at least 12 characters. Leave **Administrator** unchecked.
3. Press **Create user**. recto says **The account alex was created.**, and Alex appears in the
   list, with **Password** as the way to sign in.

Press **Sign out** at the top, then sign in as `alex`. The menu has no **Administration** entry:
Alex can print and scan, but not manage accounts.

## 11. Clean up

Stop recto and the CUPS server, and delete their volumes (the database among them), the override
and the files you made:

```sh
docker compose down -v
rm -r compose.override.yml tutorial printed
sudo rm -r consume
```

## What you have learned

You ran recto next to a CUPS server, created its first administrator and a second account,
printed on a CUPS printer and scanned from a SANE scanner, then saved the scan to a folder, and
made a device password for a computer's Print dialog. With
real hardware, only two steps change: [Add a printer](../how-to/add-printer.md) and
[Add a scanner](../how-to/add-scanner.md). To run recto for good, follow
[Install recto](../how-to/install.md); to sign in with an identity provider you already have, see
[Set up single sign-on](../how-to/sign-in.md).
