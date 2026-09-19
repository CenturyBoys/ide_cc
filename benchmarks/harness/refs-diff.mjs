// Diagnóstico: por que vtsls e tsgo discordam na contagem de referências?
// Coleta a lista REAL de localizações de cada server e imprime a diferença simétrica,
// para inspeção manual no código-fonte (qual server está certo).
//
// Uso: node refs-diff.mjs --fixture ../../fixtures/zod --file src/types.ts \
//        --search "class ZodType" --symbol ZodType
import { spawn } from 'node:child_process';
import { readFileSync } from 'node:fs';
import { fileURLToPath, pathToFileURL } from 'node:url';
import { dirname, resolve, join, relative } from 'node:path';
import { StreamMessageReader, StreamMessageWriter, createMessageConnection } from 'vscode-jsonrpc/node';

const __dirname = dirname(fileURLToPath(import.meta.url));
const args = Object.fromEntries(process.argv.slice(2).reduce((a, x, i, arr) => {
  if (x.startsWith('--')) a.push([x.slice(2), arr[i + 1]]); return a;
}, []));
const FIXTURE = resolve(__dirname, args.fixture ?? '../../fixtures/zod');
const FILE = args.file ?? 'src/types.ts';
const SEARCH = args.search ?? 'class ZodType';
const SYMBOL = args.symbol ?? 'ZodType';

const SERVERS = {
  vtsls: { cmd: join(__dirname, 'node_modules/.bin/vtsls'), args: ['--stdio'] },
  tsgo: { cmd: join(__dirname, 'node_modules/.bin/tsgo'), args: ['--lsp', '-stdio'] },
};

function loc(fixture, l) {
  const rel = relative(fixture, fileURLToPath(l.uri));
  return `${rel}:${l.range.start.line + 1}:${l.range.start.character + 1}`;
}

async function collect(name) {
  const spec = SERVERS[name];
  const child = spawn(spec.cmd, spec.args, { cwd: FIXTURE, stdio: ['pipe', 'pipe', 'pipe'] });
  child.stderr.on('data', () => {});
  const conn = createMessageConnection(new StreamMessageReader(child.stdout), new StreamMessageWriter(child.stdin));
  conn.onRequest('workspace/configuration', (p) => (p.items ?? []).map(() => ({})));
  conn.onRequest('client/registerCapability', () => null);
  conn.onRequest('window/workDoneProgress/create', () => null);
  conn.onRequest('workspace/semanticTokens/refresh', () => null);
  conn.onNotification(() => {});
  conn.onUnhandledNotification(() => {});
  conn.listen();
  const rootUri = pathToFileURL(FIXTURE + '/').toString();
  await conn.sendRequest('initialize', {
    processId: process.pid, rootUri, workspaceFolders: [{ uri: rootUri, name: 'fx' }],
    capabilities: { textDocument: { references: {}, synchronization: {} }, workspace: { workspaceFolders: true, configuration: true } },
  });
  conn.sendNotification('initialized', {});

  const absFile = join(FIXTURE, FILE);
  const text = readFileSync(absFile, 'utf8');
  const lines = text.split('\n');
  let line = lines.findIndex((L) => L.includes(SEARCH));
  const character = lines[line].indexOf(SYMBOL);
  const uri = pathToFileURL(absFile).toString();
  conn.sendNotification('textDocument/didOpen', { textDocument: { uri, languageId: 'typescript', version: 1, text } });

  // poll até estabilizar (evita cold truncation atrapalhar a comparação)
  let prev = -1, res = [];
  for (let i = 0; i < 60; i++) {
    res = await conn.sendRequest('textDocument/references', {
      textDocument: { uri }, position: { line, character }, context: { includeDeclaration: true },
    }) ?? [];
    if (res.length === prev && res.length > 0) break;
    prev = res.length;
    await new Promise((r) => setTimeout(r, 400));
  }
  const set = new Set(res.map((l) => loc(FIXTURE, l)));
  try { await conn.sendRequest('shutdown'); conn.sendNotification('exit'); } catch {}
  child.kill('SIGKILL'); conn.dispose();
  return set;
}

const a = await collect('vtsls');
const b = await collect('tsgo');
const onlyVtsls = [...a].filter((x) => !b.has(x)).sort();
const onlyTsgo = [...b].filter((x) => !a.has(x)).sort();

console.log(`\n=== referências a ${SYMBOL} (${FILE}) ===`);
console.log(`vtsls: ${a.size}   tsgo: ${b.size}   (comuns: ${[...a].filter((x) => b.has(x)).length})`);
console.log(`\n-- só no vtsls (${onlyVtsls.length}) --`);
onlyVtsls.forEach((x) => console.log('  ' + x));
console.log(`\n-- só no tsgo (${onlyTsgo.length}) --`);
onlyTsgo.forEach((x) => console.log('  ' + x));
