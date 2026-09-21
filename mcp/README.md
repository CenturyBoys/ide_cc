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
| `validate_build` | build da linguagem | `cargo check`/`dart analyze`/`dotnet build`… | roda o build NO DISCO e reporta erros (2ª camada de segurança) |
| `doctor` | — | checagem de setup | verifica LSP disponível + config de workspace por linguagem; `fix=true` corrige (ex.: cria `pyrightconfig.json`); sonda contagem de refs e config incompleta (avisos no campo `warnings`) |
| `organize_imports` | **vtsls**/backend | `source.organizeImports` | organiza imports (preserva side-effect + type-only usado) + apply→verify |
| `safe_delete` | nav backend | `references` + delete | deleta símbolo **só se 0 refs externas**; senão recusa listando os locais; cold → `index_not_ready` (nunca falso "0 refs") |
| `simulate_edit` · `preview_edit` · `safe_apply` | — (núcleo) | `net_delta` em memória | simula / mostra (diff+blast) / aplica uma edição proposta pelo agente — `safe_apply` só aplica se `net_delta≤0` |
| `replace_symbol_body` · `insert_before_symbol` · `insert_after_symbol` | nav backend | `documentSymbol` + edit | edita por **nome** do símbolo (sem coordenadas cruas) + apply→verify |
| `blast_radius` | composto | `references` + `call_hierarchy` | superfície de risco (refs + callers, particionado test vs produção) **antes** de editar; read-only |
| `quick_fix` | code-action | `quickfix` | aplica UMA correção de diagnóstico de uma linha (executor interno, sem `code_action` cru) + apply→verify |
| `change_signature` | hand-built / nativo | `call_hierarchy` + edit | add/remove/reordena parâmetro na declaração **e** em todos os call-sites; recusa em vez de aplicar edição parcial/perigosa |
| `move_file` | `willRenameFiles`/tsserver | fileRename edits | move arquivo inteiro + conserta importers/re-exports; **reverte se o build quebrar** (rede anti basedpyright #1888); desambiguado de `move_symbol` |
| `instructions` | — | guidance server-side | manual de uso (roteamento grep-vs-semântico, preview→apply, blast antes de editar) — **fonte única** herdada por QUALQUER cliente MCP (Codex/Cursor/Cline/Zed), também no campo `instructions` do `initialize` |

As tools de **localização** (`find_references`, `find_symbol`, `workspace_symbols`,
`document_symbols`, `call_hierarchy`) retornam `path:linha:content` + ~2 linhas de contexto (helper
`format_location`, UTF-8-safe) e locais agrupados/contados — em vez do payload LSP cru — reduzindo
re-leituras de arquivo pelo agente (I1).

Além disso, `rename`/`extract`/`move`/`safe_delete`/`organize_imports`/`change_signature`/`move_file`
aceitam `verify_build: true` (com `apply=true`): após
escrever no disco, rodam o build da linguagem e **revertem se falhar** — fecha o buraco do
`net_delta` em memória (ex.: erros que só o `cargo check` do Rust vê).

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

O [`../.mcp.json`](../.mcp.json) do repo é **portátil** — sem caminho por máquina. Aponta para o
launcher [`scripts/code-intel-mcp.sh`](../scripts/code-intel-mcp.sh), que se auto-localiza e resolve
os language servers (tsgo/vtsls/basedpyright do `node_modules`; dart/rust-analyzer/csharp-ls do PATH):

```json
{
  "mcpServers": {
    "code-intel": {
      "command": "${CLAUDE_PROJECT_DIR:-.}/scripts/code-intel-mcp.sh"
    }
  }
}
```

`${CLAUDE_PROJECT_DIR}` é injetado pelo Claude Code na raiz do projeto (fallback `.` para outros
clientes que rodam a partir da raiz). Basta `cargo build --release` antes. Para sobrescrever um bin,
exporte a env correspondente (`TSGO_BIN` etc.) — o launcher respeita. As tools recebem `project`
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

**Validação de build (Fase 5)** — [`test-validate.jsonl`](test-validate.jsonl), 2ª camada de segurança:

| Cenário | Resultado |
|---|---|
| `validate_build(rust-demo)` fixture limpo | **build_ok=true**, sem erros |
| `rename(Account→make_account, apply=true, verify_build=true)` @ rust-demo | **applied=false** — `net_delta` (memória) passou (0), mas `cargo check` pegou **E0252 (import duplicado)** → **revertido** |

> Fecha o buraco do Rust: a simulação em memória não vê erros de `cargo check` (lê disco); a
> validação de build pós-apply vê. Comando por linguagem (cargo/dart/dotnet/…), override via env
> `<LANG>_CHECK_CMD`. TypeScript/Python não têm comando default (configure via env se quiser).

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
- `src/main.rs` — servidor MCP + gate de warmup + apply→verify + **23 tools** + `mod guidance`
  (manual de uso, fonte única do campo `instructions` e da tool `instructions`).

O `net_delta` usa **pull diagnostics** (não push): como o tsgo é pull-based e o server processa
mensagens em ordem, um `textDocument/diagnostic` após o `didChange` reflete deterministicamente
o estado pós-edição — sem heurística de "esperar estabilizar".

## Frescor e cache entre sessões (Fase 5)

**Frescor (freshness) — sempre ligado.** Um server persistente serviria conteúdo obsoleto se um
arquivo fosse editado **fora do Claude** (outro editor, `git checkout`). O `ensure_open` detecta
mudança de **mtime** no disco e **re-sincroniza** (`didChange`) antes de operar. Verificado em
[`../benchmarks/harness/freshness-test.mjs`](../benchmarks/harness/freshness-test.mjs): edição
externa (função `two` adicionada no disco) aparece na consulta seguinte.

**Cache entre sessões (daemon) — opt-in via `CODE_INTEL_DAEMON=1`.** O Claude Code recria o
processo MCP a cada sessão, matando os LSPs quentes → paga o cold-start de novo (rust-analyzer
~30s, csharp-ls ~24s). Com o daemon, um processo separado é dono dos LSPs e **sobrevive ao
restart** do MCP (que vira um proxy fino sobre um Unix socket). O frescor garante que arquivos
mudados entre sessões são re-sincronizados, então é seguro.

Medido ([`../benchmarks/harness/daemon-cache-test.mjs`](../benchmarks/harness/daemon-cache-test.mjs)),
2 sessões MCP separadas no mesmo projeto Rust:

| Sessão | | Tempo |
|---|---|---|
| 1 (cold, sobe daemon + rust-analyzer) | 524 refs | **27,9 s** |
| 2 (MCP novo, reconecta ao daemon quente) | 524 refs | **1,2 s** (**23× mais rápido**) |

Vale a pena só para servers de cold-start pesado (Rust, C#) **e** fluxo multi-sessão. Numa sessão
longa única, a persistência in-process (default) já resolve. O daemon encerra após 30 min ocioso.
Rode o daemon manualmente com `code-intel-mcp --daemon` (ou deixe o MCP subir sob demanda).

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
