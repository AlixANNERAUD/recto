# Save scans into Paperless-ngx

[Paperless-ngx](https://docs.paperless-ngx.com) imports every file that appears in its *consume*
folder. Point recto's **Save to folder** button at that folder, and a scan becomes a Paperless
document in a click.

## 1. Mount the consume folder into recto

In `compose.yml`, replace `./consume` with the consume folder of Paperless-ngx on the host, for
example:

```yaml
    environment:
      RECTO_SCAN_OUTPUT_DIR: /consume
    volumes:
      - /srv/paperless/consume:/consume
```

If Paperless-ngx runs in the same Compose project, mount the same host folder in both services.

## 2. Let recto write there

recto runs as user 65532. The consume folder usually belongs to the user Paperless-ngx runs as
(`USERMAP_UID` and `USERMAP_GID` in its configuration, 1000 by default). Run recto as that user,
on the `recto` service:

```yaml
    user: "1000:1000"
```

Then recreate recto: `docker compose up -d recto`.

The other way round, `sudo chown 65532:65532` on the folder, also works if Paperless-ngx can still
read and delete the files.

## 3. Prefer PDF

Paperless-ngx accepts PDF, PNG and JPEG, but a PDF keeps every page of a feeder batch in one
document, where PNG and JPEG keep only the first. Choose **PDF** on the Scan page.

## Check

Scan a page and press **Save to folder**. recto answers **Saved as /consume/scan-….pdf**, and a
few seconds later the document appears in Paperless-ngx. If recto answers **Cannot write to
/consume**, the user recto runs as cannot write to the folder: check step 2.

Files are named after the time of the scan, such as `scan-20261115-143000.pdf`, and never replace
an existing file: a clash gets a suffix, `scan-20261115-143000-1.pdf`. recto writes each file
under a hidden name and moves it into place in one step, so Paperless-ngx never picks up a
half-written scan. [How scans are kept and saved](../explanation/scans.md) explains the
exceptions on network shares.
