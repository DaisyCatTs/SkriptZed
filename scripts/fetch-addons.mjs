#!/usr/bin/env node
// Downloads SkriptHub's addon syntax catalog for use as test data.
//
// This is community-contributed data with no stated licence, so it is neither
// committed here nor bundled into the extension — the language server fetches
// it at runtime and caches it, exactly like Skript's own docs.json. This script
// just puts a copy where the test suite can find it.
//
//   node scripts/fetch-addons.mjs
//
// Without it, the tests that exercise all 12,877 real addon patterns skip
// rather than fail, so a fresh clone still gets a green `cargo test`.

import { mkdirSync, writeFileSync } from 'node:fs';
import { dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';

const root = join(dirname(fileURLToPath(import.meta.url)), '..');
const url = process.env.SKRIPTHUB_URL ?? 'https://skripthub.net/api/v1/addonsyntaxlist/';
const out = join(root, 'vendor', 'addons.json');

console.log(`==> ${url}`);
console.log('    (7.3 MB, ~1.2 MB over the wire — there is no per-addon endpoint)');

// gzip cuts this from 7.3 MB to 1.2 MB and the server honours it.
const response = await fetch(url, { headers: { 'Accept-Encoding': 'gzip' } });
if (!response.ok) {
  console.error(`failed: HTTP ${response.status} ${response.statusText}`);
  process.exit(1);
}

const text = await response.text();

let records;
try {
  records = JSON.parse(text);
} catch (error) {
  console.error(`failed: the response is not valid JSON (${error.message})`);
  process.exit(1);
}

if (!Array.isArray(records)) {
  console.error('failed: expected a JSON array of syntax records');
  process.exit(1);
}

const addons = new Map();
let patterns = 0;
for (const record of records) {
  const name = record.addon?.name ?? '(unknown)';
  addons.set(name, (addons.get(name) ?? 0) + 1);
  patterns += String(record.syntax_pattern ?? '')
    .split('\n')
    .filter(line => line.trim()).length;
}

mkdirSync(dirname(out), { recursive: true });
writeFileSync(out, text);

const top = [...addons.entries()].sort((a, b) => b[1] - a[1]).slice(0, 5);
console.log(
  `==> ${records.length} entries, ${addons.size} addons, ${patterns} patterns ` +
    `(${text.length} bytes) -> vendor/addons.json`,
);
console.log('    largest: ' + top.map(([name, count]) => `${name} (${count})`).join(', '));
