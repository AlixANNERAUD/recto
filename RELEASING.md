# Releasing

Releases are automated with [release-plz](https://release-plz.dev) through a release pull
request. recto is not published to crates.io: a release is a `vX.Y.Z` git tag, a GitHub release
with the changelog, and container images on `ghcr.io/alixanneraud/recto` and Docker Hub.

Every change reaches the protected `main` through a pull request, squash-merged once the
`CI passed` and `Pull request title` checks succeed, so each pull request title is one commit
that release-plz reads. The release pull request is merged by the owner, to release.

## Flow

1. Pull requests are squash-merged into `main`.
2. The `Release-plz` workflow opens or updates a release pull request, `chore: release vX.Y.Z`,
   that bumps the workspace version, updates `Cargo.lock` and adds the version's entry to
   `CHANGELOG.md`.
3. Review it; edit the version or the changelog in the pull request if needed (the version is
   `[workspace.package].version` in `Cargo.toml`).
4. Merge it when a release is wanted. The workflow then tags `vX.Y.Z`, drafts the GitHub
   release with the changelog entry, and calls `release.yml`, which builds both architectures
   natively, pushes the images tagged `X.Y.Z`, `X.Y`, `X` and `latest`, and publishes the
   release. It copies the same tags to Docker Hub, as `<DOCKERHUB_USERNAME>/recto`, and updates
   the repository's description there from the README (with the repository variable
   `DOCKERHUB_USERNAME` and the secret `DOCKERHUB_TOKEN`, an access token with read, write and
   delete access; without the variable the copy is skipped, and a failed copy does not stop the
   release).

Ordinary pushes never release (`release_always = false`). If the build fails, the release stays
a draft: fix the cause and run `Release` again for the tag from the Actions tab.

## Versions and the changelog

Every crate inherits the workspace version, and the versions are read from the `vX.Y.Z` tags
(`git_only`). The bump comes from the Conventional Commits since the last release:

| Commit | `>= 1.0.0` | `0.x.y` |
| --- | --- | --- |
| `fix:`, `perf:`, other types | patch | patch |
| `feat:` | minor | patch |
| `feat!:`, `fix!:` or a `BREAKING CHANGE:` footer | major | minor |

The changelog groups commits by type (Added, Fixed, Changed, and so on).

release-plz attributes a commit to the crates whose files it changes, so commits touching only
files outside `crates/` (the Dockerfile, workflows, documentation) neither appear in the
changelog nor open a release pull request; they ship with the next release. To release such a
change alone, bump the version, then push a tag by hand (`git tag v1.2.4 && git push origin
v1.2.4`): `release.yml` runs for it and creates the release with generated notes.

## Required setup

- `main` is protected (Settings, Branches): the status checks `CI passed` and `Pull request
  title` are required, with a linear history and no force pushes or deletions. Administrators
  may bypass the protection when a fix cannot wait for a pull request. Settings, General, Pull Requests: squash merging only, with the pull request title
  and description as the commit, auto-merge allowed and head branches deleted after merge.
- Settings, Actions, General, Workflow permissions: "Allow GitHub Actions to create and approve
  pull requests", so the workflow can open the release pull request. Pull requests opened with
  `GITHUB_TOKEN` do not start CI; push an empty commit to the release branch, or close and
  reopen the pull request, to run it.
- Repository secret `CODECOV_TOKEN` to upload coverage to Codecov; without it the upload is
  skipped without failing CI.
- Repository variable `DOCKERHUB_USERNAME` and secret `DOCKERHUB_TOKEN` for the Docker Hub copy.
- Repository secret `NANOBOM_TOKEN` to upload the SBOM to nanobom on each push to `main`
  (`.github/workflows/sbom.yml`); without it the upload is skipped.
