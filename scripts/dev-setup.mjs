#!/usr/bin/env node
// Prepares the repo for Zed's "install dev extension".
//
// Zed resolves `[grammars.skript]` in extension.toml by running
//   git remote add origin <repository>
//   git fetch --depth 1 origin <rev>
// so the grammar must be a git repository with a real commit, even when
// `repository` is a local file:// URL. This script makes that commit and
// rewrites `rev` to match.
//
//   node scripts/dev-setup.mjs
//
// Re-run it after every change to grammar.js or scanner.c, otherwise Zed keeps
// building the previously committed parser.
//
// Written in Node rather than shell so it behaves identically on Windows,
// macOS and Linux — Node is already required for the tree-sitter CLI.

import { execFileSync } from 'node:child_process';
import { readFileSync, writeFileSync } from 'node:fs';
import { dirname, join } from 'node:path';
import { pathToFileURL } from 'node:url';
import { fileURLToPath } from 'node:url';

const root = join(dirname(fileURLToPath(import.meta.url)), '..');
const grammarDir = join(root, 'tree-sitter-skript');
const manifest = join(root, 'extension', 'extension.toml');

// The real binary, not the `.bin` shim: the shim is a POSIX shell script on
// Unix and a `.cmd` on Windows, which Node refuses to spawn since
// CVE-2024-27980.
const treeSitter = join(
  grammarDir,
  'node_modules',
  'tree-sitter-cli',
  process.platform === 'win32' ? 'tree-sitter.exe' : 'tree-sitter',
);

function run(command, args, options = {}) {
  return execFileSync(command, args, {
    encoding: 'utf8',
    stdio: options.quiet ? 'pipe' : 'inherit',
    ...options,
  });
}

function fail(message) {
  console.error(`\nerror: ${message}`);
  process.exit(1);
}

try {
  run('git', ['rev-parse', '--git-dir'], { cwd: root, quiet: true });
} catch {
  fail(`${root} is not a git repository — run "git init" first`);
}

console.log('==> regenerating the parser');
try {
  run(treeSitter, ['generate'], { cwd: grammarDir });
} catch {
  fail('tree-sitter generate failed');
}

console.log('==> grammar tests');
try {
  run(treeSitter, ['test'], { cwd: grammarDir });
} catch {
  fail('grammar tests failed — fix them before installing the extension');
}

// The upstream corpus is a development gate, not a build dependency: a fresh
// clone has not fetched it yet, and that must not stop anyone installing the
// extension. parse-corpus.mjs exits 2 when the corpus is absent.
console.log('==> upstream corpus');
try {
  run(process.execPath, [join(root, 'scripts', 'parse-corpus.mjs'), '--strict'], { cwd: root });
} catch (error) {
  if (error.status === 2) {
    console.log('    (skipped: vendor/skript-corpus is not present)');
  } else {
    fail('the upstream corpus did not parse cleanly');
  }
}

console.log('==> committing the grammar so Zed has a rev to fetch');
run('git', ['add', '--', 'tree-sitter-skript'], { cwd: root, quiet: true });

let hasChanges = true;
try {
  run('git', ['diff', '--cached', '--quiet', '--', 'tree-sitter-skript'], {
    cwd: root,
    quiet: true,
  });
  hasChanges = false;
} catch {
  hasChanges = true;
}

if (hasChanges) {
  // No Co-Authored-By trailer, by request.
  run('git', ['commit', '-q', '-m', 'chore(grammar): dev snapshot for the Zed extension'], {
    cwd: root,
    quiet: true,
  });
  console.log('    committed');
} else {
  console.log('    (no grammar changes to commit)');
}

const rev = run('git', ['rev-parse', 'HEAD'], { cwd: root, quiet: true }).trim();
// pathToFileURL gets the Windows drive-letter form right (file:///C:/...).
const url = pathToFileURL(grammarDir).href;

console.log(`==> pointing extension.toml at ${rev.slice(0, 12)}`);
let text = readFileSync(manifest, 'utf8');
text = text.replace(/^repository = "file:\/\/.*"$/m, `repository = "${url}"`);
text = text.replace(/^rev = ".*"$/m, `rev = "${rev}"`);
writeFileSync(manifest, text);

console.log(`
Done.

In Zed:  command palette -> "zed: install dev extension" -> pick
  ${join(root, 'extension')}

Run Zed with "zed --foreground" to see the extension's stdout.`);
