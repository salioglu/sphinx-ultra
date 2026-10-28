## Summary

<!-- What changes, and why. Link the issue if there is one: Fixes #123 -->

## Type of change

- [ ] Bug fix
- [ ] New feature
- [ ] Breaking change (a `!` in the commit type, or a `BREAKING CHANGE:` footer)
- [ ] Documentation
- [ ] Tests or fixtures
- [ ] CI / tooling

## Evidence

<!--
Compatibility claims need an oracle. Say which differential suite pins the
behaviour (docutils, Sphinx read phase, environment, pattern, inventory,
search, HTML) and whether you added cases or removed an exemption entry.
If you regenerated a fixture, name the generator and the pins it asserted.
-->

## Checklist

- [ ] Commit messages follow Conventional Commits (`feat:`, `fix:`, `docs:`, `test:`, `ci:`, `chore:`, `deps:`); release-please builds the changelog from them
- [ ] `scripts/dev.sh check` passes (`cargo fmt --check`, `cargo clippy -D warnings`, `cargo test --locked`)
- [ ] New behaviour has a test; a Sphinx-compatibility claim has an oracle case
- [ ] `docs/IMPLEMENTATION_STATUS.md` (and `docs/ROADMAP.md` §2) are updated if a subsystem's status changed
- [ ] No fixture under `tests/fixtures/` was edited by hand
