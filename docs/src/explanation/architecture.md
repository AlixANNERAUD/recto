# How recto works

recto is a web layer. The hard parts of printing and scanning, drivers, page description
languages and device protocols, are left to two mature projects: CUPS and SANE. recto turns them
into one web page anyone in the house can open, behind the sign-in they already have.

```text
 browser ──HTTPS──> reverse proxy ──HTTP──> recto ──IPP over HTTP──> CUPS ──> printers
                                              │
                                              ├──libsane──> SANE backends ──> scanners
                                              │
                                              └──HTTPS──> OpenID Connect provider
```

## Printing: an IPP client of CUPS

CUPS speaks IPP, the Internet Printing Protocol, which is HTTP carrying binary requests. recto is
an IPP client, like the print dialog of a desktop. It asks CUPS for its printers and their
attributes (state, supported options, supply levels), sends documents with `Print-Job`, lists jobs
with `Get-Jobs` and cancels them with `Cancel-Job`.

recto sends PDF, JPEG and PNG as they are. CUPS converts them for each printer with its filters,
which is why recto needs no drivers and no conversion of its own. The options shown on the Print
page are the ones CUPS says the printer supports, so a printer without two-sided printing shows
no Sides field.

CUPS runs in its own container, with its spool and its printers in volumes. recto keeps nothing
about printers: every page load asks CUPS.

## Scanning: libsane in a worker thread

SANE is a C library with one backend per family of scanners. recto loads libsane inside its own
process, through the Rust bindings of the `libsane` crate, rather than calling `scanimage`. That
gives it the scanner's real options (resolutions, modes, sources, the size of the glass) and
precise errors (jammed, cover open, no documents in the feeder).

libsane is not thread-safe and allows one instance per process, so a single thread owns it. Every
request to list scanners, read options or scan is a message to that thread, answered when done.
Scans therefore run one at a time, in arrival order: a second person's scan waits for the first.
If the worker thread panics, it is started again and the scan in progress reports an error.

A scan reads each page from the scanner, compresses it at once, and assembles the file (PDF, PNG
or JPEG) at the end. [How scans are kept and saved](scans.md) follows a scan from there.

## The interface: rendered on the server, islands where needed

The pages are built with Leptos and rendered on the server: each one arrives as complete HTML.
Printing and cancelling are plain HTML forms, so they work without JavaScript, on any browser.

Only the parts that need to react are *islands*, small WebAssembly components that take over a
piece of the page: the print form (to show only the options of the chosen printer and catch
oversized files), the job list (to refresh itself), and the whole scan page (preview, area
selection and results).

## State

recto keeps almost nothing:

| What | Where | Lifetime |
| --- | --- | --- |
| Sessions | memory | 12 hours without a request, or until recto restarts |
| Scan results | `/tmp/recto-scans` | 1 hour, or until recto restarts |
| The scanner list | memory | until someone looks again |
| Configuration | environment | the life of the process |

Printers, jobs and users live in CUPS and in the identity provider. That is why recto has no
database, no migrations and nothing to back up, and why an upgrade only replaces the container.
The price is that a restart signs everyone out.
