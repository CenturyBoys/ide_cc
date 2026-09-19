// Gera um crate Rust sintético para benchmark/testes do rust-analyzer, com um símbolo-alvo
// (struct Account) referenciado por vários módulos. Espelha os geradores TS/Python/Dart (padrão).
//
// Uso: node gen-rustproject.mjs [--modules 20] [--refs 8] [--out ../../fixtures/rust-demo]
import { mkdirSync, writeFileSync, rmSync } from 'node:fs';
import { fileURLToPath } from 'node:url';
import { dirname, resolve, join } from 'node:path';

const __dirname = dirname(fileURLToPath(import.meta.url));
const args = Object.fromEntries(process.argv.slice(2).reduce((a, x, i, arr) => {
  if (x.startsWith('--')) a.push([x.slice(2), arr[i + 1]]); return a;
}, []));
const N = +(args.modules ?? 20);
const REFS = +(args.refs ?? 8);
const OUT = resolve(__dirname, args.out ?? '../../fixtures/rust-demo');

rmSync(OUT, { recursive: true, force: true });
mkdirSync(join(OUT, 'src'), { recursive: true });
const w = (p, s) => { mkdirSync(dirname(p), { recursive: true }); writeFileSync(p, s); };

w(join(OUT, 'Cargo.toml'),
`[package]
name = "rustdemo"
version = "0.0.0"
edition = "2021"

[lib]
path = "src/lib.rs"
`);

// --- src/models.rs: define o símbolo-alvo Account -----------------------
w(join(OUT, 'src/models.rs'),
`pub struct Account {
    pub value: i64,
}

impl Account {
    pub fn balance(&self) -> i64 {
        self.value
    }
}

pub fn make_account() -> Account {
    Account { value: 0 }
}
`);

// --- src/svc_NN.rs: cada um referencia Account REFS vezes ---------------
const mods = ['models'];
let total = 3; // models.rs: struct decl + retorno de make_account + literal Account{}
for (let i = 0; i < N; i++) {
  const name = `svc_${String(i).padStart(2, '0')}`;
  const lines = [`use crate::models::{Account, make_account};`, ''];
  total += 1; // o `use ... Account` conta como referência
  for (let r = 0; r < REFS; r++) {
    // 3 referências a Account por função: retorno + tipo da var + literal
    lines.push(`pub fn ${name}_${r}() -> Account {`);
    lines.push(`    let a: Account = Account { value: 0 };`);
    lines.push(`    a`);
    lines.push(`}`);
    lines.push('');
    total += 3;
  }
  lines.push(`pub fn ${name}_use() -> Account {`);
  lines.push(`    make_account()`);
  lines.push(`}`);
  lines.push('');
  total += 1; // retorno
  w(join(OUT, `src/${name}.rs`), lines.join('\n'));
  mods.push(name);
}

// --- src/lib.rs: declara os módulos -------------------------------------
w(join(OUT, 'src/lib.rs'), mods.map((m) => `pub mod ${m};`).join('\n') + '\n');

w(join(OUT, 'EXPECTED.json'), JSON.stringify({
  modules: N, refsPerModule: REFS, targetSymbol: 'Account', targetFile: 'src/models.rs',
  expectedReferences: total,
  note: 'Aproximado; o benchmark mede o valor estavel do rust-analyzer.',
}, null, 2));

console.log(`crate Rust gerado em ${OUT}`);
console.log(`  ${N} módulos × ${REFS} refs + core`);
console.log(`  referências aproximadas a Account: ${total}`);
