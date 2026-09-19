# code-intel-mcp — camada MCP fina e rápida sobre tsgo (POC Fase 1)

Servidor MCP em Rust que dá ao Claude Code operações semânticas **rápidas e seguras** sobre
TypeScript, usando o **tsgo** (`@typescript/native-preview`) como backend — o server que a
Fase 0 elegeu por ser o único rápido **e** correto (ver [`../benchmarks/results/RESULTS.md`](../benchmarks/results/RESULTS.md)).

## Os dois diferenciais

**1. Gate de warmup.** O risco #1 (cold-index race, [claude-code#76870](https://github.com/anthropics/claude-code/issues/76870))
é retornar referências **parciais em silêncio** antes do índice carregar — um agente então
renomeia/deleta destruindo usos reais. Este servidor **nunca** devolve uma contagem instável:
repete `find_references` até estabilizar (N iguais seguidas) e marca `stable: true/false`.

**2. apply → verify com `net_delta`.** Antes de aplicar qualquer edição (rename, extract, move),
ele **simula em memória** (via `didChange`, ou `didOpen` de arquivos novos — sem tocar o disco),
mede os erros *antes* e *depois*, e calcula `net_delta = erros_introduzidos − erros_resolvidos`.
Com `apply=true`, **só persiste se `net_delta ≤ 0`**; senão reverte e reporta os erros que
introduziria. É a defesa contra refactoring destrutivo, **compartilhada por rename/extract/move**.

Diagnostics são **híbridos**: PULL (`textDocument/diagnostic`, determinístico) quando o server
suporta (tsgo); senão PUSH (`publishDiagnostics` com espera de estabilização) — necessário porque
o vtsls é push-based. A camada detecta o modo pela capability do `initialize`.

## Ferramentas

| Tool | Backend | LSP por baixo | O que faz |
|---|---|---|---|
| `find_references` | tsgo | `textDocument/references` | referências semânticas + gate de warmup |
| `rename_symbol` | tsgo | `rename` + diagnostics | rename com apply→verify (`net_delta`) |
| `document_symbols` | tsgo | `documentSymbol` | árvore de símbolos (classes → métodos) |
| `find_symbol` | tsgo | `documentSymbol` | acha símbolo por name_path, posição exata |
| `workspace_symbols` | tsgo | `workspace/symbol` | busca símbolo no projeto inteiro |
| `call_hierarchy` | tsgo | `prepareCallHierarchy`+`incomingCalls` | quem chama este símbolo |
| `extract_function` | **vtsls** | `codeAction`+`resolve` | extrai linhas p/ nova função + apply→verify |
| `move_symbol` | **vtsls** | `codeAction`+`resolve` | move símbolo p/ novo arquivo (cria + atualiza imports) + apply→verify |

**Roteamento por linguagem × operação:** o servidor escolhe o backend por extensão e operação,
mantendo **um processo por (projeto × backend)**, tudo persistente:

| Linguagem | navegação / rename | refactorings (extract/move) |
|---|---|---|
| TypeScript (`.ts/.tsx/.js`) | **tsgo** (rápido, não trunca) | **vtsls** (tsgo não implementa refactorings) |
| Python (`.py`) | **basedpyright** | basedpyright |
| Dart (`.dart`) | **dart language-server** (não trunca) | dart |
| Rust (`.rs`) | **rust-analyzer** (cold ~30s, trunca; gate 60s) | rust-analyzer |
| C# (`.cs`) | **csharp-ls** (Roslyn; cold ~24s, não trunca) | csharp-ls |

Adicionar uma linguagem = um backend novo + um match em `nav_backend`/`refactor_backend`.

Resolução de posição é **semântica** (via `documentSymbol`, com refino textual da coluna no
identificador), com fallback textual — resolve "método dentro de classe" e desambigua.

## Build

```bash
cd mcp
cargo build --release          # gera target/release/code-intel-mcp
```

## Uso no Claude Code (.mcp.json)

Exemplo em [`../.mcp.json`](../.mcp.json). Ajuste os caminhos absolutos:

```json
{
  "mcpServers": {
    "code-intel": {
      "command": "/CAMINHO/ABS/mcp/target/release/code-intel-mcp",
      "env": { "TSGO_BIN": "...tsgo", "VTSLS_BIN": "...vtsls", "BASEDPYRIGHT_BIN": "...basedpyright-langserver" }
    }
  }
}
```

`TSGO_BIN` aponta para o binário do tsgo (default: `tsgo` no PATH). As tools recebem `project`
(caminho absoluto da raiz) em cada chamada, então um único servidor atende vários projetos —
cada um mantém seu processo tsgo **persistente** (o warmup é pago uma vez, depois é ~ms).

## Verificação end-to-end (reproduzível)

```bash
cd mcp
TSGO_BIN=../benchmarks/harness/node_modules/.bin/tsgo \
  ./target/release/code-intel-mcp < test-mcp.jsonl
```

Também há [`test-phase2.jsonl`](test-phase2.jsonl) cobrindo navegação + apply→verify:

```bash
TSGO_BIN=../benchmarks/harness/node_modules/.bin/tsgo \
  ./target/release/code-intel-mcp < test-phase2.jsonl
```

Resultados medidos (2026-09-18), com tsgo persistente:

| Cenário | Resultado |
|---|---|
| `find_references(ZodType)` @ zod | **64 refs, stable=true, warmup 1,2 s** (= ground-truth tsgo) |
| `find_references(Widget)` @ mono-ts | **653 refs, stable=true, warmup 1,5 s** ← onde vtsls dava **3** em silêncio |
| `document_symbols` / `find_symbol(Widget/render)` @ mono | árvore correta; posição exata |
| `call_hierarchy(makeWidget)` @ mono | **25 callers** (p00_use…p24_use) |
| `rename(Widget→Gadget, apply=true)` @ mono | **applied=true, net_delta=0** — 26 arquivos/653 edições persistidos |
| `rename(Widget→makeWidget, apply=true)` @ mono | **applied=false, net_delta=29** — colisão detectada, revertido, disco intacto |
| `extract_function(2..4, apply=true)` @ refactor-ts | **applied=true** — cria `function newFunction(a,b)`, substitui por chamada |
| `move_symbol(K, apply=true)` @ refactor-ts | **applied=true** — cria `src/K.ts`, adiciona `import { K }`, remove decl |

Reproduzível com [`test-refactor.jsonl`](test-refactor.jsonl) (precisa de `VTSLS_BIN` além de `TSGO_BIN`).

**Python** (basedpyright) — [`test-python.jsonl`](test-python.jsonl), precisa de `BASEDPYRIGHT_BIN`:

| Cenário | Resultado |
|---|---|
| `find_references(Account)` @ py-demo | **523 refs, stable** — o gate entregou o total (server sozinho trunca 3→523) |
| `document_symbols` / `call_hierarchy(make_account)` | classe+métodos; **20 callers** |
| `rename(Account→Ledger)` preview | net_delta=0, 21 arquivos/523 edições |
| `rename(Account→make_account, apply=true)` | **applied=false, net_delta=342** — colisão detectada via PUSH diagnostics |

**Dart** (Dart Analysis Server) — [`test-dart.jsonl`](test-dart.jsonl), precisa de `DART_BIN` e `dart pub get` no projeto:

| Cenário | Resultado |
|---|---|
| `find_references(Account)` @ dart-demo | **503 refs, stable** (Dart é eager, não trunca) |
| `document_symbols` / `call_hierarchy(makeAccount)` | classe+métodos; **20 callers** |
| `rename(Account→Ledger)` preview | net_delta=0, 21 arquivos/503 edições |
| `rename(Account→makeAccount, apply=true)` | **rejected_by_server** — Dart valida e recusa a colisão na origem |

**Rust** (rust-analyzer) — [`test-rust.jsonl`](test-rust.jsonl), precisa de `RUST_ANALYZER_BIN` (`rustup component add rust-analyzer`):

| Cenário | Resultado |
|---|---|
| `find_references(Account)` @ rust-demo | **524 refs, stable** — gate esperou ~22s do cold index (cargo check) |
| `document_symbols` | Account(Struct), balance(Method), make_account(Function) |
| `rename(Account→Ledger)` preview | net_delta=0, 21 arquivos/524 edições |
| `rename(Account→i64, apply=true)` | **applied=false, net_delta=161** ("expected i64, found i32", nativo) |

> **Nota Rust:** o `net_delta` captura diagnostics **nativos** do rust-analyzer, mas **não** os que
> só o `cargo check` (flycheck) reporta — ele lê do disco e não vê a simulação em memória. Para
> segurança total em Rust, use a validação pós-apply (`cargo check`/testes), prevista na Fase 5.

**C#** (csharp-ls / Roslyn) — [`test-csharp.jsonl`](test-csharp.jsonl), precisa de **.NET SDK**,
`DOTNET_ROOT`, `CSHARP_LS_BIN` (`dotnet tool install --global csharp-ls`):

| Cenário | Resultado |
|---|---|
| `find_references(Account)` @ cs-demo | **503 refs, stable** — gate cobriu ~13s de carga MSBuild+Roslyn |
| `document_symbols` | Namespace, Account(Class), Value(Field), Balance()(Method), Factory(Class) |
| `rename(Account→Ledger)` preview | net_delta=0, 21 arquivos/503 edições |
| `rename(Account→Factory, apply=true)` | **applied=false, net_delta=505** — Roslyn (em memória) detecta a colisão |

> Ao contrário do Rust, o Roslyn analisa **em memória** (vê o `didChange`), então o `net_delta`
> é confiável para C#.

## Arquitetura (Fases 1–2)

```
Claude Code ──MCP(stdio, JSON/linha)──> code-intel-mcp (Rust)
                                              │  registry: project -> LspClient (persistente)
                                              │  gate de warmup + apply→verify (net_delta)
                                              ▼
                                  tsgo --lsp -stdio  (Content-Length, JSON-RPC)
```

- `src/lsp.rs` — cliente LSP: processo persistente, thread leitora dedicada roteando por id,
  responde requests server-initiated (senão o workspace não carrega), requests síncronos,
  `did_change` (full sync, para simular em memória) e `pull_diagnostics`.
- `src/main.rs` — servidor MCP + gate de warmup + apply→verify + 6 tools.

O `net_delta` usa **pull diagnostics** (não push): como o tsgo é pull-based e o server processa
mensagens em ordem, um `textDocument/diagnostic` após o `didChange` reflete deterministicamente
o estado pós-edição — sem heurística de "esperar estabilizar".

## Limitações conhecidas (POC) / próximos passos

- `net_delta` mede diagnostics apenas dos **arquivos afetados** pela edição; um erro introduzido
  em arquivo fora do WorkspaceEdit não é detectado (raro, mas registrar).
- `extract_function` usa o nome default do tsserver (`newFunction`); falta parametrizar o nome.
- `documentSymbol` do tsgo vem "achatado" (sem aninhar métodos sob a classe); a resolução
  compensa por sufixo/último-segmento, mas name_paths hierárquicos são aproximados.
- Só TypeScript (tsgo/vtsls). Adapters para Python/Dart/Rust/C# são a Fase 4 (o roteamento por
  backend e o diagnostics híbrido já foram desenhados pensando nisso).
- Sem métricas de memória nem cache persistente entre execuções do servidor.
- Edições com `apply=true` não fazem backup em disco antes de escrever (a segurança vem do
  `net_delta`, não de snapshot/rollback de arquivos) — adicionar snapshot p/ produção.
