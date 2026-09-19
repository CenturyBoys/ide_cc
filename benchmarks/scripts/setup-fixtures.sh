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

# --- projeto Python sintético (Fase 4: basedpyright) --------------------
echo ">> gerando projeto Python sintético (20 módulos)..."
node "$ROOT/benchmarks/scripts/gen-pyproject.mjs" --modules 20 --refs 8 --out "$FIX/py-demo"

echo ">> fixtures prontos em $FIX"
