# Release Scripts

This directory contains scripts to help manage releases safely and prevent version mismatches.

## Scripts

### `check-version.sh`

Validates version consistency between `Cargo.toml` and git tags.

**Usage:**
```bash
./scripts/check-version.sh
```

**What it checks:**
- ✅ Cargo.toml version matches current git tag (if tagged)
- ⚠️  Warns if Cargo.toml version matches latest tag but commit isn't tagged
- ✅ Validates readiness for new releases

### `release.sh`

Automates the entire release process safely.

**Usage:**
```bash
# Bump patch version (0.1.2 → 0.1.3)
./scripts/release.sh --patch

# Bump minor version (0.1.2 → 0.2.0)
./scripts/release.sh --minor

# Bump major version (0.1.2 → 1.0.0)
./scripts/release.sh --major

# Set specific version
./scripts/release.sh 1.5.0

# Dry run to see what would happen
./scripts/release.sh --dry-run --patch
```

**What it does:**
1. 🔍 Validates git status and branch
2. 🧪 Runs tests to ensure quality
3. 📝 Updates `Cargo.toml` version
4. 🏷️  Creates and pushes git tag
5. 🚀 Triggers GitHub Actions release workflow

**Options:**
- `--dry-run`: Show what would be done without making changes
- `--force`: Force release even with uncommitted changes
- `--patch/--minor/--major`: Semantic version bumping
- `--help`: Show detailed help

## Release Process

### Safe Release (Recommended)

```bash
# 1. Make sure you're on main branch with clean working tree
git checkout main
git pull origin main

# 2. Run tests manually (optional, script will do this too)
cargo test

# 3. Use the release script
./scripts/release.sh --patch  # or --minor, --major, or specific version

# 4. The script will:
#    - Run tests
#    - Update Cargo.toml
#    - Create git tag
#    - Push to GitHub
#    - Trigger release workflow
```

### Manual Release (Not Recommended)

If you must do it manually:

```bash
# 1. Update version in Cargo.toml
vim Cargo.toml

# 2. Validate version consistency
./scripts/check-version.sh

# 3. Create and push tag
git add Cargo.toml
git commit -m "Bump version to X.Y.Z"
git tag vX.Y.Z
git push origin main
git push origin vX.Y.Z
```

## GitHub Workflow Integration

The release workflow (`.github/workflows/release.yml`) includes:

1. **Version Validation**: Automatically checks that git tag matches Cargo.toml version
2. **Build Matrix**: Builds for multiple platforms (Linux, macOS, Windows)
3. **Artifact Upload**: Uploads binaries to GitHub release
4. **Crates.io Publishing**: Optionally publishes to crates.io

## Troubleshooting

### Version Mismatch Error

If you see "Version mismatch detected" in GitHub Actions:

```bash
# Option 1: Fix Cargo.toml to match tag
vim Cargo.toml  # Update version to match tag
git add Cargo.toml
git commit -m "Fix version to match tag"
git push origin main

# Option 2: Delete tag and recreate with correct version
git tag -d vX.Y.Z  # Delete local tag
git push origin :refs/tags/vX.Y.Z  # Delete remote tag
./scripts/release.sh X.Y.Z  # Use release script with correct version
```

### Uncommitted Changes

```bash
# Either commit your changes
git add .
git commit -m "Your changes"

# Or use --force (not recommended)
./scripts/release.sh --force --patch
```

### Tests Failing

```bash
# Fix the failing tests first
cargo test

# Then retry the release
./scripts/release.sh --patch
```

## Security Features

- ✅ **Version Consistency**: Prevents tag/Cargo.toml mismatches
- ✅ **Test Validation**: Ensures tests pass before release
- ✅ **Git Status Check**: Prevents releases with uncommitted changes
- ✅ **Branch Validation**: Warns when not on main/master
- ✅ **Dry Run Mode**: Preview changes before execution
- ✅ **Tag Collision Prevention**: Prevents duplicate tags
- ✅ **Automated Workflow**: Reduces human error
