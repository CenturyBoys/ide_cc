import { resolve, dirname } from 'node:path';
import { fileURLToPath } from 'node:url';
import { Client } from '@modelcontextprotocol/sdk/client/index.js';
import { StdioClientTransport } from '@modelcontextprotocol/sdk/client/stdio.js';
const __dirname = dirname(fileURLToPath(import.meta.url));
const FIXTURE = resolve(__dirname, '../../fixtures/mono-ts');
const transport = new StdioClientTransport({
  command: 'uvx', args: ['--from', 'git+https://github.com/oraios/serena', 'serena',
    'start-mcp-server', '--transport', 'stdio', '--context', 'ide-assistant', '--project', FIXTURE],
  cwd: FIXTURE, stderr: 'ignore',
});
const client = new Client({ name: 'dbg', version: '0.0.0' }, { capabilities: {} });
const dump = (label, r) => {
  console.log(`\n### ${label} (isError=${r?.isError}) ###`);
  console.log((r?.content ?? []).map((c) => c.text ?? JSON.stringify(c)).join('\n').slice(0, 1200));
};
await client.connect(transport);
console.log('conectado');
for (const [label, name, arguments_] of [
  ['find_symbol Widget', 'find_symbol', { name_path: 'Widget', relative_path: 'packages/core/src/index.ts' }],
  ['find_referencing_symbols Widget', 'find_referencing_symbols', { name_path: 'Widget', relative_path: 'packages/core/src/index.ts' }],
]) {
  const t = Date.now();
  try { const r = await client.callTool({ name, arguments: arguments_ }); dump(`${label} [${Date.now()-t}ms]`, r); }
  catch (e) { console.log(`\n### ${label} THREW:`, String(e).slice(0, 400)); }
}
await client.close();
