# Estratégia de testes — estar à frente do problema

> Motivação: mesmo **depois** de construir a suíte e2e (v0.7.1) para pegar os bugs de campo, o
> **P8** (`rename_symbol` reporta `safe:true` num rename que QUEBRA o código, em Python) passou
> batido. Este documento explica **por que** passou, e define uma disciplina de teste que
> antecipa essa classe de falha em vez de reagir a relatórios.

## 1. A lacuna (root cause do gap de teste)

Nossa suíte e2e valida **caminhos felizes**: "o rename funciona", "o find_references acha",
"o move cria arquivo". Mas a **garantia central do produto** é o inverso:

> *o LLM decide o quê; a ferramenta faz o mecânico; o validador **confere** e **se recusa a
> aplicar** uma edição que introduz erro.*

Nunca testamos essa recusa. Todo caso e2e era **positivo** (operação boa → sucesso). Faltou o
**adversarial**: operação **sabidamente destrutiva → tem que ser bloqueada**. Um teste que só
confirma que uma rede de segurança deixa o certo passar **não detecta a rede desligada** —
só um teste que exige que ela **barre o errado** detecta.

### Por que cada achado passou

| Achado | Por que o teste não pegou |
|---|---|
| **P8** rename false-safe (Python) | `net_delta` depende de diagnósticos; com `typeCheckingMode: off` o basedpyright emite **0 diagnósticos** → `net_delta` é sempre `0` → tudo `safe:true`. Nunca testamos (a) um rename que DEVE ser inseguro, nem (b) sob a config `off` (comum em repo grande). |
| **P12** `new_name` sem validação | Nunca testamos nome inválido/keyword nem `new==old`. |
| **P11** extract/move em Python | Nunca testamos operação **não suportada** por linguagem. |
| **P10** lang válida sem fontes | Nunca testamos `workspace_symbols` com lang sem arquivos no projeto. |
| **C# move/find_symbol** | Só foram pegos **depois** de virarem caso e2e — reativo, não preventivo. |

Padrão comum: **cegueira de configuração** (uma config por linguagem) e **ausência de casos
negativos/adversariais**.

## 2. O princípio

**Testar a GARANTIA, não a feature.** Para cada rede de segurança, existe um teste que
**falharia se a rede estivesse inerte**. Concretamente, três eixos que faltavam:

1. **Casos adversariais** — a operação errada tem que ser barrada (não só a certa passar).
2. **Matriz de configuração** — a mesma operação sob configs que mudam o comportamento da rede
   (ex.: `typeCheckingMode: off` vs `basic`).
3. **Matriz de capacidade por linguagem** — o que cada backend suporta; o não-suportado tem que
   dar erro **explícito**, não silencioso nem enganoso.

## 3. Plano de CORREÇÃO (os bugs)

Prioridade pela severidade do relato (#3 + relatório C#):

- **P8 — rename false-safe (SÉRIO).** A segurança do rename não pode depender **só** de
  `net_delta` (diagnósticos). Adicionar **detecção explícita de colisão de nome no mesmo escopo**:
  antes de aplicar `old→new`, olhar os `document_symbols` do container do alvo; se já existe um
  irmão chamado `new`, é colisão → `safe:false`/rejeitado, **independente de diagnósticos**.
  Também: quando `typeCheckingMode: off` (net_delta não confiável), sinalizar no resultado que a
  rede baseada em diagnóstico está inerte.
- **P9 — `validate_build`/`verify_build` Python no-op.** Definir um comando default sensato
  (`basedpyright --outputjson <project>` ou `python -m py_compile`) e, quando `verify_build=true`
  sem comando disponível, **avisar** em vez de silenciar.
- **P12 — validação de `new_name` + noop.** Rejeitar `new_name` inválido (não-identificador) ou
  keyword da linguagem; tratar `new==old` como **noop** explícito.
- **P11 — extract/move não suportado em Python.** Erro **explícito** "unsupported for `<lang>`"
  em vez de "nenhum refactoring disponível"; `doctor`/descrições declaram o suporte por linguagem.
- **P10 — lang válida sem fontes.** Antes do warmup de 60s, se o projeto não tem fontes da lang
  pedida, retornar rápido "nenhum fonte `<lang>` encontrado".
- **C# (relatório):** itens 1 (`move_symbol` grava overrides) e 2 (`find_symbol` composto) já
  resolvidos no 0.7.1 — **fixar com casos e2e adversariais** (abaixo) para não regredir. Item 3
  (`record` como `kind:Class`) fica como baixo (LSP não tem kind Record).

## 4. Plano de PREVENÇÃO (a camada que faltava)

Nova seção adversarial na suíte e2e (`mcp/e2e/`), além dos casos positivos atuais:

### 4.1 Casos negativos por rede de segurança
Para cada tool com garantia, um caso que **exige o bloqueio**:
- `rename_symbol` para nome que **colide** no mesmo escopo → `safe:false` (Python, TS, C#).
- `rename_symbol` para **keyword**/nome inválido → rejeitado (não noop).
- `rename_symbol` `new==old` → **noop** explícito.
- `extract_function` de bloco **inseguro** (variável usada depois) → `safe:false`.
- `move_symbol` que não cria arquivo → **`move_no_op`** (C#).
- operação **não suportada** por linguagem (extract/move em Python) → erro `unsupported`.

### 4.2 Matriz de configuração
Rodar o fixture Python em **duas** configs — `typeCheckingMode: off` e `basic` — e exigir que a
**colisão de rename seja pega nas DUAS** (prova que a segurança não depende do modo de type-check).

### 4.3 Canary da rede de segurança
Um caso "canário": injeta uma edição **sabidamente destrutiva** e exige `safe:false`. Se algum dia
uma refatoração deixar a rede inerte (como o `net_delta` sempre-0), o canário fica vermelho na hora.

### 4.4 Matriz de capacidade por linguagem
Tabela versionada (o que cada backend suporta: nav/rename/extract/move/workspace) validada por
casos e2e — o não-suportado precisa de erro explícito. Vira também documentação para o usuário.

## 5. Ordem sugerida

1. **P8 + canary + caso adversarial de colisão** (fecha o buraco mais perigoso e o previne juntos).
2. **P9** (destrava a 2ª rede de segurança do rename em Python).
3. **P12 / P11 / P10** (validação + clareza de suporte) com seus casos negativos.
4. **Matriz de config** (Python off/basic) e **matriz de capacidade**.
5. Cada correção entra com seu caso e2e **no mesmo PR** (nada de fix sem teste que o guarde).

## 6. Regra permanente (IA-first)

> Toda correção de bug de segurança/correção entra com **dois** testes: um que mostra a operação
> boa passando **e** um adversarial que mostra a operação ruim sendo **barrada**. Sem o adversarial,
> o PR está incompleto — porque é o adversarial que detecta a rede desligada.

## 7. Status (implementado)

- [x] **4.1 Casos negativos** — colisão, keyword, noop, extract/move unsupported, move_no_op, build
  verde (em `mcp/e2e/cases.json`).
- [x] **4.2 Matriz de configuração** — fixtures `python` (`typeCheckingMode=off`) e `python-basic`
  (`basic`): a colisão de rename é barrada nas DUAS → a rede não depende de diagnósticos.
- [x] **4.3 Canary** — o fixture `python` com `off` é o canário: fica vermelho se o rename voltar a
  depender só do `net_delta`.
- [x] **4.4 Matriz de capacidade por linguagem** — tabela versionada em `mcp/e2e/README.md`,
  validada pelos casos (não-suportado → erro explícito).
- [x] **Correções** P8, P9, P10, P11, P12 + `validate_build` C# + `record`→`Record` (v0.7.2).

Cobertura atual: **17 testes unitários + 28 casos e2e** (5 language servers reais no CI) + modo
`--real`. Todo verde.
