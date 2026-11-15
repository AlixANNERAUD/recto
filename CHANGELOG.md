# Changelog

All notable changes to this project are documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [0.1.0](https://github.com/AlixANNERAUD/recto/releases/tag/v0.1.0) - 2026-11-15

### Added

- *(printing)* model printers, jobs and print options
- *(scanning)* drive SANE scanners through libsane
- serve printing, scanning and sign-in routes
- *(server)* add development mocks for mise run dev
- *(app)* print, jobs, printers and scan pages
- *(server)* send security headers with every response
- *(authentication)* read administration and groups from the sign-in token
- *(server)* keep users and sessions in the database

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

### Miscellaneous

- set up the workspace, tooling and an empty app
- *(server)* cover refused and failed single sign-on completions
