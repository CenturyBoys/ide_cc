#!/usr/bin/env bash
# Instalador do code-intel-mcp: baixa o binário do último release para a sua plataforma e
# verifica quais language servers você já tem. Uso:
#   curl -fsSL https://raw.githubusercontent.com/CenturyBoys/ide_cc/main/install.sh | bash
# Opções (env): BIN_DIR=~/.local/bin  VERSION=latest  WRITE_MCP=1 (escreve .mcp.json no cwd)
set -euo pipefail

REPO="CenturyBoys/ide_cc"
BIN_DIR="${BIN_DIR:-$HOME/.local/bin}"
VERSION="${VERSION:-latest}"

# 1. detecta plataforma -> alvo do release
os="$(uname -s)"; arch="$(uname -m)"
case "$os/$arch" in
  Linux/x86_64)          target="x86_64-unknown-linux-gnu" ;;
  Linux/aarch64|Linux/arm64) target="aarch64-unknown-linux-gnu" ;;
  Darwin/arm64)          target="aarch64-apple-darwin" ;;
  Darwin/x86_64)         target="x86_64-apple-darwin" ;;
  *) echo "!! plataforma não suportada: $os/$arch"; exit 1 ;;
esac

# 2. baixa e instala o binário
if [ "$VERSION" = "latest" ]; then
  url="https://github.com/$REPO/releases/latest/download/code-intel-mcp-$target.tar.gz"
else
  url="https://github.com/$REPO/releases/download/$VERSION/code-intel-mcp-$target.tar.gz"
fi
mkdir -p "$BIN_DIR"
echo ">> baixando ($target): $url"
curl -fsSL "$url" | tar xz -C "$BIN_DIR"
chmod +x "$BIN_DIR/code-intel-mcp"
echo ">> instalado: $BIN_DIR/code-intel-mcp"
"$BIN_DIR/code-intel-mcp" --version >/dev/null 2>&1 || true

# 3. verifica os language servers (instale só os das linguagens que usar)
echo; echo ">> language servers:"
check() { if command -v "$1" >/dev/null 2>&1; then echo "  [ok]    $1"; else echo "  [falta] $1  ->  $2"; fi; }
check tsgo                    "npm i -g @typescript/native-preview"
check vtsls                   "npm i -g @vtsls/language-server"
check basedpyright-langserver "pip install basedpyright   (ou: npm i -g basedpyright)"
check dart                    "instale o Dart/Flutter SDK"
check rust-analyzer           "rustup component add rust-analyzer"
check csharp-ls               "dotnet tool install --global csharp-ls"

# 4. opcional: escreve um .mcp.json no diretório atual
if [ "${WRITE_MCP:-0}" = "1" ]; then
  cat > .mcp.json <<EOF
{
  "mcpServers": {
    "code-intel": {
      "command": "$BIN_DIR/code-intel-mcp",
      "env": {
        "TSGO_BIN": "tsgo", "VTSLS_BIN": "vtsls",
        "BASEDPYRIGHT_BIN": "basedpyright-langserver",
        "DART_BIN": "dart", "RUST_ANALYZER_BIN": "rust-analyzer",
        "CSHARP_LS_BIN": "csharp-ls"
      }
    }
  }
}
EOF
  echo; echo ">> .mcp.json escrito em $(pwd)/.mcp.json"
fi

# 5. PATH + próximos passos
case ":$PATH:" in
  *":$BIN_DIR:"*) ;;
  *) echo; echo ">> adicione ao PATH:  export PATH=\"$BIN_DIR:\$PATH\"" ;;
esac
echo; echo ">> pronto. Crie um .mcp.json no seu projeto (ou rode com WRITE_MCP=1) e plugue no Claude Code."
echo "   docs: https://github.com/$REPO#instalação"
