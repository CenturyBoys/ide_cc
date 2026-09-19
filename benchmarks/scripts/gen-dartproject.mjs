// Gera um pacote Dart sintético para benchmark/testes do Dart Analysis Server, com um símbolo-alvo
// (classe Account) referenciado por vários arquivos. Espelha os geradores TS/Python (padrão).
//
// Uso: node gen-dartproject.mjs [--modules 20] [--refs 8] [--out ../../fixtures/dart-demo]
// Depois: `dart pub get` no diretório (gera .dart_tool/package_config.json).
import { mkdirSync, writeFileSync, rmSync } from 'node:fs';
import { fileURLToPath } from 'node:url';
import { dirname, resolve, join } from 'node:path';

const __dirname = dirname(fileURLToPath(import.meta.url));
const args = Object.fromEntries(process.argv.slice(2).reduce((a, x, i, arr) => {
  if (x.startsWith('--')) a.push([x.slice(2), arr[i + 1]]); return a;
}, []));
const N = +(args.modules ?? 20);
const REFS = +(args.refs ?? 8);
const OUT = resolve(__dirname, args.out ?? '../../fixtures/dart-demo');

rmSync(OUT, { recursive: true, force: true });
mkdirSync(join(OUT, 'lib'), { recursive: true });
const w = (p, s) => { mkdirSync(dirname(p), { recursive: true }); writeFileSync(p, s); };

w(join(OUT, 'pubspec.yaml'),
`name: dartdemo
environment:
  sdk: '>=3.0.0 <4.0.0'
`);

// --- lib/models.dart: define o símbolo-alvo Account ---------------------
w(join(OUT, 'lib/models.dart'),
`class Account {
  int value = 0;
  int balance() => value;
}

Account makeAccount() => Account();
`);

// --- lib/svc_NN.dart: cada um referencia Account REFS vezes -------------
let total = 3; // models.dart: classe (decl) + retorno de makeAccount + Account() no corpo
for (let i = 0; i < N; i++) {
  const name = `svc_${String(i).padStart(2, '0')}`;
  const lines = [`import 'models.dart';`, ''];
  for (let r = 0; r < REFS; r++) {
    // 3 referências a Account por função: retorno + tipo da var + construtor
    lines.push(`Account ${name}_${r}() {`);
    lines.push(`  Account a = Account();`);
    lines.push(`  return a;`);
    lines.push(`}`);
    lines.push('');
    total += 3;
  }
  lines.push(`Account ${name}_use() => makeAccount();`);
  lines.push('');
  total += 1; // retorno
  w(join(OUT, `lib/${name}.dart`), lines.join('\n'));
}

w(join(OUT, 'EXPECTED.json'), JSON.stringify({
  modules: N, refsPerModule: REFS, targetSymbol: 'Account', targetFile: 'lib/models.dart',
  expectedReferences: total,
  note: 'Aproximado; imports de arquivo NAO contam como referencia ao simbolo em Dart. O benchmark mede o valor estavel.',
}, null, 2));

console.log(`pacote Dart gerado em ${OUT}`);
console.log(`  ${N} módulos × ${REFS} refs + core`);
console.log(`  referências aproximadas a Account: ${total}`);
console.log(`  >>> rode: (cd ${OUT} && dart pub get)`);
