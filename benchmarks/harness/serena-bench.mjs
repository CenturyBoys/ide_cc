// Benchmark do Serena (servidor MCP sobre LSP). Diferente do lsp-bench.mjs (que fala LSP cru),
// aqui falamos MCP e chamamos as ferramentas semânticas do Serena, para medir:
//   - overhead da camada MCP + indexação própria do Serena (cold-start real de ponta a ponta)
//   - find_referencing_symbols: latência, contagem, e se TRUNCA (Serena promete warmup)
//
// Uso: node serena-bench.mjs --fixture ../../fixtures/mono-ts \
//        --namepath Widget --relpath packages/core/src/index.ts
import { writeFileSync, mkdirSync } from 'node:fs';
import { fileURLToPath } from 'node:url';
import { dirname, resolve, join } from 'node:path';
import { Client } from '@modelcontextprotocol/sdk/client/index.js';
import { StdioClientTransport } from '@modelcontextprotocol/sdk/client/stdio.js';

const __dirname = dirname(fileURLToPath(import.meta.url));
const args = Object.fromEntries(process.argv.slice(2).reduce((a, x, i, arr) => {
  if (x.startsWith('--')) a.push([x.slice(2), arr[i + 1]]); return a;
}, []));
const FIXTURE = resolve(__dirname, args.fixture ?? '../../fixtures/mono-ts');
const NAME_PATH = args.namepath ?? 'Widget';
const REL_PATH = args.relpath ?? 'packages/core/src/index.ts';
const now = () => Number(process.hrtime.bigint() / 1000n) / 1000;

const result = { server: 'serena', fixture: FIXTURE, target: { NAME_PATH, REL_PATH }, steps: {} };

// Conta referências e arquivos numa resposta de find_referencing_symbols do Serena.
// Formato: { "<file>": { "File":[...], "Constant":[...], ... }, ... } — possivelmente
// precedido por uma linha "The answer is too long...". Retorna {refs, files}.
function countRefs(res) {
  const txt = (res?.content ?? []).map((c) => c.text ?? '').join('\n');
  const start = txt.indexOf('{');
  if (start < 0) return { refs: 0, files: 0, raw: txt.slice(0, 200) };
  let obj;
  try { obj = JSON.parse(txt.slice(start)); } catch { return { refs: -1, files: -1, raw: txt.slice(0, 200) }; }
  let refs = 0, files = 0;
  for (const f of Object.keys(obj)) { files++; for (const kind of Object.keys(obj[f])) refs += obj[f][kind].length; }
  return { refs, files };
}

const transport = new StdioClientTransport({
  command: 'uvx',
  args: ['--from', 'git+https://github.com/oraios/serena', 'serena', 'start-mcp-server',
    '--transport', 'stdio', '--context', 'ide-assistant', '--project', FIXTURE],
  cwd: FIXTURE,
  stderr: 'ignore',
});
const client = new Client({ name: 'bench', version: '0.0.0' }, { capabilities: {} });

try {
  const t0 = now();
  await client.connect(transport);
  result.steps.connectMs = +(now() - t0).toFixed(1);

  const tl = now();
  const tools = await client.listTools();
  result.steps.toolsListMs = +(now() - tl).toFixed(1);
  result.tools = tools.tools.map((t) => t.name);
  const has = (n) => result.tools.includes(n);

  // find_symbol (leve) — localizar o símbolo
  if (has('find_symbol')) {
    const ts = now();
    const r = await client.callTool({ name: 'find_symbol', arguments: { name_path: NAME_PATH, relative_path: REL_PATH } });
    result.steps.findSymbol = { ms: +(now() - ts).toFixed(1), isError: !!r.isError };
  }

  // find_referencing_symbols: 1ª chamada (dispara index) e 2ª (quente).
  // max_answer_chars alto para obter a lista completa e contar sem corte de saída.
  if (has('find_referencing_symbols')) {
    const call = async () => {
      const ts = now();
      const r = await client.callTool({
        name: 'find_referencing_symbols',
        arguments: { name_path: NAME_PATH, relative_path: REL_PATH, max_answer_chars: 50000000 },
      });
      return { ms: +(now() - ts).toFixed(1), ...countRefs(r) };
    };
    const cold = await call();
    const warm = await call();
    result.steps.references = {
      coldMs: cold.ms, coldRefs: cold.refs, coldFiles: cold.files,
      warmMs: warm.ms, warmRefs: warm.refs, warmFiles: warm.files,
      truncatedFirstResponse: cold.refs !== warm.refs,
    };
  } else {
    result.steps.references = { error: 'find_referencing_symbols ausente' };
  }

  result.ok = true;
} catch (e) {
  result.ok = false; result.error = String(e?.stack ?? e);
} finally {
  try { await client.close(); } catch {}
}

const outDir = resolve(__dirname, '../results');
mkdirSync(outDir, { recursive: true });
const fixtureName = FIXTURE.split('/').filter(Boolean).pop();
writeFileSync(join(outDir, `serena-${fixtureName}.json`), JSON.stringify(result, null, 2));

console.log('\n===== SERENA (MCP) BENCHMARK @ ' + fixtureName + ' =====');
if (!result.ok) { console.log('FALHOU:', result.error); process.exit(1); }
console.log(`connect (spawn->MCP ready): ${result.steps.connectMs} ms`);
console.log(`tools/list:                 ${result.steps.toolsListMs} ms (${result.tools.length} tools)`);
if (result.steps.findSymbol) console.log(`find_symbol:                ${result.steps.findSymbol.ms} ms`);
const rf = result.steps.references;
if (rf.error) { console.log('references:', rf.error); }
else {
  console.log(`\nfind_referencing_symbols(${NAME_PATH}):`);
  console.log(`  cold: ${rf.coldRefs} refs em ${rf.coldFiles} arquivos @ ${rf.coldMs} ms`);
  console.log(`  warm: ${rf.warmRefs} refs em ${rf.warmFiles} arquivos @ ${rf.warmMs} ms`);
  console.log(`  TRUNCOU (cold≠warm)?: ${rf.truncatedFirstResponse ? 'SIM' : 'não'}`);
}
console.log(`\ntools (${result.tools.length}): ${result.tools.join(', ')}`);
