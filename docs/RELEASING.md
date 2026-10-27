# Releasing

Releases are automated with [release-please](https://github.com/googleapis/release-please).
Nobody edits `Cargo.toml`'s version or `CHANGELOG.md` by hand, and nobody
pushes tags.

## How a release happens

1. **Commits land on `main`** through pull requests, with
   [Conventional Commit](https://www.conventionalcommits.org/) messages. The
   type decides the next version while the project is pre-1.0:
   `feat` (and any breaking change) bumps the minor, `fix` and `perf` bump
   the patch, everything else is listed without a bump.
2. **release-please maintains a release PR.** On every push to `main` the
   [`release-please.yml`](../.github/workflows/release-please.yml) workflow
   opens or updates a pull request titled `chore(main): release X.Y.Z` that
   bumps `Cargo.toml` and `Cargo.lock`, prepends the new section to
   `CHANGELOG.md` (grouped as Added / Fixed / Performance / Dependencies /
   Documentation), and updates `.release-please-manifest.json`. Review the
   generated notes there; edit the PR body to reword them, or push a
   `fix:`/`feat:` commit to `main` to change what is in the release.
3. **Merge the release PR** when the notes read well. release-please then
   creates the tag `vX.Y.Z` and the GitHub release with those notes.
4. **The release build runs.** `release-please.yml` calls
   [`release.yml`](../.github/workflows/release.yml), which validates that
   the tag matches `Cargo.toml` and that `Cargo.lock` is in sync, builds the
   six platform archives with `--locked`, attaches each archive, its
   `.sha256` and a build-provenance attestation to the release, and runs
   `cargo publish --locked` with `CARGO_REGISTRY_TOKEN`.
5. **Check the release page** once the workflow is green: six archives, six
   checksums, the crate on crates.io, and docs.rs building.

## One-time setup

| Secret | Purpose | Without it |
|---|---|---|
| `RELEASE_PLEASE_TOKEN` | A fine-grained personal access token for this repository with **Contents: read and write** and **Pull requests: read and write**. release-please opens the release PR with it, so CI runs on that PR and the required checks report | The PR is opened with `GITHUB_TOKEN`, which does not trigger workflows: the required checks never run and only an admin bypass can merge it |
| `CARGO_REGISTRY_TOKEN` | A crates.io API token with `publish-update` scope for `sphinx-ultra` | The `Publish to crates.io` job prints a warning and skips; publish by hand from the tag with `cargo publish --locked` |
| `CODECOV_TOKEN` | Coverage upload (CI, not release) | Coverage upload is skipped |

Tokens expire. The release workflow states clearly which one is missing.

## Verifying a release

```bash
sha256sum -c sphinx-ultra-0.7.0-linux-x86_64.tar.gz.sha256
gh attestation verify sphinx-ultra-0.7.0-linux-x86_64.tar.gz --repo salioglu/sphinx-ultra
```

The attestation proves the archive was built by this repository's
`release.yml` from the tagged commit.

## Manual fallback

If release-please cannot be used (the action is broken, or a hotfix must go
out from a branch), a tag pushed by hand still produces a complete release:

```bash
# on a clean checkout of the commit to release
scripts/check-version.sh                 # Cargo.toml must already carry the new version
cargo update --workspace                 # keep Cargo.lock in sync with the bump
git commit -am "chore(main): release 0.7.1"
git tag v0.7.1 && git push origin v0.7.1
```

`release.yml` runs on the tag push, creates the GitHub release if it does
not exist, and attaches the artifacts. The pre-push hook installed by
`scripts/dev.sh install-hooks` refuses a `vX.Y.Z` tag whose version differs
from `Cargo.toml`. Afterwards, set `.release-please-manifest.json` to the
released version on `main` so release-please continues from it.

## Version policy

Pre-1.0: a breaking change bumps the minor version, not the major
(`bump-minor-pre-major` in [`release-please-config.json`](../release-please-config.json)).
The 1.0 criteria are the M7 acceptance criteria in [ROADMAP.md](ROADMAP.md).
Milestone version labels in the roadmap are estimates; the released version
is whatever the commits since the last release add up to.

## Release notes for older releases

`CHANGELOG.md` was backfilled for 0.4.1, 0.5.0 and 0.6.0 from the merged
pull requests, and the matching GitHub releases carry the same text. From
0.7.0 on, release-please writes both.
