# Releasing

Releases are prepared by [release-please](https://github.com/googleapis/release-please)
and published by hand: merging an ordinary pull request never releases
anything, and nothing reaches the GitHub release or crates.io until a
maintainer approves it. Nobody edits `Cargo.toml`'s version or
`CHANGELOG.md` by hand.

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
3. **Merge the release PR** when you want to release and the notes read
   well. release-please then creates the tag `vX.Y.Z` and the GitHub release
   with those notes.
4. **The release build runs.** `release-please.yml` calls
   [`release.yml`](../.github/workflows/release.yml), which validates that
   the tag matches `Cargo.toml` and that `Cargo.lock` is in sync, and builds
   the six platform archives with `--locked`.
5. **Approve the release.** The two publish jobs run in the
   [`release` environment](#the-release-environment) and wait for a
   reviewer. Open the run (Actions → release-please), check the builds, and
   choose **Review deployments → Approve**. Each archive, its `.sha256` and a
   build-provenance attestation are then attached to the release, and
   `cargo publish --locked` runs (see [crates.io publishing](#cratesio-publishing)).
   If you reject, the tag and the release stay without archives and nothing
   reaches crates.io; `gh run rerun <run-id> --failed` asks again.
6. **Check the release page** once the workflow is green: six archives, six
   checksums, the crate on crates.io, and docs.rs building.

## One-time setup

### The `release` environment

In the repository settings, under **Environments**, create `release` with:

- **Required reviewers:** whoever may publish. You can approve your own runs
  while "Prevent self-review" stays off.
- **Deployment branches and tags:** selected branches, `main` only.

Create it before the first release. A job that names a missing environment
creates it without protection rules, and then publishes without waiting.
From a shell:

```bash
gh api -X PUT repos/salioglu/sphinx-ultra/environments/release --input - <<EOF
{"reviewers": [{"type": "User", "id": $(gh api users/salioglu --jq .id)}],
 "deployment_branch_policy": {"protected_branches": false, "custom_branch_policies": true}}
EOF
gh api -X POST repos/salioglu/sphinx-ultra/environments/release/deployment-branch-policies -f name=main -f type=branch
```

### Secrets

| Secret | Purpose | Without it |
|---|---|---|
| `RELEASE_PLEASE_TOKEN` | A fine-grained personal access token for this repository with **Contents: read and write** and **Pull requests: read and write**. release-please opens the release PR with it, so CI runs on that PR and the required checks report | The PR is opened with `GITHUB_TOKEN`, which does not trigger workflows: the required checks never run and only an admin bypass can merge it |
| `CARGO_REGISTRY_TOKEN` | Legacy: a crates.io API token with `publish-update` scope for `sphinx-ultra`. While it is set, it is used instead of Trusted Publishing | `cargo publish` authenticates with Trusted Publishing (below) |
| `CODECOV_TOKEN` | Coverage upload (CI, not release) | Coverage upload is skipped |

Tokens expire. The release workflow states clearly which one is missing.

### crates.io publishing

`cargo publish` authenticates with crates.io Trusted Publishing: a
short-lived token minted from the workflow's OIDC identity, so no crates.io
secret is stored. In the crate's settings on crates.io, add a GitHub trusted
publisher for `salioglu/sphinx-ultra` for each of these workflows, with the
environment set to `release` so that only an approved job can publish:

- `release-please.yml`, for a normal release. crates.io matches the run's
  top-level workflow (the OIDC `workflow_ref` claim), and on this path that
  is the caller, not `release.yml`.
- `release.yml`, for a [release made by hand](#releasing-by-hand).

Then delete the `CARGO_REGISTRY_TOKEN` secret and revoke that token on
crates.io; while the secret exists, the workflow uses it instead.

## Verifying a release

```bash
sha256sum -c sphinx-ultra-0.7.0-linux-x86_64.tar.gz.sha256
gh attestation verify sphinx-ultra-0.7.0-linux-x86_64.tar.gz --repo salioglu/sphinx-ultra
```

The attestation proves the archive was built by this repository's
`release.yml` from the tagged commit.

## Releasing by hand

Use this when release-please did not tag a merged release PR, or when a
hotfix must go out from a branch. It ends in the same build and the same
approval as a normal release.

release-please skips a merged release PR that it cannot match to the
package; the `Release PR / tag` log then says `PR component: … does not
match configured component`. That is how 0.7.0 was missed, while
`separate-pull-requests` was `false` in
[`release-please-config.json`](../release-please-config.json): with a single
package, leave that option unset.

1. **The commit carries the version.** A merged release PR already bumped
   `Cargo.toml`, `Cargo.lock`, `CHANGELOG.md` and
   `.release-please-manifest.json`. For a hotfix, make the same changes
   (`cargo update --workspace` syncs `Cargo.lock`), check them with
   `scripts/check-version.sh`, and push the commit.
2. **Create the tag and the release** on that commit, with its
   `CHANGELOG.md` section as the notes (`<sha>` is the full commit SHA):
   ```bash
   git show <sha>:CHANGELOG.md | awk -v v=0.7.1 '/^## /{p = index($0, "[" v "]")} p' | gh release create v0.7.1 --target <sha> --title v0.7.1 --notes-file -
   ```
3. **Mark the release PR as released**, if there is one. Until then,
   release-please refuses to open the next release PR ("There are untagged,
   merged release PRs outstanding"):
   ```bash
   gh label create "autorelease: tagged" --color ededed --force
   gh pr edit <number> --remove-label "autorelease: pending" --add-label "autorelease: tagged"
   ```
4. **Build it.** The publish jobs wait for approval as in a normal release:
   ```bash
   gh workflow run release.yml -f tag=v0.7.1
   ```
5. **After a hotfix**, set `.release-please-manifest.json` to the released
   version on `main`, so release-please continues from it.

The same `gh workflow run` rebuilds the archives of an existing release; its
crates.io job then fails, since that version is already published.

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
