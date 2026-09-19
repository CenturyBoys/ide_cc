# Experimento A/B — "com e sem a camada" (Time to Correct Change)

A pergunta que iniciou o projeto: *o agente com a camada semântica faz a **mudança correta** com
menos risco/tempo que sem ela?* Este experimento mede isso numa tarefa de rename.

## O fixture-armadilha

`fixtures/ab-rename` (gerado por `benchmarks/scripts/gen-ab-rename.mjs`). Tarefa: **renomear a
CLASSE `Widget` para `Gadget`**. O fixture contém armadilhas onde o rename por TEXTO erra e o
SEMÂNTICO acerta:

- uma **const `Widget` não-relacionada** em outro módulo (símbolo diferente, mesmo nome);
- **strings literais** `"Widget"`;
- um **comentário** mencionando Widget;
- **`WidgetFactory`** (substring).

Correção = build limpo (`tsc --noEmit`) **E** armadilhas intactas **E** classe renomeada.

## Nível 1 — A/B mecânico (determinístico, reproduzível)

Compara o mecanismo diretamente: texto-cru (`\bWidget\b`→sed, o que um agente sem tools faria) vs.
`rename_symbol` do MCP.

```bash
bash benchmarks/scripts/setup-fixtures.sh          # gera o fixture
cd benchmarks/harness && npm install
node ab-rename.mjs
```

**Resultado (2026-09-18):**

| | Correto? | Tempo | Observação |
|---|---|---|---|
| **SEM** (texto-cru) | **NÃO** | 2 ms | corrompeu as strings `"Widget"` e renomeou a **const não-relacionada** |
| **COM** (semântico) | **SIM** | 1446 ms | classe renomeada, todas as armadilhas intactas, build limpo |

**A lição-chave:** o texto-cru foi 700× mais rápido **e ainda compilou** — mas está **semanticamente
errado** (mudou literais de string e um símbolo alheio). É um **bug silencioso**: o agente sem a
camada nem percebe que quebrou. O "time to *correct* change" do texto-cru é, na prática, muito maior
(exige detectar e desfazer a corrupção). O semântico chega ao correto de primeira.

## Nível 2 — A/B do agente (Claude Code real, com/sem o MCP)

Mede o loop completo do agente. Requer o CLI `claude` autenticado. Roda a MESMA tarefa em duas
condições — `.mcp.json` presente (COM) e escondido (SEM) — e checa a correção do resultado.

```bash
bash benchmarks/scripts/ab-agent.sh
```

O script (ver `benchmarks/scripts/ab-agent.sh`): copia o fixture, roda `claude -p "renomeie a
classe Widget para Gadget"` nas duas condições, mede tempo e avalia a correção (mesmo critério do
Nível 1). No modo COM, a skill `semantic-refactor` orienta o agente a usar `rename_symbol`.

> Nota: o Nível 2 tem ruído (não-determinismo do agente) e depende de auth/ambiente; o Nível 1 é a
> prova determinística do mecanismo. Rode o Nível 2 para a métrica end-to-end quando quiser.
