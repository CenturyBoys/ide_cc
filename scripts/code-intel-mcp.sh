#!/usr/bin/env bash
# Launcher PORTÁTIL do code-intel-mcp.
#
# Resolve tudo relativo à posição DESTE script (não importa o cwd de quem chama), então o
# .mcp.json / config do Codex não precisa de caminhos absolutos por máquina. Ideia (à la
# Serena/agent-lsp): a config só aponta pra este script; ele descobre o resto.
#
# Precedência de cada binário: env do usuário  >  bin do repo (node_modules do harness)  >
# PATH (fallback embutido no próprio code-intel-mcp). Assim: funciona out-of-the-box após
# `cargo build`, mas o usuário pode sobrescrever qualquer bin exportando a env correspondente.
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"

BIN="${ROOT}/mcp/target/release/code-intel-mcp"
if [[ ! -x "$BIN" ]]; then
  echo "code-intel-mcp: binário não encontrado em '$BIN'." >&2
  echo "  Build: cargo build --release --manifest-path '${ROOT}/mcp/Cargo.toml'" >&2
  exit 1
fi

# TS (tsgo/vtsls) e Python (basedpyright) vivem no node_modules do harness. Só apontamos se
# existirem e o usuário não tiver setado; senão o binário usa o PATH.
NM="${ROOT}/benchmarks/harness/node_modules/.bin"
_set_if_unset() { # _set_if_unset VAR caminho_absoluto
  local var="$1" path="$2"
  if [[ -z "${!var:-}" && -x "$path" ]]; then export "$var=$path"; fi
}
_set_if_unset TSGO_BIN         "${NM}/tsgo"
_set_if_unset VTSLS_BIN        "${NM}/vtsls"
_set_if_unset BASEDPYRIGHT_BIN "${NM}/basedpyright-langserver"

# C#: descobre o .NET SDK em ~/.dotnet se não setado; garante o SDK e os global tools no PATH
# (csharp-ls é instalado via `dotnet tool install --global csharp-ls`).
if [[ -z "${DOTNET_ROOT:-}" && -d "${HOME}/.dotnet" ]]; then export DOTNET_ROOT="${HOME}/.dotnet"; fi
if [[ -n "${DOTNET_ROOT:-}" ]]; then export PATH="${DOTNET_ROOT}:${DOTNET_ROOT}/tools:${PATH}"; fi

# dart / rust-analyzer / csharp-ls: ficam no PATH (default do binário) a menos que o usuário
# exporte DART_BIN / RUST_ANALYZER_BIN / CSHARP_LS_BIN.

exec "$BIN" "$@"
