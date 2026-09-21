---
name: diagnostics-fix-loop
description: Use ao corrigir erros de compilação/tipo que se espalham por VÁRIOS arquivos ou ao estabilizar o projeto depois de uma edição (a suíte falhando, erros em arquivos que você não tocou). Ensina o loop de nível-projeto — rodar check/suíte → ler diagnostics estruturados → corrigir → re-check até limpo — SEM duplicar a auto-verificação que a tool já faz na própria edição.
---

# Loop de correção de diagnostics (nível-projeto)

Depois de uma mudança, o projeto pode ficar com erros **fora do arquivo que você editou**:
callers em outros pacotes, testes que assumiam a assinatura antiga, imports quebrados. Esse é um
loop de **nível-projeto** — rodar o check da linguagem, ler os erros estruturados, corrigir, e
**re-checar até limpo**.

## Divisão de papéis (não duplique a tool)

- **A tool garante o build da EDIÇÃO.** `rename_symbol`/`extract_function`/`move_symbol` já rodam
  `net_delta` (simulam em memória; só aplicam se não introduzirem erros) e, com `verify_build: true`,
  `validate_build` no disco (reverte se o build quebrar). Ou seja: a operação semântica em si já
  chega verificada. **Não** re-verifique manualmente o resultado de um rename que passou.
- **Esta skill cuida do resto do projeto.** Erros que a edição não causou (ou que aparecem só quando
  você roda a suíte de testes), diagnostics em arquivos que a operação não abriu, quebras de lógica
  que o compilador não pega. É aí que o loop entra.

## O loop

1. **Check.** Rode o verificador da linguagem (`validate_build` do MCP, ou `cargo check` / `tsc`
   `--noEmit` / `pyright` / `dart analyze`, e a suíte de testes quando fizer sentido).
2. **Ler erros estruturados.** Leia o diagnostic como dado: `arquivo:linha:coluna` + mensagem +
   código. Não deduza o erro "de cabeça" — use o que o checker reporta.
3. **Corrigir a causa, não o sintoma.** Vá ao símbolo apontado (use `find_symbol`/`document_symbols`
   para localizar sem chutar linha). Se a correção é de novo um refactor de símbolo, use a tool
   semântica (que se auto-verifica) em vez de Edit textual.
4. **Re-check.** Rode o check de novo. Repita 1–4 **até zero erros**. Corrija os erros em ordem —
   um erro na base costuma gerar vários derivados que somem juntos.

## Regras

- **Não pare no "compilou".** Rode também os testes relevantes: o build passar não prova
  comportamento. Erros de lógica não aparecem no `net_delta`.
- **Um erro por vez quando estiverem encadeados.** Resolva o primeiro/mais fundamental e re-check —
  evita corrigir sintomas que já iam sumir.
- **Nunca silencie diagnostics** (cast, `any`, `# type: ignore`, `#[allow]`) para "fechar o loop".
  Isso mascara o problema — exatamente o bug silencioso que a camada tenta evitar.
- **Confie na verificação da tool.** Se a edição semântica passou pelo `net_delta`/`validate_build`,
  ela está certa mecanicamente; foque o loop no que está ao redor.

## Fluxo típico

```
rename_symbol(..., apply:true, verify_build:true)   # a EDIÇÃO já vem verificada pela tool
validate_build(project)                             # nível-projeto: sobrou erro em outro arquivo?
# ler diagnostics → corrigir caller/teste → validate_build de novo → repetir até limpo
# rodar a suíte de testes → iterar
```

Você decide **o quê** corrigir; a tool garante o build da própria edição; o loop garante que o
**projeto inteiro** volta ao verde.
