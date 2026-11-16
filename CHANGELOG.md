# Changelog

All notable changes to this project are documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [0.1.0](https://github.com/AlixANNERAUD/recto/releases/tag/v0.1.0) - 2026-11-16

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

### Changed

- keep the signed-in user in the interface crate
- tidy branding, theme and routing code
- tidy the device routes and doc comments

### Documentation

- align the agent rules with working on main and tidy the config tests

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

### Miscellaneous

- set up the workspace, tooling and an empty app
- *(server)* cover refused and failed single sign-on completions
- tighten theme and offline page tests
- mock printer administration in the dev server
