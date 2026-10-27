#!/usr/bin/env bash
# Developer helper for sphinx-ultra.
#
# Usage: scripts/dev.sh <command>
#
# Every command runs from the repository root, whatever the current
# directory is, and uses the same flags CI uses, so a green `check` here is
# a green CI run there.

set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT"

if [ -t 2 ]; then
    BLUE='\033[0;34m'; GREEN='\033[0;32m'; YELLOW='\033[1;33m'; RED='\033[0;31m'; NC='\033[0m'
else
    BLUE=''; GREEN=''; YELLOW=''; RED=''; NC=''
fi
info()    { printf '%b[info]%b %s\n'  "$BLUE"   "$NC" "$*" >&2; }
success() { printf '%b[ok]%b %s\n'    "$GREEN"  "$NC" "$*" >&2; }
warn()    { printf '%b[warn]%b %s\n'  "$YELLOW" "$NC" "$*" >&2; }
fail()    { printf '%b[error]%b %s\n' "$RED"    "$NC" "$*" >&2; exit 1; }

usage() {
    cat <<'EOF'
sphinx-ultra developer helper

Usage: scripts/dev.sh <command>

Setup
  setup          Install toolchain components and cargo tools used by CI
                 (rustfmt, clippy, cargo-audit, cargo-deny, cargo-llvm-cov)
  install-hooks  Install the git pre-commit and pre-push hooks

Everyday
  check          fmt --check, clippy -D warnings, cargo test --locked (what CI gates on)
  fmt            Format the code (cargo fmt --all)
  clippy         cargo clippy --all-targets --all-features -- -D warnings
  test           cargo test --locked --all-features
  build          cargo build --release
  bench          cargo bench --locked --no-run (the benchmarks compile; see docs/testing.md)
  clean          cargo clean and remove _build/

Supply chain
  audit          cargo audit (RustSec advisories against Cargo.lock)
  deny           cargo deny check (advisories, licenses, bans, sources; see deny.toml)

Documentation and oracles
  docs           Build the API docs (cargo doc) and open them
  pages          Build the GitHub Pages tree into target/pages (what docs.yml deploys)
  oracle [FILTER]
                 Run the HTML differential against the release binary; red by design
                 until the HTML writer lands. FILTER narrows to matching cases.

Other
  install        cargo install --path . --locked --force
  version        Check that Cargo.toml and the git tag agree (scripts/check-version.sh)
  help           This text
EOF
}

require_cargo() {
    command -v cargo >/dev/null 2>&1 || fail "cargo not found; install Rust from https://rustup.rs/"
}

install_cargo_tool() {
    # $1 = binary name, $2 = crate name
    if command -v "$1" >/dev/null 2>&1; then
        info "$1 already installed"
    else
        info "installing $2"
        cargo install "$2" --locked
    fi
}

cmd_setup() {
    require_cargo
    if command -v rustup >/dev/null 2>&1; then
        rustup component add rustfmt clippy llvm-tools-preview
    else
        warn "rustup not found; make sure rustfmt and clippy are available"
    fi
    install_cargo_tool cargo-audit cargo-audit
    install_cargo_tool cargo-deny cargo-deny
    install_cargo_tool cargo-llvm-cov cargo-llvm-cov
    success "development environment ready; next: scripts/dev.sh install-hooks"
}

cmd_install_hooks() {
    [ -d .git ] || fail "not a git repository"
    cat > .git/hooks/pre-commit <<'EOF'
#!/bin/sh
# Installed by scripts/dev.sh install-hooks. Runs the same gates as CI.
exec scripts/dev.sh pre-commit
EOF
    chmod +x .git/hooks/pre-commit
    cp scripts/pre-push .git/hooks/pre-push
    chmod +x .git/hooks/pre-push
    success "installed .git/hooks/pre-commit and .git/hooks/pre-push"
}

cmd_fmt()    { require_cargo; cargo fmt --all; success "formatted"; }
cmd_clippy() { require_cargo; cargo clippy --all-targets --all-features -- -D warnings; success "clippy clean"; }
cmd_test()   { require_cargo; cargo test --locked --all-features "$@"; success "tests passed"; }
cmd_build()  { require_cargo; cargo build --release; success "binary at target/release/sphinx-ultra"; }
cmd_bench()  { require_cargo; cargo bench --locked --no-run; success "benchmarks compile"; }
cmd_audit()  { require_cargo; command -v cargo-audit >/dev/null 2>&1 || fail "cargo-audit missing; run scripts/dev.sh setup"; cargo audit; }
cmd_deny()   { require_cargo; command -v cargo-deny  >/dev/null 2>&1 || fail "cargo-deny missing; run scripts/dev.sh setup";  cargo deny check; }
cmd_install(){ require_cargo; cargo install --path . --locked --force; success "installed sphinx-ultra"; }
cmd_version(){ scripts/check-version.sh; }

cmd_pre_commit() {
    require_cargo
    info "cargo fmt --check"
    cargo fmt --all -- --check || fail "formatting differs; run scripts/dev.sh fmt"
    info "cargo clippy"
    cargo clippy --all-targets --all-features -- -D warnings || fail "clippy found issues"
    info "cargo check"
    cargo check --all-targets --all-features || fail "compilation failed"
    success "pre-commit checks passed"
}

cmd_check() {
    require_cargo
    info "cargo fmt --check"
    cargo fmt --all -- --check || fail "formatting differs; run scripts/dev.sh fmt"
    info "cargo clippy --locked --all-targets --all-features -- -D warnings"
    cargo clippy --locked --all-targets --all-features -- -D warnings
    info "cargo test --locked --all-features"
    cargo test --locked --all-features
    success "all checks passed"
}

cmd_clean() {
    require_cargo
    cargo clean
    rm -rf _build
    success "clean"
}

cmd_docs() {
    require_cargo
    cargo doc --all-features --no-deps --open
}

# The GitHub Pages tree: API docs under api/ plus a landing page. Built into
# target/pages so that docs/ stays a source directory (it holds the markdown
# documentation) and nothing generated is ever committed.
cmd_pages() {
    require_cargo
    local out="target/pages"
    cargo doc --all-features --no-deps
    rm -rf "$out"
    mkdir -p "$out"
    cp -r target/doc "$out/api"
    cp assets/icon.svg "$out/icon.svg"
    cp assets/logo-dark.svg "$out/logo.svg"
    touch "$out/.nojekyll"
    cat > "$out/index.html" <<'EOF'
<!DOCTYPE html>
<html lang="en">
<head>
  <meta charset="utf-8">
  <meta name="viewport" content="width=device-width, initial-scale=1">
  <title>sphinx-ultra</title>
  <link rel="icon" href="icon.svg" type="image/svg+xml">
  <style>
    :root { color-scheme: light dark; --bg: #14213D; --fg: #FBE3D3; --accent: #F2743D; --muted: #A9B4C6; }
    body { margin: 0; min-height: 100vh; display: grid; place-items: center; background: var(--bg); color: var(--fg);
           font-family: -apple-system, BlinkMacSystemFont, "Segoe UI", Helvetica, Arial, sans-serif; }
    main { max-width: 40rem; padding: 3rem 1.5rem; text-align: center; }
    img { width: 160px; height: 160px; }
    h1 { font-size: 2.5rem; margin: 1rem 0 0.25rem; letter-spacing: -0.02em; }
    p { color: var(--muted); line-height: 1.5; }
    nav a { display: inline-block; margin: 0.5rem 0.4rem; padding: 0.6rem 1.1rem; border-radius: 0.5rem;
            background: var(--accent); color: var(--bg); font-weight: 600; text-decoration: none; }
    nav a.secondary { background: transparent; color: var(--fg); border: 1px solid var(--muted); }
  </style>
</head>
<body>
  <main>
    <img src="logo.svg" alt="sphinx-ultra logo">
    <h1>sphinx-ultra</h1>
    <p><code>sphinx-build</code>, reimplemented in Rust. Parity verified against real Sphinx 9.1 output.</p>
    <nav>
      <a href="api/sphinx_ultra/index.html">API documentation</a>
      <a class="secondary" href="https://github.com/salioglu/sphinx-ultra#readme">README</a>
      <a class="secondary" href="https://github.com/salioglu/sphinx-ultra/tree/main/docs">Guides</a>
      <a class="secondary" href="https://crates.io/crates/sphinx-ultra">crates.io</a>
    </nav>
  </main>
</body>
</html>
EOF
    success "pages tree in $out"
}

cmd_oracle() {
    require_cargo
    local filter="${1:-}"
    if [ -n "$filter" ]; then
        info "HTML differential, filter: $filter"
        HTML_ORACLE_FILTER="$filter" cargo test --release --test html_differential html_oracle_exhaustive -- --ignored --nocapture || true
    else
        info "HTML differential, every runnable case (red by design until the HTML writer)"
        cargo test --release --test html_differential html_oracle_exhaustive -- --ignored --nocapture || true
    fi
    info "report: target/html-oracle/report.md"
}

cmd="${1:-help}"
shift || true
case "$cmd" in
    setup)         cmd_setup ;;
    install-hooks) cmd_install_hooks ;;
    check)         cmd_check ;;
    pre-commit)    cmd_pre_commit ;;
    fmt)           cmd_fmt ;;
    clippy)        cmd_clippy ;;
    test)          cmd_test "$@" ;;
    build)         cmd_build ;;
    bench)         cmd_bench ;;
    clean)         cmd_clean ;;
    audit)         cmd_audit ;;
    deny)          cmd_deny ;;
    docs)          cmd_docs ;;
    pages)         cmd_pages ;;
    oracle)        cmd_oracle "$@" ;;
    install)       cmd_install ;;
    version)       cmd_version ;;
    help|-h|--help) usage ;;
    *) usage; fail "unknown command: $cmd" ;;
esac
