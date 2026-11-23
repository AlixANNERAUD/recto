# Changelog

All notable changes to this project are documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [0.1.3](https://github.com/AlixANNERAUD/recto/compare/v0.1.2...v0.1.3) - 2026-11-23

### Added

- *(printing)* read and run CUPS maintenance commands
- run a printer's maintenance commands from Administration > Printers

### Changed

- *(printing)* share the IPP transport of Cups and CupsAdmin
- *(printing)* share the IPP value helpers
- *(printing)* run page counts and raster conversions through Bounded
- *(printing)* build a queue's address with one printer_url
- *(domain)* insert held and incoming jobs through one insert
- *(domain)* share the password write of set and reset
- *(domain)* look up an identity's user in one place
- *(app)* show a refused form's alert with one refusal_alert
- *(app)* name duplex and color choices with shared labels
- *(app)* define coded enums with one coded! macro
- *(app)* filter usable printers and scanners with one function
- *(server)* build the direct printing user with session::user
- *(server)* share the refusal, give-back and redirect of a print
- *(server)* run held document I/O through HeldFiles::blocking
- *(server)* read IPP and eSCL bodies with one bounded reader
- *(server)* answer a missing resource with one not_found
- *(server)* keep client and address helpers in device_auth
- *(server)* start SANE through one start_sane
- *(scanning)* label a scanner with ScannerInfo::label
- cut text on a character boundary with floor_char_boundary
- *(server)* write a refusal's address with one refused_location
- *(scanning)* run the result store's I/O through ResultStore::blocking
- *(domain)* pick the rules on several printers with applicable_on
- *(app)* define the theme with the coded! macro
- *(server)* type the settings again with getargs-derive 0.1.2

### Documentation

- *(printing)* drop the stray comment above the string helper
- say CUPS lists a maintenance job under recto's own name

### Fixed

- *(server)* bound web form bodies in time and uploads per user
- *(app)* write amounts of money with the language's decimal mark
- *(app)* tell people without a usable printer or scanner that none is theirs
- *(server)* give large print uploads time to arrive, and say when one did not

### Miscellaneous

- *(deps)* parse the command line with getargs-derive instead of clap

### Performance

- *(domain)* read a user's standings on every rule at once
- *(server)* lock the user's row once per quota-checked job
- *(server)* look up only the job being canceled
- *(server)* print a held job with the pages counted on arrival
- *(app)* keep what scanners said they can do for the scan page

## [0.1.2](https://github.com/AlixANNERAUD/recto/compare/v0.1.1...v0.1.2) - 2026-11-23

### Changed

- *(domain)* one helper to lock rows on PostgreSQL

### Fixed

- *(scanning)* size scanned pages from the resolution used
- *(printing)* show a paused printer as paused, not offline
- *(domain)* report a taken username when creating it races
- *(domain)* store quota grants and resets in the rule's transaction
- *(authentication)* refuse an empty claim when only presence is required
- *(app)* say what releasing without a color choice does
- *(app)* keep the print form's options in step with its printer
- *(app)* load a scanner's settings again on Refresh after a failure
- *(scanning)* keep scan results in a directory no one else can make
- *(app)* render the Account page whole, without JavaScript
- *(server)* register mDNS services off the async runtime
- *(ipp)* honour job-ids in Get-Jobs
- *(domain)* check quotas against the rules that chose the locks
- *(escl)* keep anonymous clients' scan jobs apart
- *(escl)* answer ScannerCapabilities while the scanner is busy

### Performance

- *(scanning)* publish without links from the bytes in hand
- *(printing)* send and read IPP bodies without copying them
- *(printing)* tell the default printer from printer-type
- *(printing)* ask CUPS for both job lists at once
- *(migration)* drop the index on print_jobs.user_id alone
- *(domain)* read claimed groups at once when signing in
- *(domain)* count device passwords with GROUP BY
- *(app)* list the printers on the Jobs page only for held jobs
- *(app)* read the Print page's printers and quotas together
- *(server)* serve static files outside the session layers
- *(server)* build the service worker and the manifest once
- *(domain)* poll unfinished ledger entries without reading them twice
- *(server)* read the Users page's quota usage per rule
- *(server)* count a device's waiting jobs in the database
- *(server)* cancel a device's waiting jobs in one query

## [0.1.1](https://github.com/AlixANNERAUD/recto/compare/v0.1.0...v0.1.1) - 2026-11-23

### Added

- *(app)* translate the interface into French

### Fixed

- *(app)* let the last valid locale cookie win
- *(app)* write file sizes with the locale's decimal separator
- *(app)* fit the header and the navigation on phones
- *(app)* keep the switches off the page showing a new device password
- *(ipp)* answer devices in the language they ask for
- *(server)* keep pages and the offline page in the right language

## [0.1.0](https://github.com/AlixANNERAUD/recto/releases/tag/v0.1.0) - 2026-11-23

### Added

- *(printing)* model printers, jobs and print options
- *(scanning)* drive SANE scanners through libsane
- serve printing, scanning and sign-in routes
- *(server)* add development mocks for mise run dev
- *(app)* print, jobs, printers and scan pages
- *(server)* send security headers with every response
- *(authentication)* read administration and groups from the sign-in token
- *(server)* keep users and sessions in the database
- sign in with a local account
- *(server)* add migrate, backup and administrator commands
- *(app)* add the users and groups administration pages
- let people choose a light or dark theme
- let the operator brand recto
- make recto installable as an app
- record the jobs recto submits
- keep each person's jobs private on request
- limit printers and scanners to groups
- *(app)* add the printers and scanners administration pages
- *(printing)* add a CUPS administration client
- manage printers from the administration page
- count every printed job in the ledger
- refund jobs that printed nothing
- price printed pages per printer
- *(domain)* evaluate print quota rules
- enforce print quotas at submission
- *(app)* add the quotas administration page
- *(app)* grant extra and reset quotas from the users page
- log every scan
- *(app)* add the usage administration page
- export usage as CSV
- add device passwords for printers and scanners on the OS
- *(domain)* keep held print jobs until released or expired
- receive print jobs from computers and phones
- release held print jobs from the jobs page
- *(server)* describe recto for DNS-SD and the LAN listener's TLS
- accept raster documents and printing directly from devices
- download an AirPrint profile for a device password
- scan from the OS over eSCL
- announce recto's printer and scanners on the local network

### Changed

- keep the signed-in user in the interface crate
- tidy branding, theme and routing code
- tidy the device routes and doc comments
- share the print path between the form and held jobs

### Documentation

- align the agent rules with working on main and tidy the config tests
- fix two rustdoc links to private items

### Fixed

- expire idle sessions and keep the interface behind sign-in
- keep signed-out sessions from coming back
- *(scanning)* refuse scanner ids that are not in the scanner list
- *(server)* refuse a required value without a claim and a public URL with a path
- *(scanning)* compress pages as they are read and bound the size of a page
- *(authentication)* read the profile from UserInfo and sign out without an ID token
- *(app)* tell signed-out sessions and proxy timeouts apart, and stop idle refreshes
- *(server)* list the scanners in the background at startup
- *(server)* refuse to back up a database that does not exist
- refuse manual changes to administration granted by the identity provider
- *(server)* describe the issuer option without internal notes
- *(server)* go straight to the provider until an account has a password
- *(server)* end the provider's session only after single sign-on
- *(server)* check the password minimum length and command input
- *(server)* count password changes and IPv6 networks in the rate limit
- *(app)* give the administration section an address and page titles
- *(server)* forget the previous user when a single sign-on completes
- keep brand accent text readable and cap the icon size
- make the service worker safer and the offline page self-contained
- check printer access under the name CUPS answers with
- keep job records with their users and tighten device access
- refuse invalid printer names and addresses with credentials
- refuse to add a printer whose name is already taken
- manage queues by CUPS's name rule and bound printer texts
- bound the reading of uploaded PDFs and finish old ledger rows
- *(print)* keep the charge when CUPS may have queued the job
- *(quotas)* cap amounts and keep the metric of adjusted rules
- *(print)* decide quota enforcement inside the write transaction
- *(app)* word a user's quota usage through one locale key
- *(app)* keep the query when switching theme
- wait at most 2 seconds for the scan log
- name uncounted jobs and refuse repeated usage dates
- keep device secrets out of redirects and logs
- stop revoked devices from locking out the ones beside them
- end device passwords when a user's access ends
- turn OS integration off by default until devices can print
- say when a device name has a control character
- *(server)* share one device limiter across device routes
- *(server)* bound IPP request bodies in size, time and number
- drop an unconfirmed release's document and preview only waiting jobs
- *(printing)* name an unknown which-jobs in the Get-Jobs refusal
- *(printing)* refuse deeply nested IPP requests before parsing them
- *(server)* read at most 64 KiB of an IPP request without a document
- *(server)* close idle connections and cap them per address
- *(scanning)* keep a canceled eSCL job's scanner busy until SANE returns
- *(server)* answer a busy eSCL scanner at once
- *(server)* finish a release from the Jobs page when the tab closes
- *(printing)* follow RFC 8011 in IPP refusals, versions and job attributes
- *(scanning)* scan one sheet for an eSCL preview and end busy jobs
- *(server)* keep DNS-SD scanner names within 63 bytes and need OS integration
- *(server)* put back held jobs that were counted but never sent

### Miscellaneous

- set up the workspace, tooling and an empty app
- *(server)* cover refused and failed single sign-on completions
- tighten theme and offline page tests
- mock printer administration in the dev server
- pin the cross-site check on the device password forms
- make the device and release tests prove their names
- format the connections module
- pin the review's gaps in held jobs, the sweep and LAN addresses
- wait for the held document too in the sweep test
