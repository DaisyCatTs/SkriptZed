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
  `skripthub.net` (and `smdocs.hapily.me` with `"platform": "minestom"`), and
  caches it in your platform cache directory
  (`%LOCALAPPDATA%`, `~/Library/Caches`, or `$XDG_CACHE_HOME`). No request
  carries credentials or sends anything about your code, your project or your
  machine. They can be turned off: set `addonSyntaxSource` to `"off"`
  and point `docsPath` at a local file.

* **It reads your server directory.** Addon detection opens `.jar` files under
  `plugins/` and parses the manifest inside each one. It reads; it never writes
  there, and detection never executes anything it finds (but see SkAnalyzer
  below). Set `addons` to `"off"` to
  disable it entirely.

* **It executes nothing from your scripts.** Skript files are parsed, never run.
  The language server has no evaluator.

* **If you enable SkAnalyzer, it starts a Java process you configured.** With
  `analyzer.jar` set (and `analyzer.enabled` not `false`), the server runs the `java` executable and SkAnalyzer jar
  named in your settings, which load Skript and the addon JARs listed in
  `analyzer.plugins` (by default, the ones detected in `plugins/`). Those addon
  JARs are Java code, so this does run them, outside a Minecraft server. It is
  off by default; only enable it with JARs you trust.

* **If you enable RCON reload, it logs in to your server and runs a command.**
  With `rcon.enabled`, each saved script is reloaded with `skript reload` over
  RCON, which changes what that server runs. RCON sends the password in plain
  text and Zed stores it in plain text in your settings, so keep the RCON port
  local (or on a private network) and never expose it to the internet. The
  password is never written to logs or shown in messages. Off by default.

* **The optional AI helper (`skript-lsp mcp`) is read-only.** The extension does
  not register it; it only runs if you add it to Zed's `context_servers`
  yourself. It answers questions about syntax and your project's scripts, never
  writes a file, and never contacts a game server. Its only network use is the
  same syntax download the language server already does.

## Reporting a vulnerability

Report privately through
[GitHub Security Advisories](https://github.com/DaisyCatTs/SkriptZed/security/advisories/new).

Please do not open a public issue for a vulnerability. Expect a first response
within 72 hours.
