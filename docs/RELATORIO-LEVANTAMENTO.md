# Code Intelligence Layer para LLM — Levantamento & Plano de POC

> "Uma IDE na mão da LLM": o modelo decide **o quê**; ferramentas semânticas fazem a
> operação **mecânica**; validadores **conferem** o resultado.
> Foco da POC: **velocidade** — a métrica é *tempo até a mudança correta*, não "funciona?".

Data do levantamento: 2026-09. Linguagens-alvo: **Python, Dart, Rust, C#, TypeScript**.

---

## 0. Resposta à pergunta central: é viagem?

**Não.** A tese ("LLM raciocina + ferramenta executa + validador confere") é uma arquitetura
real, madura e com projetos de referência funcionando. Mas o cenário mudou desde a conversa
inicial e três premissas precisam de correção:

1. **O Claude Code já tem LSP nativo** (desde v2.0.74, dez/2025, flag `ENABLE_LSP_TOOL=1`).
   Ele já faz `goToDefinition`, `findReferences`, `documentSymbol`, `hover`, `getDiagnostics`
   nativamente. Ou seja: **navegação semântica já está resolvida de graça.**
2. **O que o LSP nativo NÃO faz** é justamente o coração da ideia: **rename semântico,
   call hierarchy, code actions/quick-fixes, workspace symbols, format, e refactorings
   com preview/validação.** É aqui que a POC agrega valor.
3. Dos projetos citados na conversa original: **Serena é real e forte**; **"refactory" não
   existe** (confusão com `refactor-mcp`, que é regex, não semântico); "lsp-mcp" existe mas
   é fraco (Haskell, sem rename).

**Conclusão estratégica:** não reinventar o cérebro da IDE. Conectar os cérebros que já
existem (language servers) ao agente, e construir só a camada fina que falta —
**refactoring com preview→apply→verify + gestão de processos rápida.**

---

## 1. Estado atual do Claude Code (o que já vem pronto)

| Camada | Ferramentas | Natureza |
|---|---|---|
| Texto/shell (sempre) | Read, Edit, Write, Grep, Glob, Bash | textual/regex |
| **LSP nativo** (`ENABLE_LSP_TOOL=1`, v2.0.74+) | definition, references, documentSymbol, hover, diagnostics | **semântico (navegação)** |
| Diagnostics via IDE | recebe lint/erros da extensão VS Code/JetBrains | passivo |
| Extensão (MCP) | qualquer ferramenta nova via `.mcp.json` | ilimitado |

**Buraco que a POC preenche (não-nativo mesmo com LSP tool):**
`rename_symbol` · `move_symbol/file` · `extract_function` · `organize_imports` ·
`code_action/quick_fix` · `workspace_symbols` · `call_hierarchy` · `format` ·
**e o fluxo preview→apply→verify.**

> Nota: o LSP nativo ainda está estabilizando (bug de init em algumas releases). Real, mas novo.

---

## 2. Projetos existentes — o que reusar vs. construir

| Projeto | Repo | Maturidade | Faz o quê | Veredito |
|---|---|---|---|---|
| **Serena** | `oraios/serena` | ~29.6k★, muito ativo | find/rename/move/symbol-edit, diagnostics, **40+ langs** | **Reusar como baseline** |
| **agent-lsp** | `blackwell-systems/agent-lsp` | referência de arquitetura | **preview→apply→verify com `net_delta`**, ~20 skills | **Copiar o padrão** |
| cclsp | `ktnyt/cclsp` | ~675★, focado em Claude Code | find/rename/diagnostics | Alternativa enxuta |
| mcp-language-server | `isaacphi/...` | ~1.6k★ | definition/references/rename/diagnostics | LSP cru |
| multilspy / solidlsp | `microsoft/multilspy` | base do Serena | camada uniforme LSP p/ ~12 langs (Python) | **Base de implementação** |
| lsp-tools (plugin) | `zircote/lsp-tools` | plugin | força o LSP nativo (não é server) | Referência de "skill" |
| ~~refactory~~ | — | **não existe** | — | ignorar |

**Peças-chave descobertas:**
- **Serena** resolve gestão de processos: 3 camadas (`Project → LanguageServerManager →
  SolidLanguageServer`), lazy init, self-healing (restart transparente), file-sync por
  polling de mtime, cache por content-hash persistido em `.serena/cache/`.
- **agent-lsp** resolve edição segura: `simulate_edit_atomic` aplica a mudança num documento
  virtual **em memória**, roda diagnostics, e retorna `net_delta` (erros introduzidos −
  resolvidos) **sem tocar no disco**. Gate: `net_delta > 0` → não commita.

---

## 3. Melhor language server por linguagem (matriz)

Legenda: ✅ ok · ⚠️ parcial · ✗ ausente

| Capacidade | Py: **basedpyright** | Py: ty (beta) | **Dart Analysis** | **rust-analyzer** | C#: **Roslyn LS** | TS: **vtsls** | TS: tsgo (preview) |
|---|---|---|---|---|---|---|---|
| definition/references | ✅¹ | ✅ | ✅ | ✅ | ✅ | ✅² | ✅ (project refs) |
| rename multi-arquivo | ✅ | ✅ | ✅ | ✅ | ✅ (melhor) | ✅ | ✅ |
| workspace symbols | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ⚠️ |
| call hierarchy | ✅ | ✅ | ✅ (+type) | ✅ | ✅ | ⚠️ | ✅ |
| code actions/refactor | ⚠️ | ⚠️ | ✅ | ✅ (+SSR) | ✅ (mais rico) | ✅ | ⚠️ |
| format | ✗ (usar Ruff) | ✗ (usar Ruff) | ✅ | ✅ | ✅ | ✅ | ✅ |
| diagnostics type-aware | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ |
| licença | MIT | MIT | BSD-3 | MIT/Apache | MIT (binário) | Apache-2 | Apache-2 |

¹ precisa de indexação de workspace completa (não `openFilesOnly`) para references completas.
² limitado pelo tsserver: incompleto em monorepo até carregar todos os projetos.

**Recomendação por linguagem:**

- **Python → `basedpyright`** (MIT, no PyPI, fácil de embarcar). Alternativa de menor latência:
  **`ty`** (Astral/Rust, ~80× mais rápido incremental, mas beta). Format com Ruff à parte.
  *Evite Pylance (proprietário), pylsp/jedi (lazy/lento).*
- **Dart → `Dart Analysis Server`** (`dart language-server`, LSP nativo). Única opção e boa.
  *Rode `flutter pub get` antes; startup caro em Flutter; ~1GB+ RAM.*
- **Rust → `rust-analyzer`**. Única production-grade. *Indexação inicial cara; RAM multi-GB.*
- **C# → `Roslyn LS`** standalone (`Microsoft.CodeAnalysis.LanguageServer`, binário MIT).
  Fallback: **`csharp-ls`** (menor risco jurídico se redistribuir). *Evite OmniSharp.*
- **TypeScript → `vtsls`** hoje (sólido); **`tsgo`** (TS 7, nativo Go) para monorepo/futuro
  (~10× typecheck, project references funcionam). *tsgo é preview: use `--dry-run` no rename.*

---

## 4. Riscos técnicos de primeira classe

### 4.1 Cold-index race (o risco #1) — issue Claude Code #76870
Chamar `find_references`/`rename` **antes** do server terminar de indexar retorna resultados
**parciais silenciosamente** (ex.: 1 referência quando existem 241). O agente então
deleta/refatora achando que viu tudo. **Afeta todos os servers** (pyright, tsserver,
rust-analyzer, Roslyn, Dart).

**Defesa (requisito, não opcional):**
- Warmup via `$/progress`/`workDoneProgress` — esperar `kind:"end"` da indexação.
- Responder aos requests server-initiated no `initialize` (`client/registerCapability`,
  `workspace/configuration`) senão o workspace nunca carrega.
- **A ferramenta deve retornar "índice não pronto / resultado possivelmente incompleto"**
  em vez de um número parcial. O modelo respeita erro estruturado > regra textual.
- Pré-abrir um arquivo por `tsconfig.json` (TS) / forçar workspace-mode (pyright).

### 4.2 Estado obsoleto após edição externa
Arquivo alterado via Bash/git/formatter fica invisível ao server. Emitir
`workspace/didChangeWatchedFiles` após qualquer comando que toque arquivos (Serena usa
polling de mtime).

### 4.3 Custo de memória
Pool multi-linguagem pode segurar **vários GB** (rust-analyzer sozinho: 10GB+ em repos
grandes). A POC precisa medir e possivelmente subir servers sob demanda por linguagem.

---

## 5. Arquitetura proposta

```
                         CLAUDE CODE (raciocínio)
                                  │  MCP (.mcp.json, stdio)
                                  ▼
                    ┌──────── TOOL GATEWAY ────────┐
                    │  interface pequena, semântica │
                    │  (não expõe LSP cru ao modelo)│
                    └───────────────┬───────────────┘
                                    │
                    CodeIntelligenceProvider (abstrato)
        ┌───────────┬───────────┬───────────┬───────────┐
     PythonAdapter DartAdapter RustAdapter C#Adapter  TSAdapter
     basedpyright  dart LS    rust-analyzer Roslyn LS  vtsls/tsgo
        └───────────┴─────┬─────┴───────────┴───────────┘
                          │  processos LSP PERSISTENTES (pool)
                          │  warmup · cache · file-sync · self-heal
                          ▼
              ┌──── PREVIEW → APPLY → VERIFY ────┐
              │ 1. dry-run WorkspaceEdit + blast  │
              │ 2. simulate em memória → net_delta│
              │ 3. gate: net_delta>0 ⇒ não aplica │
              │ 4. apply atômico (checa freshness)│
              │ 5. diagnostics after + testes     │
              └───────────────────────────────────┘
```

**Superfície de ferramentas (pequena e semântica):**
`find_definition · find_references · find_symbol · workspace_symbols · call_hierarchy ·
hover · document_symbols · diagnostics · rename_symbol · rename_file · move_symbol ·
extract_function · organize_imports · code_action · format · run_tests · run_linter · build`

**Decisões de arquitetura herdadas da pesquisa:**
- Pool **in-process** primeiro (simples, à la Serena). Migrar para **daemon + refcount**
  (à la `karellen-lsp-mcp`) só se quiser warm-index sobrevivendo entre sessões.
- Requests LSP **síncronos numa thread dedicada** por server (Serena) — simplicidade > async.
- Base de implementação: **`solidlsp`/`multilspy`** (não escrever cliente LSP do zero).
- Toda mutação loga em **JSONL com diagnostics before/after** → auditabilidade e rollback.
- **Skills** (`SKILL.md`) forçam a ordem: references antes de rename; diagnostics before/after.

---

## 6. Plano em fases

| Fase | Objetivo | Entrega |
|---|---|---|
| **0. Baseline & benchmark** | Instalar **Serena** e/ou **agent-lsp**, medir latência real nas 5 linguagens. Responder: "o que já existe é bom o bastante?" | Números (cold/warm p50/p95, RAM) + decisão build vs. reuse |
| **1. Prova mínima** | 1 linguagem (TS ou Python) + 5 tools: definition, references, symbols, **rename**, diagnostics, com **warmup correto** | POC que renomeia sem cold-index race |
| **2. Edição segura** | preview→apply→verify com `net_delta`; + rename_file, organize_imports, format, code_action | Refactoring confiável |
| **3. Validação** | diagnostics + lint + tests + build no loop | Gate de qualidade automático |
| **4. Cinco linguagens** | Adapters: Python, Dart, Rust, C#, TS | Cobertura completa |
| **5. Otimização** | processos persistentes, warmup, cache, paralelismo, telemetria | Velocidade de produção |

---

## 7. Métrica de sucesso: *Time to Correct Change*

Não medir "conseguiu?" e sim o custo da mudança correta:

```
          Claude sozinho (grep+edit)      Claude + Code Intelligence
pensamento         ~18s                          ~9s
localizar          ~14s (grep+leitura)           ~0.3s (find_references)
editar             ~22s (edições manuais)        ~0.1s (rename_symbol)
diagnostics        —                             ~3s
testes             ~18s (conserta quebras)       ~4s
────────────────────────────────────────────────────────────────────
TOTAL              ~72s                          ~16s   (ilustrativo)
```

Benchmark próprio (não existe suite pública de latência LSP para agentes): repos-fixture de
tamanhos variados × 5 linguagens; medir cold-start, p50/p95 warm, RAM, **taxa de truncamento**
(o modo de falha #76870), e warm-hit entre sessões.

---

## 8. Decisões em aberto (para o próximo passo)

1. **Build-on-Serena vs. greenfield:** o valor incremental sobre "instalar Serena + agent-lsp"
   é **velocidade** e **as 5 linguagens específicas**. A Fase 0 (benchmark do que existe)
   responde isso antes de escrever código.
2. **Linguagem da prova mínima:** TS (ecossistema LSP maduro) vs. Python (basedpyright fácil
   de embarcar) vs. Dart (seu caso Flutter real).
3. **Linguagem de implementação da camada:** Python (reusa `multilspy`/`solidlsp` direto) vs.
   Go/Rust (mais rápido, à la agent-lsp/tsgo).

---

## Fontes principais
Serena `github.com/oraios/serena` · agent-lsp `blog.blackwell-systems.com/posts/agent-lsp` ·
multilspy `github.com/microsoft/multilspy` · Claude Code LSP nativo (HN 46355165, v2.0.74) ·
cold-index race `github.com/anthropics/claude-code/issues/76870` · LSP 3.17 spec ·
basedpyright `github.com/DetachHead/basedpyright` · ty `astral.sh/blog/ty` ·
Dart `dart.dev/tools/analyzer-performance` · rust-analyzer `rust-analyzer.github.io` ·
Roslyn LS `nuget.org/packages/roslyn-language-server` · tsgo `devblogs.microsoft.com/typescript` (dez/2025).
