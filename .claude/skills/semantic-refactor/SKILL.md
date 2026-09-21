---
name: semantic-refactor
description: Use ao renomear, mover, extrair ou deletar SÍMBOLOS de código (classe, função, método, variável, tipo) em TypeScript, Python, Dart, Rust ou C#. Garante que a operação seja SEMÂNTICA (via LSP) e não textual (grep/sed), evitando corromper strings, comentários e símbolos homônimos.
---

# Refactoring semântico (não use grep/sed para renomear símbolos)

> **Fonte da verdade = o servidor.** A orientação de uso completa vive **no próprio MCP `code-intel`**
> (é portátil a qualquer cliente, não só ao Claude Code). Puxe-a com a tool **`instructions`** do
> `code-intel` — ela traz o manual inteiro: roteamento grep-vs-semântico, confiar no gate de warmup,
> preview/simulate antes de aplicar, `blast_radius` antes de editar amplo, `safe_delete`, grep-sweep
> pós-rename e o loop de diagnostics. Esta skill é só um **ponteiro** para não duplicar (e divergir
> de) aquele texto.

## O essencial (o resto está na tool `instructions`)

- **Nunca** renomeie/mova/delete um símbolo com edição de texto (Edit/sed sobre as ocorrências de um
  nome): corrompe strings, comentários e homônimos — e muitas vezes **ainda compila** (bug
  silencioso). Use as tools semânticas do `code-intel` (`rename_symbol`, `move_symbol`,
  `extract_function`, `safe_delete`, `find_symbol`/`document_symbols`).
- **Confie no resultado do server** (passa pelo gate de warmup e mede o `net_delta`): confirme
  `stable: true` e **não releia** arquivos só para "conferir" referências de código.
- **Grep-sweep pós-rename** é o único caso em que o grep textual entra: varrer o nome **antigo**
  apenas em comentários/strings/docs/config (que `find_references` ignora de propósito) e **perguntar
  antes de tocar**. Detalhes e a justificativa de por que isso não contradiz "confie no semântico"
  estão no manual da tool `instructions`.

Chame `instructions` (do `code-intel`) sempre que precisar do guia completo. Você decide **o quê**;
a ferramenta garante a precisão mecânica e confere o resultado.
