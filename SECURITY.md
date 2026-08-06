# Security policy

## Supported versions

The latest released version only. Zed keeps extensions up to date automatically.

## What this extension does that is worth scrutinising

This is a language extension, so it downloads and runs code. That is worth being
explicit about rather than burying.

* **It downloads and runs a binary.** On first use, `extension/src/lib.rs`
  fetches a `skript-lsp` release asset from
  [this repository's releases](https://github.com/DaisyCatTs/SkriptZed/releases)
  and runs it. The download is declared in `extension.toml`'s `[[capabilities]]`
  block, scoped to this repository, and Zed gates it behind the capabilities you
  grant. The binary is never bundled with the extension — the Zed registry
  forbids that, and it means you can audit what you are running.

  A `skript-lsp` already on your `PATH`, or one named by
  `lsp.skript-lsp.binary.path`, is **always preferred** over downloading. If you
  would rather build it yourself, that is the supported path:

  ```sh
  cd language-server && cargo build --release -p skript-lsp
  ```

* **It fetches JSON over HTTPS at runtime** from `docs.skriptlang.org` and
  `skripthub.net`, and caches it in your platform cache directory
  (`%LOCALAPPDATA%`, `~/Library/Caches`, or `$XDG_CACHE_HOME`). Neither request
  carries credentials, and neither sends anything about your code, your project
  or your machine. Both can be turned off — set `addonSyntaxSource` to `"off"`
  and point `docsPath` at a local file.

* **It reads your server directory.** Addon detection opens `.jar` files under
  `plugins/` and parses the manifest inside each one. It reads; it never writes
  there, and it never executes anything it finds. Set `addons` to `"off"` to
  disable it entirely.

* **It executes nothing from your scripts.** Skript files are parsed, never run.
  The language server has no evaluator.

## Reporting a vulnerability

Report privately through
[GitHub Security Advisories](https://github.com/DaisyCatTs/SkriptZed/security/advisories/new).

Please do not open a public issue for a vulnerability. Expect a first response
within 72 hours.
