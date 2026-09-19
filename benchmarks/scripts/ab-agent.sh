#!/usr/bin/env bash
# A/B de AGENTE (Nível 2): roda o Claude Code real na mesma tarefa de rename, COM e SEM o MCP
# code-intel, e avalia a correção do resultado. Requer o CLI `claude` autenticado.
#
# Uso: bash benchmarks/scripts/ab-agent.sh
set -uo pipefail
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
FIX="$ROOT/fixtures/ab-rename"
MCP="$ROOT/mcp/target/release/code-intel-mcp"
TSGO="$ROOT/benchmarks/harness/node_modules/.bin/tsgo"
TSC="$ROOT/benchmarks/harness/node_modules/.bin/tsc"
TASK='Renomeie a CLASSE Widget para Gadget neste projeto. Não altere strings, comentários, a classe WidgetFactory nem a constante Widget não-relacionada em src/other.ts.'

[ -d "$FIX" ] || { echo "gere o fixture: bash benchmarks/scripts/setup-fixtures.sh"; exit 1; }
command -v claude >/dev/null || { echo "CLI 'claude' não encontrado no PATH"; exit 1; }

# avalia a correção de um diretório (mesmo critério do A/B mecânico)
evaluate() {
  local d="$1"
  local cg wf sw cw
  cg=$(grep -rho 'class Gadget' "$d/src" | wc -l)
  wf=$(grep -rho 'WidgetFactory' "$d/src" | wc -l)
  sw=$(grep -rho '"Widget"' "$d/src" | wc -l)
  cw=$(grep -c 'export const Widget' "$d/src/other.ts")
  local build=NO; ( cd "$d" && "$TSC" --noEmit -p tsconfig.json >/dev/null 2>&1 ) && build=OK
  echo "  class Gadget=$cg (quer 1) | WidgetFactory=$wf (quer 3) | \"Widget\"=$sw (quer 2) | const Widget=$cw (quer 1) | build=$build"
  [ "$cg" = 1 ] && [ "$wf" = 3 ] && [ "$sw" = 2 ] && [ "$cw" = 1 ] && [ "$build" = OK ] && echo "  => CORRETO" || echo "  => INCORRETO"
}

run_condition() {
  local label="$1" with_mcp="$2"
  local d; d=$(mktemp -d)
  cp -r "$FIX/." "$d/"
  if [ "$with_mcp" = yes ]; then
    mkdir -p "$d/.claude/skills"
    cp -r "$ROOT/.claude/skills/semantic-refactor" "$d/.claude/skills/" 2>/dev/null || true
    cat > "$d/.mcp.json" <<EOF
{ "mcpServers": { "code-intel": { "command": "$MCP", "env": { "TSGO_BIN": "$TSGO" } } } }
EOF
  fi
  echo "=== $label ==="
  local t0 t1; t0=$(date +%s)
  ( cd "$d" && claude -p "$TASK" --dangerously-skip-permissions >/dev/null 2>&1 )
  t1=$(date +%s)
  echo "  tempo: $((t1 - t0))s"
  evaluate "$d"
  rm -rf "$d"
  echo
}

echo "A/B de agente — tarefa: renomear a classe Widget -> Gadget"
echo
run_condition "SEM a camada (sem .mcp.json)" no
run_condition "COM a camada (.mcp.json + skill semantic-refactor)" yes
echo "Compare a correção e o tempo das duas condições acima."
