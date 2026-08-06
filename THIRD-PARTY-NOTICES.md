# Third-party notices

Skript for Zed is MIT licensed. This file records the third-party code and data
it depends on, and the obligations that come with them.

Nothing here is copyleft. No GPL, AGPL, SSPL or non-commercial-licensed code is
linked into any shipped artifact.

## Data that is fetched, never bundled

Two syntax databases are downloaded at runtime and cached; neither is vendored
into this repository or embedded in any binary.

| Source | Licence | Why it is not bundled |
|---|---|---|
| [Skript's `docs.json`](https://docs.skriptlang.org) | GPL-3.0 | Bundling a GPL-3.0 database in an MIT project would impose GPL terms on the whole work. It is fetched on demand and cached in the OS cache directory. |
| [SkriptHub's addon syntax list](https://skripthub.net) | No stated licence | Third-party data with no grant to redistribute, so it is not redistributed. |

`vendor/` is gitignored precisely so neither can be committed by accident, and
CI fetches them at job time rather than storing them.

The built-in fallback catalog (`crates/skript-docs/src/fallback.json`) is the
one piece of syntax data compiled into the binary. Its descriptions and examples
were written for this project. Its `patterns` strings are Skript's own syntax —
`[local] function <.+>`, `(wait|halt) [for] %timespan%` and similar — which are
the language's grammar rather than creative expression, and there is no way to
describe Skript's syntax without writing Skript's syntax.

## Notable dependency licences

Everything in the dependency graph is MIT, Apache-2.0, ISC or BSD. Three are
worth calling out because a licence scanner will stop on them:

**`zopfli` — Apache-2.0 only.** Reached through `zip`, used to read plugin
manifests out of JARs. Apache-2.0 is MIT-compatible but carries a NOTICE
requirement and an explicit patent grant; distributing `skript-lsp` therefore
carries Apache-2.0 obligations for that component. This file is that notice.

**`ring` — ISC/MIT/OpenSSL-derived, declared via `license-file`.** Reached
through `ureq` → `rustls` for HTTPS. Because it declares a licence *file* rather
than an SPDX expression, tools like `cargo deny` and `cargo about` report it as
unlicensed. It is not; see its `LICENSE` for the BoringSSL-derived terms.

**`webpki-roots` — CDLA-Permissive-2.0.** Mozilla's CA certificate set, used to
validate the HTTPS connections above. Permissive with no reciprocity, but it is
a *data* licence and is not covered by a blanket "this project is MIT" claim.

## Regenerating this list

```sh
cd language-server
cargo tree --format '{p} {l}'
```

For a full machine-readable inventory, `cargo about generate` or
`cargo deny check licenses` will need an allow-list entry for `ring`.
