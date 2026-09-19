# ROADMAP.md — plano e status (Code Intelligence Layer)

Plano vivo. Atualize o status ao concluir uma fase. Detalhes/porquês em
[`RELATORIO-LEVANTAMENTO.md`](RELATORIO-LEVANTAMENTO.md), [`../benchmarks/results/RESULTS.md`](../benchmarks/results/RESULTS.md)
e [`../mcp/README.md`](../mcp/README.md).

## Visão

Servidor MCP que dá ao LLM operações semânticas **rápidas e seguras**. O LLM decide *o quê*; o
language server faz a operação *mecânica*; o `net_delta` *confere*. Métrica: **tempo até a
mudança correta**.

## Decisões registradas

| # | Decisão | Motivo |
|---|---|---|
| D1 | Estratégia **benchmark-first** | decidir build-vs-reuse com números |
| D2 | Backend TS: **tsgo** (nav/rename) + **vtsls** (extract/move) | tsgo é rápido+correto mas não faz refactorings |
| D3 | Camada em **Rust** | foco em velocidade; Go ausente no ambiente |
| D4 | **Gate de warmup** obrigatório | cold-index race #76870 (truncamento silencioso) |
| D5 | **net_delta** (simular em memória, aplicar se seguro) | segurança de edição sem snapshot |
| D6 | Diagnostics **híbrido** (pull/push) | tsgo é pull, vtsls/pyright são push |
| D7 | Linguagens-alvo: Python, Dart, Rust, C#, TypeScript | escopo da POC |

## Fases

| Fase | Objetivo | Status |
|---|---|---|
| **0. Benchmark** | medir tsgo/vtsls/Serena; eleger backend; reproduzir o truncamento | ✅ **concluída** |
| **1. Prova mínima** | MCP em Rust + gate de warmup + find_references/rename | ✅ **concluída** |
| **2. Edição segura** | apply→verify com net_delta | ✅ **concluída** |
| **2b. Navegação** | document_symbols, find_symbol, workspace_symbols, call_hierarchy | ✅ **concluída** |
| **2c. Refactorings** | extract_function, move_symbol (codeAction→resolve) | ✅ **concluída** |
| **4. Multi-linguagem** | **TS ✅ · Python ✅ · Dart ✅ · Rust ✅ · C# ✅** | ✅ **CONCLUÍDA (5/5)** |
| **5. Otimização** | **validação build/test ✅** · cache persistente, warmup dirigido, paralelismo, telemetria, RAM | 🟡 **em curso** (validação feita) |

### Fase 4 — Python: CONCLUÍDO (2026-09-18)

`feature/phase-4-python`. basedpyright plugado; roteamento por **linguagem × operação**
(`nav_backend`/`refactor_backend` por extensão). Provado em `fixtures/py-demo` (20 módulos):
- find_references: **523 refs, stable** (gate entregou o total; sozinho o server truncava 3→523, Run 007).
- document_symbols, call_hierarchy (20 callers), rename preview (net_delta 0) — OK.
- rename colisão apply=true → **net_delta 342, bloqueado** (net_delta via **PUSH** diagnostics — basedpyright é push).
- Confirma: camada **agnóstica**, gate **cross-language**, diagnostics **híbrido** funcionam. Teste: `mcp/test-python.jsonl`.
- Decisão D2b: **basedpyright** (maturidade, MIT); `ty` (Rust, ~80× incremental) é troca futura quando sair do beta.

### Fase 4 — Dart: CONCLUÍDO (2026-09-18)

`feature/phase-4-dart`. Dart Analysis Server (`dart language-server`, LSP nativo). Provado em
`fixtures/dart-demo` (20 módulos, requer `dart pub get`):
- find_references: **503, stable de primeira** (Dart é eager, NÃO trunca — como o tsgo; Run 008).
- document_symbols, call_hierarchy (20 callers), rename preview (net_delta 0) — OK.
- rename colisão apply=true → **rejeitado pelo server** (`rejected_by_server`, "Library already declares...").
  Achado: backends têm defesas distintas — tsgo/basedpyright geram o edit (nosso net_delta pega),
  Dart valida e recusa na origem. Ambos: não aplicado, disco intacto. Teste: `mcp/test-dart.jsonl`.
- cold-start baixo (291 ms) em pacote puro; **Flutter será mais lento** (SDK+deps).

### Fase 4 — Rust: CONCLUÍDO (2026-09-18)

`feature/phase-4-rust`. rust-analyzer (`rustup component add rust-analyzer`). Provado em
`fixtures/rust-demo`:
- find_references: **524, stable** — server mais pesado (cold ~30s: cargo metadata+check); trunca 0→524.
  Gate ajustado: timeout 60s + resiliência a erro (rust-analyzer lança erro enquanto indexa). Run 009.
- document_symbols, rename preview (net_delta 0) — OK.
- rename `Account→i64` apply=true → **net_delta 161, bloqueado** (erros nativos "expected i64, found i32").
- **Limitação documentada:** net_delta em memória vê diagnostics NATIVOS, não os do `cargo check`
  (flycheck lê disco). Segurança total em Rust exige a Fase de validação (build/test) pós-apply.

### Fase 4 — C#: CONCLUÍDO (2026-09-18)

`feature/phase-4-csharp`. .NET SDK 10.0.401 (dotnet-install) + csharp-ls 0.28.0 (Roslyn, dotnet
tool). Provado em `fixtures/cs-demo` (requer `DOTNET_ROOT`):
- find_references: **503, stable** — não trunca (bloqueia ~13-24s na carga MSBuild+Roslyn). Run 010.
- document_symbols, rename preview (net_delta 0) — OK.
- rename `Account→Factory` apply=true → **net_delta 505, bloqueado** (Roslyn detecta colisão em memória).
- Roslyn analisa **em memória** (vê didChange) → net_delta confiável (≠ Rust/cargo check).

### Fase 4 — CONCLUÍDA: 5/5 linguagens

TypeScript (tsgo/vtsls) · Python (basedpyright) · Dart · Rust (rust-analyzer) · C# (csharp-ls).
A camada é comprovadamente **agnóstica**: cada linguagem = 1 backend + matches em
`lang_id`/`nav_backend`/`refactor_backend`. Runs 007–010 no RESULTS.md.

### Reforço do design (transversal, 4 linguagens medidas)

- **Truncam (lazy):** vtsls, basedpyright, rust-analyzer. **Não truncam (eager):** tsgo, Dart.
- rust-analyzer é o cold-start mais caro (~30s) → warmup+persistência são ainda mais críticos.
- Defesas de rename variam: net_delta (tsgo/basedpyright/vtsls/rust-nativo) vs rejeição no server (Dart).
- net_delta em memória não cobre erros de build externo (Rust/cargo check) → Fase de validação.

## Estado atual (8 ferramentas, TypeScript)

`find_references` · `rename_symbol` · `document_symbols` · `find_symbol` · `workspace_symbols`
· `call_hierarchy` · `extract_function` · `move_symbol` — todas com verificação reproduzível
(`mcp/test-*.jsonl`).

## Fase 4 — plano (Python primeiro)

Objetivo: provar que a camada é **agnóstica de linguagem** plugando **basedpyright**.
Terreno já preparado: roteamento por backend + diagnostics híbrido (basedpyright é push-based).

Passos previstos (branch `feature/phase-4-python`):
1. Instalar basedpyright; detectar invocação stdio (`basedpyright-langserver --stdio`).
2. `detect_language(file)` por extensão → escolher servidor por linguagem.
3. Registry de backend por linguagem (generalizar o atual NAV/REFACTOR).
4. Fixture Python (pacote com classe + referências cross-file, contagem conhecida).
5. Validar: find_references, rename (apply→verify), document_symbols, call_hierarchy.
6. Benchmark de latência do basedpyright no harness; registrar em RESULTS.md.
7. Documentar quirks (basedpyright é push; completude de references exige workspace-mode).

### Fase 5 — Validação de build: CONCLUÍDO (2026-09-18)

`feature/phase-5-validation`. Defesa em **duas camadas**:
1. `net_delta` em memória (rápido; erros nativos do server).
2. `verify_build`/`validate_build`: roda o build da linguagem NO DISCO (cargo check / dart analyze /
   dotnet build / …), pega erros que a simulação em memória não vê e **reverte** se falhar.
- Nova tool `validate_build`; opção `verify_build:true` em rename/extract/move (9 tools no total).
- Provado no Rust: rename `Account→make_account` — net_delta memória=0 (passou), mas `cargo check`
  pegou **E0252 (import duplicado)** → **revertido**, disco intacto. Comando por linguagem,
  override via env `<LANG>_CHECK_CMD`. Teste: `mcp/test-validate.jsonl`.

Resto da Fase 5 (pendente): RAM residente por server, cache persistente entre sessões, warmup
dirigido (pré-abrir tsconfigs/projetos), paralelismo, telemetria de tempo por operação.

## Pendências transversais

- `net_delta` cobre só arquivos afetados pela edição (erro em arquivo externo não é pego).
- `extract_function` usa nome default (`newFunction`) — falta parametrizar.
- Snapshot/rollback de disco antes de `apply=true`.
- Medir RAM por backend; monorepo maior (100+ pacotes).
- `documentSymbol` do tsgo vem achatado (name_paths hierárquicos aproximados).
