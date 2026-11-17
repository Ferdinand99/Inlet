# Releasing Inlet

Every Inlet release is a pre-release (a beta) and is never marked as the latest release. Releases are cut from `main` with [release-please](https://github.com/googleapis/release-please). Nothing is released by merging a normal pull request.

## How a beta is cut

1. Work lands on `v0` and, when `v0` is ready, is merged into `main`. Commits follow Conventional Commits; the release notes are built from them.
2. On every push to `main`, `release-please.yml` opens or updates one release PR, titled `chore(main): release X.Y.Z`. It changes the version in `Cargo.toml`, in the workspace crates in `Cargo.lock` and in the same crates in the fuzz crate's `crates/inlet-media/fuzz/Cargo.lock`, and adds the new section to `CHANGELOG.md`.
3. The maintainer reads the release PR and merges it. This is the approval to release. A pull request made by release-please starts no CI, so right before merging, close and reopen the release PR: the reopening comes from the maintainer, and `build.yml` then runs on it. Reopen it last, because later pushes by release-please to the PR start nothing again. Merge only when the checks are green: once the tag and the draft exist, a bad commit costs a version number.
4. release-please creates the tag `vX.Y.Z` and a draft pre-release for it. The draft holds the changelog as its notes and is not visible to the public.
5. `release-please.yml` then calls `release.yml` with the tag. It builds the three platforms, then pauses before macOS signing.
6. The maintainer approves the signing job in the protected environment `release` (Actions run page, "Review deployments"). Only `main` and `v*` tags may use that environment.
7. `release.yml` collects the three downloads, an SBOM for each platform (CycloneDX, `*.cdx.json`), `LICENSE`, `THIRD-PARTY-NOTICES` and `SHA256SUMS.txt`, checks that the licence files are in place, and signs build provenance for every one of those files. Only then does it attach them to the draft and publish it as a pre-release. It fails if the result is not a published pre-release, or if it is the latest release. A download can be checked with `gh attestation verify <file> --repo Ferdinand99/Inlet`.

## Version numbers

While the version starts with `0`, a breaking change or a `feat` raises the minor number and a `fix` raises the patch number (`bump-minor-pre-major`). The manifest starts at `0.0.1`, so the first release PR proposes `0.1.0` as soon as it holds a `feat` commit. To pick a version by hand, add a `Release-As: 0.1.0` footer to a commit on `main`.

Changelog sections: `feat` is Features, `fix` is Bug Fixes, `perf` is Performance Improvements, `revert` is Reverts. `docs`, `chore`, `refactor`, `style`, `test`, `build` and `ci` are left out.

## Why it is built this way

- **Draft first.** release-please cannot make a tag without a release, so it makes a draft pre-release. The draft becomes public only when `release.yml` has built, signed and attached everything. A failed build leaves a draft, not a broken public release.
- **One release per tag.** `release.yml` fills the draft for its tag. When a tag is pushed by hand and no release exists, it creates one. It never changes a release that is already published, so no tag can end up with two releases.
- **No token or secret for this.** Tags and pull requests made with the default `GITHUB_TOKEN` do not start other workflows, so `release-please.yml` calls `release.yml` directly (a reusable workflow with the tag as input) instead of waiting for a tag push. `release.yml` first checks that the tag points at the commit it is about to build.
- **Always a pre-release.** `release.yml` publishes with `prerelease=true` and `make_latest=false` for every tag. release-please itself marks the draft as a pre-release only while the version starts with `0` or has a suffix. From `1.0.0` the draft is a normal draft: never publish a draft by hand in the GitHub UI, because it would go out as a full release and as the latest one. When Inlet leaves beta, change this on purpose.
- **Checks on the release PR.** A pull request made with `GITHUB_TOKEN` does not start the CI checks (see step 3 for the reopen). The full CI also runs on `main` right after the merge, and again inside the release build.
- **One release run at a time.** The `Release Please` run keeps its concurrency group until the macOS signing is approved, which can take a while. Pushes to `main` in that time wait for it, and only the newest waiting run is kept.

## What the app updates from

Installed copies of Inlet update themselves from the releases cut here (see `packaging/README.md`, "Updates"). Two things follow for whoever cuts a release:

- The update feed is part of the release. Besides the downloads, `release.yml` attaches, for each of Windows and Linux, `no.opland.inlet-<version>-beta-<os>-full.nupkg` and `releases.beta-<os>.json`. The app reads the newest published release that has the feed file of its own channel. A release without these files offers no update to anyone.
- A release is found once it is published. A draft is invisible to the app, so a failed build never reaches users. Because every release is a pre-release and none is "latest", the app lists releases through the API (the newest ten) instead of the "latest release" address.
- The "What's new" text the app shows is the section of `CHANGELOG.md` for the version, taken when the packages are built. A manual tag on a commit without that section ships without notes.
- An update is never pulled back: the app does not go to an older version. A broken release is fixed with the next version, not by deleting the release (the copies that already updated stay on it).

## If something goes wrong

- The build or signing failed for a reason outside the code (a network error, a missing secret, an approval that timed out): use "Re-run failed jobs" on the `Release Please` run. The draft and the tag stay; assets with the same name are replaced. A re-run uses the same commit and the same workflow files, so a fix in code or in a workflow needs a new commit and the release becomes the next version.
- The tag does not point at the commit of the run (another commit reached `main`, or another `Release Please` run was in flight, when the release PR was merged): `release.yml` stops at its first job. The tag is on the right commit; only the run is on another one. Keep the draft. Delete the remote tag and push it again at the same commit with your own credentials: `git fetch --tags && git push origin :refs/tags/vX.Y.Z && git push origin vX.Y.Z`. That push starts `release.yml` through the tag path, which fills the existing draft, changelog included. Do not delete the draft: release-please would then propose the next version, not this one.
- The `Build provenance` job failed (Sigstore or the attestation service had a problem): nothing was published yet, because publishing waits for it. Use "Re-run failed jobs"; the collected files are kept for 7 days.
- A release was published by mistake: edit it on GitHub and mark it as a pre-release; never as the latest.
- A manual tag (`git tag vX.Y.Z && git push origin vX.Y.Z`) runs the same workflow and creates the release itself. Use it only to recover from a problem as above.

## Notes for contributors

- The workspace version lives in `[workspace.package]` of `Cargo.toml`. Do not edit it by hand.
- `Cargo.lock` is updated by release-please for every crate whose name starts with `inlet` and that has no registry source, so new workspace crates need no change here. The fuzz crate has its own lock file, `crates/inlet-media/fuzz/Cargo.lock`, that holds the workspace crates it uses (`inlet-media`). It gets the same bump, except for `inlet-media-fuzz` (the fuzz crate, version `0.0.0`). Without it `cargo deny --locked` and `scripts/check-fuzz-lock.sh` fail on the release commit.
- `THIRD-PARTY-NOTICES` lists the workspace crates without a version, so a version bump does not make the notices check fail.
