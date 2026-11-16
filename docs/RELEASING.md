# Releasing Inlet

Every Inlet release is a pre-release (a beta) and is never marked as the latest release. Releases are cut from `main` with [release-please](https://github.com/googleapis/release-please). Nothing is released by merging a normal pull request.

## How a beta is cut

1. Work lands on `v0` and, when `v0` is ready, is merged into `main`. Commits follow Conventional Commits; the release notes are built from them.
2. On every push to `main`, `release-please.yml` opens or updates one release PR, titled `chore(main): release X.Y.Z`. It changes the version in `Cargo.toml` and in the workspace crates in `Cargo.lock`, and adds the new section to `CHANGELOG.md`.
3. The maintainer reads the release PR and merges it. This is the approval to release.
4. release-please creates the tag `vX.Y.Z` and a draft pre-release for it. The draft holds the changelog as its notes and is not visible to the public.
5. `release-please.yml` then calls `release.yml` with the tag. It builds the three platforms, then pauses before macOS signing.
6. The maintainer approves the signing job in the protected environment `release` (Actions run page, "Review deployments"). Only `main` and `v*` tags may use that environment.
7. `release.yml` attaches the files and `SHA256SUMS.txt` to the draft and publishes it as a pre-release. It fails if the result is not a published pre-release, or if it is the latest release.

## Version numbers

While the version starts with `0`, a breaking change or a `feat` raises the minor number and a `fix` raises the patch number (`bump-minor-pre-major`). The manifest starts at `0.0.1`, so the first release PR proposes `0.1.0` as soon as it holds a `feat` commit. To pick a version by hand, add a `Release-As: 0.1.0` footer to a commit on `main`.

Changelog sections: `feat` is Features, `fix` is Bug Fixes, `perf` is Performance Improvements, `revert` is Reverts. `docs`, `chore`, `refactor`, `style`, `test`, `build` and `ci` are left out.

## Why it is built this way

- **Draft first.** release-please cannot make a tag without a release, so it makes a draft pre-release. The draft becomes public only when `release.yml` has built, signed and attached everything. A failed build leaves a draft, not a broken public release.
- **One release per tag.** `release.yml` fills the draft for its tag. When a tag is pushed by hand and no release exists, it creates one. It never changes a release that is already published, so no tag can end up with two releases.
- **No token or secret for this.** Tags and pull requests made with the default `GITHUB_TOKEN` do not start other workflows, so `release-please.yml` calls `release.yml` directly (a reusable workflow with the tag as input) instead of waiting for a tag push. `release.yml` first checks that the tag points at the commit it is about to build.
- **Always a pre-release.** `prerelease` is set in `release-please-config.json`, and `release.yml` publishes with `prerelease=true` and `make_latest=false` for every tag. When Inlet leaves beta, change both on purpose.
- **Checks on the release PR.** A pull request made with `GITHUB_TOKEN` does not start the CI checks. The change is only version strings and the changelog; the full CI runs on `main` right after the merge, and again inside the release build.

## If something goes wrong

- The build or signing failed: fix the cause, then use "Re-run failed jobs" on the `Release Please` run. The draft and the tag stay; assets with the same name are replaced.
- The tag does not point at the commit of the run (another commit reached `main` after the release PR was merged): `release.yml` stops at its first job. Do not move the tag. Delete the draft release and the tag, and let release-please make a new release PR.
- A release was published by mistake: edit it on GitHub and mark it as a pre-release; never as the latest.
- A manual tag (`git tag vX.Y.Z && git push origin vX.Y.Z`) runs the same workflow and creates the release itself. Use it only to recover from a problem as above.

## Notes for contributors

- The workspace version lives in `[workspace.package]` of `Cargo.toml`. Do not edit it by hand.
- `Cargo.lock` is updated by release-please for every crate whose name starts with `inlet` and that has no registry source, so new workspace crates need no change here.
- `THIRD-PARTY-NOTICES` lists the workspace crates without a version, so a version bump does not make the notices check fail.
