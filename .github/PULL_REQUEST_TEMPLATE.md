## What this changes

<!-- One or two sentences. Why, not just what. -->

## Checks

- [ ] `tree-sitter test`
- [ ] `node scripts/parse-corpus.mjs --strict` — still 540/540
- [ ] `cargo test`, `cargo clippy --all-targets -- -D warnings`, `cargo fmt --all --check`
- [ ] `node scripts/smoke-lsp.mjs`
- [ ] Grammar changed? `src/` regenerated and committed
- [ ] User-visible? Added to `CHANGELOG.md` under `## [Unreleased]`
