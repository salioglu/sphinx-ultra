# Security Policy

## Supported Versions

We actively support and provide security updates for the following versions:

| Version | Supported          |
| ------- | ------------------ |
| 0.1.x   | :white_check_mark: |

## Reporting a Vulnerability

We take security vulnerabilities seriously. If you discover a security vulnerability, please follow these steps:

### For High-Severity Issues

For critical security vulnerabilities that could compromise user data or system security:

1. **Do NOT** open a public GitHub issue
2. Email us directly at: <security@sphinx-ultra.com>
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

### Network Security

- Use HTTPS for external resources
- Validate SSL certificates
- Be cautious with live reload in production

### Configuration Security

- Protect configuration files with sensitive data
- Use environment variables for secrets
- Regular review of configuration settings

## Known Security Considerations

### Template Rendering

- Handlebars templates are sandboxed
- Custom templates should be reviewed for XSS vulnerabilities
- User-provided content is escaped by default

### File Processing

- Large files may cause memory exhaustion
- Symbolic links are followed (potential security risk)
- Binary files are properly handled

### Network Features

- Development server is not hardened for production
- WebSocket connections for live reload are not authenticated
- CORS is configured permissively for development

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

- Security issues: <security@sphinx-ultra.com>
- General questions: <contact@sphinx-ultra.com>
- GitHub: [Security Advisories](https://github.com/salioglu/sphinx-ultra/security)

---

Thank you for helping keep Sphinx Ultra secure!
