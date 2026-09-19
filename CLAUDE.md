# CLAUDE.md — guia do repositório (IA-first)

Este repositório é **IA-first**: foi estruturado para que um agente (Claude Code) entenda o
contexto, reproduza resultados e trabalhe com segurança **sem re-derivar decisões**. Leia este
arquivo antes de agir; ele aponta para tudo que importa.

## O que é este projeto

POC de um **Code Intelligence Layer para LLM** — "uma IDE na mão da LLM". Um servidor MCP que dá
ao agente operações semânticas de código (navegar, renomear, extrair, mover) **rápidas** e
**seguras**, delegando a parte mecânica a language servers e conferindo o resultado.

Princípio: *o LLM decide **o quê**; a ferramenta faz a operação **mecânica**; o validador
**confere** (net_delta).* Métrica-alvo: **tempo até a mudança correta**.

## Estrutura

```
docs/       RELATORIO-LEVANTAMENTO.md (pesquisa), ROADMAP.md (plano+status), WORKFLOW.md (regras git)
benchmarks/ harness de latência LSP (Fase 0) — README.md + results/RESULTS.md
mcp/        code-intel-mcp (Rust) — o servidor MCP; README.md
fixtures/   projetos de teste (GITIGNORED; recriados por benchmarks/scripts/setup-fixtures.sh)
.mcp.json   config para plugar o servidor no Claude Code
```

## Como buildar e testar

```bash
# fixtures (zod + monorepo sintético + refactor-ts)
bash benchmarks/scripts/setup-fixtures.sh
# harness de benchmark
cd benchmarks/harness && npm install
# servidor MCP
cargo build --release --manifest-path mcp/Cargo.toml
# testes e-2-e do MCP (JSON-RPC por linha)
cd mcp && TSGO_BIN=../benchmarks/harness/node_modules/.bin/tsgo \
          VTSLS_BIN=../benchmarks/harness/node_modules/.bin/vtsls \
          ./target/release/code-intel-mcp < test-phase2.jsonl
```

## Decisões-chave já tomadas (NÃO re-derivar — ver docs/ para o porquê)

- **Backend TypeScript = tsgo** para navegação/rename (rápido + correto; não trunca em monorepo),
  **vtsls** para refactorings (extract/move — tsgo não os implementa). Roteamento por operação.
- **Risco #1 = cold-index race** (#76870): a camada tem um **gate de warmup** e nunca retorna
  contagem parcial durante indexação.
- **Segurança de edição = 2 camadas**: (1) `net_delta` em memória (simula, mede erros antes/depois,
  só aplica se não introduzir erros; helper `verify_and_apply` compartilhado por rename/extract/move);
  (2) `verify_build`/`validate_build` roda o build da linguagem no disco e reverte se falhar — pega
  erros que a simulação em memória não vê (ex.: `cargo check` do Rust).
- **Diagnostics híbrido**: PULL (tsgo) ou PUSH (vtsls/pyright), detectado por capability.
- **Frescor**: `ensure_open` re-sincroniza (didChange) quando o mtime do disco muda → edições
  feitas fora do Claude são refletidas. **Cache entre sessões** (opt-in `CODE_INTEL_DAEMON=1`):
  daemon dono dos LSPs sobrevive ao restart do MCP (proxy Unix socket) — 2ª sessão ~23× + rápida.
- Camada em **Rust**; navegação pura o LSP nativo do Claude Code já faz — nosso valor é a
  **edição mecânica segura**.

## Convenções de trabalho (IA-first)

1. **Reprodutibilidade acima de tudo.** Todo resultado (benchmark, teste) tem script + versões
   fixadas + markdown documentando como reproduzir. Nada de número sem origem.
2. **Documentar junto com o código.** Toda mudança atualiza o README/RESULTS/ROADMAP relevante
   na mesma unidade de trabalho.
3. **Testes como artefato versionado** (`mcp/test-*.jsonl`).
4. **Git**: gitflow + Conventional Commits + PR — ver [`docs/WORKFLOW.md`](docs/WORKFLOW.md).
5. **Plano vivo**: o estado das fases fica em [`docs/ROADMAP.md`](docs/ROADMAP.md); atualize ao
   concluir uma fase.
6. **Honestidade técnica**: registrar limitações e pendências, não só sucessos.
