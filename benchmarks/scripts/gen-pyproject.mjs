// Gera um projeto Python sintético para benchmark/testes do basedpyright, com um símbolo-alvo
// (classe Account) referenciado por vários módulos, com contagem CONHECIDA de referências.
// Espelha o gen-monorepo.mjs (TS) para manter o padrão entre linguagens.
//
// Uso: node gen-pyproject.mjs [--modules 20] [--refs 8] [--out ../../fixtures/py-demo]
import { mkdirSync, writeFileSync, rmSync } from 'node:fs';
import { fileURLToPath } from 'node:url';
import { dirname, resolve, join } from 'node:path';

const __dirname = dirname(fileURLToPath(import.meta.url));
const args = Object.fromEntries(process.argv.slice(2).reduce((a, x, i, arr) => {
  if (x.startsWith('--')) a.push([x.slice(2), arr[i + 1]]); return a;
}, []));
const N = +(args.modules ?? 20);
const REFS = +(args.refs ?? 8);
const OUT = resolve(__dirname, args.out ?? '../../fixtures/py-demo');

rmSync(OUT, { recursive: true, force: true });
mkdirSync(OUT, { recursive: true });
const w = (p, s) => { mkdirSync(dirname(p), { recursive: true }); writeFileSync(p, s); };

// --- módulo core: define o símbolo-alvo Account -------------------------
w(join(OUT, 'models.py'),
`class Account:
    def __init__(self) -> None:
        self.value = 0

    def balance(self) -> int:
        return self.value


def make_account() -> Account:
    return Account()
`);

// --- módulos consumidores: cada um referencia Account REFS vezes --------
let total = 0;
for (let i = 0; i < N; i++) {
  const name = `svc_${String(i).padStart(2, '0')}`;
  const lines = [`from models import Account, make_account`, ''];
  total += 1; // o import de Account conta como referência
  for (let r = 0; r < REFS; r++) {
    // 2 referências a Account por função: anotação de tipo + construtor
    lines.push(`def ${name}_${r}() -> Account:`);
    lines.push(`    a: Account = Account()`);
    lines.push(`    return a`);
    lines.push('');
    total += 3;
  }
  lines.push(`def ${name}_use() -> Account:`);
  lines.push(`    return make_account()`);
  lines.push('');
  total += 1; // anotação de retorno
  w(join(OUT, `${name}.py`), lines.join('\n'));
}

// pyright resolve imports pela raiz do projeto
w(join(OUT, 'pyrightconfig.json'), JSON.stringify({ include: ['.'], typeCheckingMode: 'basic' }, null, 2));

const expected = total + 1; // + a declaração da classe (includeDeclaration)
w(join(OUT, 'EXPECTED.json'), JSON.stringify({
  modules: N, refsPerModule: REFS, targetSymbol: 'Account', targetFile: 'models.py',
  expectedReferences: expected,
  note: 'find_references(includeDeclaration=true) deve convergir para expectedReferences com o índice completo.',
}, null, 2));

console.log(`projeto Python gerado em ${OUT}`);
console.log(`  ${N} módulos × ${REFS} refs + core`);
console.log(`  referências esperadas a Account (com declaração): ${expected}`);
