// Sonda: o que o tsgo retorna para codeAction (extract function / move to file)?
// Precisamos saber o mecanismo: edit direto? data + codeAction/resolve? command + workspace/applyEdit?
import { spawn } from 'node:child_process';
import { readFileSync } from 'node:fs';
import { fileURLToPath, pathToFileURL } from 'node:url';
import { dirname, resolve, join } from 'node:path';
import { StreamMessageReader, StreamMessageWriter, createMessageConnection } from 'vscode-jsonrpc/node';

const __dirname = dirname(fileURLToPath(import.meta.url));
const FIXTURE = resolve(__dirname, '../../fixtures/refactor-ts');
const FILE = join(FIXTURE, 'src/main.ts');
const SERVER = process.env.SERVER ?? 'tsgo';
const SPEC = SERVER === 'vtsls'
  ? { cmd: join(__dirname, 'node_modules/.bin/vtsls'), args: ['--stdio'] }
  : { cmd: join(__dirname, 'node_modules/.bin/tsgo'), args: ['--lsp', '-stdio'] };
console.log(`### SERVER = ${SERVER} ###`);
const child = spawn(SPEC.cmd, SPEC.args, { cwd: FIXTURE, stdio: ['pipe', 'pipe', 'pipe'] });
child.stderr.on('data', () => {});
const conn = createMessageConnection(new StreamMessageReader(child.stdout), new StreamMessageWriter(child.stdin));
conn.onRequest('workspace/configuration', (p) => (p.items ?? []).map(() => ({})));
conn.onRequest('client/registerCapability', () => null);
conn.onRequest('window/workDoneProgress/create', () => null);
conn.onRequest('workspace/semanticTokens/refresh', () => null);
// captura applyEdit (mecanismo de command)
let capturedApplyEdit = null;
conn.onRequest('workspace/applyEdit', (p) => { capturedApplyEdit = p.edit; return { applied: true }; });
conn.onNotification(() => {});
conn.onUnhandledNotification(() => {});
conn.listen();

const rootUri = pathToFileURL(FIXTURE + '/').toString();
await conn.sendRequest('initialize', {
  processId: process.pid, rootUri, workspaceFolders: [{ uri: rootUri, name: 'fx' }],
  capabilities: {
    textDocument: {
      codeAction: { codeActionLiteralSupport: { codeActionKind: { valueSet: ['refactor', 'refactor.extract', 'refactor.move'] } }, resolveSupport: { properties: ['edit'] }, dataSupport: true },
      synchronization: {},
    },
    workspace: { workspaceFolders: true, configuration: true, applyEdit: true, executeCommand: {} },
  },
});
conn.sendNotification('initialized', {});
const uri = pathToFileURL(FILE).toString();
const text = readFileSync(FILE, 'utf8');
conn.sendNotification('textDocument/didOpen', { textDocument: { uri, languageId: 'typescript', version: 1, text } });
await new Promise((r) => setTimeout(r, 2500)); // warmup

async function codeActions(range, only, label) {
  const res = await conn.sendRequest('textDocument/codeAction', {
    textDocument: { uri }, range, context: { diagnostics: [], only },
  }).catch((e) => ({ error: String(e) }));
  console.log(`\n===== ${label} (only=${JSON.stringify(only)}) =====`);
  if (!Array.isArray(res)) { console.log('  resposta:', JSON.stringify(res).slice(0, 300)); return []; }
  for (const a of res) {
    console.log(`  • title="${a.title}" kind=${a.kind} hasEdit=${!!a.edit} hasData=${!!a.data} hasCommand=${!!a.command}`);
  }
  return res;
}

// requisição AMPLA (sem filtro): mostra tudo que o server oferece nessa seleção
await codeActions(
  { start: { line: 1, character: 2 }, end: { line: 3, character: 18 } },
  undefined,
  'TODAS as ações (seleção const)'
);
// EXTRACT: seleciona as 3 linhas de const dentro de compute()
const extract = await codeActions(
  { start: { line: 1, character: 2 }, end: { line: 3, character: 18 } },
  ['refactor.extract', 'refactor'],
  'EXTRACT FUNCTION (linhas const)'
);
// tenta resolver a 1ª ação de extract com data
const toResolve = extract.find((a) => a.data && !a.edit);
if (toResolve) {
  const resolved = await conn.sendRequest('codeAction/resolve', toResolve).catch((e) => ({ error: String(e) }));
  console.log(`\n  -> codeAction/resolve("${toResolve.title}"): hasEdit=${!!resolved.edit}`);
  if (resolved.edit) console.log('     edit=', JSON.stringify(resolved.edit).slice(0, 500));
}
// se alguma ação tem command, executa e vê se dispara applyEdit
const withCmd = extract.find((a) => a.command);
if (withCmd) {
  const c = withCmd.command.command ? withCmd.command : withCmd;
  console.log(`\n  executando command "${c.command}"...`);
  await conn.sendRequest('workspace/executeCommand', { command: c.command, arguments: c.arguments }).catch((e) => console.log('   execErr', String(e).slice(0,200)));
  console.log('   capturedApplyEdit?', !!capturedApplyEdit, capturedApplyEdit ? JSON.stringify(capturedApplyEdit).slice(0,400) : '');
}

// MOVE: cursor sobre o símbolo K (linha 7) para "move to file"
await codeActions(
  { start: { line: 7, character: 13 }, end: { line: 7, character: 14 } },
  ['refactor.move', 'refactor'],
  'MOVE TO FILE (símbolo K)'
);

try { await conn.sendRequest('shutdown'); conn.sendNotification('exit'); } catch {}
child.kill('SIGKILL'); conn.dispose();
