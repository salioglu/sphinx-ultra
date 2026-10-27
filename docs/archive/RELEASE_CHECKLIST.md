# Release Checklist

This checklist ensures that all necessary steps are completed before publishing a new release.

## Pre-Release

### Code Quality
- [ ] All tests pass (`cargo test`)
- [ ] All benchmarks run successfully (`cargo bench`)
- [ ] Code is properly formatted (`cargo fmt`)
- [ ] No clippy warnings (`cargo clippy`)
- [ ] Security audit passes (`cargo audit`)

### Documentation
- [ ] README.md is up to date
- [ ] CHANGELOG.md includes new changes
- [ ] API documentation is current (`cargo doc`)
- [ ] Examples work with new version
- [ ] Quick start guide is accurate

### Version Management
- [ ] Version bumped in `Cargo.toml`
- [ ] Version updated in install script
- [ ] CHANGELOG.md has correct version
- [ ] Git tags are ready

### Testing
- [ ] Manual testing on Linux
- [ ] Manual testing on macOS
- [ ] Manual testing on Windows
- [ ] Large project performance test
- [ ] Integration tests pass

## Release Process

### GitHub Release
- [ ] Create release branch
- [ ] Tag version (`git tag v0.x.x`)
- [ ] Push tags (`git push origin --tags`)
- [ ] GitHub Actions build completes
- [ ] Release artifacts are generated
- [ ] Release notes are published

### Crates.io
- [ ] `cargo publish --dry-run` succeeds
- [ ] `cargo publish` completes
- [ ] Package appears on crates.io
- [ ] Documentation builds on docs.rs

### Distribution
- [ ] Install script works with new version
- [ ] Binaries work on all platforms
- [ ] Package managers updated (if applicable)

## Post-Release

### Verification
- [ ] Install script downloads correct version
- [ ] `cargo install sphinx-ultra` works
- [ ] Release artifacts are accessible
- [ ] Documentation links work

### Communication
- [ ] Release announcement prepared
- [ ] Community notified
- [ ] Social media posts
- [ ] Update project websites

### Monitoring
- [ ] Download statistics checked
- [ ] User feedback collected
- [ ] Issues tracked for next release

## Rollback Plan

If issues are discovered:

1. **Critical Issues**:
   - [ ] Yank problematic version from crates.io
   - [ ] Update install script to previous version
   - [ ] Post advisory on GitHub

2. **Minor Issues**:
   - [ ] Document known issues
   - [ ] Plan hotfix release
   - [ ] Communicate to users

## Version Strategy

- **Patch (0.1.x)**: Bug fixes, performance improvements
- **Minor (0.x.0)**: New features, non-breaking changes
- **Major (x.0.0)**: Breaking changes, major rewrites

## Automation

The following are automated via GitHub Actions:

- [ ] CI/CD pipeline runs on tag
- [ ] Release binaries are built
- [ ] Crates.io publishing
- [ ] GitHub release creation
- [ ] Security scanning

## Manual Steps

These require manual intervention:

- [ ] Version bumping
- [ ] Changelog updates
- [ ] Release notes
- [ ] Social media
- [ ] Community communication
