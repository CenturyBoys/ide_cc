// LSP benchmark harness — mede o comportamento fundamental de um language server
// sobre o qual qualquer camada (Serena/nossa) se apoia:
//   1. cold-start (spawn -> initialize/initialized)
//   2. find_references: latência + TRUNCAMENTO (a 1ª resposta bate com a estável?)
//   3. rename dry-run: latência + tamanho do WorkspaceEdit (blast radius)
//
// Foco: responder objetivamente "quão rápido, e quando o resultado é confiável".
//
// Uso: node lsp-bench.mjs --server vtsls --fixture ../../fixtures/zod
//
import { spawn } from 'node:child_process';
import { readFileSync, writeFileSync, mkdirSync } from 'node:fs';
import { fileURLToPath, pathToFileURL } from 'node:url';
import { dirname, resolve, join } from 'node:path';
import {
  StreamMessageReader, StreamMessageWriter, createMessageConnection,
} from 'vscode-jsonrpc/node';

const __dirname = dirname(fileURLToPath(import.meta.url));

// ---- args -------------------------------------------------------------
const args = Object.fromEntries(
  process.argv.slice(2).reduce((acc, a, i, arr) => {
    if (a.startsWith('--')) acc.push([a.slice(2), arr[i + 1]]);
    return acc;
  }, [])
);
const SERVER = args.server ?? 'vtsls';
const FIXTURE = resolve(__dirname, args.fixture ?? '../../fixtures/zod');
// alvo parametrizável (default: zod). --file relativo ao fixture; --search localiza a linha;
// --symbol é o token cuja posição medimos; --newname é o nome do rename dry-run.
const TARGET_FILE = args.file ?? 'src/types.ts';
const TARGET_SEARCH = args.search ?? 'class ZodType';
const TARGET_SYMBOL = args.symbol ?? 'ZodType';
const NEW_NAME = args.newname ?? 'ZodSchemaBase';
const now = () => Number(process.hrtime.bigint() / 1000n) / 1000; // ms, float

// ---- server launch commands ------------------------------------------
const SERVERS = {
  vtsls: { cmd: join(__dirname, 'node_modules/.bin/vtsls'), args: ['--stdio'] },
  tsgo: { cmd: join(__dirname, 'node_modules/.bin/tsgo'), args: ['--lsp', '-stdio'] },
  basedpyright: { cmd: join(__dirname, 'node_modules/.bin/basedpyright-langserver'), args: ['--stdio'] },
  dart: { cmd: 'dart', args: ['language-server'] },
  'rust-analyzer': { cmd: `${process.env.HOME}/.cargo/bin/rust-analyzer`, args: [] },
  'csharp-ls': { cmd: `${process.env.HOME}/.dotnet/tools/csharp-ls`, args: [] },
};

// languageId do LSP a partir da extensão (o harness é multi-linguagem agora)
function langId(file) {
  if (file.endsWith('.py')) return 'python';
  if (file.endsWith('.dart')) return 'dart';
  if (file.endsWith('.rs')) return 'rust';
  if (file.endsWith('.cs')) return 'csharp';
  if (file.endsWith('.tsx')) return 'typescriptreact';
  if (file.endsWith('.jsx')) return 'javascriptreact';
  if (file.endsWith('.js') || file.endsWith('.mjs')) return 'javascript';
  return 'typescript';
}
const spec = SERVERS[SERVER];
if (!spec) { console.error(`server desconhecido: ${SERVER}`); process.exit(1); }

// ---- helper: localizar a posição (0-indexed) de um símbolo no arquivo --
function findSymbolPosition(absFile, needle) {
  const text = readFileSync(absFile, 'utf8');
  const lines = text.split('\n');
  for (let l = 0; l < lines.length; l++) {
    const idx = lines[l].indexOf(needle);
    if (idx >= 0) return { line: l, character: idx, text };
  }
  throw new Error(`símbolo "${needle}" não achado em ${absFile}`);
}

// ---- main -------------------------------------------------------------
const result = { server: SERVER, fixture: FIXTURE, steps: {} };

const t0 = now();
const child = spawn(spec.cmd, spec.args, { cwd: FIXTURE, stdio: ['pipe', 'pipe', 'pipe'] });
child.stderr.on('data', () => {}); // silencia ruído do server

const conn = createMessageConnection(
  new StreamMessageReader(child.stdout),
  new StreamMessageWriter(child.stdin),
);
// responde requests server-initiated (senão o workspace nunca carrega — lição do agent-lsp)
conn.onRequest('workspace/configuration', (p) => (p.items ?? []).map(() => ({})));
conn.onRequest('client/registerCapability', () => null);
conn.onRequest('window/workDoneProgress/create', () => null);
conn.onRequest('workspace/semanticTokens/refresh', () => null);
conn.onNotification(() => {});
conn.onUnhandledNotification(() => {});
conn.listen();

const rootUri = pathToFileURL(FIXTURE + '/').toString();

async function initialize() {
  const ti = now();
  await conn.sendRequest('initialize', {
    processId: process.pid,
    rootUri,
    workspaceFolders: [{ uri: rootUri, name: 'fixture' }],
    capabilities: {
      textDocument: {
        references: {}, rename: { prepareSupport: true },
        definition: {}, documentSymbol: {}, hover: {},
        synchronization: { didSave: true, dynamicRegistration: true },
      },
      workspace: { workspaceFolders: true, configuration: true, didChangeWatchedFiles: {} },
      window: { workDoneProgress: true },
    },
  });
  conn.sendNotification('initialized', {});
  result.steps.coldStartMs = +(now() - ti).toFixed(1);
}

function openDoc(absFile) {
  const text = readFileSync(absFile, 'utf8');
  conn.sendNotification('textDocument/didOpen', {
    textDocument: { uri: pathToFileURL(absFile).toString(), languageId: langId(absFile), version: 1, text },
  });
}

async function references(uri, pos) {
  const r = await conn.sendRequest('textDocument/references', {
    textDocument: { uri }, position: pos, context: { includeDeclaration: true },
  });
  return Array.isArray(r) ? r.length : 0;
}

// Sonda de TRUNCAMENTO: repete find_references até a contagem estabilizar,
// registrando a curva. É o modo de falha #76870 (resultado parcial em silêncio).
async function truncationProbe(uri, pos, { maxMs = 60000, everyMs = 300, stableHits = 4 } = {}) {
  const curve = [];
  const start = now();
  let last = -1, stable = 0, firstCount = null, firstMs = null;
  while (now() - start < maxMs) {
    const ts = now();
    // servers como rust-analyzer LANÇAM erro enquanto indexam ('No references found at position').
    // Tratamos como "ainda não pronto" (count = -1) e continuamos o polling.
    let count;
    try { count = await references(uri, pos); } catch { count = -1; }
    const at = +(now() - start).toFixed(1);
    curve.push({ atMs: at, count, latMs: +(now() - ts).toFixed(1) });
    if (count >= 0 && firstCount === null) { firstCount = count; firstMs = at; }
    if (count === last && count > 0) { stable++; if (stable >= stableHits) break; } else { stable = 0; }
    last = count;
    await new Promise((r) => setTimeout(r, everyMs));
  }
  const stableCount = last;
  return {
    firstCount, firstMs, stableCount, stableAtMs: curve[curve.length - 1]?.atMs,
    truncatedFirstResponse: firstCount !== stableCount,
    curve,
  };
}

async function renameDryRun(uri, pos, newName) {
  const tp = now();
  let prepared = true;
  try { await conn.sendRequest('textDocument/prepareRename', { textDocument: { uri }, position: pos }); }
  catch { prepared = false; }
  const tr = now();
  const edit = await conn.sendRequest('textDocument/rename', {
    textDocument: { uri }, position: pos, newName,
  });
  // NÃO aplicamos — só medimos o blast radius do WorkspaceEdit.
  const changes = edit?.changes ?? {};
  const docChanges = edit?.documentChanges ?? [];
  let files = 0, edits = 0;
  for (const k of Object.keys(changes)) { files++; edits += changes[k].length; }
  for (const dc of docChanges) { if (dc.edits) { files++; edits += dc.edits.length; } }
  return {
    prepareMs: +(tr - tp).toFixed(1), renameMs: +(now() - tr).toFixed(1),
    prepared, filesTouched: files, totalEdits: edits,
  };
}

async function warmLatency(uri, pos, n = 20) {
  const lats = [];
  for (let i = 0; i < n; i++) { const t = now(); await references(uri, pos); lats.push(now() - t); }
  lats.sort((a, b) => a - b);
  const pct = (p) => +lats[Math.min(lats.length - 1, Math.floor(p * lats.length))].toFixed(2);
  return { p50: pct(0.5), p95: pct(0.95), min: +lats[0].toFixed(2), max: +lats[lats.length - 1].toFixed(2) };
}

try {
  await initialize();

  const typesFile = join(FIXTURE, TARGET_FILE);
  const target = findSymbolPosition(typesFile, TARGET_SEARCH);
  // posição do TOKEN alvo dentro da linha localizada por TARGET_SEARCH
  const zodTypePos = { line: target.line, character: target.text.split('\n')[target.line].indexOf(TARGET_SYMBOL) };
  const uri = pathToFileURL(typesFile).toString();

  result.target = { file: TARGET_FILE, symbol: TARGET_SYMBOL, pos: zodTypePos };

  // abre o doc-alvo antes de qualquer operação semântica (didOpen dispara load do projeto)
  const tOpen = now();
  openDoc(typesFile);
  result.steps.didOpenMs = +(now() - tOpen).toFixed(1);

  // 1) sonda de truncamento (cold -> estável)
  result.steps.truncation = await truncationProbe(uri, zodTypePos);

  // 2) latência warm de find_references
  result.steps.refsWarm = await warmLatency(uri, zodTypePos);

  // 3) rename dry-run (blast radius)
  result.steps.renameDryRun = await renameDryRun(uri, zodTypePos, NEW_NAME);

  result.totalMs = +(now() - t0).toFixed(1);
  result.ok = true;
} catch (e) {
  result.ok = false; result.error = String(e?.stack ?? e);
} finally {
  try { await conn.sendRequest('shutdown'); conn.sendNotification('exit'); } catch {}
  child.kill('SIGKILL');
  conn.dispose();
}

// ---- output -----------------------------------------------------------
const outDir = resolve(__dirname, '../results');
mkdirSync(outDir, { recursive: true });
const fixtureName = FIXTURE.split('/').filter(Boolean).pop();
const outFile = join(outDir, `${SERVER}-${fixtureName}.json`);
writeFileSync(outFile, JSON.stringify(result, null, 2));

const s = result.steps;
console.log('\n===== LSP BENCHMARK: ' + SERVER + ' @ ' + fixtureName + ' =====');
if (!result.ok) { console.log('FALHOU:', result.error); process.exit(1); }
console.log(`cold-start (init->initialized): ${s.coldStartMs} ms`);
console.log(`didOpen:                        ${s.didOpenMs} ms`);
console.log(`\nfind_references(${TARGET_SYMBOL}):`);
console.log(`  1ª resposta:  ${s.truncation.firstCount} refs @ ${s.truncation.firstMs} ms`);
console.log(`  estável:      ${s.truncation.stableCount} refs @ ${s.truncation.stableAtMs} ms`);
console.log(`  TRUNCOU 1ª?:  ${s.truncation.truncatedFirstResponse ? 'SIM  <-- cold-index race (#76870)' : 'não'}`);
console.log(`  warm p50/p95: ${s.refsWarm.p50} / ${s.refsWarm.p95} ms`);
console.log(`\nrename ${TARGET_SYMBOL} -> ${NEW_NAME} (dry-run):`);
console.log(`  prepareRename: ${s.renameDryRun.prepared ? 'ok' : 'não suportado'} (${s.renameDryRun.prepareMs} ms)`);
console.log(`  rename:        ${s.renameDryRun.renameMs} ms`);
console.log(`  blast radius:  ${s.renameDryRun.filesTouched} arquivos, ${s.renameDryRun.totalEdits} edições`);
console.log(`\ntotal: ${result.totalMs} ms  ->  ${outFile}`);
