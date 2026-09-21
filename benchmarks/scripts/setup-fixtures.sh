#!/usr/bin/env bash
# Baixa os fixtures dos benchmarks em versões FIXADAS (reprodutibilidade).
# Uso: bash benchmarks/scripts/setup-fixtures.sh
set -euo pipefail
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
FIX="$ROOT/fixtures"
mkdir -p "$FIX"

# --- zod v3.23.8 (fixture TS médio: ~15k LOC, 82 arquivos) -------------
if [ ! -d "$FIX/zod" ]; then
  echo ">> clonando zod v3.23.8..."
  git clone --depth 1 --branch v3.23.8 https://github.com/colinhacks/zod.git "$FIX/zod"
else
  echo ">> zod já presente ($(cd "$FIX/zod" && git describe --tags 2>/dev/null || echo '?'))"
fi

# --- monorepo TS sintético (reproduz o truncamento silencioso #76870) --
echo ">> gerando monorepo sintético (25 pacotes)..."
node "$ROOT/benchmarks/scripts/gen-monorepo.mjs" --packages 25 --refs 12 --out "$FIX/mono-ts"

# --- fixture de refactoring (extract/move) ------------------------------
echo ">> gerando fixture de refactoring..."
mkdir -p "$FIX/refactor-ts/src"
cat > "$FIX/refactor-ts/tsconfig.json" <<'EOF'
{
  "compilerOptions": {
    "target": "ES2020", "module": "NodeNext", "moduleResolution": "NodeNext",
    "strict": true, "declaration": true
  },
  "include": ["src"]
}
EOF
cat > "$FIX/refactor-ts/src/main.ts" <<'EOF'
export function compute(a: number, b: number): number {
  const x = a + b;
  const y = x * 2;
  const z = y - a;
  return z;
}

export const K = 10;

export function useCompute(): number {
  return compute(K, 2);
}
EOF

# --- fixture organize_imports (F2): type-only + side-effect import -------
echo ">> gerando fixture organize_imports (F2)..."
mkdir -p "$FIX/organize-ts/src"
cat > "$FIX/organize-ts/tsconfig.json" <<'EOF'
{
  "compilerOptions": {
    "target": "ES2020", "module": "NodeNext", "moduleResolution": "NodeNext",
    "strict": true, "verbatimModuleSyntax": true, "declaration": true
  },
  "include": ["src"]
}
EOF
cat > "$FIX/organize-ts/src/types.ts" <<'EOF'
export type Fruit = { name: string };
export type Veggie = { color: string };
export function unusedExport(): number {
  return 1;
}
EOF
cat > "$FIX/organize-ts/src/polyfill.ts" <<'EOF'
// side-effect only module: importing it must run this code; it has no exports to "use".
(globalThis as unknown as { __poly?: boolean }).__poly = true;
EOF
cat > "$FIX/organize-ts/src/main.ts" <<'EOF'
import "./polyfill"; // side-effect import: MUST survive organizeImports (no textual removal)
import type { Fruit } from "./types"; // type-only, USED below → must survive

export function label(f: Fruit): string {
  return f.name;
}
EOF

# --- fixture safe_delete (F3): 0-ref (deleta) vs referenciado (recusa) ---
echo ">> gerando fixture safe_delete (F3)..."
mkdir -p "$FIX/safe-delete-ts/src"
cat > "$FIX/safe-delete-ts/tsconfig.json" <<'EOF'
{
  "compilerOptions": {
    "target": "ES2020", "module": "NodeNext", "moduleResolution": "NodeNext",
    "strict": true, "declaration": true
  },
  "include": ["src"]
}
EOF
cat > "$FIX/safe-delete-ts/src/main.ts" <<'EOF'
// usedHelper is referenced by consumer() below → safe_delete must REFUSE with locations.
export function usedHelper(a: number): number {
  return a * 2;
}

// unusedHelper has ZERO references outside its own definition → safe_delete deletes it.
export function unusedHelper(a: number): number {
  return a + 1;
}

export function consumer(): number {
  return usedHelper(21);
}
EOF

# --- fixture F4/F6 (edições por símbolo + blast_radius): callers em test e prod ---
echo ">> gerando fixture F4/F6 (symbol-edits + blast_radius)..."
mkdir -p "$FIX/symbol-edit-ts/src" "$FIX/symbol-edit-ts/test"
cat > "$FIX/symbol-edit-ts/tsconfig.json" <<'EOF'
{
  "compilerOptions": {
    "target": "ES2020", "module": "NodeNext", "moduleResolution": "NodeNext",
    "strict": true, "declaration": true
  },
  "include": ["src", "test"]
}
EOF
# core.ts: 'compute' é chamado por prod (src/app.ts) E por teste (test/core.test.ts) → blast_radius
# deve particionar. 'greet' é alvo das edições por símbolo F4 (replace/insert).
cat > "$FIX/symbol-edit-ts/src/core.ts" <<'EOF'
export function compute(a: number, b: number): number {
  return a + b;
}

export function greet(name: string): string {
  return "hi " + name;
}
EOF
cat > "$FIX/symbol-edit-ts/src/app.ts" <<'EOF'
import { compute } from "./core"; // caller de PRODUÇÃO

export function total(): number {
  return compute(2, 3);
}
EOF
cat > "$FIX/symbol-edit-ts/test/core.test.ts" <<'EOF'
import { compute } from "../src/core"; // caller de TESTE

export function checkCompute(): boolean {
  return compute(1, 1) === 2;
}
EOF

# --- fixture F7 (quick_fix): diagnóstico corrigível (declaração não-usada) + caso sem fix ---
echo ">> gerando fixture F7 (quick_fix)..."
mkdir -p "$FIX/quickfix-ts/src"
cat > "$FIX/quickfix-ts/tsconfig.json" <<'EOF'
{
  "compilerOptions": {
    "target": "ES2020", "module": "NodeNext", "moduleResolution": "NodeNext",
    "strict": true, "noUnusedLocals": true, "declaration": true
  },
  "include": ["src"]
}
EOF
# 'unusedLocal' na linha 2 dispara um diagnóstico com quickfix "Remove unused declaration".
# 'clean' (linha 6) NÃO tem diagnóstico → quick_fix deve retornar none/unsupported.
cat > "$FIX/quickfix-ts/src/main.ts" <<'EOF'
export function withUnused(): number {
  const unusedLocal = 42;
  return 1;
}

export function clean(): number {
  return 7;
}
EOF

# --- fixture F5 (change_signature): função chamada em VÁRIOS arquivos -------------------------
echo ">> gerando fixture F5 (change_signature)..."
mkdir -p "$FIX/change-sig-ts/src"
cat > "$FIX/change-sig-ts/tsconfig.json" <<'EOF'
{
  "compilerOptions": {
    "target": "ES2020", "module": "NodeNext", "moduleResolution": "NodeNext",
    "strict": true, "declaration": true
  },
  "include": ["src"]
}
EOF
# 'compute(a, b)' declarado em core.ts e chamado por dois arquivos (a.ts, b.ts). change_signature
# deve reescrever a declaração E ambos os call-sites JUNTOS. reorder [1,0] só troca a ordem dos
# params na decl e dos args nos callers → net_delta<=0 (tipos iguais). Uma spec que quebre a aridade
# (ex.: remove o 2º param sem ajustar o corpo, que usa 'b') introduz erro → change_signature RECUSA.
cat > "$FIX/change-sig-ts/src/core.ts" <<'EOF'
export function compute(a: number, b: number): number {
  return a + b;
}
EOF
cat > "$FIX/change-sig-ts/src/a.ts" <<'EOF'
import { compute } from "./core";
export const ra = compute(1, 2);
EOF
cat > "$FIX/change-sig-ts/src/b.ts" <<'EOF'
import { compute } from "./core";
export const rb = compute(10, 20);
EOF

# --- fixture F8 (move_file): módulo importado por outro arquivo -------------------------------
echo ">> gerando fixture F8 (move_file)..."
mkdir -p "$FIX/move-file-ts/src/util"
cat > "$FIX/move-file-ts/tsconfig.json" <<'EOF'
{
  "compilerOptions": {
    "target": "ES2020", "module": "NodeNext", "moduleResolution": "NodeNext",
    "strict": true, "declaration": true
  },
  "include": ["src"]
}
EOF
# helper.ts exporta 'twice'; consumer.ts o importa por caminho relativo. move_file de
# src/helper.ts -> src/util/helper.ts deve reescrever o import em consumer.ts (./helper -> ./util/helper).
cat > "$FIX/move-file-ts/src/helper.ts" <<'EOF'
export function twice(n: number): number {
  return n * 2;
}
EOF
cat > "$FIX/move-file-ts/src/consumer.ts" <<'EOF'
import { twice } from "./helper";
export const four = twice(2);
EOF

# --- projeto Python sintético (Fase 4: basedpyright) --------------------
echo ">> gerando projeto Python sintético (20 módulos)..."
node "$ROOT/benchmarks/scripts/gen-pyproject.mjs" --modules 20 --refs 8 --out "$FIX/py-demo"

# --- pacote Dart sintético (Fase 4: Dart Analysis Server) ---------------
echo ">> gerando pacote Dart sintético (20 módulos)..."
node "$ROOT/benchmarks/scripts/gen-dartproject.mjs" --modules 20 --refs 8 --out "$FIX/dart-demo"
if command -v dart >/dev/null 2>&1; then
  (cd "$FIX/dart-demo" && dart pub get >/dev/null 2>&1) && echo "   dart pub get ok"
else
  echo "   (dart ausente; rode 'dart pub get' em $FIX/dart-demo antes de usar)"
fi

# --- crate Rust sintético (Fase 4: rust-analyzer) -----------------------
echo ">> gerando crate Rust sintético (20 módulos)..."
node "$ROOT/benchmarks/scripts/gen-rustproject.mjs" --modules 20 --refs 8 --out "$FIX/rust-demo"

# --- projeto C# sintético (Fase 4: csharp-ls/Roslyn) --------------------
CS_TFM="${CS_TFM:-net10.0}"
echo ">> gerando projeto C# sintético (20 módulos, TFM $CS_TFM)..."
node "$ROOT/benchmarks/scripts/gen-csproject.mjs" --modules 20 --refs 8 --tfm "$CS_TFM" --out "$FIX/cs-demo"
echo "   (C# requer .NET SDK + 'dotnet tool install --global csharp-ls'; ajuste CS_TFM ao SDK instalado)"

# --- fixture-armadilha do experimento A/B (rename semântico vs texto) ---
echo ">> gerando fixture-armadilha A/B..."
node "$ROOT/benchmarks/scripts/gen-ab-rename.mjs" --out "$FIX/ab-rename"

echo ">> fixtures prontos em $FIX"
