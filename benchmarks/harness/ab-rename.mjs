// Experimento A/B "com e sem a camada": renomear a CLASSE Widget -> Gadget no fixture-armadilha,
// de duas formas, medindo CORREÇÃO + TEMPO:
//   SEM  (texto-cru): \bWidget\b -> Gadget em todos os .ts (o que um agente sem tools faria)
//   COM  (semântico): rename_symbol do MCP (tsgo + net_delta)
// Correção = build limpo (tsc --noEmit) E armadilhas intactas E classe renomeada.
import { spawn, execFileSync, execSync } from 'node:child_process';
import { cpSync, rmSync, readFileSync, readdirSync, writeFileSync, mkdtempSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { createInterface } from 'node:readline';

const HOME = process.env.HOME;
const HARNESS = import.meta.dirname;
const FIXTURE = join(HOME, 'Projects/ide_cc/fixtures/ab-rename');
const MCP = join(HOME, 'Projects/ide_cc/mcp/target/release/code-intel-mcp');
const TSGO = join(HARNESS, 'node_modules/.bin/tsgo');
const TSC = join(HARNESS, 'node_modules/.bin/tsc');

function tsFiles(dir) { return readdirSync(join(dir, 'src')).filter((f) => f.endsWith('.ts')).map((f) => join(dir, 'src', f)); }
function concatTs(dir) { return tsFiles(dir).map((f) => readFileSync(f, 'utf8')).join('\n'); }
function count(text, re) { return (text.match(re) || []).length; }

function buildOk(dir) {
  try { execFileSync(TSC, ['--noEmit', '-p', join(dir, 'tsconfig.json')], { stdio: 'pipe' }); return true; }
  catch { return false; }
}

// avalia a correção pós-rename contra o ground-truth
function evaluate(dir) {
  const t = concatTs(dir);
  const checks = {
    'classe renomeada (class Gadget)': count(t, /class Gadget\b/g) === 1,
    'WidgetFactory intacto (3)': count(t, /WidgetFactory/g) === 3,
    'strings "Widget" intactas (2)': count(t, /"Widget"/g) === 2,
    'const Widget nao-relacionada intacta': /export const Widget\b/.test(t),
    'build tsc --noEmit limpo': buildOk(dir),
  };
  const ok = Object.values(checks).every(Boolean);
  const corrupted = Object.entries(checks).filter(([, v]) => !v).map(([k]) => k);
  return { ok, corrupted };
}

// SEM a camada: rename por texto com \bWidget\b (grep/sed "cuidadoso")
function runNaive() {
  const dir = mkdtempSync(join(tmpdir(), 'ab-naive-'));
  cpSync(FIXTURE, dir, { recursive: true });
  const t0 = Date.now();
  for (const f of tsFiles(dir)) writeFileSync(f, readFileSync(f, 'utf8').replace(/\bWidget\b/g, 'Gadget'));
  const ms = Date.now() - t0;
  const res = evaluate(dir);
  rmSync(dir, { recursive: true, force: true });
  return { ms, ...res };
}

// COM a camada: rename_symbol semântico do MCP
function runSemantic() {
  return new Promise((resolve) => {
    const dir = mkdtempSync(join(tmpdir(), 'ab-sem-'));
    cpSync(FIXTURE, dir, { recursive: true });
    const child = spawn(MCP, [], { env: { ...process.env, TSGO_BIN: TSGO }, stdio: ['pipe', 'pipe', 'ignore'] });
    const rl = createInterface({ input: child.stdout });
    const pending = new Map();
    rl.on('line', (l) => { let m; try { m = JSON.parse(l); } catch { return; } if (m.id && pending.has(m.id)) { pending.get(m.id)(m); pending.delete(m.id); } });
    let idc = 0;
    const call = (method, params) => new Promise((res) => { const id = ++idc; pending.set(id, res); child.stdin.write(JSON.stringify({ jsonrpc: '2.0', id, method, params }) + '\n'); });
    (async () => {
      await call('initialize', {});
      const t0 = Date.now();
      const r = await call('tools/call', { name: 'rename_symbol', arguments: { project: dir, file: 'src/widget.ts', symbol: 'Widget', new_name: 'Gadget', line: 2, apply: true } });
      const ms = Date.now() - t0;
      let applied = null; try { applied = JSON.parse(r.result.content[0].text).applied; } catch {}
      child.kill('SIGKILL');
      const res = evaluate(dir);
      rmSync(dir, { recursive: true, force: true });
      resolve({ ms, applied, ...res });
    })();
  });
}

console.log('=== A/B: renomear a CLASSE Widget -> Gadget (fixture-armadilha) ===\n');
const naive = runNaive();
console.log(`SEM a camada (texto-cru \\bWidget\\b -> sed):`);
console.log(`  correto: ${naive.ok ? 'SIM' : 'NÃO'}   tempo: ${naive.ms} ms`);
if (naive.corrupted.length) console.log(`  falhas: ${naive.corrupted.join(' | ')}`);

const sem = await runSemantic();
console.log(`\nCOM a camada (rename_symbol semântico, applied=${sem.applied}):`);
console.log(`  correto: ${sem.ok ? 'SIM' : 'NÃO'}   tempo: ${sem.ms} ms`);
if (sem.corrupted.length) console.log(`  falhas: ${sem.corrupted.join(' | ')}`);

console.log('\n=== VEREDITO ===');
console.log(`SEM: ${naive.ok ? 'correto' : 'INCORRETO'} em ${naive.ms}ms  |  COM: ${sem.ok ? 'correto' : 'INCORRETO'} em ${sem.ms}ms`);
if (sem.ok && !naive.ok) console.log('✅ A camada produziu a mudança CORRETA; o texto-cru corrompeu as armadilhas.');
