// Prova o cache entre sessões: a mesma operação PESADA (rust-analyzer, ~25s cold) em duas sessões
// MCP separadas (simulando o Claude Code reiniciar). Com CODE_INTEL_DAEMON=1, a sessão 2 reconecta
// a um daemon que manteve o language server QUENTE -> deve ser ~instantânea.
import { spawn, execSync } from 'node:child_process';
import { join } from 'node:path';
import { createInterface } from 'node:readline';

const MCP = process.env.MCP_BIN ?? join(process.env.HOME, 'Projects/ide_cc/mcp/target/release/code-intel-mcp');
const PROJECT = join(process.env.HOME, 'Projects/ide_cc/fixtures/rust-demo');
const RA = join(process.env.HOME, '.cargo/bin/rust-analyzer');
const env = { ...process.env, CODE_INTEL_DAEMON: '1', RUST_ANALYZER_BIN: RA, PATH: `${process.env.HOME}/.cargo/bin:${process.env.PATH}` };

const clean = () => { try { execSync('pkill -f "code-intel-mcp --daemon"'); } catch {} try { execSync('rm -f /tmp/code-intel-mcp*.sock'); } catch {} };

function session() {
  return new Promise((resolve) => {
    const child = spawn(MCP, [], { env, stdio: ['pipe', 'pipe', 'ignore'] });
    const rl = createInterface({ input: child.stdout });
    const pending = new Map();
    rl.on('line', (l) => { let m; try { m = JSON.parse(l); } catch { return; } if (m.id && pending.has(m.id)) { pending.get(m.id)(m); pending.delete(m.id); } });
    let idc = 0;
    const call = (method, params) => new Promise((res) => { const id = ++idc; pending.set(id, res); child.stdin.write(JSON.stringify({ jsonrpc: '2.0', id, method, params }) + '\n'); });
    (async () => {
      await call('initialize', {});
      const t = Date.now();
      const r = await call('tools/call', { name: 'find_references', arguments: { project: PROJECT, file: 'src/models.rs', symbol: 'Account' } });
      const ms = Date.now() - t;
      let count = -1; try { count = JSON.parse(r.result.content[0].text).count; } catch {}
      child.kill('SIGKILL'); // encerra o MCP (NÃO o daemon)
      resolve({ count, ms });
    })();
  });
}

clean();
await new Promise((r) => setTimeout(r, 500));
console.log('SESSÃO 1 (sobe daemon + rust-analyzer COLD)...');
const s1 = await session();
console.log(`  count=${s1.count}  tempo=${(s1.ms / 1000).toFixed(1)}s`);
await new Promise((r) => setTimeout(r, 500));
console.log('SESSÃO 2 (novo MCP, reconecta ao daemon QUENTE)...');
const s2 = await session();
console.log(`  count=${s2.count}  tempo=${(s2.ms / 1000).toFixed(1)}s`);
const daemonAlive = (() => { try { execSync('pgrep -f "code-intel-mcp --daemon"'); return true; } catch { return false; } })();
console.log(`\ndaemon vivo entre sessões: ${daemonAlive ? 'sim' : 'não'}`);
if (s1.count === s2.count && s2.ms < s1.ms / 3) console.log(`✅ CACHE OK: sessão 2 ${(s1.ms / s2.ms).toFixed(0)}x mais rápida, mesmo resultado (${s2.count})`);
else console.log('resultado:', s1, s2);
clean();
console.log('(daemon encerrado)');
