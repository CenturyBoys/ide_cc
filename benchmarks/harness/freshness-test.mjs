// Testa o "freshness": o MCP reflete uma edição feita no DISCO por FORA do Claude?
// Cria um projeto TS temporário, consulta document_symbols, edita o arquivo no disco entre as
// chamadas (simulando outro editor / git), e verifica que o novo símbolo aparece.
import { spawn } from 'node:child_process';
import { mkdtempSync, writeFileSync, appendFileSync, rmSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { createInterface } from 'node:readline';

const MCP = process.env.MCP_BIN ?? join(process.env.HOME, 'Projects/ide_cc/mcp/target/release/code-intel-mcp');
const TSGO = process.env.TSGO_BIN ?? join(import.meta.dirname, 'node_modules/.bin/tsgo');

const dir = mkdtempSync(join(tmpdir(), 'fresh-'));
writeFileSync(join(dir, 'tsconfig.json'), JSON.stringify({ compilerOptions: { strict: true }, include: ['*.ts'] }));
const file = join(dir, 'a.ts');
writeFileSync(file, 'export function one(): number { return 1; }\n');

const child = spawn(MCP, [], { env: { ...process.env, TSGO_BIN: TSGO }, stdio: ['pipe', 'pipe', 'ignore'] });
const rl = createInterface({ input: child.stdout });
const pending = new Map();
rl.on('line', (l) => {
  let m; try { m = JSON.parse(l); } catch { return; }
  if (m.id && pending.has(m.id)) { pending.get(m.id)(m); pending.delete(m.id); }
});
let idc = 0;
const call = (method, params) => new Promise((res) => { const id = ++idc; pending.set(id, res); child.stdin.write(JSON.stringify({ jsonrpc: '2.0', id, method, params }) + '\n'); });
const symbols = (r) => { try { return JSON.parse(r.result.content[0].text).symbols.map((s) => s.name_path); } catch { return []; } };

try {
  await call('initialize', {});
  const before = symbols(await call('tools/call', { name: 'document_symbols', arguments: { project: dir, file: 'a.ts' } }));
  console.log('antes:', before);

  // EDIÇÃO EXTERNA no disco (outro processo, não o Claude)
  await new Promise((r) => setTimeout(r, 30)); // garante mtime diferente
  appendFileSync(file, 'export function two(): number { return 2; }\n');

  const after = symbols(await call('tools/call', { name: 'document_symbols', arguments: { project: dir, file: 'a.ts' } }));
  console.log('depois:', after);

  const ok = !before.includes('two') && after.includes('two');
  console.log(ok ? '\n✅ FRESHNESS OK: edição externa refletida (two apareceu)' : '\n❌ FALHOU: edição externa NÃO refletida');
  process.exitCode = ok ? 0 : 1;
} finally {
  child.kill('SIGKILL');
  rmSync(dir, { recursive: true, force: true });
}
