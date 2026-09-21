# AGENTS.md — code-intel (IDE na mão da LLM)

Este repositório expõe o MCP **`code-intel`**: operações **semânticas** de código (navegar,
renomear, mover, extrair, deletar, mudar assinatura) rápidas e com **edição verificada** (o server
simula `net_delta` e confere o build antes de aplicar). Funciona em qualquer cliente MCP (Codex,
Cursor, Cline, Zed, Claude Code).

## A orientação de uso vive NO SERVER (fonte única da verdade)

Não duplicamos as regras aqui — elas ficariam desatualizadas. O próprio MCP entrega a guidance:

- No **`initialize`** o server manda um **excerto curto** no campo `instructions` (o cliente entrega
  ao modelo automaticamente).
- Para o **manual completo**, chame a tool **`instructions`** do `code-intel` (sem argumentos). Ela
  cobre: roteamento **grep vs. semântico** (tools semânticas para SÍMBOLOS; grep só para texto
  literal), **confiar no gate de warmup** (não reler para conferir), **preview/`simulate_edit`** antes
  de **`safe_apply`**, **`blast_radius`** antes de uma edição ampla, **`safe_delete`** em vez de
  delete cego, **grep-sweep** do nome antigo pós-rename e o **loop de diagnostics** de nível-projeto.

Regra de ouro: para qualquer coisa sobre SÍMBOLOS use as tools do `code-intel`, **não** grep/sed
(grep textual corrompe strings/comentários/homônimos e às vezes ainda compila = bug silencioso).
As edit-tools têm `apply=false` por default (uma chamada já é preview).

## Config do Codex (`~/.codex/config.toml`)

```toml
[mcp_servers.code-intel]
command = "/home/ximit/Projects/ide_cc/mcp/target/release/code-intel-mcp"

[mcp_servers.code-intel.env]
TSGO_BIN = "/home/ximit/Projects/ide_cc/benchmarks/harness/node_modules/.bin/tsgo"
VTSLS_BIN = "/home/ximit/Projects/ide_cc/benchmarks/harness/node_modules/.bin/vtsls"
BASEDPYRIGHT_BIN = "/home/ximit/Projects/ide_cc/benchmarks/harness/node_modules/.bin/basedpyright-langserver"
DART_BIN = "dart"
RUST_ANALYZER_BIN = "rust-analyzer"
CSHARP_LS_BIN = "csharp-ls"
```

> Paths absolutos são específicos desta máquina — ajuste ao seu checkout. Depois de plugar, rode a
> tool `doctor` (valida language server + config de workspace por linguagem) e a tool `instructions`
> para o manual completo. Ligue `CODE_INTEL_DAEMON=1` para cache de LSP entre sessões.
