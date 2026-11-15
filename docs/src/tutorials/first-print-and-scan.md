# Your first print and scan

In this tutorial you run recto on your own machine, print a document on a virtual printer that
writes PDF files, and scan a test pattern from SANE's virtual scanner into a folder. You need no
printer and no scanner. It takes about twenty minutes.

You need:

- Docker with Compose, and git.
- An OpenID Connect provider in which you can create an application, such as Zitadel, Keycloak
  or Authentik. recto has no accounts of its own: everyone signs in through it.
- Port 8080 free on your machine.

## 1. Register recto with your identity provider

In your provider, create an application for recto:

- **Type:** a public client (no client secret) using the authorization code flow with **PKCE**.
  Zitadel calls it a *User Agent* application, Keycloak a client with *Client authentication*
  off.
- **Redirect URI:** `http://localhost:8080/auth/callback`
- **Post-logout redirect URI:** `http://localhost:8080/auth/signed-out`

Some providers refuse `http://` addresses unless told otherwise; in Zitadel, turn on
*Development mode* in the application. Copy the **client id**, and note the **issuer URL** of your
provider, such as `https://id.example.org`.

[Set up sign-in](../how-to/sign-in.md) has the details for each provider.

## 2. Get recto

Clone the repository, which contains a ready `compose.yml`, and create the folder where scans
will be saved. recto runs as user 65532, so that user must own it:

```sh
git clone https://github.com/AlixANNERAUD/recto.git
cd recto
mkdir -p consume && sudo chown 65532:65532 consume
```

Write your provider's details into a `.env` file next to `compose.yml`:

```sh
cat > .env <<'ENV'
RECTO_PUBLIC_URL=http://localhost:8080
RECTO_OIDC_ISSUER=https://id.example.org
RECTO_OIDC_CLIENT_ID=your-client-id
ENV
```

Replace the last two values with yours.

## 3. Add a virtual scanner

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

## 4. Start recto

```sh
docker compose up -d --wait
```

The command returns once recto reports itself healthy. Before the first release is published,
the image may not exist yet; if Compose says so, run `docker compose up -d --wait --build` to
build it from the checkout (it takes several minutes).

Check that recto sees the virtual scanner:

```sh
docker compose exec recto scanimage -L
```

```text
device `test:0' is a Noname frontend-tester virtual device
```

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

Open <http://localhost:8080>. recto sends you to your provider; sign in there. You come back to
the **Print** page, with your name at the top.

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

## 9. Clean up

Stop recto and the CUPS server, and delete their volumes, the override and the files you made:

```sh
docker compose down -v
rm -r compose.override.yml tutorial printed
sudo rm -r consume
```

## What you have learned

You ran recto next to a CUPS server, signed in through your provider, printed on a CUPS printer
and scanned from a SANE scanner, then saved the scan to a folder. With real hardware, only two
steps change: [Add a printer](../how-to/add-printer.md) and [Add a scanner](../how-to/add-scanner.md).
To run recto for good, follow [Install recto](../how-to/install.md).
