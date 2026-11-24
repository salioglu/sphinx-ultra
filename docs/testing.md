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
cargo test --locked --all-features        # what CI runs: 1,371 tests (+6 ignored), 14 targets
scripts/dev.sh check                       # fmt --check, clippy -D warnings, then the suite
```

| Target | Tests | What it pins |
|---|---|---|
| lib unit tests (`src/`) | 1,111 | parser, read transforms, environment, domains, search, inventory, config |
| bin unit tests (`src/main.rs`) | 7 | argument-mode dispatch |
| `tests/e2e_cli.rs` | 89 | the real binary against fixture projects: exit codes, the warning stream on stderr and `-w`, record order at `-j 1` and `-j 4`, output trees, `--config` routing, incremental rebuilds, the three Sphinx artifacts |
| `tests/html_differential.rs` | 62 (+3 ignored) | the HTML oracle harness: ledger schema, comparators, diagnostics, report, and the strict aux gate over every runnable core case |
| `tests/env_differential.rs` | 61 | the environment oracle, one test per compared key group |
| `tests/rst_proptest.rs` | 14 | the parser never panics on arbitrary input, signatures, annotations, include options |
| `tests/pattern_compatibility_tests.rs` | 10 | hand-written Sphinx 9.1 pattern semantics |
| `tests/sphinx_doctree_differential.rs` | 5 (+2 ignored) | the Sphinx read-phase oracle on the post-read tree, and the HTML oracle's docutils snippets post-read and resolved |
| `tests/inventory_roundtrip.rs` | 5 | `objects.inv` reader and writer, byte-exact |
| `tests/read_state_differential.rs` | 1 (+1 ignored) | the read-state oracle |
| `tests/doctree_serde.rs`, `doctree_intern_cap.rs`, `doctree_differential.rs`, `pattern_differential.rs` | 6 | bincode round-trip, interner bound, the docutils and pattern oracles |

The ignored tests are the exhaustive HTML oracle run, its diagnostics helper,
and the censuses that print each strict table's current rows.

## The oracles

| Oracle | Fixture | Cases | Generator | Consumer |
|---|---|---|---|---|
| docutils doctree (parse layer) | `tests/fixtures/doctree_differential.json` | 735 | `tools/gen_doctree_fixture.py` | `tests/doctree_differential.rs` |
| Sphinx read phase (post-read doctree) | `tests/fixtures/sphinx_doctree_differential.json` | 493 | `tools/gen_sphinx_fixture.py` | `tests/sphinx_doctree_differential.rs` |
| docutils snippets under Sphinx (post-read and dummy-resolved doctrees) | `tests/fixtures/sphinx_docutils_doctree_differential.json` | 734 (the HTML oracle's runnable docutils cases) | `tools/gen_sphinx_fixture.py --corpus html-oracle-docutils` | `tests/sphinx_doctree_differential.rs` |
| Read state (ids, registries, final position, warnings) | `tests/fixtures/read_state_differential.json` | 99 | `tools/gen_read_state_fixture.py` | `tests/read_state_differential.rs` |
| Build environment | `tests/fixtures/env_differential.json` | 31 projects, 89 documents, 17 compared keys each | `tools/gen_env_fixture.py` | `tests/env_differential.rs` |
| Pattern matching | `tests/fixtures/pattern_differential.json` | 881 | `tools/gen_pattern_fixture.py` | `tests/pattern_differential.rs` |
| `objects.inv` | `tests/fixtures/inventories/*.inv` | 12 (4 written by sphinx-build, 8 handcrafted incl. 5 malformed) | `tools/gen_inventory_fixture.py` | `tests/inventory_roundtrip.rs`, `src/intersphinx/tests.rs` |
| English search terms | `tests/fixtures/search_english.tsv` | 25,000 words | `tools/gen_search_english_fixture.py` | `src/search.rs` unit tests |
| HTML output, `core` profile | `tests/fixtures/html_oracle/core/` | 1,264 (1,260 built, 3 reference crashes, 1 build error) | `tools/gen_html_oracle.py` | `tests/html_differential.rs` |
| HTML output, `local_needs` profile | `tests/fixtures/html_oracle/local_needs/` | 142 sphinx-needs projects (87 built, 49 need PlantUML, 3 need network, 2 build errors, 1 reference crash) | same, `--profile local_needs` | same |
| Unicode tables | `src/rst/digits.rs`, `src/rst/punctuation.rs` | generated source | `tools/gen_digit_tables.py`, `tools/gen_punctuation_tables.py` | compiled in |
| Pygments lexer aliases | `src/rst/pygments_aliases.rs` | generated source | `tools/gen_pygments_aliases.py` | compiled in |

### Exemption tables

Every known gap is pinned in a strict, self-cleaning table: a listed case
that stops diverging fails the test, and so does an unlisted case that
diverges, so the tables are an accurate list of what is left. As of M2 wave
5a-1:

| Table | Rows | Owner |
|---|---|---|
| `KNOWN_AUX_GAPS` (`tests/html_differential.rs`): warning stream, exit class, `objects.inv`, `searchindex.js` of the 1,261 runnable core HTML-oracle cases | 3 | the HTML writer (5b): three image-scale warning streams |
| `KNOWN_READ_STATE_GAPS` (`tests/read_state_differential.rs`) | 4 | PR 5a-2's structure directives: one case's pending serials |
| `KNOWN_POST_READ_GAPS`, `KNOWN_RESOLVED_STANDALONE_GAPS` (`tests/sphinx_doctree_differential.rs`) | 0 | — |
| `KNOWN_TOC_GAPS`, `KNOWN_STD_GAPS`, `KNOWN_WARNING_GAPS`, `KNOWN_RESOLVED_GAPS` (`tests/env_differential.rs`) | 0 | — |

Each table's ignored census test prints its current rows, and
`exemption_arithmetic_matches_the_documented_numbers` holds the environment
oracle's documented counts (`DOCUMENTED_*`) to the tables and the fixture.

## Regenerating fixtures

Regenerate only when a pin or the case corpus changes, never to make
sphinx-ultra pass. `PYTHONNOUSERSITE=1` is not optional: `uv run` keeps the
user site-packages on `sys.path`, and a user-site Pygments silently changes
every `code::` case.

```bash
PYTHONNOUSERSITE=1 uv run --python 3.12 --with docutils==0.22.4 python tools/gen_doctree_fixture.py
PYTHONNOUSERSITE=1 uv run --locked --python 3.12 --project tools/oracle_profiles/core python tools/gen_sphinx_fixture.py
PYTHONNOUSERSITE=1 uv run --locked --python 3.12 --project tools/oracle_profiles/core python tools/gen_sphinx_fixture.py --corpus html-oracle-docutils
PYTHONNOUSERSITE=1 uv run --locked --python 3.12 --project tools/oracle_profiles/core python tools/gen_read_state_fixture.py
PYTHONNOUSERSITE=1 uv run --python 3.12 --with 'sphinx==9.1.0' --with 'docutils==0.22.4' python tools/gen_env_fixture.py
PYTHONNOUSERSITE=1 uv run --python 3.12 --with 'sphinx==9.1.0' --with 'docutils==0.22.4' python tools/gen_inventory_fixture.py
PYTHONNOUSERSITE=1 uv run --python 3.12 --with 'sphinx>=9.1,<9.2' python tools/gen_pattern_fixture.py

uv sync --locked --python 3.12 --project tools/oracle_profiles/core
PYTHONNOUSERSITE=1 uv run --locked --python 3.12 --project tools/oracle_profiles/core python tools/gen_search_english_fixture.py

PYTHONNOUSERSITE=1 uv run --python 3.12 python tools/gen_digit_tables.py && cargo fmt --all
PYTHONNOUSERSITE=1 uv run --python 3.12 --with docutils==0.22.4 python tools/gen_punctuation_tables.py && cargo fmt --all
PYTHONNOUSERSITE=1 uv run --locked --python 3.12 --project tools/oracle_profiles/core python tools/gen_pygments_aliases.py && cargo fmt --all
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
writer lands; a nonzero exit is the normal signal. (What it compares besides the
pages — the warning stream, exit class, `objects.inv` and `searchindex.js` — is
gated on every test run by `html_oracle_aux_gate`.) It runs every runnable case
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
