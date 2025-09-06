# Contributing to Sphinx Ultra

We love your input! We want to make contributing to Sphinx Ultra as easy and transparent as possible, whether it's:

- Reporting a bug
- Discussing the current state of the code
- Submitting a fix
- Proposing new features
- Becoming a maintainer

## Development Process

We use GitHub to host code, to track issues and feature requests, as well as accept pull requests.

## Pull Requests

Pull requests are the best way to propose changes to the codebase. We actively welcome your pull requests:

1. Fork the repo and create your branch from `main`.
2. If you've added code that should be tested, add tests.
3. If you've changed APIs, update the documentation.
4. Ensure the test suite passes.
5. Make sure your code lints.
6. Issue that pull request!

## Development Setup

### Prerequisites

- Rust 1.70 or higher
- Git

### Setup

1. Clone the repository:
   ```bash
   git clone https://github.com/salioglu/sphinx-ultra.git
   cd sphinx-ultra
   ```

2. Build the project:
   ```bash
   ./build.sh
   ```

3. Run tests:
   ```bash
   cargo test
   ```

4. Run benchmarks:
   ```bash
   cargo bench
   ```

## Code Style

We use the standard Rust formatting and linting tools:

- **Formatting**: `cargo fmt`
- **Linting**: `cargo clippy`

Please run these before submitting your pull request.

## Testing

We aim for high test coverage. When adding new features:

1. Add unit tests for individual functions
2. Add integration tests for major workflows
3. Add benchmarks for performance-critical code

Run the full test suite with:
```bash
cargo test --all
```

## Performance Considerations

Sphinx Ultra is built for performance. When contributing:

- Profile your changes with `cargo bench`
- Consider memory usage and allocations
- Use `rayon` for parallelization where appropriate
- Leverage the caching system for expensive operations

## Documentation

- Keep the README.md updated with any new features
- Add inline documentation for public APIs
- Update examples if you change functionality

## Reporting Bugs

We use GitHub issues to track public bugs. Report a bug by [opening a new issue](https://github.com/salioglu/sphinx-ultra/issues).

**Great Bug Reports** tend to have:

- A quick summary and/or background
- Steps to reproduce
  - Be specific!
  - Give sample code if you can
- What you expected would happen
- What actually happens
- Notes (possibly including why you think this might be happening, or stuff you tried that didn't work)

## Feature Requests

We love feature requests! Please [open an issue](https://github.com/salioglu/sphinx-ultra/issues) with:

- Clear description of the feature
- Why you want this feature (use case)
- How it should work (if you have ideas)

## License

By contributing, you agree that your contributions will be licensed under the MIT License.

## References

This document was adapted from the open-source contribution guidelines for [Facebook's Draft](https://github.com/facebook/draft-js/blob/a9316a723f9e918afde44dea68b5f9f39b7d9b00/CONTRIBUTING.md).
