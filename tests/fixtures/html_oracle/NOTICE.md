# HTML oracle corpus notices

The oracle stores generated Sphinx output and read-only inputs from the
following source sets. Licensing is recorded once per source set, not per
captured file.

| Source set | Material | License / provenance |
| --- | --- | --- |
| `docutils_snippets` | Docutils-derived reStructuredText snippets | BSD-2-Clause |
| `sphinx_read_snippets` | Sphinx and Docutils-derived snippets | BSD-2-Clause |
| `environment_projects` | Checked-in Sphinx fixture projects | repository license |
| `html_projects` | Checked-in HTML fixture projects | repository license |
| `inventory_projects` | Sphinx-built inventory fixture projects | BSD-2-Clause |
| `sphinx_needs_doc_tests` | Sphinx-Needs documentation test projects | MIT |
| Sphinx and alabaster theme assets | Static files emitted by the Sphinx HTML builder | BSD-2-Clause |

The 881 records in `tests/fixtures/pattern_differential.json` are parser-only
pattern cases. They are outside this HTML corpus and are not materialized or
ledgered here. The handcrafted inventory files under
`tests/fixtures/inventories` remain parser fixtures; only the four
Sphinx-built inventory projects are included above.
