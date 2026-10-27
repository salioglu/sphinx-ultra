# Testing and the oracles

Every compatibility claim in this repository is backed by a differential
fixture: a Python generator runs the pinned upstream toolchain, records what
it produces, and a Rust test replays the same input through sphinx-ultra and
compares. The fixtures are committed, so `cargo test` needs no Python.

Pins: `sphinx==9.1.0`, `docutils==0.22.4`, `snowballstemmer==3.1.1`;
for the sphinx-needs profile `sphinx-needs==8.5.0` at commit `58bcb59` with
`docutils==0.21.2`. Every generator asserts its pins at runtime.

## Running the suite

```bash
cargo test --locked --all-features        # what CI runs: 1,125 tests, 14 targets
scripts/dev.sh check                       # fmt --check, clippy -D warnings, then the suite
```

| Target | Tests | What it pins |
|---|---|---|
| lib unit tests (`src/`) | 903 | parser, environment, domains, search, inventory, config |
| bin unit tests (`src/main.rs`) | 7 | argument-mode dispatch |
| `tests/e2e_cli.rs` | 62 | the real binary against fixture projects: exit codes, warning text, output trees, `--config` routing, incremental rebuilds, the three Sphinx artifacts |
| `tests/env_differential.rs` | 61 | the environment oracle, one test per compared key group |
| `tests/html_differential.rs` | 55 (+2 ignored) | the HTML oracle harness: ledger schema, comparators, diagnostics, report |
| `tests/rst_proptest.rs` | 14 | the parser never panics on arbitrary input, signatures, annotations, include options |
| `tests/pattern_compatibility_tests.rs` | 10 | hand-written Sphinx 9.1 pattern semantics |
| `tests/inventory_roundtrip.rs` | 5 | `objects.inv` reader and writer, byte-exact |
| `tests/sphinx_doctree_differential.rs` | 3 | the Sphinx read-phase oracle |
| `tests/doctree_serde.rs`, `doctree_intern_cap.rs`, `doctree_differential.rs`, `pattern_differential.rs` | 5 | bincode round-trip, interner bound, the docutils and pattern oracles |

## The oracles

| Oracle | Fixture | Cases | Generator | Consumer |
|---|---|---|---|---|
| docutils doctree (parse layer) | `tests/fixtures/doctree_differential.json` | 735 | `tools/gen_doctree_fixture.py` | `tests/doctree_differential.rs` |
| Sphinx read phase | `tests/fixtures/sphinx_doctree_differential.json` | 489 (py 57, pyconf 33, pysig 32, admonitions 38, body 32, directives 73, image 9, plain 153, roles 28, std 34) | `tools/gen_sphinx_fixture.py` | `tests/sphinx_doctree_differential.rs` |
| Build environment | `tests/fixtures/env_differential.json` | 29 projects, 84 documents, 17 compared keys each | `tools/gen_env_fixture.py` | `tests/env_differential.rs` |
| Pattern matching | `tests/fixtures/pattern_differential.json` | 881 | `tools/gen_pattern_fixture.py` | `tests/pattern_differential.rs` |
| `objects.inv` | `tests/fixtures/inventories/*.inv` | 12 (4 written by sphinx-build, 8 handcrafted incl. 5 malformed) | `tools/gen_inventory_fixture.py` | `tests/inventory_roundtrip.rs`, `src/intersphinx/tests.rs` |
| English search terms | `tests/fixtures/search_english.tsv` | 25,000 words | `tools/gen_search_english_fixture.py` | `src/search.rs` unit tests |
| HTML output, `core` profile | `tests/fixtures/html_oracle/core/` | 1,264 (1,260 built, 3 reference crashes, 1 build error) | `tools/gen_html_oracle.py` | `tests/html_differential.rs` |
| HTML output, `local_needs` profile | `tests/fixtures/html_oracle/local_needs/` | 142 sphinx-needs projects (87 built, 49 need PlantUML, 3 need network, 2 build errors, 1 reference crash) | same, `--profile local_needs` | same |
| Unicode tables | `src/rst/digits.rs`, `src/rst/punctuation.rs` | generated source | `tools/gen_digit_tables.py`, `tools/gen_punctuation_tables.py` | compiled in |

### Exemption tables

The environment oracle is at zero divergence on every compared key, with
known gaps pinned explicitly in `tests/env_differential.rs`: 43
resolved-doctree entries (`KNOWN_RESOLVED_GAPS`: write-phase toctree
resolution, module targets, `PropagateTargets`, image candidates, docinfo
removal) and 5 warning-stream entries (`KNOWN_WARNING_GAPS`). Each table is
self-cleaning: fixing a gap fails the test until its entry is removed, so the
tables are an accurate list of what is left.

## Regenerating fixtures

Regenerate only when a pin or the case corpus changes, never to make
sphinx-ultra pass. `PYTHONNOUSERSITE=1` is not optional: `uv run` keeps the
user site-packages on `sys.path`, and a user-site Pygments silently changes
every `code::` case.

```bash
PYTHONNOUSERSITE=1 uv run --python 3.12 --with docutils==0.22.4 python tools/gen_doctree_fixture.py
PYTHONNOUSERSITE=1 uv run --python 3.12 --with 'sphinx==9.1.0' --with 'docutils==0.22.4' python tools/gen_sphinx_fixture.py
PYTHONNOUSERSITE=1 uv run --python 3.12 --with 'sphinx==9.1.0' --with 'docutils==0.22.4' python tools/gen_env_fixture.py
PYTHONNOUSERSITE=1 uv run --python 3.12 --with 'sphinx==9.1.0' --with 'docutils==0.22.4' python tools/gen_inventory_fixture.py
PYTHONNOUSERSITE=1 uv run --python 3.12 --with 'sphinx>=9.1,<9.2' python tools/gen_pattern_fixture.py

uv sync --locked --python 3.12 --project tools/oracle_profiles/core
PYTHONNOUSERSITE=1 uv run --locked --python 3.12 --project tools/oracle_profiles/core python tools/gen_search_english_fixture.py

PYTHONNOUSERSITE=1 uv run --python 3.12 python tools/gen_digit_tables.py && cargo fmt --all
PYTHONNOUSERSITE=1 uv run --python 3.12 --with docutils==0.22.4 python tools/gen_punctuation_tables.py && cargo fmt --all
```

Each generator's module docstring is the authoritative command and explains
the held-out cases.

### The HTML oracle corpus

The corpus is generated on Linux only (paths and separators would otherwise
leak into the references) by the manual `HTML oracle` workflow
(`.github/workflows/html-oracle.yml`), which syncs both locked profiles under
`tools/oracle_profiles/`, runs the generator's own pytest, generates and
verifies both profiles, and uploads the result as an artifact. Two
regenerations must be byte-identical; check with
`git diff --exit-code tests/fixtures/html_oracle`. The `.claude/skills/html-oracle/SKILL.md`
file documents the generator's determinism shims and gotchas.

## Running the HTML differential

The exhaustive comparison is `#[ignore]`d and **red by design** until the HTML
writer lands; a nonzero exit is the normal signal. It runs every runnable case
through the real binary and writes a report:

```bash
cargo test --release --test html_differential html_oracle_exhaustive -- --ignored --nocapture
HTML_ORACLE_FILTER=core/sphinx_read_snippets/<case> cargo test --release --test html_differential html_oracle_exhaustive -- --ignored --nocapture
HTML_ORACLE_ULTRA_BIN=/path/to/sphinx-ultra ...     # compare a binary built elsewhere
HTML_ORACLE_KEEP=all ...                            # keep run directories of passing cases too
```

`target/html-oracle/report.md` (and `report.json`) lists per-source-set
pass/fail counts, the mismatches shared by the most cases, the most common
first divergences, and a rerun filter per case; each failing case leaves
`expected/` and `actual/` trees under `target/html-oracle/runs/` for a plain
`diff -r`.

## Continuous integration

`.github/workflows/ci.yml` runs on every push and pull request:

| Job | What it runs |
|---|---|
| Test (ubuntu/windows/macos × stable, plus beta on ubuntu) | `cargo fmt --check`, `cargo clippy --all-targets --all-features -D warnings` (stable only), `cargo test --locked --all-features` |
| MSRV (1.85) | `cargo check --locked --all-targets` on the declared minimum |
| Musl build | `cross build --locked --target x86_64-unknown-linux-musl` |
| Benchmark | `cargo bench --locked --no-run` (compile only) |
| Security audit | `cargo audit` on the lockfile; `cargo deny check` for licenses, bans and sources |
| Code coverage | `cargo llvm-cov` uploaded to Codecov |

## Benchmarks

`benches/builder_benchmark.rs` compiles but its first group hands the parser a
`test.rst` path that does not exist, so `cargo bench` panics on the first
iteration; CI only compiles it. The benchmarks are rewritten together with
the HTML writer, when there is a real page to measure (ROADMAP M2 wave 5 and
M7).
