#!/usr/bin/env node
// Downloads Skript's published syntax database for use as test data.
//
// docs.json is generated from SkriptLang/Skript, which is GPL-3.0, so it is
// deliberately NOT committed to this MIT repository — the language server also
// fetches it at runtime rather than bundling it. This script just puts a copy
// where the test suite can find it.
//
//   node scripts/fetch-docs.mjs                 # latest
//   node scripts/fetch-docs.mjs 2.15.3          # a pinned version
//
// Without it, the tests that exercise every real pattern (2,703 in Skript
// 2.17.1) skip rather than fail, so a fresh clone still gets a green
// `cargo test`.

import { mkdirSync, writeFileSync } from 'node:fs';
import { dirname, join } from 'node:path';

import { repoRoot as root } from './lib/paths.mjs';
const version = process.argv[2];

const url =
  process.env.SKRIPT_DOCS_URL ??
  (version
    ? `https://docs.skriptlang.org/archives/${version}/docs.json`
    : 'https://docs.skriptlang.org/docs.json');

const out = join(root, 'vendor', 'docs.json');

console.log(`==> ${url}`);

const response = await fetch(url);
if (!response.ok) {
  console.error(`failed: HTTP ${response.status} ${response.statusText}`);
  if (version) {
    console.error(`is "${version}" a published Skript version? See https://docs.skriptlang.org/archives/`);
  }
  process.exit(1);
}

const text = await response.text();

// Only write something that parses — a truncated download would make the test
// suite report a grammar problem that does not exist.
let parsed;
try {
  parsed = JSON.parse(text);
} catch (error) {
  console.error(`failed: the response is not valid JSON (${error.message})`);
  process.exit(1);
}

// Every top-level array counts, types and functions included, which is how the
// figures quoted in the docs (1,235 entries / 2,703 patterns for 2.17.1) are
// measured.
const lists = Object.values(parsed).filter(Array.isArray);
const entries = lists.reduce((total, list) => total + list.length, 0);
const patterns = lists
  .flat()
  .reduce((total, entry) => total + (entry?.patterns?.length ?? 0), 0);

mkdirSync(dirname(out), { recursive: true });
writeFileSync(out, text);

console.log(
  `==> Skript ${parsed.source?.version ?? '?'}: ${entries} entries, ${patterns} patterns ` +
    `(${text.length} bytes) -> vendor/docs.json`,
);
