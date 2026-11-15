# How scans are kept and saved

## From the scanner to a file

A scanner sends raw pixels: an A4 page in color at 300 dpi is about 26 MB of them, and a feeder
batch of twenty pages half a gigabyte. recto compresses each page as soon as it is read, to JPEG
for a PDF or JPEG result and to PNG for a PNG result, and drops the raw pixels. Memory then holds
one raw page at a time plus the compressed pages read so far, so a long batch costs little more
than a short one.

A single page may take at most 300 MB raw, about an A4 page in color at 1000 dpi. The limit
keeps one oversized scan from exhausting the server's memory; recto refuses the page with a message
to lower the resolution or the area.

When the last page is read, recto assembles the file. A PDF wraps the JPEG pages as they are,
without decoding them again, one page per sheet, sized to the scanned area at the scan's
resolution, so it prints at the true size of the original.

If the feeder jams in the middle of a batch, the pages read so far are dropped and the error is
shown. Keeping half a batch would mean deciding which pages made it, which the person standing at
the scanner does better by re-feeding the stack.

## Results belong to one person, for an hour

The finished file is written to `/tmp/recto-scans` under a random id, and the page shows **Scan
ready**. Only the person who scanned can download or save it: a result found under someone
else's id answers as if it did not exist.

Results are deleted an hour after the scan, and all of them when recto restarts, so the
container's disk never fills with forgotten scans. A result is not deleted when it is downloaded
or saved, so you can do both, or download it again.

## Saving never overwrites

**Save to folder** is designed for folders that something else watches, the consume folder of
Paperless-ngx above all. Two things matter there: the watcher must never see a half-written
file, and a new scan must never replace an older one.

recto writes the file under a hidden name (starting with a dot, which watchers skip), flushes it
to disk, then gives it its final name in one step that refuses to replace an existing file. If
the name is taken, it tries `-1`, `-2` and so on. The watcher sees a complete file appear,
exactly once.

Some file systems cannot do that refusing rename. recto then falls back:

1. **A hard link** to the final name, which also refuses to replace a file, then deletes the
   hidden name. The file still appears complete.
2. **Writing the final name directly**, created only if it does not exist. SMB and NFS shares
   that refuse both of the above end up here. A watcher may see the file while it is being
   written; Paperless-ngx waits for files to stop changing, so it copes.

In every case, an existing file is never replaced.
