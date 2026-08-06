#!/bin/bash

# Build script for Sphinx Ultra Rust Builder

set -e

echo "🦀 Building Sphinx Ultra Rust Builder..."

# Check if Rust is installed
if ! command -v rustc &> /dev/null; then
    echo "❌ Rust is not installed. Please install Rust from https://rustup.rs/"
    exit 1
fi

# Check Rust version
RUST_VERSION=$(rustc --version | cut -d' ' -f2)
echo "✅ Using Rust version: $RUST_VERSION"

# Navigate to rust-builder directory
cd "$(dirname "$0")"

# Clean previous build
echo "🧹 Cleaning previous build..."
cargo clean

# Build in release mode for maximum performance
echo "🚀 Building in release mode..."
cargo build --release

# Run tests
echo "🧪 Running tests..."
cargo test

# Run benchmarks (optional, only if --bench flag is passed)
if [ "$1" = "--bench" ]; then
    echo "📊 Running benchmarks..."
    cargo bench
fi

# Create symlink for easy access
BINARY_PATH="target/release/sphinx-ultra"
SYMLINK_PATH="./sphinx-ultra"

if [ -f "$BINARY_PATH" ]; then
    echo "✅ Build successful!"
    echo "📍 Binary location: $BINARY_PATH"

    # Create symlink in parent directory
    if [ -L "$SYMLINK_PATH" ]; then
        rm "$SYMLINK_PATH"
    fi
    ln -s "$BINARY_PATH" "$SYMLINK_PATH"
    echo "🔗 Created symlink: $SYMLINK_PATH"

    # Show binary size
    BINARY_SIZE=$(ls -lh "$BINARY_PATH" | awk '{print $5}')
    echo "📦 Binary size: $BINARY_SIZE"

    echo ""
    echo "🎉 Sphinx Ultra Rust Builder is ready to use!"
    echo ""
    echo "Usage examples:"
    echo "  ./sphinx-ultra build --source docs --output _build"
    echo "  ./sphinx-ultra stats --source docs"
    echo ""
else
    echo "❌ Build failed!"
    exit 1
fi
