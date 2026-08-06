# Publishing

## 1. Split the grammar out

Zed clones the grammar from a URL at a pinned revision, so it must be its own
public repository before release. `tree-sitter-skript/` is already
self-contained — it has no imports from the rest of this monorepo.

```sh
git subtree split --prefix=tree-sitter-skript -b grammar-only
# push that branch to https://github.com/DaisyCatTs/tree-sitter-skript
```

Confirm the generated sources are committed — `src/parser.c`,
`src/grammar.json`, `src/node-types.json`, `src/tree_sitter/*.h`. Zed runs clang
on them directly and never invokes `tree-sitter generate`; a repository that
gitignores `src/` cannot be used as a Zed grammar at all.

Then point `extension.toml` at it:

```toml
[grammars.skript]
repository = "https://github.com/DaisyCatTs/tree-sitter-skript"
rev = "<the commit you just pushed>"
```

## 2. Release the language server

Tag the repository; CI builds `skript-lsp` for all five targets and attaches
them to the release. The asset names must keep matching what
`extension/src/lib.rs` looks for:

```
skript-lsp-x86_64-unknown-linux-gnu.tar.gz
skript-lsp-aarch64-unknown-linux-gnu.tar.gz
skript-lsp-x86_64-apple-darwin.tar.gz
skript-lsp-aarch64-apple-darwin.tar.gz
skript-lsp-x86_64-pc-windows-msvc.zip
```

The extension must never bundle the binary — the Zed registry forbids it, and
`lib.rs` already prefers a user-installed copy on `$PATH` anyway.

## 3. Submit to the Zed registry

Fork [`zed-industries/extensions`](https://github.com/zed-industries/extensions)
to a **personal** account (not an org — Zed staff need push access to fix your
PR).

```sh
git submodule add https://github.com/DaisyCatTs/SkriptZed.git extensions/skript
git add extensions/skript
```

```toml
# extensions.toml
[skript]
submodule = "extensions/skript"
version = "0.1.0"
path = "extension"     # extension.toml is not at the repo root
```

```sh
pnpm sort-extensions
```

### Requirements the registry CI enforces

* **HTTPS** submodule URLs — the SSH form is rejected.
* The submodule commit must be **on a branch**, not detached.
* A `LICENSE` file at the extension root. Ours is `extension/LICENSE`; accepted
  licences are Apache-2.0, BSD-2/3, CC BY 4.0, GPLv3, LGPLv3, MIT, Unlicense,
  zlib.
* The extension ID must not contain `zed` or `extension`. Ours is `skript`.
* `version` must match between `extension.toml` and `extensions.toml` at that
  commit.

Extension IDs are permanent.

## 4. Updating

```sh
git submodule update --remote extensions/skript
```

Bump `version` in **both** `extension.toml` and `extensions.toml`.

## Checklist

- [ ] `[grammars.skript]` uses the public HTTPS URL, not the `file://` one that
      `dev-setup.mjs` writes
- [ ] Generated parser sources are committed and current
- [ ] `extension/LICENSE` exists
- [ ] The `download_file` capability path matches the release repository
- [ ] `cargo build --release --target wasm32-wasip2` succeeds
- [ ] Release assets exist with the names `lib.rs` expects
- [ ] All four gates green: grammar tests, corpus, `cargo test`, smoke test
