# code-intel-mcp — uma IDE na mão da LLM

[![CI](https://github.com/CenturyBoys/ide_cc/actions/workflows/ci.yml/badge.svg)](https://github.com/CenturyBoys/ide_cc/actions/workflows/ci.yml)
[![Release](https://img.shields.io/github/v/release/CenturyBoys/ide_cc?sort=semver)](https://github.com/CenturyBoys/ide_cc/releases)
[![License: MIT](https://img.shields.io/badge/license-MIT-blue.svg)](LICENSE)

Servidor **MCP** (Model Context Protocol) que dá ao seu agente (Claude Code e afins) operações
**semânticas** de código — navegar, renomear, mover, extrair — **rápidas** e **seguras**, delegando
a parte mecânica a language servers reais e **conferindo** o resultado.

> Princípio: o LLM decide **o quê**; a ferramenta faz a operação **mecânica**; o validador
> **confere**. O texto-cru (grep/sed) corrompe strings, comentários e símbolos homônimos — muitas
> vezes **em silêncio** (ainda compila). Aqui a mudança é **correta por construção**.

## Linguagens

TypeScript/JavaScript · Python · Dart · Rust · C# — cada uma via o melhor language server, com
roteamento por linguagem × operação.

| Linguagem | Navegação / rename | Refactorings | Language server |
|---|---|---|---|
| TypeScript | tsgo | vtsls | `@typescript/native-preview`, `@vtsls/language-server` |
| Python | basedpyright | basedpyright | `basedpyright` |
| Dart | dart language-server | dart | Dart SDK |
| Rust | rust-analyzer | rust-analyzer | `rustup component add rust-analyzer` |
| C# | csharp-ls | csharp-ls | `dotnet tool install --global csharp-ls` |

## Ferramentas (9)

`find_references` · `rename_symbol` · `document_symbols` · `find_symbol` · `workspace_symbols` ·
`call_hierarchy` · `extract_function` · `move_symbol` · `validate_build`

**Diferenciais:**
- **Gate de warmup** — nunca retorna contagem de referências parcial durante a indexação
  (defesa contra o *cold-index race*, um bug real de agentes).
- **apply → verify (`net_delta`)** — simula a edição em memória, mede erros antes/depois e só
  aplica se não introduzir erros; opcional `verify_build` roda o build e reverte se falhar.
- **Frescor** — reflete edições feitas fora do agente (re-sync por mtime).
- **Cache entre sessões** (opt-in) — daemon mantém os LSPs quentes entre reinícios (~23× na 2ª sessão).

## Instalação

### 1. O binário `code-intel-mcp`

**Opção A — baixar o release** (recomendado):
```bash
# Linux x86_64 (troque pelo seu alvo em github.com/CenturyBoys/ide_cc/releases)
curl -fsSL https://github.com/CenturyBoys/ide_cc/releases/latest/download/code-intel-mcp-x86_64-unknown-linux-gnu.tar.gz \
  | tar xz -C ~/.local/bin
```

**Opção B — compilar do fonte** (precisa de Rust):
```bash
git clone https://github.com/CenturyBoys/ide_cc && cd REPO
cargo build --release --manifest-path mcp/Cargo.toml
# binário em mcp/target/release/code-intel-mcp
```

### 2. Os language servers que você usa

Instale só os das linguagens que precisa (ver tabela acima). Ex.: `npm i -g @vtsls/language-server
@typescript/native-preview`, `pip install basedpyright`, `rustup component add rust-analyzer`.

### 3. Plugar no Claude Code (`.mcp.json` na raiz do projeto)

```json
{
  "mcpServers": {
    "code-intel": {
      "command": "/caminho/para/code-intel-mcp",
      "env": {
        "TSGO_BIN": "tsgo",
        "VTSLS_BIN": "vtsls",
        "BASEDPYRIGHT_BIN": "basedpyright-langserver",
        "DART_BIN": "dart",
        "RUST_ANALYZER_BIN": "rust-analyzer",
        "CSHARP_LS_BIN": "csharp-ls",
        "DOTNET_ROOT": "/caminho/do/dotnet"
      }
    }
  }
}
```
Cada `*_BIN` aponta para o executável do language server (default: o nome no PATH). Cache entre
sessões: adicione `"CODE_INTEL_DAEMON": "1"` ao `env`.

## Uso

Peça naturalmente ("renomeie a classe `Widget` para `Gadget`"); o agente usa `rename_symbol` e a
mudança é semântica e verificada. A skill `.claude/skills/semantic-refactor` orienta o agente a
preferir as ferramentas ao grep/sed.

## Prova de valor

Rename de uma classe num projeto com armadilhas (const homônima, strings, substrings):

| | Correto? | |
|---|---|---|
| Texto-cru (`\bWidget\b`→sed) | **NÃO** | corrompe strings + símbolo alheio (mas compila → bug silencioso) |
| `rename_symbol` (semântico) | **SIM** | classe renomeada, armadilhas intactas, build limpo |

Detalhes em [`docs/AB-EXPERIMENT.md`](docs/AB-EXPERIMENT.md). Benchmarks de latência dos 5 language
servers em [`benchmarks/results/RESULTS.md`](benchmarks/results/RESULTS.md).

## Documentação

- [`CLAUDE.md`](CLAUDE.md) — guia do repositório (IA-first)
- [`docs/ROADMAP.md`](docs/ROADMAP.md) · [`docs/RELATORIO-LEVANTAMENTO.md`](docs/RELATORIO-LEVANTAMENTO.md) · [`docs/WORKFLOW.md`](docs/WORKFLOW.md)
- [`mcp/README.md`](mcp/README.md) — detalhes do servidor · [`benchmarks/README.md`](benchmarks/README.md)

## Licença

MIT — ver [LICENSE](LICENSE).
