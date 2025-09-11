#!/bin/bash
set -euo pipefail

# Colors
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
NC='\033[0m'

echo -e "${YELLOW}Setting up Sphinx Ultra development environment...${NC}\n"

# Check if we're in a git repository
if ! git rev-parse --git-dir > /dev/null 2>&1; then
    echo "Error: Not in a git repository"
    exit 1
fi

# Install pre-push hook
if [ -f ".git/hooks/pre-push" ]; then
    echo "Pre-push hook already exists. Backing up to pre-push.backup"
    cp .git/hooks/pre-push .git/hooks/pre-push.backup
fi

cp scripts/pre-push .git/hooks/pre-push
chmod +x .git/hooks/pre-push

echo -e "${GREEN}✅ Installed pre-push hook for version validation${NC}"

# Make sure other scripts are executable
chmod +x scripts/check-version.sh
chmod +x scripts/release.sh

echo -e "${GREEN}✅ Made release scripts executable${NC}"

# Validate current state
echo -e "\n${YELLOW}Running initial version check...${NC}"
if ./scripts/check-version.sh; then
    echo -e "${GREEN}✅ Version check passed${NC}"
else
    echo -e "${YELLOW}⚠️  Version check completed with warnings${NC}"
fi

echo -e "\n${GREEN}🎉 Setup complete!${NC}"
echo -e "\nNext steps:"
echo -e "1. Use ${YELLOW}./scripts/release.sh --patch${NC} for releases"
echo -e "2. Run ${YELLOW}./scripts/check-version.sh${NC} to validate versions"
echo -e "3. See ${YELLOW}scripts/README.md${NC} for detailed documentation"
