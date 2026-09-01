# Security Policy

## Supported Versions

We actively support and provide security updates for the following versions:

| Version | Supported          |
| ------- | ------------------ |
| 0.3.x   | :white_check_mark: |
| < 0.3   | :x:                |

## Reporting a Vulnerability

We take security vulnerabilities seriously. If you discover a security vulnerability, please follow these steps:

### For High-Severity Issues

For critical security vulnerabilities that could compromise user data or system security:

1. **Do NOT** open a public GitHub issue
2. Email us directly at: <sinan@alioglu.org>
3. Use the GitHub Security Advisory feature: [Report a vulnerability](https://github.com/salioglu/sphinx-ultra/security/advisories/new)

### For Lower-Severity Issues

For minor security concerns or potential vulnerabilities:

1. Open a private GitHub issue or discussion
2. Use the "🔒 Security" label
3. Provide detailed reproduction steps

## What to Include

When reporting a security vulnerability, please include:

- **Description**: Clear description of the vulnerability
- **Impact**: What an attacker could achieve
- **Reproduction**: Step-by-step instructions to reproduce
- **Environment**: OS, Rust version, Sphinx Ultra version
- **Suggested Fix**: If you have ideas for a fix (optional)

## Response Timeline

- **Acknowledgment**: Within 24 hours
- **Initial Assessment**: Within 72 hours
- **Status Updates**: Weekly until resolved
- **Fix Release**: Within 30 days for critical issues

## Security Best Practices

When using Sphinx Ultra:

### Input Validation

- Always validate and sanitize documentation source files
- Be cautious with user-provided configuration files
- Avoid processing untrusted RST/Markdown content

### File System Security

- Run with minimal required permissions
- Use dedicated build directories
- Avoid building in system directories

### Configuration Security

- Protect configuration files with sensitive data
- Use environment variables for secrets
- Regular review of configuration settings

## Known Security Considerations

The actual attack surface of the current binary is:

### Input Parsing

- RST/Markdown source files are parsed with hand-written scanners; treat
  untrusted documentation sources with caution
- `conf.py` files are **parsed, not executed** (no Python interpreter is
  invoked), but values from them flow into build configuration
- YAML/JSON configuration files are deserialized with serde; malformed input
  is rejected rather than executed

### File Processing

- Large files may cause memory exhaustion
- Symbolic links are followed (potential security risk)
- Output paths are derived from source paths under the configured output
  directory

### Network Access

As of the unreleased intersphinx support, `sphinx-ultra build` can make
outbound network requests. It does so for exactly one purpose, and only
when configured to:

- **What**: HTTPS `GET`s of the `objects.inv` inventories named by
  `intersphinx_mapping` in `conf.py`. Nothing else in the binary opens a
  socket — there is no telemetry, no update check, and no fetching of
  images, stylesheets or any other document content
- **When**: only if `intersphinx_mapping` is non-empty. It is empty by
  default, so a default build makes no network requests at all
- **Where**: the URLs come from the project's own configuration. Treat an
  untrusted `conf.py` as able to make the build contact a host of its
  choosing
- **TLS**: certificate verification is **on** by default (`tls_verify`).
  Setting `tls_verify = False` turns it off for these requests;
  `tls_cacerts` supplies a CA bundle, either one path for every host or a
  per-host mapping. `user_agent` sets the request's User-Agent, and
  `intersphinx_timeout` its timeout
- **Credentials**: basic-auth credentials embedded in an inventory URL are
  sent to that host and are stripped from any link the build publishes
- **On disk**: fetched inventories are cached under the build's cache
  directory (`__intersphinx_cache__`), so an inventory's contents persist
  between builds

### Not Applicable (yet)

Earlier versions of this document described a development server, WebSocket
live reload, CORS policy, and Handlebars template sandboxing. None of those
subsystems exist in the current binary (a dev server is planned — see
ROADMAP M3); this document will be updated when they ship.

## Security Updates

Security patches will be:

- Released as soon as possible
- Clearly marked in release notes
- Communicated through GitHub Security Advisories
- Include detailed remediation steps

## Third-Party Dependencies

We regularly audit our dependencies using:

- `cargo audit` for known vulnerabilities
- Dependabot for automated updates
- Manual review of security advisories

## Contact

- Security issues: <sinan@alioglu.org>
- General questions: <sinan@alioglu.org>
- GitHub: [Security Advisories](https://github.com/salioglu/sphinx-ultra/security)

---

Thank you for helping keep Sphinx Ultra secure!
