#!/bin/bash
set -euo pipefail

# Colors for output
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
NC='\033[0m' # No Color

# Function to extract version from Cargo.toml
get_cargo_version() {
    grep '^version = ' Cargo.toml | head -n1 | sed 's/version = "\(.*\)"/\1/'
}

# Function to get the current git tag (if any)
get_git_tag() {
    if git describe --exact-match --tags HEAD 2>/dev/null; then
        return 0
    else
        return 1
    fi
}

# Function to get the latest git tag
get_latest_git_tag() {
    git describe --tags --abbrev=0 2>/dev/null || echo "none"
}

# Main validation function
validate_version() {
    local cargo_version
    local git_tag
    local latest_tag
    
    cargo_version=$(get_cargo_version)
    echo -e "${YELLOW}Cargo.toml version:${NC} $cargo_version"
    
    if git_tag=$(get_git_tag); then
        # Remove 'v' prefix if present
        git_tag_clean=${git_tag#v}
        echo -e "${YELLOW}Current git tag:${NC} $git_tag ($git_tag_clean)"
        
        if [ "$cargo_version" = "$git_tag_clean" ]; then
            echo -e "${GREEN}✓ Version consistency check passed!${NC}"
            return 0
        else
            echo -e "${RED}✗ Version mismatch detected!${NC}"
            echo -e "  Cargo.toml: $cargo_version"
            echo -e "  Git tag: $git_tag_clean"
            return 1
        fi
    else
        latest_tag=$(get_latest_git_tag)
        echo -e "${YELLOW}No tag on current commit${NC}"
        echo -e "${YELLOW}Latest tag:${NC} $latest_tag"
        
        if [ "$latest_tag" != "none" ]; then
            latest_tag_clean=${latest_tag#v}
            if [ "$cargo_version" = "$latest_tag_clean" ]; then
                echo -e "${YELLOW}⚠ Warning: Cargo.toml version matches latest tag but commit is not tagged${NC}"
                echo -e "  Consider bumping the version before creating a new release"
                return 2
            fi
        fi
        
        echo -e "${GREEN}✓ Ready for release (no tag conflicts)${NC}"
        return 0
    fi
}

# Check if we're in a git repository
if ! git rev-parse --git-dir > /dev/null 2>&1; then
    echo -e "${RED}Error: Not in a git repository${NC}"
    exit 1
fi

# Check if Cargo.toml exists
if [ ! -f "Cargo.toml" ]; then
    echo -e "${RED}Error: Cargo.toml not found${NC}"
    exit 1
fi

# Run validation
validate_version
exit_code=$?

if [ $exit_code -eq 1 ]; then
    echo -e "\n${RED}Recommendations:${NC}"
    echo -e "1. Update Cargo.toml version to match the git tag, OR"
    echo -e "2. Delete the current tag and create a new one with the correct version"
    echo -e "3. Use the release script (scripts/release.sh) to automate this process"
elif [ $exit_code -eq 2 ]; then
    echo -e "\n${YELLOW}Recommendations:${NC}"
    echo -e "1. Bump the version in Cargo.toml before creating a new release"
    echo -e "2. Use the release script (scripts/release.sh) to automate this process"
fi

exit $exit_code
