// A minimal LSP client over a child process's stdio, for the test harnesses.
//
// Framing is byte-exact: `Content-Length` counts UTF-8 bytes, so a reader that
// works on characters breaks on the first non-ASCII payload — and Skript is
// full of `§`, `¦` and non-English messages.

import { spawn } from 'node:child_process';

/**
 * Starts `binary` and speaks JSON-RPC to it.
 *
 * @param {string} binary
 * @param {{ onStderr?: (chunk: Buffer) => void }} [options]
 */
export function startLsp(binary, { onStderr } = {}) {
  const server = spawn(binary, [], { stdio: ['pipe', 'pipe', 'pipe'] });
  server.stderr.on('data', chunk => onStderr?.(chunk));
  // `exit` makes the server close its stdin, so a write racing the shutdown
  // gets EPIPE. That is the protocol working, not a failure.
  server.stdin.on('error', () => {});

  let nextId = 1;
  const pending = new Map();
  /** Every server-to-client notification, in arrival order. */
  const notifications = [];

  function write(message) {
    const body = Buffer.from(JSON.stringify(message), 'utf8');
    server.stdin.write(`Content-Length: ${body.length}\r\n\r\n`);
    server.stdin.write(body);
  }

  let buffer = Buffer.alloc(0);
  server.stdout.on('data', chunk => {
    buffer = Buffer.concat([buffer, chunk]);
    for (;;) {
      const headerEnd = buffer.indexOf('\r\n\r\n');
      if (headerEnd < 0) return;
      const match = /content-length:\s*(\d+)/i.exec(
        buffer.subarray(0, headerEnd).toString('ascii'),
      );
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

  return {
    process: server,
    notifications,
    /** Sends a request and resolves with the whole response message. */
    request(method, params) {
      const id = nextId++;
      write({ jsonrpc: '2.0', id, method, params });
      return new Promise(resolve => pending.set(id, resolve));
    },
    notify(method, params) {
      write({ jsonrpc: '2.0', method, params });
    },
    /** `shutdown`, then `exit`, then make sure the process is gone. */
    async stop() {
      await this.request('shutdown', {});
      this.notify('exit', {});
      server.kill();
    },
  };
}
