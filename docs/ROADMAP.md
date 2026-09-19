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
| **4. Multi-linguagem** | **Python (basedpyright) ✅** → depois Dart, Rust, C# | 🟡 **em curso** (Python feito) |
| **5. Otimização** | cache persistente, warmup dirigido, paralelismo, telemetria, RAM | ⬜ pendente |

### Fase 4 — Python: CONCLUÍDO (2026-09-18)

`feature/phase-4-python`. basedpyright plugado; roteamento por **linguagem × operação**
(`nav_backend`/`refactor_backend` por extensão). Provado em `fixtures/py-demo` (20 módulos):
- find_references: **523 refs, stable** (gate entregou o total; sozinho o server truncava 3→523, Run 007).
- document_symbols, call_hierarchy (20 callers), rename preview (net_delta 0) — OK.
- rename colisão apply=true → **net_delta 342, bloqueado** (net_delta via **PUSH** diagnostics — basedpyright é push).
- Confirma: camada **agnóstica**, gate **cross-language**, diagnostics **híbrido** funcionam. Teste: `mcp/test-python.jsonl`.
- Decisão D2b: **basedpyright** (maturidade, MIT); `ty` (Rust, ~80× incremental) é troca futura quando sair do beta.

Próximo dentro da Fase 4: Dart (Analysis Server), depois Rust (rust-analyzer) e C# (Roslyn LS).

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

## Pendências transversais

- `net_delta` cobre só arquivos afetados pela edição (erro em arquivo externo não é pego).
- `extract_function` usa nome default (`newFunction`) — falta parametrizar.
- Snapshot/rollback de disco antes de `apply=true`.
- Medir RAM por backend; monorepo maior (100+ pacotes).
- `documentSymbol` do tsgo vem achatado (name_paths hierárquicos aproximados).
