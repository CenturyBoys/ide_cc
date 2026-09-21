---
name: context-pack-by-symbol
description: Use ao explorar/entender código antes de editar — para carregar contexto por SÍMBOLO em vez de ler arquivos inteiros. Ensina a estratégia document_symbols (visão geral) → ler só o corpo do símbolo-alvo → expandir via find_references/call_hierarchy sob demanda. Evita "engolir" o arquivo todo, mantém o contexto enxuto e aumenta a taxa de acerto.
---

# Empacotar contexto por símbolo (não engula o arquivo inteiro)

Ler um arquivo grande inteiro para achar um símbolo enche o contexto de código irrelevante: gasta
tokens, dilui a atenção e **piora** a taxa de acerto da tarefa. A estratégia é carregar contexto
**por símbolo**, de fora pra dentro, expandindo só o necessário.

## A estratégia (3 passos, sob demanda)

1. **Visão geral primeiro — `document_symbols`.** Pegue o *mapa* do arquivo (classes, funções,
   métodos, tipos e onde cada um começa) sem ler o corpo de nada. É a "planta baixa".
2. **Ler só o corpo do símbolo-alvo.** Localizado o símbolo (`find_symbol` com name_path
   `Classe/metodo`, ou o range vindo de `document_symbols`), leia **apenas aquele trecho** — não o
   arquivo todo. Prefira um `Read` com `offset`/`limit` no range do símbolo.
3. **Expandir sob demanda.** Só quando precisar entender o entorno:
   - `find_references` → onde o símbolo é usado (callers, dependentes).
   - `call_hierarchy` → quem chama / quem é chamado (a cadeia de execução).
   - `workspace_symbols` → achar um símbolo relacionado em outro arquivo pelo nome.

   Puxe cada um só quando a pergunta atual exigir; não pré-carregue "por via das dúvidas".

## Por que (mantém o contexto enxuto)

- **Menos tokens = mais atenção onde importa.** Contexto cheio de código irrelevante degrada a
  qualidade da resposta; símbolos relevantes se perdem no meio do ruído.
- **Precisão semântica.** `find_references`/`call_hierarchy` dão o alcance **real** do símbolo (via
  LSP), sem falsos positivos de homônimos que um grep textual traria.
- **Maior taxa de acerto por tarefa.** Ler focado por símbolo reduz re-leituras e leva à mudança
  correta mais rápido — que é a métrica-alvo da camada.

## Regras

- **Nunca** leia o arquivo inteiro só para localizar um símbolo — use `document_symbols`/`find_symbol`.
- **Não** pré-expanda references/hierarchy antes de precisar; expanda **sob demanda**.
- Ao editar/renomear o símbolo depois, use as tools semânticas (`rename_symbol` etc.); o contexto
  que você montou por símbolo já indica o alcance.
- Se o símbolo estiver espalhado por vários arquivos, siga `find_references` de um em um — leia o
  corpo de cada ocorrência relevante, não o arquivo que a contém.

## Fluxo típico

```
document_symbols(project, file)                 # planta baixa do arquivo
find_symbol(project, "Classe/metodo")           # localizar o alvo (sem chutar linha)
Read(file, offset=<início do símbolo>, limit=<tamanho do corpo>)   # só o corpo
find_references(project, file, symbol)           # SÓ se precisar do alcance real
call_hierarchy(project, file, symbol)            # SÓ se precisar da cadeia de chamadas
```

Carregue de fora pra dentro, símbolo a símbolo. Contexto enxuto → resposta melhor.
