#!/usr/bin/env node
// Fails unless extension.toml's grammar can be installed by a stranger.
//
// `scripts/dev-setup.mjs` points `[grammars.skript]` at a local `file://`
// checkout and a commit that exists on one machine only. Shipped, that breaks
// highlighting for every user with nothing in the error to say why. This checks
// the URL, then fetches the pinned commit exactly the way Zed does.
//
//   node scripts/check-grammar-source.mjs

import { execFileSync } from 'node:child_process';
import { existsSync, mkdtempSync, readFileSync, rmSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';

const root = join(dirname(fileURLToPath(import.meta.url)), '..');
const manifest = readFileSync(join(root, 'extension', 'extension.toml'), 'utf8');

const section = manifest.split(/^\[grammars\.skript\]\s*$/m)[1]?.split(/^\[/m)[0] ?? '';
const value = key => new RegExp(`^${key} = "(.*)"`, 'm').exec(section)?.[1];
const repository = value('repository');
const rev = value('rev');
const path = value('path') ?? '.';

function fail(message) {
  console.error(`error: ${message}`);
  process.exit(1);
}

if (!repository || !rev) fail('extension.toml has no [grammars.skript] repository/rev');
if (!repository.startsWith('https://github.com/')) {
  fail(`grammar repository is "${repository}" — restore the GitHub URL (dev-setup.mjs rewrites it)`);
}

const scratch = mkdtempSync(join(tmpdir(), 'grammar-source-'));
try {
  const git = (...args) => execFileSync('git', args, { cwd: scratch, stdio: 'pipe' });
  git('init', '-q');
  git('remote', 'add', 'origin', repository);
  try {
    git('fetch', '-q', '--depth', '1', 'origin', rev);
  } catch {
    fail(`grammar rev ${rev} cannot be fetched from ${repository} — push it to main first`);
  }
  git('checkout', '-q', 'FETCH_HEAD');
  if (!existsSync(join(scratch, path, 'src', 'parser.c'))) {
    fail(`${path}/src/parser.c is missing at ${rev} — the generated parser must be committed`);
  }
  console.log(`grammar source ok: ${repository} @ ${rev.slice(0, 12)}`);
} finally {
  rmSync(scratch, { recursive: true, force: true });
}
