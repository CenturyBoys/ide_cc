#!/usr/bin/env bash
# Regressão (#18): validate_build + TypeScript — o override de check-cmd deve ser alcançável pelo
# nome CANÔNICO (TYPESCRIPT_CHECK_CMD, igual à msg de erro/doc) E pelo ALIAS legado (TS_CHECK_CMD).
#
# Bug de campo original (v0.9.0): o código lia só o env abreviado `TS_CHECK_CMD`, mas o erro
# ("defina TYPESCRIPT_CHECK_CMD") e a doc (<LANG>_CHECK_CMD) mandavam setar TYPESCRIPT_CHECK_CMD —
# que era IGNORADO. A alavanca existia, mas era inalcançável pelo nome documentado.
#
# Este script COMPARA o comportamento nos três cenários e falha (exit!=0) se a regressão voltar:
# agora os casos B (canônico) e C (alias) DEVEM destravar o validate_build; só o A (sem env) erra.
#
# Uso:  bash mcp/e2e/repro-ts-check-cmd.sh
set -u

HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
MCP="$(dirname "$HERE")"
BIN="${CODE_INTEL_BIN:-$MCP/target/release/code-intel-mcp}"
FIX="$HERE/fixtures/ts-monorepo"
TSGO="${TSGO_BIN:-tsgo}"
CHECK_CMD="$TSGO --noEmit"

if [ ! -x "$BIN" ]; then
  echo "!! binário não encontrado: $BIN"
  echo "   build: cargo build --release --manifest-path mcp/Cargo.toml"
  exit 2
fi
if ! command -v "$TSGO" >/dev/null 2>&1 && [ ! -x "$TSGO" ]; then
  echo "== SKIP: tsgo ausente (defina TSGO_BIN) =="
  exit 0
fi

REQ="$(printf '%s\n%s\n' \
  '{"jsonrpc":"2.0","id":1,"method":"initialize","params":{}}' \
  '{"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"validate_build","arguments":{"project":"'"$FIX"'","lang":"typescript"}}}')"

# roda uma sessão one-shot e extrai o texto da última resposta (id=2)
run_case() { printf '%s\n' "$REQ" | env "$@" "$BIN" 2>/dev/null \
  | python3 -c 'import sys,json
last=None
for l in sys.stdin:
    l=l.strip()
    if not l: continue
    try: m=json.loads(l)
    except: continue
    if m.get("id")==2: last=m
try: print(last["result"]["content"][0]["text"].splitlines()[0])
except Exception: print("<sem resposta>")'; }

echo "### CASO A — sem env (TS não tem default → erro acionável)"
A="$(run_case -u TS_CHECK_CMD -u TYPESCRIPT_CHECK_CMD)"; echo "   $A"

echo "### CASO B — TYPESCRIPT_CHECK_CMD (nome canônico, o que o erro/doc mandam setar)"
B="$(run_case -u TS_CHECK_CMD TYPESCRIPT_CHECK_CMD="$CHECK_CMD")"; echo "   $B"

echo "### CASO C — TS_CHECK_CMD (alias legado, retrocompatível)"
C="$(run_case -u TYPESCRIPT_CHECK_CMD TS_CHECK_CMD="$CHECK_CMD")"; echo "   $C"

fail=0
# A: sem env → erro que aponta para o nome canônico TYPESCRIPT_CHECK_CMD
case "$A" in *"defina TYPESCRIPT_CHECK_CMD"*) echo "✓ A: sem env → erro acionável aponta TYPESCRIPT_CHECK_CMD";;
  *) echo "✗ A: esperava erro 'defina TYPESCRIPT_CHECK_CMD', veio: $A"; fail=1;; esac
# B: o nome canônico (igual ao erro/doc) DEVE destravar — este era O bug
case "$B" in *"defina"*|"<sem resposta>") echo "✗ B: TYPESCRIPT_CHECK_CMD deveria destravar o validate_build (regressão do #18)"; fail=1;;
  *) echo "✓ B: TYPESCRIPT_CHECK_CMD (canônico) destrava o validate_build";; esac
# C: o alias legado continua funcionando
case "$C" in *"defina"*|"<sem resposta>") echo "✗ C: alias legado TS_CHECK_CMD deveria continuar funcionando"; fail=1;;
  *) echo "✓ C: alias legado TS_CHECK_CMD continua funcionando";; esac

echo
if [ "$fail" = 0 ]; then
  echo "==== OK (#18): B e C destravam, só A (sem env) erra — override alcançável pelos dois nomes. ===="
  exit 0
else
  echo "==== FALHOU: regressão do #18 — o override de TS voltou a ser inalcançável. ===="
  exit 1
fi
