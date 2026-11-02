// Locations every script needs: the repository root, the tree-sitter CLI and
// the skript-lsp binary.

import { existsSync } from 'node:fs';
import { dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';

const windows = process.platform === 'win32';

export const repoRoot = join(dirname(fileURLToPath(import.meta.url)), '..', '..');
export const grammarDir = join(repoRoot, 'tree-sitter-skript');

// The real binary that tree-sitter-cli installs, not the `.bin` shim: the shim
// is a POSIX shell script on Unix and a `.cmd` on Windows, and Node refuses to
// spawn `.cmd` files without a shell since CVE-2024-27980.
export const treeSitterBinary = join(
  grammarDir,
  'node_modules',
  'tree-sitter-cli',
  windows ? 'tree-sitter.exe' : 'tree-sitter',
);

const lspExe = windows ? 'skript-lsp.exe' : 'skript-lsp';

/**
 * The skript-lsp binary to test: `SKRIPT_LSP_BINARY` if set, else a local
 * release build, else a local debug build.
 *
 * `SKRIPT_LSP_BINARY` exists to test a binary that is not a local build — the
 * downloaded release asset, most usefully. A local `cargo build` passing proves
 * nothing about the artifact a user receives, and v0.1.0 shipped several times
 * before that gap was noticed. So set-but-wrong is an error, never a fallback:
 * a typo, or a release download that silently produced nothing, would
 * otherwise pass against a local build while appearing to have tested the
 * published artifact.
 *
 * @returns {{ binary: string | null, error: string | null }}
 */
export function resolveSkriptLsp() {
  const override = process.env.SKRIPT_LSP_BINARY;
  if (override) {
    return existsSync(override)
      ? { binary: override, error: null }
      : {
          binary: null,
          error: `SKRIPT_LSP_BINARY is set to a path that does not exist:\n  ${override}`,
        };
  }
  const binary = [
    join(repoRoot, 'language-server', 'target', 'release', lspExe),
    join(repoRoot, 'language-server', 'target', 'debug', lspExe),
  ].find(candidate => existsSync(candidate));
  return binary
    ? { binary, error: null }
    : {
        binary: null,
        error: 'skript-lsp not built — run: cd language-server && cargo build -p skript-lsp',
      };
}
