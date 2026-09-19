// A/B de PRECISÃO DE REFERÊNCIAS (base de "é seguro deletar?" / análise de impacto):
// "quantas referências a CLASSE Widget existem?" — grep (texto) vs find_references (semântico).
// O grep conta demais (strings, comentários, const homônima, substrings WidgetFactory/useWidget);
// o semântico conta só as referências reais da classe.
import { spawn, execSync } from 'node:child_process';
import { join } from 'node:path';
import { createInterface } from 'node:readline';

const HOME = process.env.HOME;
const HARNESS = import.meta.dirname;
const FIXTURE = join(HOME, 'Projects/ide_cc/fixtures/ab-rename');
const MCP = join(HOME, 'Projects/ide_cc/mcp/target/release/code-intel-mcp');
const TSGO = join(HARNESS, 'node_modules/.bin/tsgo');

// ground-truth: referências REAIS à classe Widget (decl + tipos + new + import), contadas à mão
const GROUND_TRUTH = 8;

const grepCount = (pat) => {
  try { return parseInt(execSync(`grep -roE ${pat} ${FIXTURE}/src | wc -l`).toString().trim(), 10); }
  catch { return 0; }
};
const grepRaw = grepCount("'Widget'");        // substring: pega WidgetFactory, useWidget também
const grepWord = grepCount("'\\bWidget\\b'"); // palavra: exclui substrings, mas ainda pega string/comentário/const

function semanticCount() {
  return new Promise((resolve) => {
    const child = spawn(MCP, [], { env: { ...process.env, TSGO_BIN: TSGO }, stdio: ['pipe', 'pipe', 'ignore'] });
    const rl = createInterface({ input: child.stdout });
    const pending = new Map();
    rl.on('line', (l) => { let m; try { m = JSON.parse(l); } catch { return; } if (m.id && pending.has(m.id)) { pending.get(m.id)(m); pending.delete(m.id); } });
    let idc = 0;
    const call = (method, params) => new Promise((res) => { const id = ++idc; pending.set(id, res); child.stdin.write(JSON.stringify({ jsonrpc: '2.0', id, method, params }) + '\n'); });
    (async () => {
      await call('initialize', {});
      const r = await call('tools/call', { name: 'find_references', arguments: { project: FIXTURE, file: 'src/widget.ts', symbol: 'Widget', line: 2 } });
      let count = -1; try { count = JSON.parse(r.result.content[0].text).count; } catch {}
      child.kill('SIGKILL');
      resolve(count);
    })();
  });
}

console.log('=== A/B: "quantas referências à CLASSE Widget?" (base de delete/impacto) ===\n');
console.log(`ground-truth (referências REAIS da classe): ${GROUND_TRUTH}\n`);
console.log(`grep "Widget"      (substring):  ${grepRaw}   -> conta WidgetFactory, useWidget, strings, comentário, const...`);
console.log(`grep "\\bWidget\\b"  (palavra):    ${grepWord}   -> ainda conta strings, comentário, e a const NÃO-relacionada`);
const sem = await semanticCount();
console.log(`find_references    (semântico):  ${sem}   -> só as referências reais da classe`);
console.log(`\nVEREDITO: um agente que decide "é seguro deletar?" pelo grep vê ${grepWord}-${grepRaw} usos (errado);`);
console.log(`o semântico vê ${sem} (= ground-truth ${GROUND_TRUTH}: ${sem === GROUND_TRUTH ? 'exato ✅' : 'divergiu'}).`);
