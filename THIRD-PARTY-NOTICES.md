# Third-party notices

Skript for Zed is MIT licensed. This file records the third-party code and data
it depends on, and the obligations that come with them.

No GPL, AGPL, SSPL or non-commercial-licensed code is linked into any shipped
artifact. The copyleft pieces this project touches, Skript's syntax database
and the optional SkAnalyzer tooling, are fetched or installed by the user at
runtime and never bundled; see below.

## Data that is fetched, never bundled

Three syntax sources are downloaded at runtime and cached; none is vendored
into this repository or embedded in any binary.

| Source | Licence | Why it is not bundled |
|---|---|---|
| [Skript's `docs.json`](https://docs.skriptlang.org) | GPL-3.0 | Bundling a GPL-3.0 database in an MIT project would impose GPL terms on the whole work. It is fetched on demand and cached in the OS cache directory. |
| [SkriptHub's addon syntax list](https://skripthub.net) | No stated licence | Third-party data with no grant to redistribute, so it is not redistributed. |
| [skript-minestom's docs site](https://smdocs.hapily.me) | Unverified | Fetched only with `"platform": "minestom"`. No licence for the data could be confirmed, so it is treated like SkriptHub's: used at runtime, never redistributed. |

`vendor/` is gitignored precisely so none of them can be committed by accident,
and CI fetches them at job time rather than storing them.

The built-in fallback catalog (`crates/skript-docs/src/fallback.json`) is the
one piece of syntax data compiled into the binary. Its descriptions and examples
were written for this project. Its `patterns` strings are Skript's own syntax
(`[local] function <.+>`, `(wait|halt) [for] %timespan%` and similar), which are
the language's grammar rather than creative expression, and there is no way to
describe Skript's syntax without writing Skript's syntax.

## Tools the user installs, never bundled

The opt-in "check against real Skript" feature drives two programs the user
downloads and points the language server at:

| Program | Licence | How it is used |
|---|---|---|
| [SkAnalyzer](https://github.com/SkEditorTeam/SkAnalyzer) | LGPL-3.0 | Started as a separate Java process; the language server writes commands to its stdin and reads its log output. Nothing is linked. |
| [Skratches](https://github.com/SkEditorTeam/Skratches) (patched Skript builds) | GPL-3.0 | Loaded by SkAnalyzer inside that separate process. The language server never loads or distributes it. |

Neither is shipped with the extension or the language server, and the feature
is off unless the user enables it.

## Notable dependency licences

Every crate in the dependency graphs of `language-server/` and `extension/` is
under a permissive licence: MIT, Apache-2.0, ISC, BSD-3-Clause, Zlib, 0BSD,
Unlicense, Unicode-3.0 or CDLA-Permissive-2.0, most of them dual-licensed
`MIT OR Apache-2.0`. Some are worth calling out because a licence scanner will
stop on them, or because a blanket "this project is MIT" claim does not cover
them:

**`zed_extension_api` — Apache-2.0 only.** Used by the extension. Apache-2.0
is MIT-compatible but carries a NOTICE requirement and an explicit patent
grant; distributing the extension therefore carries Apache-2.0 obligations for
that component. This file is that notice. (`skript-lsp` no longer has an
Apache-only dependency: `zopfli` left with the move to `zip` 9, whose
read-only feature set does not pull it in.)

**`ring` — Apache-2.0 AND ISC.** Reached through `ureq` → `rustls` for HTTPS.
Both licences apply together, and parts of it derive from BoringSSL; see its
`LICENSE` for the full terms.

**`webpki-roots` — CDLA-Permissive-2.0.** Mozilla's CA certificate set, used to
validate the HTTPS connections above. Permissive with no reciprocity, but it is
a *data* licence.

**Unicode-3.0.** The ICU4X crates (`icu_*`, `zerovec`, `tinystr` and others),
reached through `url` → `idna`, and `unicode-ident`. Permissive, with a notice
requirement.

**Zlib.** `foldhash`, reached through `serde_json` and `hashbrown`, and
`zlib-rs`, the deflate backend `zip` uses to read JARs (and, through the shared
`flate2`, `ureq`'s gzip decoding). `zlib-rs` is also in the extension's
lockfile but not built there. Permissive, with a notice requirement for
altered source versions.

**0BSD.** `adler2` offers `0BSD OR MIT OR Apache-2.0`.

## Regenerating this list

```sh
cd language-server
cargo metadata --format-version 1 --locked
```

Each package's `license` field is in that output; repeat in `extension/`. For a
full machine-readable inventory, `cargo about generate` or
`cargo deny check licenses` will need an allow-list covering the licences
above.
