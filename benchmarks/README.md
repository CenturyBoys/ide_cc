# Benchmarks — Code Intelligence Layer (Fase 0)

> **Regra do projeto:** todo benchmark é **documentado em markdown** e **reproduzível**
> a partir deste repositório, com fixtures e versões de ferramentas **fixadas**.
> Resultados ficam em [`results/RESULTS.md`](results/RESULTS.md).

Objetivo da Fase 0: medir o comportamento **fundamental** de um language server (o que
qualquer camada — Serena ou a nossa — herda) para responder objetivamente:
**quão rápido, e a partir de quando o resultado é confiável.**

## O que é medido

| Métrica | Por quê |
|---|---|
| **cold-start** (spawn → `initialize`/`initialized`) | custo fixo pago 1× por processo |
| **find_references: 1ª resposta vs. estável** | detecta o **truncamento silencioso** (cold-index race, [claude-code#76870](https://github.com/anthropics/claude-code/issues/76870)) |
| **find_references warm p50/p95** | latência de navegação com índice quente |
| **rename dry-run: latência + blast radius** | custo da operação semântica mecânica (sem aplicar no disco) |

## Reprodução (do zero)

Requisitos: `node` ≥ 20, `git`. Testado com node v24.18.0.

```bash
# 1. a partir da raiz do repositório
bash benchmarks/scripts/setup-fixtures.sh    # clona zod v3.23.8 + gera monorepo sintético

# 2. instalar o harness (vtsls + tsgo + TypeScript + libs LSP/MCP)
cd benchmarks/harness
npm install

# 3a. LSP cru — vtsls vs tsgo, em pacote único (zod) e monorepo (mono-ts)
node lsp-bench.mjs --server vtsls --fixture ../../fixtures/zod
node lsp-bench.mjs --server tsgo  --fixture ../../fixtures/zod
node lsp-bench.mjs --server vtsls --fixture ../../fixtures/mono-ts \
  --file packages/core/src/index.ts --search "class Widget" --symbol Widget --newname Gadget
node lsp-bench.mjs --server tsgo  --fixture ../../fixtures/mono-ts \
  --file packages/core/src/index.ts --search "class Widget" --symbol Widget --newname Gadget

# 3b. Serena (MCP sobre LSP) — mesmo monorepo
node serena-bench.mjs --fixture ../../fixtures/mono-ts \
  --namepath Widget --relpath packages/core/src/index.ts
```

Saída: tabela no terminal + JSON bruto em `benchmarks/results/<server>-<fixture>.json`.
Depois de rodar, **registre os números em [`results/RESULTS.md`](results/RESULTS.md)** com
a data e as versões (veja o template lá).

## Versões fixadas (ambiente da última execução)

| Componente | Versão |
|---|---|
| node | v24.18.0 |
| @vtsls/language-server | 0.3.0 |
| @typescript/native-preview (tsgo) | 7.0.0-dev.20260707 |
| @modelcontextprotocol/sdk | (última) |
| typescript | 7.0.2 |
| fixture zod | v3.23.8 (`ca42965`) |
| fixture mono-ts | gerado (25 pacotes, 653 refs) |

## Estrutura

```
benchmarks/
├── README.md            # este arquivo (como reproduzir)
├── harness/
│   ├── lsp-bench.mjs     # harness: driva o LSP via stdio e mede
│   └── package.json      # deps do harness (vtsls, typescript, vscode-jsonrpc)
├── scripts/
│   └── setup-fixtures.sh # baixa fixtures em versões fixadas
└── results/
    ├── RESULTS.md        # resultados documentados (commitado)
    └── *.json            # saída bruta (gitignored)
```

## Alvo do benchmark

Símbolo: **`ZodType`** (classe abstrata base), definida em `fixtures/zod/src/types.ts`.
É o símbolo central da lib — referenciado por toda a hierarquia de schemas, ideal para
estressar `find_references`/`rename`. O harness localiza a posição do símbolo
programaticamente (não hardcoda linha/coluna), então continua válido se o fixture mudar.

## Estado (Fase 0)

Já medidos: **vtsls**, **tsgo** e **Serena** em pacote único (zod) e monorepo (mono-ts).
Conclusão em [`results/RESULTS.md`](results/RESULTS.md) → *Síntese da Fase 0*: **tsgo** é o
único rápido **e** correto (não trunca no monorepo); vtsls trunca em silêncio; Serena é
correto mas lento.

**Python (Fase 4):** `basedpyright` medido em `py-demo` (Run 007) — trunca 3→523 igual ao vtsls,
confirmando que o cold-index race é **cross-language**. Rodar:
`node lsp-bench.mjs --server basedpyright --fixture ../../fixtures/py-demo --file models.py --search "class Account" --symbol Account --newname Ledger`

**Dart (Fase 4):** `dart language-server` medido em `dart-demo` (Run 008) — **não trunca** (eager,
como o tsgo), cold-start 291 ms em pacote puro. Rode `dart pub get` no fixture antes.

**Rust (Fase 4):** `rust-analyzer` medido em `rust-demo` (Run 009) — server **mais pesado**
(cold ~30s: cargo metadata+check), **trunca** 0→524. `rustup component add rust-analyzer`.

Próximos: memória residente por server; monorepo maior (100+ pacotes); C# (requer dotnet);
comparar `ty` (Rust) vs basedpyright; divergência tsgo(64) vs vtsls(55) no zod.
