# Contributing

Thank you for helping. The most useful contributions right now are:

- **Test reports from real documentation trees.** Run `sphinx-ultra -W`
  on a project you know and compare the warnings with `sphinx-build -W`.
  Every difference is a bug or a documented gap; please file it with a
  minimal reproduction.
- **Oracle cases.** A compatibility claim is only as good as the fixture that
  pins it. Adding a case to one of the differential corpora is a small,
  high-value change.
- **The roadmap items** in [ROADMAP.md](ROADMAP.md), starting with the HTML
  writer (M2 wave 5).

## Ground rules

1. **Compatibility is verified, never claimed.** A change that says "like
   Sphinx" needs a differential case against the pinned upstream
   (`sphinx==9.1.0`, `docutils==0.22.4`). See [testing.md](testing.md).
2. **Never edit a fixture by hand.** Regenerate it with its generator under
   `tools/`, from the locked environment, with `PYTHONNOUSERSITE=1`.
3. **Status documents are code.** If a change moves a subsystem's status,
   update [IMPLEMENTATION_STATUS.md](IMPLEMENTATION_STATUS.md) and ROADMAP §2
   in the same pull request. Deliberate divergences from Sphinx are recorded
   there with their reason.
4. **Conventional Commits.** Release notes and version bumps are generated
   from commit messages by release-please, so the type matters:

   | Type | Use for | Release effect |
   |---|---|---|
   | `feat:` | user-visible capability | minor bump |
   | `fix:` | user-visible bug fix | patch bump |
   | `perf:` | performance change | patch bump |
   | `feat!:` / `BREAKING CHANGE:` footer | an incompatible change | minor bump while pre-1.0 |
   | `deps:` | a dependency update (Dependabot uses this) | patch bump, listed under Dependencies |
   | `docs:`, `test:`, `ci:`, `build:`, `chore:`, `refactor:`, `style:` | everything else | not listed, no release on their own |

   A scope is welcome (`feat(env): …`, `fix(rst): …`). Write the subject for
   the changelog reader, not for yourself.

## Development setup

Rust 1.85 or newer (the declared MSRV; CI checks it), `git`, and for fixture
regeneration Python 3.12 with [uv](https://docs.astral.sh/uv/).

```bash
git clone https://github.com/salioglu/sphinx-ultra.git
cd sphinx-ultra
scripts/dev.sh setup           # rustfmt, clippy, cargo-audit, cargo-deny, cargo-llvm-cov
scripts/dev.sh install-hooks   # pre-commit (fmt, clippy, check) and pre-push (tag/version guard)
scripts/dev.sh check           # fmt --check, clippy -D warnings, cargo test --locked
```

`scripts/dev.sh help` lists the other commands (`build`, `test`, `bench`,
`fmt`, `clippy`, `audit`, `deny`, `docs`, `oracle`, `clean`, `install`).
[`.pre-commit-config.yaml`](../.pre-commit-config.yaml) is available for the
`pre-commit` framework too.

## Pull requests

- Branch from `main`. `main` is protected: changes land through a pull
  request, with the `Test (ubuntu-latest, stable)`, `Benchmark`,
  `Code Coverage` and `Security Audit` checks green and every review thread
  resolved.
- Keep a PR to one concern. Mechanical changes (a regenerated fixture, a
  rename) go in their own commit so the review diff stays readable.
- Fill in the pull-request template, in particular the **Evidence** section.
- Pull requests are merged with a merge commit; your commits are kept as you
  wrote them, which is why their messages must follow the convention above.

## Where things live

| Path | Contents |
|---|---|
| `src/rst/`, `src/doctree/` | the RST parser and the doctree IR |
| `src/env/` | `BuildEnvironment`: toctree, numbering, std and py domains, genindex, objects, dependencies, resolution |
| `src/py/` | the Python signature grammar and `ast.unparse` port |
| `src/intersphinx/`, `src/inventory.rs` | inventories and intersphinx |
| `src/search.rs` | the `IndexBuilder` port and the Snowball English stemmer |
| `src/builder.rs`, `src/main.rs` | the read → merge → resolve → write pipeline and the CLI |
| `src/config.rs`, `src/python_config.rs` | configuration and the `conf.py` parser |
| `tests/` | end-to-end and differential suites; `tests/fixtures/` holds the committed oracles |
| `tools/` | the Python fixture generators and the HTML oracle runner, with locked profiles under `tools/oracle_profiles/` |
| `scripts/` | shell scripts: developer helper, installer, version guard |
| `docs/` | this documentation; `docs/plans/` keeps the implementation plans and review records of each wave, `docs/research/` the upstream feature inventories |

## Code style

`cargo fmt` and `cargo clippy --all-targets --all-features -- -D warnings`
are gates. Beyond that: a port of upstream behaviour names the upstream
function it mirrors in a comment (`// sphinx/environment/__init__.py:421`),
and a deliberate divergence says why at the code site and in
IMPLEMENTATION_STATUS.

## Reporting bugs and proposing features

Use the issue templates. A bug report with a two-file reproduction project
and the `sphinx-build` output for comparison is the fastest path to a fix.
For larger proposals open a discussion first; the roadmap's scope principle
is "phased, never excluded", so the question is usually *when*, not
*whether*.

## License

By contributing you agree that your contribution is licensed under the
project's MIT license.
