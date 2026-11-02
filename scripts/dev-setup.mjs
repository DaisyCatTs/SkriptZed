#!/usr/bin/env node
// Prepares the repo for Zed's "install dev extension".
//
// Zed resolves `[grammars.skript]` in extension.toml by running
//   git remote add origin <repository>
//   git fetch --depth 1 origin <rev>
// so the grammar must be a git repository with a real commit, even when
// `repository` is a local file:// URL. This script regenerates the parser and
// runs both grammar gates, commits `tree-sitter-skript/` on the current branch,
// and points `repository`/`rev` in extension.toml at that commit. When Zed's
// own wasi-sdk is installed, it also compiles the grammar into
// `extension/grammars/skript.wasm`, so an installed dev extension picks up the
// change on the next restart instead of needing Rebuild.
//
//   node scripts/dev-setup.mjs
//
// Re-run it after every change to grammar.js or scanner.c, otherwise Zed keeps
// building the previously committed parser.
//
// Written in Node rather than shell so it behaves identically on Windows,
// macOS and Linux — Node is already required for the tree-sitter CLI.

import { execFileSync } from 'node:child_process';
import { existsSync, mkdirSync, readFileSync, renameSync, rmSync, writeFileSync } from 'node:fs';
import { homedir } from 'node:os';
import { join } from 'node:path';
import { pathToFileURL } from 'node:url';

import { grammarDir, repoRoot as root, treeSitterBinary as treeSitter } from './lib/paths.mjs';

const manifest = join(root, 'extension', 'extension.toml');

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
  run('git', ['commit', '-q', '-m', 'chore(grammar): dev snapshot for the Zed extension'], {
    cwd: root,
    quiet: true,
  });
  console.log('    committed');
} else {
  console.log('    (no grammar changes to commit)');
}

const rev = run('git', ['rev-parse', 'HEAD'], { cwd: root, quiet: true }).trim();

// The *repository root*, not the grammar directory. The grammar lives inside
// this monorepo and is not a git repository of its own, so pointing Zed at
// `…/tree-sitter-skript` fails with "does not appear to be a git repository".
// `path = "tree-sitter-skript"` in extension.toml selects the subdirectory.
//
// pathToFileURL gets the Windows drive-letter form right (file:///C:/...).
const url = pathToFileURL(root).href;

// A stale checkout of a *different* URL makes Zed refuse with "grammar
// directory already exists, but is not a git clone of ...". Only that case is
// cleared: a checkout of this repository is reused, and deleting it — or the
// compiled grammar beside it — left an installed dev extension unable to load
// the language until it was rebuilt by hand.
const grammarsDir = join(root, 'extension', 'grammars');
const checkout = join(grammarsDir, 'skript');
if (existsSync(checkout)) {
  let origin = '';
  try {
    origin = run('git', ['remote', 'get-url', 'origin'], { cwd: checkout, quiet: true }).trim();
  } catch {
    // Not a git checkout at all — just as stale.
  }
  if (origin !== url) {
    rmSync(checkout, { recursive: true, force: true });
    console.log('==> cleared a grammar checkout of a different repository');
  }
}

console.log(`==> pointing extension.toml at ${rev.slice(0, 12)}`);
let text = readFileSync(manifest, 'utf8');
// The grammar's `repository`, i.e. the one directly above `rev` — not the
// extension's own at the top of the file. Matching only an existing `file://`
// value meant a clean checkout kept the GitHub URL, and Zed then tried to fetch
// a commit that exists only on this machine.
text = text.replace(/^repository = ".*"(\r?\nrev = )/m, `repository = "${url}"$1`);
text = text.replace(/^rev = ".*"$/m, `rev = "${rev}"`);
if (!/^path = /m.test(text)) {
  text = text.replace(/^rev = ".*"$/m, match => `${match}
path = "tree-sitter-skript"`);
}
writeFileSync(manifest, text);

// An installed dev extension runs on `extension/grammars/skript.wasm`, and Zed
// does not rebuild a dev extension by itself, so a new `rev` alone changes
// nothing until someone clicks Rebuild (and a missing wasm fails the next
// start with "failed to load language Skript"). When Zed's own wasi-sdk is
// present, do what its extension builder does — check out the grammar at
// `rev` and compile it with the same clang and flags — so the installed
// extension picks up the new grammar on restart.
const zedData =
  process.platform === 'win32'
    ? join(process.env.LOCALAPPDATA ?? join(homedir(), 'AppData', 'Local'), 'Zed')
    : process.platform === 'darwin'
      ? join(homedir(), 'Library', 'Application Support', 'Zed')
      : join(process.env.XDG_DATA_HOME ?? join(homedir(), '.local', 'share'), 'zed');
const clang = join(
  zedData,
  'extensions',
  'build',
  'wasi-sdk',
  'bin',
  process.platform === 'win32' ? 'clang.exe' : 'clang',
);

let rebuilt = false;
if (existsSync(clang)) {
  console.log('==> compiling the grammar with Zed\'s wasi-sdk');
  if (!existsSync(checkout)) {
    mkdirSync(grammarsDir, { recursive: true });
    run('git', ['init', '-q', checkout]);
    run('git', ['remote', 'add', 'origin', url], { cwd: checkout });
  }
  run('git', ['fetch', '-q', '--depth', '1', 'origin', rev], { cwd: checkout });
  run('git', ['checkout', '-q', '--force', 'FETCH_HEAD'], { cwd: checkout });
  const src = join(checkout, 'tree-sitter-skript', 'src');
  const wasm = join(grammarsDir, 'skript.wasm');
  const fresh = `${wasm}.new`;
  try {
    run(clang, [
      '-fPIC',
      '-shared',
      '-Os',
      '-Wl,--export=tree_sitter_skript',
      '-o',
      fresh,
      '-I',
      src,
      join(src, 'parser.c'),
      join(src, 'scanner.c'),
    ]);
    // Swapped in only once it exists, so a failed compile never leaves the
    // installed extension without a grammar.
    try {
      renameSync(fresh, wasm);
      rebuilt = true;
    } catch {
      console.log('    Zed has the old grammar open — close Zed and run this again');
    }
  } catch {
    console.log('    compile failed — use Rebuild in Zed instead (below)');
  }
}

console.log(`
Done.

First time:  command palette -> "zed: install dev extension" -> pick
  ${join(root, 'extension')}
${
  rebuilt
    ? 'Already installed: the grammar is rebuilt, so just restart Zed.'
    : 'Already installed: open "zed: extensions", find Skript and click Rebuild,\nor Zed fails to load the language on its next start.'
}

Run Zed with "zed --foreground" to see the extension's stdout.`);
