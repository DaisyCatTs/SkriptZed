#!/usr/bin/env node
// Drives the real skript-lsp binary over stdio and checks the responses.
//
// The unit tests cover the pieces; this proves the assembled server actually
// speaks LSP — correct Content-Length framing, a well-formed initialize result,
// and every feature answering on a real project laid out on disk.
//
//   cd language-server && cargo build -p skript-lsp
//   node scripts/smoke-lsp.mjs

import { spawn } from 'node:child_process';
import { existsSync, mkdirSync, rmSync, writeFileSync } from 'node:fs';
import { writeJar } from './lib/make-jar.mjs';
import { tmpdir } from 'node:os';
import { dirname, join } from 'node:path';
import { fileURLToPath, pathToFileURL } from 'node:url';

const root = join(dirname(fileURLToPath(import.meta.url)), '..');
const exe = process.platform === 'win32' ? 'skript-lsp.exe' : 'skript-lsp';
// SKRIPT_LSP_BINARY points this at a binary that is not a local build — the
// downloaded release asset, most usefully. A local `cargo build` passing proves
// nothing about the artifact a user actually receives, and v0.1.0 shipped
// several times before that gap was noticed.
// Set but wrong must fail, not fall back. A typo — or a release download that
// silently produced nothing — would otherwise pass against a local build while
// appearing to have tested the published artifact, which is the exact failure
// this variable exists to prevent.
const override = process.env.SKRIPT_LSP_BINARY;
if (override && !existsSync(override)) {
  console.error(`SKRIPT_LSP_BINARY is set to a path that does not exist:\n  ${override}`);
  process.exit(2);
}

const binary = [
  override,
  join(root, 'language-server', 'target', 'release', exe),
  join(root, 'language-server', 'target', 'debug', exe),
].find(candidate => candidate && existsSync(candidate));

if (!binary) {
  console.error('skript-lsp not built — run: cd language-server && cargo build -p skript-lsp');
  process.exit(2);
}

console.log(`  binary: ${binary}\n`);

// ---------------------------------------------------------------- fixtures
//
// A real directory, so project-wide indexing has something to find. `library.sk`
// is never opened by the client: resolving a call into it proves the server
// indexed the folder from disk rather than only tracking open buffers.

const vendorDocs = join(root, 'vendor', 'docs.json');

const project = join(tmpdir(), `skript-lsp-smoke-${process.pid}`);
rmSync(project, { recursive: true, force: true });
mkdirSync(join(project, 'nested'), { recursive: true });

writeFileSync(
  join(project, 'nested', 'library.sk'),
  ['function shared_helper(name: text) :: text:', '\treturn "hi %{_name}%"', ''].join('\n'),
);

const MAIN = [
  'options:',
  '\tprefix: &6[Server]',
  '',
  'function greet(name: text, loud: boolean = false) :: text:',
  '\treturn "hi %{_name}%"',
  '',
  'command /hello <text>:',
  '\tpermission: skript.hello',
  '\ttrigger:',
  '\t\tset {_msg} to greet(arg-1)',
  '\t\tset {_other} to shared_helper(arg-1)',
  '\t\tsend "{@prefix} %{_msg}%" to player',
  '',
  'on join:',
  ' \tsend "mixed indent is an error"',
  '',
].join('\n');

const mainPath = join(project, 'main.sk');
writeFileSync(mainPath, MAIN);

// A real JAR carrying only `paper-plugin.yml` — the SkBee case, and the one a
// plugin.yml-only detector silently misses.
mkdirSync(join(project, 'plugins'), { recursive: true });
writeJar(join(project, 'plugins', 'SkBee-3.6.0.jar'), {
  'paper-plugin.yml': [
    'name: SkBee',
    "version: '3.6.0'",
    'main: com.shanebeestudios.skbee.SkBee',
    'dependencies:',
    '  server:',
    '    Skript:',
    '      load: BEFORE',
    '      required: true',
    '',
  ].join('\n'),
});
// An ordinary plugin, to prove non-addons are read but not treated as addons.
writeJar(join(project, 'plugins', 'LuckPerms-Bukkit-5.5.50.jar'), {
  'plugin.yml': 'name: LuckPerms\nversion: 5.5.50\nsoftdepend: [Vault]\n',
});

// Addon syntax as a local file rather than a live fetch, so the smoke test
// needs no network. The SkriptHub path itself is covered by the Rust
// integration tests against the real 8,210-record catalog.
const customSyntax = join(project, 'skbee-syntax.json');
writeFileSync(
  customSyntax,
  JSON.stringify([
    {
      id: 90001,
      title: 'Open Real Inventory',
      description: 'Open real inventories to players.',
      syntax_pattern: 'open real anvil [inventory] to %players%',
      syntax_type: 'effect',
      compatible_addon_version: '3.6.0',
      json_id: 'skbee:effect:open_real_inventory',
      addon: { name: 'SkBee', link_to_addon: 'https://github.com/ShaneBeee/SkBee/', usage_score: 534.8 },
    },
    {
      id: 90002,
      title: 'Nbt Compound',
      description: 'Syntax from an addon that is NOT installed here.',
      syntax_pattern: 'ghost syntax from %string% nowhere',
      syntax_type: 'effect',
      json_id: 'ghost:effect:nowhere',
      addon: { name: 'GhostAddon', link_to_addon: '', usage_score: 0 },
    },
  ]),
);

const uri = pathToFileURL(mainPath).href;
const projectUri = pathToFileURL(project).href;

// ------------------------------------------------------------------ client

const server = spawn(binary, [], { stdio: ['pipe', 'pipe', 'pipe'] });
server.stderr.on('data', chunk => process.stderr.write(`[server] ${chunk}`));
// `exit` makes the server close its stdin, so a write racing the shutdown gets
// EPIPE. That is the protocol working, not a failure.
server.stdin.on('error', () => {});

let nextId = 1;
const pending = new Map();
const notifications = [];

function send(method, params, isRequest = true) {
  const message = isRequest
    ? { jsonrpc: '2.0', id: nextId++, method, params }
    : { jsonrpc: '2.0', method, params };
  const body = Buffer.from(JSON.stringify(message), 'utf8');
  server.stdin.write(`Content-Length: ${body.length}\r\n\r\n`);
  server.stdin.write(body);
  if (!isRequest) return Promise.resolve();
  return new Promise(resolve => pending.set(message.id, resolve));
}

// Byte-exact framing — a character-based reader breaks on non-ASCII payloads.
let buffer = Buffer.alloc(0);
server.stdout.on('data', chunk => {
  buffer = Buffer.concat([buffer, chunk]);
  for (;;) {
    const headerEnd = buffer.indexOf('\r\n\r\n');
    if (headerEnd < 0) return;
    const match = /content-length:\s*(\d+)/i.exec(buffer.subarray(0, headerEnd).toString('ascii'));
    if (!match) return;
    const start = headerEnd + 4;
    const length = Number(match[1]);
    if (buffer.length < start + length) return;

    const message = JSON.parse(buffer.subarray(start, start + length).toString('utf8'));
    buffer = buffer.subarray(start + length);

    if (message.id !== undefined && pending.has(message.id)) {
      pending.get(message.id)(message);
      pending.delete(message.id);
    } else if (message.method) {
      notifications.push(message);
    }
  }
});

const failures = [];
function check(name, condition, detail = '') {
  if (condition) {
    console.log(`  ok    ${name}`);
  } else {
    console.log(`  FAIL  ${name}${detail ? ` — ${detail}` : ''}`);
    failures.push(name);
  }
}

const wait = ms => new Promise(resolve => setTimeout(resolve, ms));

// -------------------------------------------------------------------- checks

try {
  const init = await send('initialize', {
    processId: process.pid,
    rootUri: projectUri,
    workspaceFolders: [{ uri: projectUri, name: 'smoke' }],
    capabilities: { general: { positionEncodings: ['utf-8', 'utf-16'] } },
    initializationOptions: {
      serverPath: project,
      customSyntaxPaths: [customSyntax],
      // The live catalog is exercised by the Rust tests; keep this offline.
      addonSyntaxSource: 'off',
      // Without this the server falls back to downloading docs.json from
      // docs.skriptlang.org, and the run then passes or fails on whether that
      // download beat the first request — which is exactly what happened in
      // CI: green on Windows one run, red the next, for no local reason.
      // `vendor/docs.json` is placed by scripts/fetch-docs.mjs.
      ...(existsSync(vendorDocs) ? { docsPath: vendorDocs } : {}),
    },
  });

  const caps = init.result?.capabilities ?? {};
  check('initialize returns capabilities', !!init.result);
  check('server identifies itself', init.result?.serverInfo?.name === 'skript-lsp');
  check('negotiates utf-8 position encoding', caps.positionEncoding === 'utf-8');
  for (const capability of [
    'documentSymbolProvider',
    'foldingRangeProvider',
    'hoverProvider',
    'definitionProvider',
    'referencesProvider',
    'renameProvider',
    'completionProvider',
    'semanticTokensProvider',
    'workspaceSymbolProvider',
    'documentFormattingProvider',
    'signatureHelpProvider',
    'callHierarchyProvider',
  ]) {
    check(`advertises ${capability}`, caps[capability] !== undefined);
  }

  await send('initialized', {}, false);
  await send(
    'textDocument/didOpen',
    { textDocument: { uri, languageId: 'skript', version: 1, text: MAIN } },
    false,
  );

  // Give the background project scan a moment to finish.
  await wait(600);

  const symbols = await send('textDocument/documentSymbol', { textDocument: { uri } });
  const names = (symbols.result ?? []).map(s => s.name);
  check('finds the options block', names.includes('options'), JSON.stringify(names));
  check('finds the function', names.includes('greet'));
  check('finds the command without its slash', names.includes('hello'));
  check('finds the event', names.some(n => n.startsWith('join')));

  const command = (symbols.result ?? []).find(s => s.name === 'hello');
  check(
    'nests command entries under the command',
    (command?.children ?? []).some(c => c.name === 'trigger'),
  );

  const folds = await send('textDocument/foldingRange', { textDocument: { uri } });
  check('returns folding ranges', (folds.result ?? []).length >= 3);
  check(
    'folding ranges carry collapsed text',
    (folds.result ?? []).every(f => typeof f.collapsedText === 'string'),
  );

  const definition = await send('textDocument/definition', {
    textDocument: { uri },
    position: { line: 9, character: 21 },
  });
  check(
    'go-to-definition resolves a call in the same file',
    (definition.result ?? []).some(loc => loc.range.start.line === 3),
    JSON.stringify(definition.result),
  );

  // `shared_helper` lives in nested/library.sk, which the client never opened.
  const crossFile = await send('textDocument/definition', {
    textDocument: { uri },
    position: { line: 10, character: 25 },
  });
  check(
    'go-to-definition reaches a file that was never opened',
    (crossFile.result ?? []).some(loc => loc.uri.endsWith('library.sk')),
    JSON.stringify(crossFile.result),
  );

  const workspaceSymbols = await send('workspace/symbol', { query: 'shared' });
  check(
    'workspace symbols include unopened files',
    (workspaceSymbols.result ?? []).some(s => s.name === 'shared_helper'),
  );

  const references = await send('textDocument/references', {
    textDocument: { uri },
    position: { line: 3, character: 10 },
    context: { includeDeclaration: true },
  });
  check('find-references sees the call site', (references.result ?? []).length >= 1);

  // Call hierarchy: `greet` is called from the `/hello` trigger, so opening a
  // hierarchy on the declaration must find the command as its caller. An event
  // or a command counts as a caller in Skript — see hierarchy.rs.
  const prepared = await send('textDocument/prepareCallHierarchy', {
    textDocument: { uri },
    position: { line: 3, character: 10 },
  });
  const rootItem = (prepared.result ?? [])[0];
  check('prepares a call hierarchy on a function', rootItem?.name === 'greet');

  if (rootItem) {
    const incoming = await send('callHierarchy/incomingCalls', { item: rootItem });
    const callers = (incoming.result ?? []).map(call => call.from.name);
    check('incoming calls name the calling command', callers.includes('hello'), JSON.stringify(callers));
    check(
      'incoming calls carry the call site',
      ((incoming.result ?? [])[0]?.fromRanges ?? []).length >= 1,
    );
  }

  const rename = await send('textDocument/rename', {
    textDocument: { uri },
    position: { line: 3, character: 10 },
    newName: 'welcome',
  });
  const edits = Object.values(rename.result?.changes ?? {}).flat();
  check('rename edits both the declaration and the call', edits.length >= 2);

  const completion = await send('textDocument/completion', {
    textDocument: { uri },
    position: { line: 11, character: 2 },
  });
  const items = completion.result?.items ?? completion.result ?? [];
  check('completion offers the declared function', items.some(i => i.label === 'greet'));
  // The pattern belongs beside the name and the provenance right-aligned; one
  // crammed `detail` string made a long list unscannable.
  const syntaxItem = items.find(i => i.labelDetails?.description);
  check(
    'completion splits the signature from its provenance',
    !!syntaxItem?.labelDetails?.detail,
    JSON.stringify(syntaxItem?.labelDetails ?? null),
  );
  check(
    'completion offers a function from an unopened file',
    items.some(i => i.label === 'shared_helper'),
  );

  // Cursor just after `greet(` on line 9.
  const signature = await send('textDocument/signatureHelp', {
    textDocument: { uri },
    position: { line: 9, character: 27 },
    context: { triggerKind: 1, isRetrigger: false },
  });
  const info = signature.result?.signatures?.[0];
  check('signature help names the function', info?.label?.startsWith('greet'), JSON.stringify(info));
  check('signature help lists both parameters', (info?.parameters ?? []).length === 2);

  const formatting = await send('textDocument/formatting', {
    textDocument: { uri },
    options: { tabSize: 4, insertSpaces: false },
  });
  const formatted = formatting.result?.[0]?.newText;
  check('formatting returns an edit', typeof formatted === 'string');
  check(
    'formatting fixes the mixed indent',
    formatted?.includes('\n\tsend "mixed indent is an error"'),
    JSON.stringify(formatted?.slice(-60)),
  );
  check(
    'formatting leaves string contents alone',
    formatted?.includes('"{@prefix} %{_msg}%"'),
  );

  const tokens = await send('textDocument/semanticTokens/full', { textDocument: { uri } });
  check('returns semantic tokens', Array.isArray(tokens.result?.data));
  check(
    'semantic token data is a multiple of five',
    (tokens.result?.data?.length ?? 1) % 5 === 0,
  );

  const published = notifications.filter(n => n.method === 'textDocument/publishDiagnostics');
  const problems = published.at(-1)?.params?.diagnostics ?? [];
  check('publishes diagnostics', published.length > 0);
  check(
    'flags the mixed tab/space indent',
    problems.some(d => d.code === 'mixed-indentation'),
    JSON.stringify(problems.map(d => d.code)),
  );
  check(
    'does not report the cross-file call as an unknown function',
    !problems.some(d => d.code === 'unknown-function'),
    JSON.stringify(problems.map(d => d.code)),
  );
  check(
    'does not flag ordinary lines as unknown syntax',
    !problems.some(d => d.code === 'unknown-syntax'),
  );

  // ---- addon detection and syntax ----------------------------------------
  const logs = notifications
    .filter(n => n.method === 'window/logMessage')
    .map(n => n.params?.message ?? '');

  check(
    'detects the addon from its paper-plugin.yml',
    logs.some(line => line.includes('SkBee')),
    JSON.stringify(logs),
  );
  check(
    'reads the ordinary plugin without calling it an addon',
    logs.some(line => /1 Skript addon/.test(line)),
    JSON.stringify(logs.filter(l => l.includes('plugin'))),
  );
  check('loads the custom syntax file', logs.some(line => line.includes('custom syntax')));

  const addonCompletion = await send('textDocument/completion', {
    textDocument: { uri },
    position: { line: 11, character: 2 },
  });
  const addonItems = addonCompletion.result?.items ?? addonCompletion.result ?? [];
  const skbeeItem = addonItems.find(item => item.label === 'Open Real Inventory');
  check('offers the addon syntax in completion', !!skbeeItem, `${addonItems.length} items`);
  check(
    'names the addon in the completion detail',
    skbeeItem?.detail?.includes('SkBee'),
    JSON.stringify(skbeeItem?.detail),
  );
  check(
    'ranks core Skript above addon syntax',
    addonItems.some(i => i.sortText?.startsWith('0')) &&
      skbeeItem?.sortText?.startsWith('1'),
    JSON.stringify(skbeeItem?.sortText),
  );

  await send('shutdown', {});
  await send('exit', {}, false);
} catch (error) {
  console.error('smoke test threw:', error);
  failures.push('unexpected exception');
} finally {
  server.kill();
  rmSync(project, { recursive: true, force: true });
}

console.log('');
if (failures.length) {
  console.log(`${failures.length} check(s) failed`);
  process.exit(1);
}
console.log('all checks passed');
