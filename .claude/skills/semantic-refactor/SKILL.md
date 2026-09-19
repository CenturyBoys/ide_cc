---
name: semantic-refactor
description: Use ao renomear, mover, extrair ou deletar SÍMBOLOS de código (classe, função, método, variável, tipo) em TypeScript, Python, Dart, Rust ou C#. Garante que a operação seja SEMÂNTICA (via LSP) e não textual (grep/sed), evitando corromper strings, comentários e símbolos homônimos.
---

# Refactoring semântico (não use grep/sed para renomear símbolos)

Este projeto expõe o MCP `code-intel` com operações **semânticas** de código. Renomear/mover/
deletar um símbolo com `grep`+`sed` (texto) é **perigoso**: corrompe strings literais, comentários
e outros símbolos com o mesmo nome — e o pior é que muitas vezes **ainda compila** (bug silencioso).

## Regras

1. **Nunca** renomeie um símbolo com edição de texto (Edit/sed em cima de todas as ocorrências
   de um nome). Use `rename_symbol` do MCP `code-intel`.
2. **Antes** de renomear/deletar, chame `find_references` para ver o alcance real (e confirme que
   o resultado veio `stable: true` — senão o índice ainda está aquecendo).
3. **Ao aplicar**, use `rename_symbol` com `apply: true`. Em Rust (ou quando quiser garantia de
   build), adicione `verify_build: true` — aplica só se o build passar, senão reverte.
4. Para achar "método dentro de classe", use `find_symbol` (name_path `Classe/metodo`) ou
   `document_symbols`; não adivinhe posição por texto.
5. Extrair função / mover símbolo: `extract_function` / `move_symbol` (não recorte-e-cole à mão).

## Fluxo típico (rename)

```
find_references(project, file, symbol)         # veja o alcance; confirme stable:true
rename_symbol(project, file, symbol, new_name, apply:true, verify_build:true)
```

O servidor cuida do gate de warmup (nunca conta referências parciais), do `net_delta` (só aplica
se não introduzir erros) e da validação de build. Você decide **o quê**; a ferramenta garante a
precisão mecânica.
