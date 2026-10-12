Raw Sphinx 9.1.0 error reports captured from the ubuntu-latest html-oracle
workflow. They are deliberately unreduced: the generator and comparator tests
feed them through the error-report reduction to prove that host details
(runner paths, site-packages frames, the kernel string) never reach the
committed oracle corpus under `tests/fixtures/html_oracle/`.
