#!/bin/bash
set -euo pipefail

# Colors for output
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
BLUE='\033[0;34m'
NC='\033[0m' # No Color

# Default values
DRY_RUN=false
FORCE=false
PATCH=false
MINOR=false
MAJOR=false
CUSTOM_VERSION=""

# Function to show usage
usage() {
    cat << EOF
Usage: $0 [OPTIONS] [VERSION]

Safely create a new release by updating Cargo.toml version and creating a git tag.

OPTIONS:
    -h, --help          Show this help message
    -d, --dry-run       Show what would be done without making changes
    -f, --force         Force the release even if there are uncommitted changes
    --patch             Bump patch version (x.y.Z)
    --minor             Bump minor version (x.Y.0)
    --major             Bump major version (X.0.0)

EXAMPLES:
    $0 1.2.3                    # Release version 1.2.3
    $0 --patch                  # Bump patch version (0.1.2 -> 0.1.3)
    $0 --minor                  # Bump minor version (0.1.2 -> 0.2.0)
    $0 --major                  # Bump major version (0.1.2 -> 1.0.0)
    $0 --dry-run --patch        # Show what patch bump would do
EOF
}

# Parse command line arguments
while [[ $# -gt 0 ]]; do
    case $1 in
        -h|--help)
            usage
            exit 0
            ;;
        -d|--dry-run)
            DRY_RUN=true
            shift
            ;;
        -f|--force)
            FORCE=true
            shift
            ;;
        --patch)
            PATCH=true
            shift
            ;;
        --minor)
            MINOR=true
            shift
            ;;
        --major)
            MAJOR=true
            shift
            ;;
        -*)
            echo -e "${RED}Unknown option: $1${NC}"
            usage
            exit 1
            ;;
        *)
            if [ -n "$CUSTOM_VERSION" ]; then
                echo -e "${RED}Error: Multiple versions specified${NC}"
                exit 1
            fi
            CUSTOM_VERSION="$1"
            shift
            ;;
    esac
done

# Validation functions
check_git_status() {
    if [ "$FORCE" = "false" ] && ! git diff-index --quiet HEAD --; then
        echo -e "${RED}Error: You have uncommitted changes${NC}"
        echo -e "Commit your changes or use --force to ignore this check"
        exit 1
    fi
}

check_git_branch() {
    local current_branch
    current_branch=$(git rev-parse --abbrev-ref HEAD)
    if [ "$current_branch" != "main" ] && [ "$current_branch" != "master" ]; then
        echo -e "${YELLOW}Warning: You're on branch '$current_branch', not main/master${NC}"
        read -p "Continue? (y/N): " -n 1 -r
        echo
        if [[ ! $REPLY =~ ^[Yy]$ ]]; then
            echo "Aborted"
            exit 1
        fi
    fi
}

get_current_version() {
    grep '^version = ' Cargo.toml | head -n1 | sed 's/version = "\(.*\)"/\1/'
}

bump_version() {
    local current_version="$1"
    local bump_type="$2"
    
    # Split version into parts
    IFS='.' read -ra VERSION_PARTS <<< "$current_version"
    local major="${VERSION_PARTS[0]}"
    local minor="${VERSION_PARTS[1]}"
    local patch="${VERSION_PARTS[2]}"
    
    case $bump_type in
        "patch")
            patch=$((patch + 1))
            ;;
        "minor")
            minor=$((minor + 1))
            patch=0
            ;;
        "major")
            major=$((major + 1))
            minor=0
            patch=0
            ;;
    esac
    
    echo "${major}.${minor}.${patch}"
}

update_cargo_version() {
    local new_version="$1"
    if [ "$DRY_RUN" = "true" ]; then
        echo -e "${BLUE}[DRY RUN] Would update Cargo.toml version to: $new_version${NC}"
    else
        # Use sed to update the version in Cargo.toml
        if [[ "$OSTYPE" == "darwin"* ]]; then
            # macOS
            sed -i '' "s/^version = \".*\"/version = \"$new_version\"/" Cargo.toml
        else
            # Linux
            sed -i "s/^version = \".*\"/version = \"$new_version\"/" Cargo.toml
        fi
        echo -e "${GREEN}Updated Cargo.toml version to: $new_version${NC}"
    fi
}

create_git_tag() {
    local version="$1"
    local tag_name="v$version"
    
    if [ "$DRY_RUN" = "true" ]; then
        echo -e "${BLUE}[DRY RUN] Would create git tag: $tag_name${NC}"
        echo -e "${BLUE}[DRY RUN] Would push tag to origin${NC}"
    else
        # Check if tag already exists
        if git rev-parse "$tag_name" >/dev/null 2>&1; then
            echo -e "${RED}Error: Tag $tag_name already exists${NC}"
            exit 1
        fi
        
        # Create and push tag
        git add Cargo.toml
        git commit -m "Bump version to $version"
        git tag "$tag_name"
        git push origin main
        git push origin "$tag_name"
        echo -e "${GREEN}Created and pushed tag: $tag_name${NC}"
    fi
}

run_tests() {
    if [ "$DRY_RUN" = "true" ]; then
        echo -e "${BLUE}[DRY RUN] Would run: cargo test${NC}"
    else
        echo -e "${YELLOW}Running tests...${NC}"
        if ! cargo test; then
            echo -e "${RED}Tests failed! Aborting release.${NC}"
            exit 1
        fi
        echo -e "${GREEN}All tests passed!${NC}"
    fi
}

# Main execution
main() {
    echo -e "${BLUE}=== Sphinx Ultra Release Script ===${NC}\n"
    
    # Basic checks
    if ! git rev-parse --git-dir > /dev/null 2>&1; then
        echo -e "${RED}Error: Not in a git repository${NC}"
        exit 1
    fi
    
    if [ ! -f "Cargo.toml" ]; then
        echo -e "${RED}Error: Cargo.toml not found${NC}"
        exit 1
    fi
    
    # Check git status and branch
    check_git_status
    check_git_branch
    
    # Determine target version
    local current_version
    local target_version
    
    current_version=$(get_current_version)
    echo -e "${YELLOW}Current version:${NC} $current_version"
    
    if [ -n "$CUSTOM_VERSION" ]; then
        target_version="$CUSTOM_VERSION"
    elif [ "$PATCH" = "true" ]; then
        target_version=$(bump_version "$current_version" "patch")
    elif [ "$MINOR" = "true" ]; then
        target_version=$(bump_version "$current_version" "minor")
    elif [ "$MAJOR" = "true" ]; then
        target_version=$(bump_version "$current_version" "major")
    else
        echo -e "${RED}Error: No version specified${NC}"
        echo "Use --patch, --minor, --major, or specify a version directly"
        exit 1
    fi
    
    echo -e "${YELLOW}Target version:${NC} $target_version"
    
    # Validate version format
    if ! [[ $target_version =~ ^[0-9]+\.[0-9]+\.[0-9]+$ ]]; then
        echo -e "${RED}Error: Invalid version format. Use semantic versioning (x.y.z)${NC}"
        exit 1
    fi
    
    # Confirm unless dry run
    if [ "$DRY_RUN" = "false" ]; then
        echo
        read -p "Proceed with release $target_version? (y/N): " -n 1 -r
        echo
        if [[ ! $REPLY =~ ^[Yy]$ ]]; then
            echo "Aborted"
            exit 1
        fi
    fi
    
    # Execute release steps
    echo -e "\n${BLUE}=== Release Steps ===${NC}"
    
    # 1. Run tests
    run_tests
    
    # 2. Update Cargo.toml
    update_cargo_version "$target_version"
    
    # 3. Create git tag and push
    create_git_tag "$target_version"
    
    if [ "$DRY_RUN" = "false" ]; then
        echo -e "\n${GREEN}🎉 Release $target_version completed successfully!${NC}"
        echo -e "The GitHub Actions workflow will now build and publish the release."
    else
        echo -e "\n${BLUE}🔍 Dry run completed. Use without --dry-run to execute.${NC}"
    fi
}

# Run main function
main "$@"
