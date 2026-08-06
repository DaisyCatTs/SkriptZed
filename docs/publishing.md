# Publishing

## 1. The grammar stays in this repository

An earlier version of this document said the grammar had to be split into its
own repository before release. **That is wrong**, and following it would break a
working configuration.

Zed clones a grammar with `git remote add origin <repository>` +
`git fetch --depth 1 origin <rev>`, then runs clang on the checked-out sources.
It does not require the grammar to sit at the repository root: the `path` key
names a subdirectory, and that is what ships.

```toml
[grammars.skript]
repository = "https://github.com/DaisyCatTs/SkriptZed"
rev = "<a commit that exists on GitHub>"
path = "tree-sitter-skript"
```

This is supported by Zed's own `extension_builder.rs`, and Civet, cfml and
`zed-extensions/php` all publish grammars from a subdirectory this way. There is
no separate `tree-sitter-skript` repository and there does not need to be.

Two things this *does* still demand:

* **`rev` must name a commit reachable on GitHub**, and preferably one on
  `main`. A commit that exists only on a feature branch resolves until that
  branch is deleted, after which every install fails at grammar-fetch time with
  no highlighting and nothing in the error to explain why. `scripts/dev-setup.mjs`
  rewrites `repository` and `rev` to a local `file://` URL and a local commit —
  never release that rewrite.
* **The generated sources must be committed** — `src/parser.c`,
  `src/grammar.json`, `src/node-types.json`, `src/tree_sitter/*.h`. Zed never
  invokes `tree-sitter generate`; a repository that gitignores `src/` cannot be
  used as a Zed grammar at all. CI enforces this.

Verify before every release by replaying exactly what Zed does, from an empty
directory:

```sh
git init tmp && cd tmp
git remote add origin https://github.com/DaisyCatTs/SkriptZed
git fetch --depth 1 origin <rev> && git checkout FETCH_HEAD
ls tree-sitter-skript/src/parser.c
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

- [ ] `[grammars.skript]` uses the public HTTPS URL with `path = "tree-sitter-skript"`,
      not the `file://` one that `dev-setup.mjs` writes
- [ ] `rev` names a commit **on `main`** — verified by replaying the fetch from
      an empty directory, not by assuming
- [ ] Generated parser sources are committed and current
- [ ] The extension has been installed and used as a dev extension on a clean
      machine. The registry closes untested submissions without feedback.
- [ ] `extension/LICENSE` exists
- [ ] The `download_file` capability path matches the release repository
- [ ] `cargo build --release --target wasm32-wasip2` succeeds
- [ ] Release assets exist with the names `lib.rs` expects
- [ ] All four gates green: grammar tests, corpus, `cargo test`, smoke test
