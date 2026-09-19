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

**Resultado (2026-09-18, Claude Code 2.1.193, 3 execuções):**

| Rodada | SEM a camada | COM a camada |
|---|---|---|
| 1 | **INCORRETO** (6 s) | CORRETO (42 s) |
| 2 | CORRETO (39 s) | CORRETO (30 s) |
| 3 | CORRETO (43 s) | CORRETO (48 s) |
| **Resumo** | **2/3 corretos** | **3/3 corretos** |

**Leitura honesta.** Um modelo forte **sem** a camada é bom nesta tarefa — ele **raciocina** (lê os
arquivos, entende as armadilhas, edita cirurgicamente), não faz `sed` cego; acertou 2/3. Mas **não é
confiável**: falhou 1/3. **Com** a camada foi 3/3, e com **garantia** (net_delta + build) em vez de
depender do agente ter raciocinado certo. Os tempos foram comparáveis (a ferramenta paga o startup
do MCP; a vantagem de tempo aparece em tarefas grandes, ex.: o monorepo de 653 refs).

**Conclusão dos dois níveis.** O Nível 1 (determinístico) prova que **edição de texto cega
corrompe** (o pior caso). O Nível 2 mostra que um agente forte mitiga isso raciocinando, mas a
camada troca "geralmente certo por raciocínio" por **"correto por construção, com garantia"** — o
ganho é **confiabilidade**, e cresce com modelos menores, tarefas maiores e alto risco de correção.

> Nota: o Nível 2 tem ruído (não-determinismo do agente) e depende de auth/ambiente; o Nível 1 é a
> prova determinística do mecanismo.
