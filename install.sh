#!/bin/bash

# Sphinx Ultra Installation Script
# This script installs Sphinx Ultra on Unix-like systems

set -e

# Colors for output
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
BLUE='\033[0;34m'
NC='\033[0m' # No Color

# Configuration
REPO="salioglu/sphinx-ultra"
BINARY_NAME="sphinx-ultra"
INSTALL_DIR="${INSTALL_DIR:-$HOME/.local/bin}"

# Functions
log_info() {
    echo -e "${BLUE}[INFO]${NC} $1"
}

log_success() {
    echo -e "${GREEN}[SUCCESS]${NC} $1"
}

log_warning() {
    echo -e "${YELLOW}[WARNING]${NC} $1"
}

log_error() {
    echo -e "${RED}[ERROR]${NC} $1"
}

check_dependencies() {
    log_info "Checking dependencies..."

    if ! command -v curl &> /dev/null && ! command -v wget &> /dev/null; then
        log_error "Either curl or wget is required"
        exit 1
    fi

    if ! command -v tar &> /dev/null; then
        log_error "tar is required"
        exit 1
    fi
}

detect_platform() {
    local os=$(uname -s)
    local arch=$(uname -m)

    case "$os" in
        Linux)
            case "$arch" in
                x86_64) echo "x86_64-unknown-linux-gnu" ;;
                aarch64) echo "aarch64-unknown-linux-gnu" ;;
                *) log_error "Unsupported architecture: $arch"; exit 1 ;;
            esac
            ;;
        Darwin)
            case "$arch" in
                x86_64) echo "x86_64-apple-darwin" ;;
                arm64) echo "aarch64-apple-darwin" ;;
                *) log_error "Unsupported architecture: $arch"; exit 1 ;;
            esac
            ;;
        *)
            log_error "Unsupported operating system: $os"
            exit 1
            ;;
    esac
}

get_latest_version() {
    log_info "Fetching latest version..."

    if command -v curl &> /dev/null; then
        curl -s "https://api.github.com/repos/$REPO/releases/latest" | grep '"tag_name":' | cut -d'"' -f4
    else
        wget -qO- "https://api.github.com/repos/$REPO/releases/latest" | grep '"tag_name":' | cut -d'"' -f4
    fi
}

download_binary() {
    local version=$1
    local platform=$2
    local archive_name="sphinx-ultra-${version}-${platform}.tar.gz"
    local download_url="https://github.com/$REPO/releases/download/$version/$archive_name"
    local temp_dir=$(mktemp -d)

    log_info "Downloading Sphinx Ultra $version for $platform..."

    if command -v curl &> /dev/null; then
        curl -L "$download_url" -o "$temp_dir/$archive_name"
    else
        wget "$download_url" -O "$temp_dir/$archive_name"
    fi

    log_info "Extracting archive..."
    tar -xzf "$temp_dir/$archive_name" -C "$temp_dir"

    echo "$temp_dir/sphinx-ultra-${version}-${platform}/$BINARY_NAME"
}

install_binary() {
    local binary_path=$1

    log_info "Installing to $INSTALL_DIR..."

    # Create install directory if it doesn't exist
    mkdir -p "$INSTALL_DIR"

    # Copy binary
    cp "$binary_path" "$INSTALL_DIR/$BINARY_NAME"
    chmod +x "$INSTALL_DIR/$BINARY_NAME"

    log_success "Sphinx Ultra installed successfully!"
}

add_to_path() {
    local shell_rc=""

    case "$SHELL" in
        */bash) shell_rc="$HOME/.bashrc" ;;
        */zsh) shell_rc="$HOME/.zshrc" ;;
        */fish) shell_rc="$HOME/.config/fish/config.fish" ;;
        *) log_warning "Unknown shell, please add $INSTALL_DIR to your PATH manually" ;;
    esac

    if [[ -n "$shell_rc" ]] && [[ ! ":$PATH:" == *":$INSTALL_DIR:"* ]]; then
        echo "" >> "$shell_rc"
        echo "# Added by Sphinx Ultra installer" >> "$shell_rc"
        echo "export PATH=\"$INSTALL_DIR:\$PATH\"" >> "$shell_rc"
        log_info "Added $INSTALL_DIR to PATH in $shell_rc"
        log_warning "Please restart your shell or run: source $shell_rc"
    fi
}

verify_installation() {
    if [[ ":$PATH:" == *":$INSTALL_DIR:"* ]] || command -v "$BINARY_NAME" &> /dev/null; then
        log_success "Installation verified!"
        "$INSTALL_DIR/$BINARY_NAME" --version 2>/dev/null || true
    else
        log_warning "Binary installed but not in PATH. Run: $INSTALL_DIR/$BINARY_NAME --version"
    fi
}

main() {
    echo "Sphinx Ultra Installer"
    echo "====================="

    check_dependencies

    local platform=$(detect_platform)
    log_info "Detected platform: $platform"

    local version=$(get_latest_version)
    if [[ -z "$version" ]]; then
        log_error "Failed to fetch latest version"
        exit 1
    fi
    log_info "Latest version: $version"

    local binary_path=$(download_binary "$version" "$platform")
    install_binary "$binary_path"
    add_to_path
    verify_installation

    echo ""
    log_success "Installation complete!"
    echo ""
    echo "Get started with:"
    echo "  $BINARY_NAME --help"
    echo ""
    echo "Documentation: https://github.com/$REPO"
}

# Handle command line arguments
case "${1:-}" in
    --help|-h)
        echo "Sphinx Ultra Installer"
        echo ""
        echo "Usage: $0 [options]"
        echo ""
        echo "Options:"
        echo "  --help, -h     Show this help message"
        echo "  --version, -v  Show installer version"
        echo ""
        echo "Environment variables:"
        echo "  INSTALL_DIR    Installation directory (default: \$HOME/.local/bin)"
        exit 0
        ;;
    --version|-v)
        echo "Sphinx Ultra Installer v1.0.0"
        exit 0
        ;;
    *)
        main "$@"
        ;;
esac
