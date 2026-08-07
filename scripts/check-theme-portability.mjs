#!/usr/bin/env node
// Every capture that paints must resolve in an arbitrary theme.
//
// Zed resolves a dotted capture by walking back along its dot-prefixes:
// `string.special.symbol` -> `string.special` -> `string`. So a refinement is
// safe as long as its *root* is one themes actually define. A bare invented
// name has nowhere to fall back to and renders unstyled — no error, no warning,
// it just silently comes out as plain text in every theme that has not heard of
// it. That is the single easiest way to break this extension for someone whose
// theme is not the one it was written against.
//
//   node scripts/check-theme-portability.mjs
//
// Exits non-zero if any colour-bearing name lacks a root.

import { existsSync, readFileSync } from 'node:fs';
import { dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';

const root = join(dirname(fileURLToPath(import.meta.url)), '..');
const languageDir = join(root, 'extension', 'languages', 'skript');

// The roots Zed's bundled One theme and Catppuccin both define. Anything
// outside this set has no guaranteed colour anywhere.
const ROOTS = new Set([
  'attribute', 'boolean', 'comment', 'constant', 'constructor', 'embedded',
  'emphasis', 'enum', 'function', 'hint', 'keyword', 'label', 'link_text',
  'link_uri', 'number', 'operator', 'predictive', 'preproc', 'primary',
  'property', 'punctuation', 'string', 'tag', 'text', 'title', 'type',
  'variable',
]);

const found = new Map();
const note = (name, where) => {
  if (!found.has(name)) found.set(name, where);
};

// highlights.scm is the only query file whose captures are style names. The
// others — outline, brackets, indents, textobjects — name *features* Zed
// consumes (`@item`, `@open`, `@start.if`), and are deliberately not checked.
const highlights = join(languageDir, 'highlights.scm');
if (existsSync(highlights)) {
  const code = readFileSync(highlights, 'utf8')
    .split('\n')
    .filter(line => !line.trim().startsWith(';')) // a capture cited in prose is not a capture
    .join('\n');
  for (const match of code.matchAll(/@([a-z][a-z0-9_.]*)/g)) {
    note(match[1], 'highlights.scm');
  }
}

const rulesPath = join(languageDir, 'semantic_token_rules.json');
if (existsSync(rulesPath)) {
  const text = readFileSync(rulesPath, 'utf8')
    .split('\n')
    .filter(line => !line.trim().startsWith('//'))
    .join('\n');
  for (const rule of JSON.parse(text)) {
    for (const style of rule.style ?? []) note(style, 'semantic_token_rules.json');
  }
}

const unstyled = [...found]
  .filter(([name]) => !name.startsWith('_')) // `_helper` captures never paint
  .filter(([name]) => !ROOTS.has(name.split('.')[0]));

console.log(`checked ${found.size} colour-bearing name(s)`);

if (unstyled.length) {
  console.error('\nno theme fallback root — these render unstyled:');
  for (const [name, where] of unstyled) console.error(`  ${name}  (${where})`);
  console.error('\nPick a name whose first segment is one of:');
  console.error('  ' + [...ROOTS].join(' '));
  process.exit(1);
}

const refinements = [...found.keys()].filter(name => name.includes('.')).sort();
console.log(`all resolve; ${refinements.length} refinement(s) degrade gracefully`);
