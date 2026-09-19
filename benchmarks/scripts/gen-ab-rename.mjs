// Gera o fixture-ARMADILHA do experimento A/B: uma tarefa de "renomear a CLASSE Widget para
// Gadget" onde o rename por TEXTO (grep/sed) corrompe coisas que o rename SEMÂNTICO preserva.
//
// Armadilhas (o texto-cru erra, o semântico acerta):
//   - um símbolo NÃO-relacionado também chamado `Widget` (uma const em outro módulo)
//   - strings literais "Widget"
//   - um comentário mencionando Widget
//   - `WidgetFactory` (substring)
//
// Uso: node gen-ab-rename.mjs [--out ../../fixtures/ab-rename]
import { mkdirSync, writeFileSync, rmSync } from 'node:fs';
import { fileURLToPath } from 'node:url';
import { dirname, resolve, join } from 'node:path';

const __dirname = dirname(fileURLToPath(import.meta.url));
const args = Object.fromEntries(process.argv.slice(2).reduce((a, x, i, arr) => {
  if (x.startsWith('--')) a.push([x.slice(2), arr[i + 1]]); return a;
}, []));
const OUT = resolve(__dirname, args.out ?? '../../fixtures/ab-rename');
rmSync(OUT, { recursive: true, force: true });
mkdirSync(join(OUT, 'src'), { recursive: true });
const w = (p, s) => { mkdirSync(dirname(p), { recursive: true }); writeFileSync(p, s); };

w(join(OUT, 'tsconfig.json'), JSON.stringify({ compilerOptions: { strict: true, noEmit: true, module: 'esnext', moduleResolution: 'bundler', target: 'es2020' }, include: ['src'] }, null, 2));

// A CLASSE alvo + uma classe com substring + uma string literal + um comentário.
w(join(OUT, 'src/widget.ts'),
`// Widget is the core UI element in this app.
export class Widget {
  id = 0;
  render(): string {
    return "Widget";
  }
}

export class WidgetFactory {
  create(): Widget {
    return new Widget();
  }
}
`);

// Usos reais da classe + armadilhas (WidgetFactory, string, nome de função useWidget).
w(join(OUT, 'src/app.ts'),
`import { Widget, WidgetFactory } from "./widget";

const w: Widget = new Widget();
const factory = new WidgetFactory();
const label: string = "Widget";

export function useWidget(x: Widget): Widget {
  return x;
}

console.log(w, factory, label, useWidget(w));
`);

// Um símbolo TOTALMENTE não-relacionado, também chamado Widget (uma constante). O rename da
// CLASSE não deve tocar nisto; o grep/sed toca.
w(join(OUT, 'src/other.ts'),
`// An UNRELATED constant that just happens to be named Widget.
export const Widget = 42;

export function widgetTotal(): number {
  return Widget + 1;
}
`);

w(join(OUT, 'GROUND_TRUTH.json'), JSON.stringify({
  task: 'renomear a CLASSE Widget (src/widget.ts) para Gadget',
  must_change: ['class Widget -> class Gadget e suas referências (import, tipos, new)'],
  must_NOT_change: [
    'const Widget em src/other.ts (símbolo não-relacionado)',
    'strings "Widget" (src/widget.ts render, src/app.ts label)',
    'WidgetFactory (substring)',
    'comentário "// Widget is the core..."',
  ],
  checks_after_correct_rename: {
    'class Gadget': 1,
    'WidgetFactory (intacto)': 3,
    'string "Widget" (intacto)': 2,
    'export const Widget em other.ts (intacto)': 1,
    'build tsc --noEmit': 'clean',
  },
}, null, 2));

console.log(`fixture-armadilha A/B gerado em ${OUT}`);
