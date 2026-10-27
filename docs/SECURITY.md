# Security policy

## Supported versions

sphinx-ultra is pre-1.0. Security fixes go into the latest minor release
only; there are no maintenance branches.

| Version | Supported |
|---|---|
| 0.6.x | yes |
| < 0.6 | no, upgrade |

## Reporting a vulnerability

Please do not open a public issue for a security problem.

- Preferred: [open a private security advisory](https://github.com/salioglu/sphinx-ultra/security/advisories/new)
  on GitHub.
- Or email <sinan@alioglu.org>.

Include what the problem is, what an attacker can achieve, how to reproduce
it (a minimal documentation project is ideal), and the sphinx-ultra, Rust and
OS versions. You will get an acknowledgement within 72 hours and a fix or a
mitigation plan within 30 days for anything rated high or critical. Fixed
issues are published as GitHub Security Advisories and called out in the
release notes.

## What the binary does, and does not, do

This section describes the real attack surface of the current binary so you
can decide how much to trust an untrusted documentation tree.

### Input

- **RST sources** are parsed by a hand-written parser (a port of docutils'
  grammar). It is fuzzed with property tests for totality, so malformed input
  should produce warnings, never a crash, but treat untrusted sources as
  untrusted input all the same.
- **`conf.py` is parsed, not executed.** No Python interpreter is invoked and
  no code from the configuration file runs. Values read from it still shape
  the build: paths, patterns and, below, which hosts are contacted.
- **YAML and JSON configuration** is deserialized with serde; malformed input
  is rejected with an error.
- **`include` and `literalinclude`** read files named by the documents. Paths
  resolve relative to the source directory (or to the document), and like
  Sphinx they may point outside the source tree, including through symbolic
  links. A document can therefore disclose the contents of any file the build
  user can read into the output. This matches Sphinx's behaviour.

### Network

A build makes outbound requests for exactly one purpose, and only when
configured to: HTTPS `GET`s of the `objects.inv` inventories named by
`intersphinx_mapping`. The mapping is empty by default, so a default build
opens no socket. There is no telemetry and no update check.

- Certificate verification is on by default (`tls_verify`); `tls_cacerts`
  supplies a CA bundle, `user_agent` and `intersphinx_timeout` control the
  request.
- Basic-auth credentials embedded in an inventory URL are sent to that host
  and are stripped from every link the build publishes.
- Fetched inventories are cached under the build's cache directory
  (`__intersphinx_cache__`) and persist between builds.
- An untrusted `conf.py` can make the build contact a host of its choosing.

### File system

- Output is written only under the output directory, which must not equal or
  contain the source directory (the build refuses, exit 1).
- The cache directory (default `OUTPUTDIR/.sphinx-ultra-cache`, or `-d`)
  holds serialized environment state; it is rebuilt from scratch when its
  format version changes and should be treated as build output, not as input
  to trust.
- Symbolic links in the source tree are followed.
- Large inputs are read into memory; there is no size limit.

### Not present

There is no development server, no WebSocket live reload, no template
sandbox to configure and no extension loading: listing an extension has no
effect, and no third-party code runs during a build. This section will change
when those subsystems ship (roadmap M3 to M5).

## Installing safely

- Release archives carry a `.sha256` file; the install script verifies it
  before installing, and `cargo install sphinx-ultra --locked` builds from the
  audited lockfile.
- Release binaries are built by the public GitHub Actions workflow in this
  repository with a build-provenance attestation. Verify one with:

  ```bash
  gh attestation verify sphinx-ultra-<version>-<platform>.tar.gz --repo salioglu/sphinx-ultra
  ```

- The install script does not edit your shell configuration; add
  `~/.local/bin` to `PATH` yourself.

## Dependencies

Every CI run executes `cargo audit` against the RustSec advisory database
and `cargo deny check` (advisories, licenses, duplicate and yanked crates,
registry sources). Dependabot opens weekly update PRs for crates and
GitHub Actions, and GitHub's dependency graph and secret scanning are
enabled on the repository.
