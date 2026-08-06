#!/usr/bin/env node
// Parses every vendored SkriptLang `.sk` file and reports ERROR / MISSING nodes.
//
// This is the grammar's primary quality gate: the upstream Skript repository
// ships ~540 real scripts covering every structure, section and edge case the
// language has, and they are maintained by the people who define the language.
//
//   node scripts/parse-corpus.mjs            # summary + first failures
//   node scripts/parse-corpus.mjs --all      # list every failing file
//   node scripts/parse-corpus.mjs --strict   # exit 1 unless every file is clean

import { execFileSync } from 'node:child_process';
import { readdirSync, statSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join, dirname, relative } from 'node:path';
import { fileURLToPath } from 'node:url';

const root = join(dirname(fileURLToPath(import.meta.url)), '..');
const grammarDir = join(root, 'tree-sitter-skript');
const corpusDir = join(root, 'vendor', 'skript-corpus');
// Invoke the real binary that tree-sitter-cli installs, not the `.bin` shim:
// the shim is a POSIX shell script on Unix and a `.cmd` on Windows, and Node
// refuses to spawn `.cmd` files without a shell since CVE-2024-27980.
const cli = join(
  grammarDir, 'node_modules', 'tree-sitter-cli',
  process.platform === 'win32' ? 'tree-sitter.exe' : 'tree-sitter',
);

const showAll = process.argv.includes('--all');
const strict = process.argv.includes('--strict');

function walk(dir, out = []) {
  for (const entry of readdirSync(dir)) {
    if (entry === '.git') continue;
    const full = join(dir, entry);
    if (statSync(full).isDirectory()) walk(full, out);
    else if (entry.endsWith('.sk')) out.push(full);
  }
  return out;
}

let files;
try {
  files = walk(corpusDir);
} catch {
  console.error(
    `corpus not found at ${corpusDir}\n\n` +
      'Fetch it with:\n' +
      '  git clone --depth 1 --filter=blob:none --sparse \\\n' +
      '    https://github.com/SkriptLang/Skript.git vendor/skript-corpus\n' +
      '  cd vendor/skript-corpus\n' +
      '  git sparse-checkout set src/test/skript src/main/resources/scripts',
  );
  process.exit(2);
}

// The file list is passed via `--paths`: a command line with ~540 absolute
// paths silently overflows on Windows, which made an earlier version of this
// script report a clean run without ever invoking the parser.
const listFile = join(tmpdir(), 'tree-sitter-skript-corpus.txt');
writeFileSync(listFile, files.join('\n'), 'utf8');

let output = '';
try {
  output = execFileSync(cli, ['parse', '--quiet', '--stat', '--paths', listFile], {
    cwd: grammarDir,
    encoding: 'utf8',
    maxBuffer: 1 << 28,
  });
} catch (err) {
  output = `${err.stdout ?? ''}${err.stderr ?? ''}`;
}

const failures = output
  .split('\n')
  .filter(line => /\((ERROR|MISSING)\b/.test(line))
  .map(line => line.trim());

const summary = output.match(/Total parses: (\d+); successful parses: (\d+); failed parses: (\d+)/);
if (!summary) {
  console.error('could not read tree-sitter summary — harness is broken, not the grammar');
  console.error(output.slice(0, 2000));
  process.exit(2);
}

const total = Number(summary[1]);
const failed = Number(summary[3]);

if (total !== files.length) {
  console.error(`harness error: found ${files.length} files but parsed ${total}`);
  process.exit(2);
}

console.log(`parsed ${total} files — ${total - failed} clean, ${failed} with errors`);

if (failures.length) {
  const shown = showAll ? failures : failures.slice(0, 25);
  console.log('');
  for (const line of shown) console.log('  ' + relative(root, line).slice(0, 200));
  if (!showAll && failures.length > shown.length) {
    console.log(`  … and ${failures.length - shown.length} more (pass --all)`);
  }
}

process.exit(strict && failed > 0 ? 1 : 0);
