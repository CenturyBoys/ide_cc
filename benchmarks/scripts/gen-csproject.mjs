// Gera um projeto C# sintético para benchmark/testes do LSP C# (csharp-ls / Roslyn), com um
// símbolo-alvo (classe Account) referenciado por vários arquivos. Espelha os outros geradores.
//
// Uso: node gen-csproject.mjs [--modules 20] [--refs 8] [--tfm net8.0] [--out ../../fixtures/cs-demo]
import { mkdirSync, writeFileSync, rmSync } from 'node:fs';
import { fileURLToPath } from 'node:url';
import { dirname, resolve, join } from 'node:path';

const __dirname = dirname(fileURLToPath(import.meta.url));
const args = Object.fromEntries(process.argv.slice(2).reduce((a, x, i, arr) => {
  if (x.startsWith('--')) a.push([x.slice(2), arr[i + 1]]); return a;
}, []));
const N = +(args.modules ?? 20);
const REFS = +(args.refs ?? 8);
const TFM = args.tfm ?? 'net8.0';
const OUT = resolve(__dirname, args.out ?? '../../fixtures/cs-demo');

rmSync(OUT, { recursive: true, force: true });
mkdirSync(OUT, { recursive: true });
const w = (p, s) => { mkdirSync(dirname(p), { recursive: true }); writeFileSync(p, s); };

w(join(OUT, 'csdemo.csproj'),
`<Project Sdk="Microsoft.NET.Sdk">
  <PropertyGroup>
    <TargetFramework>${TFM}</TargetFramework>
    <Nullable>disable</Nullable>
    <ImplicitUsings>disable</ImplicitUsings>
    <OutputType>Library</OutputType>
  </PropertyGroup>
</Project>
`);

// --- Models.cs: define o símbolo-alvo Account ---------------------------
w(join(OUT, 'Models.cs'),
`namespace CsDemo;

public class Account
{
    public int Value;
    public int Balance() => Value;
}

public static class Factory
{
    public static Account MakeAccount() => new Account();
}
`);

// --- SvcNN.cs: cada um referencia Account REFS vezes --------------------
let total = 3; // Models.cs: decl da classe + retorno de MakeAccount + new Account()
for (let i = 0; i < N; i++) {
  const name = `Svc${String(i).padStart(2, '0')}`;
  const lines = [`namespace CsDemo;`, '', `public static class ${name}`, `{`];
  for (let r = 0; r < REFS; r++) {
    // 3 referências a Account por método: retorno + tipo da var + construtor
    lines.push(`    public static Account M${r}()`);
    lines.push(`    {`);
    lines.push(`        Account a = new Account();`);
    lines.push(`        return a;`);
    lines.push(`    }`);
    total += 3;
  }
  lines.push(`    public static Account Use() => Factory.MakeAccount();`);
  total += 1; // retorno
  lines.push(`}`);
  w(join(OUT, `${name}.cs`), lines.join('\n') + '\n');
}

w(join(OUT, 'EXPECTED.json'), JSON.stringify({
  modules: N, refsPerModule: REFS, targetSymbol: 'Account', targetFile: 'Models.cs',
  tfm: TFM, expectedReferences: total,
  note: 'Aproximado; o benchmark mede o valor estavel do LSP C#.',
}, null, 2));

console.log(`projeto C# gerado em ${OUT} (TFM ${TFM})`);
console.log(`  ${N} módulos × ${REFS} refs + core`);
console.log(`  referências aproximadas a Account: ${total}`);
