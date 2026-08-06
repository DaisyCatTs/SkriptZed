#!/usr/bin/env node
// Measures how much of a real script the extension actually explains.
//
// Two numbers, because they come from two different layers and fail
// independently:
//
//   tree-sitter  — how many tokens `highlights.scm` colours
//   semantic     — how many executable lines the language server classifies
//
// The second is the one that matters for "does this feel intelligent". A low
// score means lines are falling through to no hover, no classification and no
// colour beyond the structural minimum.
//
//   node scripts/coverage.mjs [file.sk ...]

import { spawn, execFileSync } from 'node:child_process';
import { existsSync, readFileSync } from 'node:fs';
import { dirname, join, resolve } from 'node:path';
import { fileURLToPath, pathToFileURL } from 'node:url';

const root = join(dirname(fileURLToPath(import.meta.url)), '..');
// Resolved up front: the tree-sitter query runs from the grammar directory, so a
// path relative to the caller's cwd would not survive the trip.
const files = process.argv.slice(2).length
  ? process.argv.slice(2).map(file => resolve(file))
  : [join(root, 'examples', 'sample-project', 'showcase.sk')];

// ---------------------------------------------------------- tree-sitter side

const treeSitter = join(
  root, 'tree-sitter-skript', 'node_modules', 'tree-sitter-cli',
  process.platform === 'win32' ? 'tree-sitter.exe' : 'tree-sitter',
);

function highlightCoverage(file) {
  const out = execFileSync(
    treeSitter,
    ['query', join(root, 'extension/languages/skript/highlights.scm'), file],
    { cwd: join(root, 'tree-sitter-skript'), encoding: 'utf8', maxBuffer: 1 << 26 },
  );
  const source = readFileSync(file, 'utf8');
  const lines = source.split('\n');
  const captured = new Set();
  for (const match of out.matchAll(/start: \((\d+), (\d+)\), end: \((\d+), (\d+)\)/g)) {
    const [, sl, sc, el, ec] = match.map(Number);
    // A capture may span lines — `block_comment` covers a whole `###` block, and
    // so does a multi-line string. Filling only the first line made every one of
    // them look uncoloured and understated the grammar's real reach.
    for (let l = sl; l <= el; l++) {
      const from = l === sl ? sc : 0;
      const to = l === el ? ec : (lines[l]?.length ?? 0);
      for (let c = from; c < to; c++) captured.add(`${l}:${c}`);
    }
  }
  let visible = 0;
  source.split('\n').forEach((line, l) => {
    for (let c = 0; c < line.length; c++) if (!/\s/.test(line[c])) visible++;
  });
  let hit = 0;
  source.split('\n').forEach((line, l) => {
    for (let c = 0; c < line.length; c++) {
      if (!/\s/.test(line[c]) && captured.has(`${l}:${c}`)) hit++;
    }
  });
  return { hit, visible, captured };
}

// How many visible characters `cells` covers, ignoring whitespace.
function coveredVisible(source, cells) {
  let hit = 0;
  source.split('\n').forEach((line, l) => {
    for (let c = 0; c < line.length; c++) {
      if (!/\s/.test(line[c]) && cells.has(`${l}:${c}`)) hit++;
    }
  });
  return hit;
}

// ------------------------------------------------------------- semantic side

const exe = process.platform === 'win32' ? 'skript-lsp.exe' : 'skript-lsp';
const binary = [
  join(root, 'language-server', 'target', 'release', exe),
  join(root, 'language-server', 'target', 'debug', exe),
].find(existsSync);

async function semanticCoverage(file) {
  if (!binary) return null;

  const server = spawn(binary, [], { stdio: ['pipe', 'pipe', 'pipe'] });
  server.stdin.on('error', () => {});
  let logs = [];
  server.stderr.on('data', () => {});

  let nextId = 1;
  const pending = new Map();
  const send = (method, params, isRequest = true) => {
    const message = isRequest
      ? { jsonrpc: '2.0', id: nextId++, method, params }
      : { jsonrpc: '2.0', method, params };
    const body = Buffer.from(JSON.stringify(message), 'utf8');
    server.stdin.write(`Content-Length: ${body.length}\r\n\r\n`);
    server.stdin.write(body);
    return isRequest ? new Promise(r => pending.set(message.id, r)) : Promise.resolve();
  };

  let buffer = Buffer.alloc(0);
  server.stdout.on('data', chunk => {
    buffer = Buffer.concat([buffer, chunk]);
    for (;;) {
      const headerEnd = buffer.indexOf('\r\n\r\n');
      if (headerEnd < 0) return;
      const m = /content-length:\s*(\d+)/i.exec(buffer.subarray(0, headerEnd).toString('ascii'));
      if (!m) return;
      const start = headerEnd + 4;
      const len = Number(m[1]);
      if (buffer.length < start + len) return;
      const msg = JSON.parse(buffer.subarray(start, start + len).toString('utf8'));
      buffer = buffer.subarray(start + len);
      if (msg.id !== undefined && pending.has(msg.id)) {
        pending.get(msg.id)(msg);
        pending.delete(msg.id);
      } else if (msg.method === 'window/logMessage') {
        logs.push(msg.params?.message ?? '');
      }
    }
  });

  const uri = pathToFileURL(file).href;
  const text = readFileSync(file, 'utf8');

  await send('initialize', {
    processId: process.pid,
    capabilities: { general: { positionEncodings: ['utf-8'] } },
    initializationOptions: {},
  });
  await send('initialized', {}, false);
  await send('textDocument/didOpen',
    { textDocument: { uri, languageId: 'skript', version: 1, text } }, false);

  // The catalog loads on a background task.
  await new Promise(r => setTimeout(r, 4000));

  // Timed on the second request: the first also pays for whatever the server
  // lazily built, and what a user feels is the steady-state cost of an edit.
  await send('textDocument/semanticTokens/full', { textDocument: { uri } });
  const started = process.hrtime.bigint();
  const tokens = await send('textDocument/semanticTokens/full', { textDocument: { uri } });
  const elapsedMs = Number(process.hrtime.bigint() - started) / 1e6;
  const data = tokens.result?.data ?? [];

  // Which lines got at least one classified token, and which characters.
  // Tokens are delta-encoded: line delta, column delta (reset each new line),
  // length, type, modifiers.
  const classified = new Set();
  const cells = new Set();
  let line = 0;
  let column = 0;
  for (let i = 0; i < data.length; i += 5) {
    if (data[i] !== 0) column = 0;
    line += data[i];
    column += data[i + 1];
    classified.add(line);
    for (let c = column; c < column + data[i + 2]; c++) cells.add(`${line}:${c}`);
  }

  // Lines that are actually executable Skript. Prose inside a `###` block is
  // not code, so counting it against the server would make the score
  // meaningless — it can never be classified and should never be.
  let executable = 0;
  let inBlockComment = false;
  const unclassified = [];
  text.split('\n').forEach((raw, index) => {
    const trimmed = raw.trim();
    if (trimmed === '###') {
      inBlockComment = !inBlockComment;
      return;
    }
    if (inBlockComment || !trimmed || trimmed.startsWith('#')) return;
    executable++;
    if (!classified.has(index)) unclassified.push(`${index + 1}: ${trimmed.slice(0, 66)}`);
  });

  await send('shutdown', {});
  await send('exit', {}, false);
  server.kill();

  return { classified: executable - unclassified.length, executable, unclassified, logs, elapsedMs, cells };
}

// -------------------------------------------------------------------- report

for (const file of files) {
  console.log(`\n${file.replace(root, '.')}`);

  const source = readFileSync(file, 'utf8');
  const hl = highlightCoverage(file);
  console.log(
    `  tree-sitter : ${hl.hit}/${hl.visible} visible characters coloured ` +
      `(${((100 * hl.hit) / hl.visible).toFixed(0)}%)`,
  );

  const sem = await semanticCoverage(file);
  if (!sem) {
    console.log('  semantic    : skipped (build skript-lsp first)');
    continue;
  }
  console.log(
    `  semantic    : ${sem.classified}/${sem.executable} executable lines classified ` +
      `(${((100 * sem.classified) / sem.executable).toFixed(0)}%)`,
  );
  console.log(
    `                ${sem.elapsedMs.toFixed(0)} ms to classify the whole file ` +
      `(${((1000 * sem.elapsedMs) / sem.executable).toFixed(0)} µs/line)`,
  );
  for (const line of sem.logs.filter(l => /loaded|indexed|targeting|fall/.test(l))) {
    console.log(`                ${line}`);
  }
  // The union is the honest headline: Skript's English prose is deliberately
  // left uncoloured by the grammar, because only the language server can tell an
  // effect from a condition. Either layer alone understates what a reader sees.
  const union = new Set([...hl.captured, ...sem.cells]);
  const combined = coveredVisible(source, union);
  console.log(
    `  combined    : ${combined}/${hl.visible} visible characters coloured ` +
      `(${((100 * combined) / hl.visible).toFixed(0)}%)`,
  );

  if (sem.unclassified.length) {
    console.log(`  unclassified lines (${sem.unclassified.length}):`);
    for (const line of sem.unclassified.slice(0, 20)) console.log(`      ${line}`);
    if (sem.unclassified.length > 20) {
      console.log(`      … and ${sem.unclassified.length - 20} more`);
    }
  }
}
