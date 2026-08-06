# Installation

## From the Zed extension registry

Once published: command palette → **`zed: extensions`** → search *Skript* →
Install. The language server downloads itself on first use.

## From source

```sh
git clone https://github.com/DaisyCatTs/SkriptZed.git
cd SkriptZed

cd tree-sitter-skript && npm install && cd ..
rustup target add wasm32-wasip2

node scripts/dev-setup.mjs
```

Then: command palette → **`zed: install dev extension`** → pick `extension/`.

A dev extension overrides the published one; the Extensions page shows
"Overridden by dev extension" while it is installed.

## The language server

The extension looks for `skript-lsp` in this order:

1. `lsp.skript-lsp.binary.path` in your settings
2. `skript-lsp` on your `PATH`
3. A download from this repository's GitHub releases

To build your own:

```sh
cd language-server && cargo build --release -p skript-lsp
```

```json
{
  "lsp": {
    "skript-lsp": {
      "binary": { "path": "/absolute/path/to/skript-lsp" }
    }
  }
}
```

Without a server you still get highlighting, indentation, indent-based folding
and snippets. You lose completion, hover, diagnostics, go-to-definition,
references, rename and semantic tokens.

## Recommended settings

```json
{
  "languages": {
    "Skript": {
      "semantic_tokens": "combined",
      "tab_size": 4,
      "hard_tabs": true
    }
  }
}
```

`semantic_tokens` defaults to `"off"` in Zed, and it is what colours effects,
conditions and expressions — see [theming.md](theming.md).

If your server runs a fork or a nightly build of Skript, generate its exact
syntax database with `/sk gen-docs` and point the server at it. (This covers
Skript itself only — addon syntax is detected separately, see
[addons.md](addons.md).)

```json
{
  "lsp": {
    "skript-lsp": {
      "initialization_options": {
        "docsPath": "/path/to/plugins/Skript/docs/docs.json"
      }
    }
  }
}
```

## Addons

Detected automatically from your server's `plugins/` directory. If your scripts
live somewhere else, point at the server:

```json
{
  "lsp": {
    "skript-lsp": {
      "initialization_options": { "serverPath": "/srv/minecraft/my-server" }
    }
  }
}
```

See [addons.md](addons.md).

## Troubleshooting

**No highlighting** — check the file is detected as Skript in the status bar.
Only `.sk` is claimed.

**No completion or hover** — the syntax database is still loading, or could not
be fetched. `dev: open language server logs` says which.

**Everything is one colour** — the theme may not define the capture roots used.
See [theming.md](theming.md).

**Indentation is wrong after a line ending in a colon** — if that line is not
meant to open a section, Skript's own marker suppresses it:

```sk
send "ratio 3:" to player #-#
```
