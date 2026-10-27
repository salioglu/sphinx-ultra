# Scripts

Shell scripts for developing, installing and releasing sphinx-ultra. The
Python fixture generators live in [`../tools/`](../tools/) and are documented
in [docs/testing.md](../docs/testing.md).

| Script | Purpose |
|---|---|
| [`dev.sh`](dev.sh) | The developer helper. `scripts/dev.sh help` lists every command; the ones you will use most are `setup`, `install-hooks`, `check` (what CI gates on), `audit`/`deny`, `pages` and `oracle`. |
| [`install.sh`](install.sh) | The end-user installer for Linux and macOS: downloads the release archive for the platform, verifies its SHA-256 checksum, installs to `~/.local/bin`. POSIX sh. Referenced from the README as a `curl … \| sh` one-liner, so its path is part of the public interface. |
| [`check-version.sh`](check-version.sh) | Checks that `Cargo.toml` and the git tag on `HEAD` (or the latest tag) agree. Used by `dev.sh version` and by the pre-push hook. |
| [`pre-push`](pre-push) | Git hook: refuses to push a `vX.Y.Z` tag whose version differs from `Cargo.toml` or whose `Cargo.lock` is out of sync. Installed by `scripts/dev.sh install-hooks`. |

## Releasing

Releases are automated by [release-please](https://github.com/googleapis/release-please):
merging the release PR it maintains creates the tag and the GitHub release,
and `.github/workflows/release.yml` then builds the binaries and publishes
the crate. The whole flow, and the manual fallback for an emergency, is
described in [docs/RELEASING.md](../docs/RELEASING.md). The former
`release.sh` (bump, tag, push by hand) is retired; `main` is protected and
only accepts pull requests.

## Conventions

- Scripts run from any directory; they `cd` to the repository root
  themselves.
- `dev.sh` is bash (it uses arrays and `pipefail`); `install.sh` is POSIX sh
  because it is piped into whatever `/bin/sh` the user has.
- Keep the flags identical to CI's. If CI changes a command, change `dev.sh`
  in the same pull request.
