// Gera um monorepo TypeScript SINTÉTICO com project references, projetado para
// reproduzir o truncamento silencioso do find_references (cold-index race, #76870).
//
// Por que sintético em vez de clonar um OSS: precisamos de um número EXATO e conhecido
// de referências cross-package para medir "quantas o server viu na 1ª resposta vs. no total".
// Um monorepo real não dá esse ground-truth controlado.
//
// Estrutura:
//   packages/core/src/index.ts   -> export class Widget {}   (o símbolo-alvo)
//   packages/pNN/src/index.ts     -> import { Widget }; usa Widget REFS_PER_PKG vezes
//   tsconfig.base.json (paths @mono/* -> packages/*/src)
//   tsconfig.json (root, references a todos os pacotes)  => composite/project refs
//
// Uso: node gen-monorepo.mjs [--packages 20] [--refs 10] [--out ../../fixtures/mono-ts]
import { mkdirSync, writeFileSync, rmSync } from 'node:fs';
import { fileURLToPath } from 'node:url';
import { dirname, resolve, join } from 'node:path';

const __dirname = dirname(fileURLToPath(import.meta.url));
const args = Object.fromEntries(process.argv.slice(2).reduce((a, x, i, arr) => {
  if (x.startsWith('--')) a.push([x.slice(2), arr[i + 1]]); return a;
}, []));
const N_PKGS = +(args.packages ?? 20);
const REFS = +(args.refs ?? 10);
const OUT = resolve(__dirname, args.out ?? '../../fixtures/mono-ts');

rmSync(OUT, { recursive: true, force: true });
mkdirSync(join(OUT, 'packages'), { recursive: true });

const w = (p, s) => { mkdirSync(dirname(p), { recursive: true }); writeFileSync(p, s); };

// --- pacote core: define o símbolo-alvo Widget --------------------------
w(join(OUT, 'packages/core/src/index.ts'),
`export class Widget {
  id = 0;
  render(): string { return 'widget'; }
}
export function makeWidget(): Widget { return new Widget(); }
`);
w(join(OUT, 'packages/core/package.json'),
  JSON.stringify({ name: '@mono/core', version: '0.0.0', types: 'src/index.ts' }, null, 2));
w(join(OUT, 'packages/core/tsconfig.json'), JSON.stringify({
  extends: '../../tsconfig.base.json',
  compilerOptions: { composite: true, rootDir: 'src', outDir: 'dist' },
  include: ['src'],
}, null, 2));

// --- pacotes consumidores: cada um referencia Widget REFS vezes ---------
const refPaths = [{ path: 'packages/core' }];
let totalRefs = 0;
for (let i = 0; i < N_PKGS; i++) {
  const name = `p${String(i).padStart(2, '0')}`;
  const lines = [`import { Widget, makeWidget } from '@mono/core';`, ''];
  for (let r = 0; r < REFS; r++) {
    // cada linha gera 2 referências a Widget (anotação de tipo + construtor)
    lines.push(`export const ${name}_w${r}: Widget = new Widget();`);
    totalRefs += 2;
  }
  lines.push(`export function ${name}_use(): Widget { return makeWidget(); }`);
  totalRefs += 1; // anotação de retorno
  w(join(OUT, `packages/${name}/src/index.ts`), lines.join('\n') + '\n');
  w(join(OUT, `packages/${name}/package.json`),
    JSON.stringify({ name: `@mono/${name}`, version: '0.0.0', types: 'src/index.ts' }, null, 2));
  w(join(OUT, `packages/${name}/tsconfig.json`), JSON.stringify({
    extends: '../../tsconfig.base.json',
    compilerOptions: { composite: true, rootDir: 'src', outDir: 'dist' },
    include: ['src'],
    references: [{ path: '../core' }],
  }, null, 2));
  refPaths.push({ path: `packages/${name}` });
}

// --- tsconfig base (resolução via paths) + root (project references) ----
w(join(OUT, 'tsconfig.base.json'), JSON.stringify({
  compilerOptions: {
    target: 'ES2020', module: 'NodeNext', moduleResolution: 'NodeNext',
    declaration: true, strict: true,
    baseUrl: '.', paths: { '@mono/core': ['packages/core/src'], '@mono/*': ['packages/*/src'] },
  },
}, null, 2));
w(join(OUT, 'tsconfig.json'), JSON.stringify({
  files: [], references: refPaths,
}, null, 2));
w(join(OUT, 'package.json'),
  JSON.stringify({ name: 'mono-ts-fixture', private: true, workspaces: ['packages/*'] }, null, 2));

// definição de Widget conta como 1 "referência" com includeDeclaration
const expected = totalRefs + 1;
w(join(OUT, 'EXPECTED.json'), JSON.stringify({
  packages: N_PKGS, refsPerPkg: REFS, targetSymbol: 'Widget',
  targetFile: 'packages/core/src/index.ts',
  expectedReferences: expected,
  note: 'expectedReferences inclui a declaração; find_references(includeDeclaration=true) deve convergir para este número quando o índice estiver completo.',
}, null, 2));

console.log(`monorepo gerado em ${OUT}`);
console.log(`  ${N_PKGS} pacotes consumidores × ${REFS} refs + core`);
console.log(`  referências esperadas a Widget (com declaração): ${expected}`);
