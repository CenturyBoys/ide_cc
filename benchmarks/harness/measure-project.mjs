// Pacote de medições de um projeto REAL via o MCP: cold-start, warm p50/p95, rename blast radius,
// net_delta e memória do language server. Uso:
//   node measure-project.mjs --project /caminho --file rel/x.py --symbol Nome --line 18 [--newname Novo]
import { spawn, execSync } from 'node:child_process';
import { readFileSync } from 'node:fs';
import { join } from 'node:path';
import { createInterface } from 'node:readline';

const args = Object.fromEntries(process.argv.slice(2).reduce((a, x, i, arr) => {
  if (x.startsWith('--')) a.push([x.slice(2), arr[i + 1]]); return a;
}, []));
const PROJECT = args.project;
const FILE = args.file;
const SYMBOL = args.symbol;
const LINE = args.line ? +args.line : undefined;
const NEWNAME = args.newname ?? (SYMBOL + 'Renamed');
const MCP = args.mcp ?? join(process.env.HOME, 'Projects/ide_cc/mcp/target/release/code-intel-mcp');
const BP = args.bp ?? join(process.env.HOME, 'Projects/ide_cc/benchmarks/harness/node_modules/.bin/basedpyright-langserver');

const now = () => Number(process.hrtime.bigint() / 1000n) / 1000;
const pct = (a, p) => { const s = [...a].sort((x, y) => x - y); return +s[Math.min(s.length - 1, Math.floor(p * s.length))].toFixed(1); };
function rssMb() {
  try {
    const out = execSync("ps -eo rss=,args= | grep -i pyright | grep -v grep", { stdio: ['ignore', 'pipe', 'ignore'] }).toString();
    let kb = 0;
    for (const line of out.trim().split('\n')) { const m = line.trim().match(/^(\d+)/); if (m) kb += +m[1]; }
    return kb ? +(kb / 1024).toFixed(0) : null;
  } catch { return null; }
}

const child = spawn(MCP, [], { env: { ...process.env, BASEDPYRIGHT_BIN: BP }, stdio: ['pipe', 'pipe', 'ignore'] });
const rl = createInterface({ input: child.stdout });
const pending = new Map();
rl.on('line', (l) => { let m; try { m = JSON.parse(l); } catch { return; } if (m.id && pending.has(m.id)) { pending.get(m.id)(m); pending.delete(m.id); } });
let idc = 0;
const call = (method, params) => new Promise((res) => { const id = ++idc; pending.set(id, res); child.stdin.write(JSON.stringify({ jsonrpc: '2.0', id, method, params }) + '\n'); });
const tool = async (name, a) => JSON.parse((await call('tools/call', { name, arguments: a })).result.content[0].text);

const R = {};
try {
  await call('initialize', {});
  const refArgs = { project: PROJECT, file: FILE, symbol: SYMBOL, line: LINE };

  // 1) COLD — 1ª find_references (inclui cold-start + warmup do server)
  let t = now();
  const cold = await tool('find_references', refArgs);
  R.cold = { wallMs: +(now() - t).toFixed(0), count: cold.count, stable: cold.stable, warmupMs: cold.warmup_ms, polls: cold.polls };
  const files = new Set((cold.references || []).map((r) => r.split(':').slice(0, -2).join(':')));
  R.cold.files = files.size;

  // 2) WARM — re-roda find_references 8x (índice quente)
  const lat = [];
  for (let i = 0; i < 8; i++) { t = now(); await tool('find_references', refArgs); lat.push(now() - t); }
  R.warm = { p50: pct(lat, 0.5), p95: pct(lat, 0.95), min: +Math.min(...lat).toFixed(1), max: +Math.max(...lat).toFixed(1) };

  // 3) MEMÓRIA do basedpyright (quente)
  R.rssMb = rssMb();

  // 4) RENAME preview (blast radius + net_delta, sem aplicar)
  t = now();
  const rn = await tool('rename_symbol', { ...refArgs, new_name: NEWNAME });
  R.rename = { wallMs: +(now() - t).toFixed(0), safe: rn.safe, netDelta: rn.net_delta, blast: rn.blast_radius, applied: rn.applied };
} catch (e) { R.error = String(e); } finally { child.kill('SIGKILL'); }

console.log('\n===== MEDIÇÃO: ' + SYMBOL + ' @ ' + PROJECT + ' =====');
console.log(JSON.stringify(R, null, 2));
