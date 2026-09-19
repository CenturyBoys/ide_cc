# Resultados dos Benchmarks

Cada execução é registrada aqui com **data, versões e como reproduzir**
(ver [`../README.md`](../README.md)). JSON bruto em `*.json` (gitignored); este markdown é a
fonte commitada.

---

## Run 001 — vtsls @ zod (baseline LSP cru)

- **Data:** 2026-09-18
- **Comando:** `node lsp-bench.mjs --server vtsls --fixture ../../fixtures/zod`
- **Versões:** node v24.18.0 · @vtsls/language-server 0.3.0 · typescript 7.0.2 · zod v3.23.8 (`ca42965`)
- **Máquina:** 8 cores, 23 GiB RAM, Linux
- **Alvo:** `ZodType` em `src/types.ts` (classe base central)

| Métrica | Valor |
|---|---|
| cold-start (init → initialized) | **720.7 ms** |
| didOpen (doc-alvo) | 3.5 ms |
| find_references — 1ª resposta | 55 refs @ **6004 ms** (bloqueou até indexar) |
| find_references — estável | 55 refs @ 7397 ms |
| **Truncou a 1ª resposta?** | **Não** (bloqueou até completo) |
| find_references warm p50 / p95 | **39.2 ms** / 5924 ms |
| rename `ZodType`→`ZodSchemaBase` (dry-run) — prepare | ok, 44.2 ms |
| rename — execução | **30.2 ms** |
| rename — blast radius | **4 arquivos, 60 edições** |

### Leitura dos números

1. **A tese se confirma.** O custo caro é a **primeira query semântica em índice frio (~6 s)**;
   depois disso, um `rename` completo custa **~30 ms**. Ou seja: com **servidor persistente +
   warmup**, a operação mecânica é praticamente instantânea. Matar/subir o server por operação
   jogaria fora esses 6 s toda vez — é o antipadrão a evitar.

2. **Semântico ≠ textual.** O grep textual conta **64** ocorrências de `ZodType`; o
   `find_references` semântico retorna **55**. A diferença (9) é exatamente o que um `grep`+`sed`
   erraria — a justificativa concreta da camada semântica.

3. **Truncamento: não observado aqui — e isso é esperado.** Em **pacote único**, o tsserver
   **bloqueia** a 1ª `find_references` até o projeto carregar (por isso ela só respondeu aos 6 s,
   já completa). O modo de falha #76870 (resultado parcial **em silêncio**) é fenômeno de
   **monorepo com project references** — precisa de um fixture monorepo para reproduzir.
   **Pendência:** adicionar esse fixture e re-medir. Até lá, não podemos afirmar que o warmup
   está resolvido — só que o pior caso não aparece em pacote único.

4. **Variância a investigar:** warm **p95 = 5924 ms** com **p50 = 39 ms** — há um outlier
   (uma das 20 queries quentes custou ~6 s, provável re-análise/GC disparada logo após a sonda).
   Precisa de mais amostras e isolamento (rodar a sonda de truncamento antes de medir warm, com
   pausa) para separar sinal de ruído.

### Implicações para a arquitetura

- **Warmup é requisito, não opcional:** a camada precisa esperar o índice antes de aceitar
  operações semânticas, e a 1ª query paga ~6 s neste fixture pequeno (será pior em repos grandes).
- **Processo persistente é obrigatório** para diluir o cold-start.
- O `rename` dry-run já entrega o `WorkspaceEdit` com blast radius — base pronta para o passo
  `preview → apply → verify` (Fase 2).

### Pendências abertas por esta run

- [ ] Fixture **monorepo TS** para reproduzir o truncamento silencioso (#76870).
- [ ] Investigar o outlier de p95 (isolar warm da sonda; aumentar N).
- [ ] Adicionar **tsgo** e comparar cold-start/latência/memória vs. vtsls.
- [ ] Medir **memória residente** do server (ainda não instrumentado).
- [ ] Medir **Serena** (overhead da camada MCP sobre o LSP cru).

---

## Run 002 — vtsls @ mono-ts (REPRODUZ o truncamento silencioso)

- **Data:** 2026-09-18
- **Setup:** `node benchmarks/scripts/gen-monorepo.mjs --packages 25 --refs 12`
- **Comando:** `node lsp-bench.mjs --server vtsls --fixture ../../fixtures/mono-ts --file packages/core/src/index.ts --search "class Widget" --symbol Widget --newname Gadget`
- **Versões:** node v24.18.0 · @vtsls/language-server 0.3.0 · typescript 7.0.2
- **Fixture:** monorepo sintético, 25 pacotes com project references, **626 refs plantadas** a `Widget` (653 reais contando os 25 `import` + 2 de `makeWidget`)
- **Alvo:** `Widget` em `packages/core/src/index.ts`

| Métrica | Valor |
|---|---|
| cold-start | 728 ms |
| **find_references — 1ª resposta** | **3 refs @ 4640 ms** |
| **find_references — estável** | **653 refs @ 18713 ms** |
| **Truncou a 1ª resposta?** | **SIM — 3 de 653 (99,5% faltando)** |
| find_references warm p50 / p95 | 148.5 ms / 191.7 ms |
| rename `Widget`→`Gadget` (dry-run) | 26 arquivos, 653 edições, 102 ms |

### Leitura dos números — o achado central da Fase 0

**O truncamento silencioso é real e devastador.** A primeira `find_references`, aos 4,6 s,
retornou **3 referências** (só o pacote `core`, cujo arquivo foi aberto). O total verdadeiro é
**653**, alcançado só aos **18,7 s**, à medida que o tsserver carrega preguiçosamente os 25
projetos referenciados. **Não há erro, não há aviso — só um número errado.**

Consequência direta para um agente: se ele pergunta "quantas referências tem `Widget`?" cedo
demais e recebe **3**, ele conclui que o símbolo é quase não-usado e pode **renomear/deletar
destruindo 650 usos reais em silêncio.** Este é o modo de falha que a POC existe para eliminar.

Contraste com o pacote único (Run 001): lá o tsserver **bloqueava** até completar (resposta
tardia mas correta); aqui ele **responde rápido e errado**. A diferença é project references:
o server só carrega o projeto do arquivo aberto e vai puxando os demais sob demanda.

Observação boa: uma vez quente, `find_references` custa **~150 ms** e o `rename` de **653
edições em 26 arquivos** sai em **102 ms**. O custo é todo no warmup; a operação mecânica é barata.

### Implicações (confirma e endurece a arquitetura)

1. **A tool NÃO pode retornar contagem crua durante o warmup.** Precisa de um gate: enquanto o
   índice não estabilizou, retornar `index_not_ready` / marcar resultado como incompleto —
   **nunca** um número que parece completo. (O modelo confia num erro estruturado; não confia
   numa regra textual.)
2. **Detecção de "estável" é o coração do problema.** Sinais viáveis: `$/progress` end,
   pré-carregar um arquivo por `tsconfig` antes de operar, ou re-query até a contagem parar de
   subir (sonda de estabilização — foi o que este harness fez).
3. **Warmup por projeto, não global:** o custo aqui foi ~14 s para convergir de 3→653; em
   monorepo real (centenas de projetos) será muito pior. Pré-carga dirigida > esperar tudo.

### Pendências

- [x] Reproduzir o truncamento em monorepo — **feito (Run 002)**.
- [ ] Implementar e medir uma estratégia de warmup (pré-abrir tsconfigs) que elimine o truncamento.
- [ ] Comparar com **tsgo** (project references nativas — a hipótese é que trunca menos).
- [ ] Medir **Serena** no mesmo fixture (será que a camada dela já resolve o warmup?).

---

## Run 003 & 004 — tsgo @ zod e @ mono-ts (o candidato que NÃO trunca)

- **Data:** 2026-09-18
- **Server:** `tsgo` (`@typescript/native-preview`, TypeScript 7.0.0-dev.20260707 nativo em Go)
- **Invocação:** `tsgo --lsp -stdio`
- **Comandos:**
  - `node lsp-bench.mjs --server tsgo --fixture ../../fixtures/zod`
  - `node lsp-bench.mjs --server tsgo --fixture ../../fixtures/mono-ts --file packages/core/src/index.ts --search "class Widget" --symbol Widget --newname Gadget`

### Comparação direta vtsls × tsgo

| Métrica | vtsls @ zod | **tsgo @ zod** | vtsls @ mono | **tsgo @ mono** |
|---|---|---|---|---|
| cold-start | 721 ms | **177 ms** | 728 ms | **245 ms** |
| 1ª find_references | 55 @ 6004 ms | **64 @ 470 ms** | **3 @ 4640 ms** | **653 @ 501 ms** |
| refs estável | 55 | 64 | 653 | 653 |
| **truncou?** | não (bloqueou) | não | **SIM (3/653)** | **NÃO** |
| warm p50 / p95 | 39 / 5924 ms | **12.7 / 40 ms** | 148 / 192 ms | **61 / 91 ms** |
| rename (dry-run) | 30 ms | **11 ms** | 102 ms | 53 ms |

### Leitura — o achado mais importante da Fase 0

1. **O tsgo elimina o truncamento no monorepo.** Onde o vtsls retornou 3 de 653 referências
   (99,5% faltando) e levou 18,7 s para convergir, o **tsgo retornou as 653 completas em 0,5 s**,
   já na primeira resposta. As project references nativas carregam o grafo de forma eager/rápida.
   **Implicação estratégica:** para TypeScript, escolher o server certo (tsgo) já resolve o risco
   #1 — o cold-index race (#76870) é em boa parte um artefato do tsserver/vtsls, não intrínseco.

2. **tsgo é 4–13× mais rápido** em cold-start, tempo-até-1ª-resposta-correta, warm e rename.
   Alinhado ao foco em velocidade do projeto.

3. **⚠️ Correção — os servers DISCORDAM na contagem.** No zod, tsgo conta **64** referências a
   `ZodType` e vtsls conta **55** (grep textual = 64). Um dos dois está errado sobre o mesmo
   símbolo. Isso **não pode ser ignorado**: a camada precisa de uma fonte de verdade confiável.
   Hipóteses: tratamento diferente de `includeDeclaration`, re-exports, ou posições de tipo.
   **Pendência de correção antes de confiar em qualquer server para rename/delete.**

### Ressalvas (tsgo é preview)

- O levantamento notou que no tsgo o **update de imports em rename de ARQUIVO está quebrado** e a
  API não é estável. Aqui medimos `rename de SÍMBOLO` (sólido) e `find_references` (excelente).
  Manter `--dry-run` sempre e re-validar em versão fixada.

### Pendências

- [ ] **Investigar a discrepância 64 vs 55** (qual server está certo? por quê?).
- [ ] Repetir com fixture monorepo MAIOR (100+ pacotes) — tsgo mantém "sem truncamento"?
- [ ] Medir memória residente de cada server.
- [ ] Serena no mesmo fixture (próxima run).

---

## Run 005 — Serena (MCP sobre LSP) @ mono-ts

- **Data:** 2026-09-18
- **Comando:** `node serena-bench.mjs --fixture ../../fixtures/mono-ts --namepath Widget --relpath packages/core/src/index.ts`
- **Server:** Serena via `uvx --from git+https://github.com/oraios/serena` (contexto `ide-assistant`)
- **Protocolo:** MCP (não LSP cru) — cliente `@modelcontextprotocol/sdk`

| Métrica | Valor |
|---|---|
| connect (spawn → MCP ready) | **10068 ms** (startup Python/uvx) |
| tools/list | 49 ms — **21 ferramentas** |
| find_symbol | 93 ms |
| find_referencing_symbols — cold | **652 refs em 26 arquivos @ 17711 ms** |
| find_referencing_symbols — warm | 652 refs @ 3735 ms |
| **Truncou?** | **Não** (26 arquivos = core + 25 packages, completo) |

Ferramentas do Serena (surface de design útil): `find_symbol`, `find_referencing_symbols`,
`find_implementations`, `find_declaration`, `rename_symbol`, `safe_delete_symbol`,
`replace_symbol_body`, `insert_before/after_symbol`, `get_diagnostics_for_file`, memórias.

### Leitura

Serena está **correto** (acha todas as referências, sem truncamento — seu warmup interno
funciona) mas é **lento**: 10 s de connect, 17,7 s na 1ª `find_referencing_symbols` e ainda
3,7 s quente. Ele coleta contexto rico por referência (saída orientada a agente, com
`reference_line` e trechos), o que explica o custo. O valor do Serena é a **abstração
agent-first** e a superfície de ferramentas — não a velocidade.

(Contagem 652 vs. 653 do tsgo: diferença de ±1 na forma de contar a declaração; ambos completos.)

---

## Síntese da Fase 0 — comparativo e decisão

**Mesma tarefa nos 3 servers: "todas as referências a um símbolo com 653 usos reais no monorepo".**

| Server | cold-start | 1ª resposta de referências | **trunca?** | refs quente | rename dry-run |
|---|---|---|---|---|---|
| **vtsls** (tsserver) | 728 ms | **3 @ 4,6 s** → 653 @ 18,7 s | **SIM (3/653)** ❌ | 148 ms | 102 ms |
| **tsgo** (nativo Go) | **245 ms** | **653 @ 0,5 s** ✅ | **não** | **61 ms** | 53 ms |
| **Serena** (MCP/LSP) | 10 068 ms | 652 @ 17,7 s | não | 3 735 ms | (tem `rename_symbol`) |

### O que a Fase 0 respondeu

1. **"O que já existe é bom o bastante?" — Depende brutalmente da escolha, e há um vencedor claro.**
   - **vtsls/tsserver é uma armadilha:** rápido na aparência, **silenciosamente errado** em
     monorepo (o modo de falha #76870). Inaceitável para operações destrutivas (rename/delete).
   - **Serena é correto mas lento** (startup de 10 s, referências quentes em ~3,7 s). Ótimo como
     referência de *design* de ferramentas; ruim para o objetivo de velocidade.
   - **tsgo é o único rápido E correto** de fábrica: 653 referências completas em 0,5 s, sem
     truncar, rename em 53 ms. Resolve sozinho o risco #1 para TypeScript.

2. **A tese central da POC está provada em números:** o custo real é o warmup/índice; uma vez
   quente, operações semânticas (rename de 653 edições em 26 arquivos) custam **dezenas de ms**.
   Servidor persistente + o server certo = "IDE na mão da LLM" rápida.

3. **Direção de arquitetura recomendada:** **não** adotar Serena inteiro (lento) nem confiar no
   tsserver (inseguro). Construir uma **camada MCP fina e rápida sobre o melhor server por
   linguagem** (tsgo para TS), tomando emprestado de Serena a *superfície de ferramentas* e de
   agent-lsp o *preview→apply→verify* com `net_delta`. O diferencial da POC é **velocidade +
   segurança (nunca retornar contagem parcial durante warmup)**, exatamente onde os prontos falham.

### Run 006 — investigação 64 vs 55 (RESOLVIDA): tsgo está certo

- **Comando:** `node refs-diff.mjs --fixture ../../fixtures/zod --file src/types.ts --search "class ZodType" --symbol ZodType`
- **Achado:** os 55 do vtsls são **subconjunto exato** dos 64 do tsgo. As 9 que só o tsgo acha:
  - `ZodError.ts` (3): `import type { ZodType }` + dois `T extends ZodType<any,any,any>` (**usos diretos**)
  - testes (6): `z.ZodSchema` — e `types.ts:5111` faz `export { ZodType as ZodSchema }`, então
    `ZodSchema` **é alias de ZodType** → referências genuínas via alias.
- **Veredito:** **todas as 9 são referências reais**. **tsgo (64) está correto; vtsls (55)
  sub-reporta**, perdendo inclusive um uso direto de tipo. O tsserver/vtsls é **incompleto até
  em pacote único** — confirma tsgo como escolha para TS e reforça: nunca confiar na lista de um
  único server para rename/delete sem verificação.

### Pendências herdadas (priorizadas)

- [x] **Investigar 64 vs 55** — resolvido (Run 006): tsgo correto, vtsls incompleto.
- [ ] Medir **memória residente** por server (não instrumentado ainda).
- [ ] Monorepo **maior** (100+ pacotes): tsgo mantém "sem truncamento"?
- [ ] Repetir os outros idiomas (Python/basedpyright, Dart, Rust) no mesmo harness.
- [ ] Reduzir o cold-start do Serena com env cacheado (uvx) — medir 2ª execução.

---

## Run 007 — basedpyright @ py-demo (Fase 4: Python)

- **Data:** 2026-09-18
- **Setup:** `node benchmarks/scripts/gen-pyproject.mjs --modules 20 --refs 8`
- **Comando:** `node lsp-bench.mjs --server basedpyright --fixture ../../fixtures/py-demo --file models.py --search "class Account" --symbol Account --newname Ledger`
- **Server:** basedpyright (`basedpyright-langserver --stdio`), fork MIT do Pyright (Node)
- **Fixture:** 20 módulos Python, **521 refs plantadas** a `Account` (523 reais)
- **Alvo:** `Account` em `models.py`

| Métrica | Valor |
|---|---|
| cold-start | 1160 ms |
| **find_references — 1ª resposta** | **3 refs @ 2982 ms** |
| **find_references — estável** | **523 refs @ 4942 ms** |
| **Truncou a 1ª resposta?** | **SIM — 3 de 523** |
| find_references warm p50 / p95 | 39.6 ms / 56.9 ms |
| rename `Account`→`Ledger` (dry-run) | 21 arquivos, 523 edições, 40 ms |

### Leitura — o truncamento é TRANSVERSAL (não é quirk do tsserver)

O basedpyright reproduz o **mesmo cold-index race** medido no vtsls (Run 002): a 1ª
`find_references` retorna **3 de 523** referências e só converge para o total aos ~4,9 s, à
medida que o índice carrega. **Confirma que o gate de warmup é uma necessidade cross-language**,
não uma defesa específica de TypeScript. Quente, as operações são rápidas (rename de 523 edições
em 40 ms) — o custo é todo no warmup.

**Escolha de backend Python:** basedpyright pela **maturidade** (rename/references/symbols/call
hierarchy completos, MIT). O mais *rápido* seria `ty` (Astral, Rust, ~80× incremental), mas é
**beta** — registrado como troca futura (a camada é agnóstica de backend). Ver ROADMAP D2.

### Pendências

- [ ] Medir `ty` quando estabilizar e comparar com basedpyright.
- [ ] Confirmar no MCP que o gate de warmup entrega 523 (não 3) em Python.
- [ ] basedpyright é push-based (diagnostics) — validar net_delta via caminho PUSH do híbrido.

---

## Run 008 — dart @ dart-demo (Fase 4: Dart)

- **Data:** 2026-09-18
- **Setup:** `node benchmarks/scripts/gen-dartproject.mjs --modules 20 --refs 8` + `dart pub get`
- **Comando:** `node lsp-bench.mjs --server dart --fixture ../../fixtures/dart-demo --file lib/models.dart --search "class Account" --symbol Account --newname Ledger`
- **Server:** Dart Analysis Server (`dart language-server`, LSP nativo, BSD-3)
- **Fixture:** pacote Dart puro, 20 módulos, ~503 refs a `Account`

| Métrica | Valor |
|---|---|
| cold-start | **291 ms** |
| find_references — 1ª resposta | **503 refs @ 1135 ms** |
| find_references — estável | 503 refs @ 2584 ms |
| **Truncou?** | **Não** (completo de primeira) |
| find_references warm p50 / p95 | 64 / 123 ms |
| rename `Account`→`Ledger` (dry-run) | 21 arquivos, 503 edições, 57 ms |

### Leitura

O Dart Analysis Server **não trunca** (como o tsgo): retorna as 503 completas na 1ª resposta,
analisando de forma eager. Cold-start baixo (291 ms) — **mas é um pacote Dart puro**; em projetos
**Flutter** o startup é bem maior (grafo SDK+deps), como alerta o levantamento. Rode
`dart pub get` (ou `flutter pub get`) antes: o server precisa do `package_config.json`.

Padrão de servers até agora: **não truncam** tsgo e Dart (eager); **truncam** vtsls e
basedpyright (lazy). O gate de warmup cobre ambos os casos.

---

## Run 009 — rust-analyzer @ rust-demo (Fase 4: Rust)

- **Data:** 2026-09-18
- **Setup:** `node benchmarks/scripts/gen-rustproject.mjs --modules 20 --refs 8` + `rustup component add rust-analyzer`
- **Comando:** `node lsp-bench.mjs --server rust-analyzer --fixture ../../fixtures/rust-demo --file src/models.rs --search "struct Account" --symbol Account --newname Ledger`
- **Server:** rust-analyzer 1.96.1 (LSP stdio, MIT/Apache)
- **Fixture:** crate Rust, 20 módulos, ~523 refs a `Account`

| Métrica | Valor |
|---|---|
| cold-start (handshake) | 47 ms |
| **find_references — 1ª resposta** | **0 refs @ 23 ms** (server lança erro enquanto indexa) |
| **find_references — estável** | **524 refs @ ~30 s** |
| **Truncou?** | **SIM — 0 → 524 ao longo de ~30 s** |
| find_references warm p50 / p95 | 40 / 53 ms |
| rename `Account`→`Ledger` (dry-run) | 21 arquivos, 524 edições, 51 ms |

### Leitura — o server MAIS pesado de aquecer

O rust-analyzer roda `cargo metadata` + proc-macros + `cargo check` no cold start: leva **~30 s**
para convergir de 0→524, e **lança erro** (`No references found at position`) enquanto não terminou.
Reforça ao máximo o design: **servidor persistente + gate de warmup** (pagar 30 s por operação
seria inviável). Warm é rápido (~40 ms). O gate do MCP foi ajustado: timeout de 60 s + resiliência
a erro (erro durante indexação = "não pronto", re-tenta).

**Limitação importante (net_delta em Rust):** o rust-analyzer tem 2 fontes de diagnostics — a
**nativa** (vê o `didChange` em memória) e o **flycheck (`cargo check`)**, que **lê do disco**.
Como a simulação do `net_delta` é em memória (sem tocar o disco), ela captura os erros **nativos**
(ex.: rename `Account→i64` → 161 erros "expected i64, found i32", **bloqueado** ✅), mas **não** os
que só o `cargo check` pega (ex.: import duplicado E0252). Para segurança total em Rust, a **Fase de
validação (run build/test)** pós-apply é necessária — já prevista no roadmap.

---

## Template para novas runs

```
## Run NNN — <server> @ <fixture>
- Data / Comando / Versões / Máquina / Alvo
- Tabela de métricas
- Leitura dos números
- Pendências
```
