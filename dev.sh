#!/bin/bash

# Development helper script for Sphinx Ultra

set -e

# Colors for output
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
BLUE='\033[0;34m'
NC='\033[0m' # No Color

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

show_help() {
    echo "Sphinx Ultra Development Helper"
    echo ""
    echo "Usage: $0 <command>"
    echo ""
    echo "Commands:"
    echo "  setup     - Initial development setup"
    echo "  build     - Build the project in release mode"
    echo "  test      - Run all tests"
    echo "  bench     - Run benchmarks"
    echo "  fmt       - Format code"
    echo "  clippy    - Run clippy lints"
    echo "  check     - Run all checks (fmt, clippy, test)"
    echo "  clean     - Clean build artifacts"
    echo "  docs      - Generate documentation for GitHub Pages"
    echo "  docs-dev  - Generate and open documentation for development"
    echo "  serve     - Start development server"
    echo "  install   - Install locally"
    echo "  package   - Create release package"
    echo "  help      - Show this help"
}

setup() {
    log_info "Setting up development environment..."
    
    # Install Rust toolchain components
    rustup component add rustfmt clippy
    
    # Install additional tools
    if ! command -v cargo-audit &> /dev/null; then
        log_info "Installing cargo-audit..."
        cargo install cargo-audit
    fi
    
    if ! command -v cargo-llvm-cov &> /dev/null; then
        log_info "Installing cargo-llvm-cov..."
        cargo install cargo-llvm-cov
    fi
    
    log_success "Development environment setup complete!"
}

build() {
    log_info "Building Sphinx Ultra in release mode..."
    cargo build --release
    log_success "Build complete! Binary at: target/release/sphinx-ultra"
}

test() {
    log_info "Running tests..."
    cargo test --all-features
    cargo test --test integration_test
    log_success "All tests passed!"
}

bench() {
    log_info "Running benchmarks..."
    cargo bench
    log_success "Benchmarks complete!"
}

fmt() {
    log_info "Formatting code..."
    cargo fmt --all
    log_success "Code formatted!"
}

clippy() {
    log_info "Running clippy..."
    cargo clippy --all-targets --all-features -- -D warnings
    log_success "Clippy passed!"
}

check() {
    log_info "Running all checks..."
    fmt
    clippy
    test
    log_success "All checks passed!"
}

clean() {
    log_info "Cleaning build artifacts..."
    cargo clean
    rm -rf _build
    log_success "Clean complete!"
}

docs() {
    log_info "Generating documentation for GitHub Pages..."
    
    # Build Rust documentation
    cargo doc --all-features --no-deps
    
    # Create docs directory if it doesn't exist
    mkdir -p docs
    
    # Clean and copy Rust docs to docs folder (this will be gitignored)
    rm -rf docs/api
    cp -r target/doc docs/api
    
    # Create .nojekyll file for GitHub Pages
    touch docs/.nojekyll
    
    # Create index.html in docs folder that redirects to main documentation
    cat > docs/index.html << 'EOF'
<!DOCTYPE html>
<html>
<head>
    <meta charset="utf-8">
    <title>Sphinx Ultra Documentation</title>
    <meta http-equiv="refresh" content="0; url=./api/sphinx_ultra/index.html">
    <style>
        body { font-family: -apple-system, BlinkMacSystemFont, "Segoe UI", sans-serif; text-align: center; padding: 50px; }
        .container { max-width: 600px; margin: 0 auto; }
        h1 { color: #333; }
        p { color: #666; line-height: 1.6; }
        a { color: #0366d6; text-decoration: none; }
        a:hover { text-decoration: underline; }
    </style>
</head>
<body>
    <div class="container">
        <h1>Sphinx Ultra Documentation</h1>
        <p>If you are not redirected automatically, <a href="./api/sphinx_ultra/index.html">click here to view the documentation</a>.</p>
        <p><a href="https://github.com/salioglu/sphinx-ultra">View on GitHub</a></p>
    </div>
</body>
</html>
EOF
    
    log_success "Documentation generated in docs/ folder for GitHub Pages!"
    log_info "Note: docs/api/ is gitignored as it contains generated files"
    log_info "Enable GitHub Pages in repository settings to publish at: https://salioglu.github.io/sphinx-ultra"
}

docs_dev() {
    log_info "Generating documentation for development (opens in browser)..."
    cargo doc --all-features --no-deps --open
    log_success "Documentation generated and opened in browser!"
}

serve() {
    log_info "Starting development server..."
    cargo run -- serve --source examples/basic --port 8000
}

install() {
    log_info "Installing locally..."
    cargo install --path . --force
    log_success "Installed successfully!"
}

package() {
    log_info "Creating release package..."
    
    # Build release
    cargo build --release
    
    # Create package directory
    VERSION=$(grep '^version = ' Cargo.toml | head -1 | cut -d'"' -f2)
    PACKAGE_NAME="sphinx-ultra-${VERSION}"
    
    mkdir -p "dist/$PACKAGE_NAME"
    
    # Copy files
    cp target/release/sphinx-ultra "dist/$PACKAGE_NAME/"
    cp README.md LICENSE CHANGELOG.md "dist/$PACKAGE_NAME/"
    cp -r examples "dist/$PACKAGE_NAME/"
    cp -r docs "dist/$PACKAGE_NAME/"
    
    # Create archive
    cd dist
    tar -czf "${PACKAGE_NAME}.tar.gz" "$PACKAGE_NAME"
    cd ..
    
    log_success "Package created: dist/${PACKAGE_NAME}.tar.gz"
}

case "${1:-help}" in
    setup)
        setup
        ;;
    build)
        build
        ;;
    test)
        test
        ;;
    bench)
        bench
        ;;
    fmt)
        fmt
        ;;
    clippy)
        clippy
        ;;
    check)
        check
        ;;
    clean)
        clean
        ;;
    docs)
        docs
        ;;
    docs-dev)
        docs_dev
        ;;
    serve)
        serve
        ;;
    install)
        install
        ;;
    package)
        package
        ;;
    help|*)
        show_help
        ;;
esac
