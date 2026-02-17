# Contributing to nanobot

Thank you for considering contributing to nanobot! 🎉

## Getting Started

1. **Fork** the repository
2. **Clone** your fork: `git clone https://github.com/YOUR_USERNAME/nanobot.git`
3. **Create a branch**: `git checkout -b feat/my-feature`
4. **Make your changes**
5. **Test** your changes: `cargo test --all`
6. **Lint** your code: `cargo clippy --all-targets`
7. **Format** your code: `cargo fmt --all`
8. **Commit** with conventional commits: `git commit -m "feat: add feature"`
9. **Push** to your fork: `git push origin feat/my-feature`
10. **Open a Pull Request**

## Development Setup

### Prerequisites
- Rust 1.75+ ([rustup.rs](https://rustup.rs))
- At least one LLM API key (OpenAI, Anthropic, or Google)

### Local Development

```bash
# Set environment variables
export OPENAI_API_KEY=sk-...
export ANTHROPIC_API_KEY=sk-ant-...

# Run locally
cargo run --release -- gateway --http --http-port 3000

# Run tests
cargo test --all

# Run with hot-reload (requires cargo-watch)
cargo install cargo-watch
cargo watch -x 'run -- gateway --http --http-port 3000'
```

## Code Style

- Follow `rustfmt` defaults
- Use `clippy` lints
- Write tests for new features
- Update documentation for API changes
- Keep commits atomic and well-described

## Commit Convention

Use [Conventional Commits](https://www.conventionalcommits.org/):

```
feat: add new feature
fix: fix bug
docs: update documentation
style: format code
refactor: refactor code
test: add tests
chore: update dependencies
```

## Pull Request Process

1. **Update tests** - Add/update tests for your changes
2. **Update docs** - Update README or docs/ if needed
3. **Pass CI** - Ensure all tests pass
4. **Get review** - 1 approving review required (admin can bypass)
5. **Merge** - Squash and merge after approval

## What to Contribute

### Good First Issues
- Add new tools to `integrations.rs`
- Improve error messages
- Add tests for existing features
- Fix typos in documentation
- Add examples to README

### Advanced Contributions
- Add new LLM providers
- Add new channel integrations
- Optimize performance
- Improve memory management
- Add new features to the core

## Project Structure

```
nanobot/
├── crates/
│   ├── nanobot-core/       # Core library
│   │   ├── service/        # HTTP, commands, auth, integrations
│   │   ├── provider/       # LLM providers
│   │   ├── channel/        # Channel integrations
│   │   └── memory/         # Long-term memory
│   └── nanobot-lambda/     # AWS Lambda wrapper
├── web/                    # Frontend SPA
├── infra/                  # AWS SAM templates
├── tests/                  # Integration tests
└── src/                    # CLI binary
```

## Testing

```bash
# Run all tests
cargo test --all

# Run specific test
cargo test test_name

# Run tests with output
cargo test -- --nocapture

# Run integration tests
cargo test --test integration_tests
```

## Reporting Issues

When reporting issues, please include:
- **OS and version** (Linux, macOS, Windows)
- **Rust version** (`rustc --version`)
- **Steps to reproduce**
- **Expected behavior**
- **Actual behavior**
- **Error messages** (if any)

## Code of Conduct

Be kind, respectful, and constructive. We're all here to build something great together.

## License

By contributing, you agree that your contributions will be licensed under the MIT License.
