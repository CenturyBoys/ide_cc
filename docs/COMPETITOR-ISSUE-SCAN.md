# Varredura de Issues de Concorrentes — Ranking de Classes de Problema

> **Objetivo.** Levantar as classes de problema recorrentes em projetos SIMILARES ao nosso
> (`CenturyBoys/ide_cc` — MCP server `code-intel-mcp`, Rust, operações semânticas sobre Language
> Servers) para construir um RANKING de riscos que provavelmente também temos, cruzando com o que
> já tratamos. Data da varredura: 2026-09-20.

## Método e cobertura honesta

Coletados TÍTULOS (e corpos de issues selecionadas) via `gh issue list`/`gh issue view` em:

| Repo | Issues acessadas | Observação |
|---|---|---|
| **oraios/serena** | ~200 (open+closed) | Fonte mais rica; Python + SolidLSP, multi-linguagem. O melhor espelho de risco funcional. |
| **bug-ops/mcpls** | ~180 (open+closed) | **Rust, arquitetura quase idêntica à nossa** (bridge LSP↔MCP). O melhor espelho de risco de implementação. |
| **isaacphi/mcp-language-server** | ~55 (open+closed) | Go; problemas de fd/spawn e cobertura de tools. |
| **jonrad/lsp-mcp** | 3 | Repo pequeno; pouco sinal. |
| **Tritlo/lsp-mcp** | 5 | Fork; pouco sinal. |
| **crepererum-oss/common-sense-coder** | 5 | Sinal de "roots"/diretórios/Windows. |
| **karellen/karellen-lsp-mcp** | 2 | Sinal de exposição de tools por linguagem. |
| **beixiyo/vsc-lsp-mcp** | 6 | Extensão VS Code; "um LSP por janela". |
| lostbean/agent-lsp | 0 | **Issues DESABILITADAS** — não acessível. |
| sminnee, alexwohletz, jaenster/ts-lsp-mcp | 0 | Sem issues abertas. |

Limite de cobertura: `lostbean/agent-lsp` tem issues desabilitadas (não deu para inspecionar).
Repos menores têm poucas issues, então o sinal estatístico vem de **serena** e **mcpls**.

Cruzamento com nosso código: inspecionei `mcp/src/lsp.rs` e `mcp/src/main.rs` para confirmar
presença/ausência real de cada mitigação (não só o CLAUDE.md).

---

## Classes de problema

### 1. Encoding de posição UTF‑16 vs code-points/bytes  ⚠️ GAP CRÍTICO

- **Concorrentes.** serena **#2064 (OPEN)** — SolidLSP manda coluna como índice de code-point Python
  e nunca negocia `general.positionEncodings`; adapters (jdtls, rust-analyzer, kotlin, sourcekit)
  anunciam `utf-16` → mismatch. mcpls **#290 (CLOSED)**, **#287 (CLOSED)** — encoding negociado /
  campo `position_encodings` parseado mas **nunca consumido** na conversão; **#413 (CLOSED)** —
  incremento u32 sem checagem em `lsp_to_mcp_position` estoura. serena **#2017 (OPEN)** e **#1558
  (CLOSED)** — `UnicodeDecodeError` quando o encoding do arquivo difere do encoding do projeto.
- **Probabilidade nossa: ALTA.** Confirmado no código: `mcp/src/main.rs` `locate()`/`find_ident()`
  (linhas ~116‑135) retornam um índice de **byte/char da string Rust** e o enviam direto como
  `character` LSP (`main.rs:226` `"character":ch`). Não há negociação de `positionEncodings` no
  `initialize` (`mcp/src/lsp.rs:144‑158` — o bloco `capabilities` não tem `general`). Para qualquer
  conteúdo não-ASCII **antes** do símbolo na mesma linha (identificador acentuado, string com emoji,
  CJK, comentário unicode), a coluna sai errada → rename/extract/find_references apontam para o lugar
  errado, silenciosamente.
- **Recomendação/teste.** (a) Enviar `capabilities.general.positionEncodings: ["utf-16"]` e ler o
  `positionEncoding` retornado. (b) Converter colunas para UTF‑16 code units ao **enviar** e de volta
  ao **receber** (contar `ch.len_utf16()` ao varrer a linha em `find_ident`/`pos_to_offset`).
  (c) Fixture de teste: arquivo com identificador precedido de `// café ☕` na mesma linha; renomear e
  conferir que o edit não corta bytes. Adicionar em `mcp/test-*.jsonl`.

### 2. Sandbox de path / path traversal / roots  ⚠️ GAP

- **Concorrentes.** mcpls **#417 (CLOSED)** — `validate_path_against_roots` **falha aberto**: se
  `workspace_roots` vazio, aceita qualquer path; **#428/#415 (CLOSED)** — respostas do LSP não
  filtradas por root; **#411 (CLOSED)** — `parse_file_uri` faz slicing cru de `file://` e **nunca
  percent-decodifica** (diverge do que o próprio server produz). crepererum **#22/#21/#19 (OPEN)** —
  suporte a MCP "roots", uso de diretórios como path, Windows. serena **#1602 (CLOSED)** —
  `is_path_in_project` falso-negativo com forward slashes (Git Bash/MSYS).
- **Probabilidade nossa: MÉDIA‑ALTA.** Confirmado: `mcp/src/lsp.rs:363‑367` converte URI com
  `format!("file://{}")` e `strip_prefix("file://")` — **sem percent-encode/decode** (paths com
  espaço, `#`, `%`, acento quebram, igual mcpls #411). Containment usa `p.starts_with(&root)`
  (`main.rs:1466`) **sem `canonicalize`** — vulnerável a `..`, symlink e prefixo parcial
  (`/root2` "começa com" `/root`). Não há gate de "path fora do root" para paths vindos do cliente.
- **Recomendação/teste.** (a) `canonicalize` no root e no path do cliente antes de `starts_with`, e
  comparar por componentes (não string). (b) Trocar a conversão URI por `url::Url::from_file_path`/
  `to_file_path` (percent-safe). (c) Testes: path com espaço/acento; path com `../` tentando sair do
  root; symlink apontando para fora → deve recusar.

### 3. Cold-start / índice / readiness race

- **Concorrentes.** serena **#1937/#1858/#1681 (waits fixos → resultados parciais)**, **#1814
  (CLOSED)** (`find_referencing_symbols` retorna `{}` após tsserver morrer de OOM, reportado como
  "indexing complete", `isError:false`), **#2076 (CLOSED)** (`find_symbol` sem `relative_path` nunca
  retorna em repo grande), **#1923 (OPEN)** (indexing marcado completo mesmo com todos os arquivos
  falhando ao abrir). mcpls **#420/#423/#445/#422 (CLOSED)** — tools read-only respondem de workspace
  não indexado (vazio indistinguível de genuinamente-vazio); gate de readiness contornado por
  call_hierarchy/workspace_symbol/diagnostics; detecção de indexação via `$/progress` faltando para
  não-rust-analyzer.
- **Probabilidade nossa: JÁ TRATADO (parcial).** Temos gate de warmup + `CODE_INTEL_WARMUP_MS` e
  "nunca retorna contagem parcial durante indexação". **Variante mais funda a checar:** (i) o gate
  cobre TODAS as tools (workspace_symbols, call_hierarchy, find_symbol) ou só find_references? mcpls
  teve que corrigir bypass tool-a-tool. (ii) Detectamos LSP morto/OOM e distinguimos "0 refs" real
  de "server caiu"? serena #1814 é o pior caso: resposta vazia mascarada como sucesso.
- **Recomendação/teste.** Teste que mata o backend no meio e confirma que a tool retorna ERRO (não
  `[]`). Confirmar que o warmup gate está no caminho de cada tool semântica, não só uma.

### 4. Sincronização de arquivo / índice obsoleto (freshness)

- **Concorrentes.** serena **#1718 (CLOSED)** — `didChangeWatchedFiles` definido mas nunca enviado →
  LS quente serve símbolo obsoleto após edição externa; **#2005 (CLOSED)** — didChange no re-open
  reusa o mesmo número de versão; **#1712 (CLOSED)** (Vue/Svelte pinados vão stale; Svelte perde as
  próprias edições); **#1593 (OPEN)** (find_symbol stale em Clojure). serena **#2077 (OPEN)** — o poll
  de freshness custa um `os.stat` por arquivo em cada chamada (custo de performance da própria
  solução de freshness).
- **Probabilidade nossa: JÁ TRATADO (com risco de custo).** Temos `ensure_open` com re-sync por
  mtime (`lsp.rs:203`) e teste de git-revert externo. **Variantes a checar:** (i) versionamento do
  didChange — incrementamos version a cada didChange? (serena #2005). (ii) O custo do `disk_mtime`
  por arquivo em cada chamada (serena #2077) escala? (iii) Nós enviamos `didChangeWatchedFiles` OU só
  reagimos por mtime no arquivo-alvo? Edições externas em arquivos **não** tocados pela chamada atual
  podem ficar stale no índice do server.
- **Recomendação/teste.** Confirmar versionamento monotônico do didChange. Teste: editar externamente
  um arquivo B, depois `find_references` de símbolo em A que referencia B → o server deve refletir B.

### 5. Rename/edit over-reach & corrupção silenciosa de edição

- **Concorrentes.** serena **#1956 (OPEN)** — `replace_symbol_body` DUPLICA `export const` num const
  top-level, retorna OK mas gera código inválido; **#1952 (OPEN)** (remove whitespace separador antes
  de função GDScript); **#1697 (CLOSED)** (linhas em branco extras no delete); **#1744 (OPEN)** (zls
  rename omite edits em arquivos não abertos → refs stale); serena **#1548 (OPEN)** (permitir renomear
  parâmetros). isaacphi **#104 (OPEN)** (rename limitado a um arquivo), **#86 (OPEN)** (references de
  classe falham mas de métodos funcionam).
- **Probabilidade nossa: MÉDIO / parcial JÁ TRATADO.** Temos `net_delta` + `verify_build` + detecção
  de colisão + validação de new_name. Isso pega corrupção que **introduz erro de compilação**. **Mas
  o pior caso serena #1956/#1952 é corrupção que passa no compilador** (duplicação de prefixo,
  whitespace) — pode não gerar erro de build e escapar do `net_delta`. **#1744 (rename cross-file
  omitindo arquivos não abertos)** é um risco direto: garantimos que TODOS os arquivos com referências
  são aplicados, mesmo os que o server não tinha aberto?
- **Recomendação/teste.** (a) Teste de rename cross-file com arquivo referenciador NUNCA aberto na
  sessão → conferir que o edit chega nele (não confiar só em `didOpen` prévio). (b) Snapshot-test
  byte-a-byte de rename em `export const` e em símbolo com whitespace ao redor.

### 6. Escopo de workspace / ruído / monorepo / multi-root

- **Concorrentes.** serena **#1939/#1766/#1627/#1586 (CLOSED/OPEN)** — monorepo TS sub-reporta
  cross-package sem project references; Metals/tsserver recebem só a raiz → só um build do monorepo é
  servido; sem como escopar LSP a N sub-roots (tudo-ou-nada); tsserver carrega como inferred project,
  não o tsconfig real. serena **#1999/#1991/#1628 (CLOSED/OPEN)** — descoberta bypassa `.gitignore`,
  startup lento (35s), walk de first-index lento. mcpls **#353 (CLOSED)** (`tool_prefix` p/
  desambiguar múltiplas bridges).
- **Probabilidade nossa: JÁ TRATADO (parcial).** Temos roteamento por-linguagem + project-scoping
  (excluir .pub-cache/SDK) + tratamento de ruído substring em workspace_symbols. **Variantes a
  checar:** multi-root de verdade (monorepo com N sub-projetos, cada um com seu tsconfig/pyproject) —
  passamos a raiz única ou o sub-root correto? serena teve que corrigir "Metals recebe a raiz do
  monorepo → um build só". Nosso `rootUri` é sempre a raiz do projeto (`lsp.rs:137`).
- **Recomendação/teste.** Fixture monorepo com 2 packages TS + project references; `find_references`
  cross-package deve achar tudo. Documentar limite se for tudo-ou-nada.

### 7. Ciclo de vida de processo / fd leak / zumbis / daemon

- **Concorrentes.** isaacphi **#149 (OPEN)** — fd por arquivo/dir indexado nunca liberado (76k fds
  num server; 56 servers esgotaram o limite do SO, quebrando DNS e spawn), **#83 (OPEN)** ("too many
  open files"). serena **#1683/#1549/#1816/#1944/#1949 (CLOSED/OPEN)** — órfãos a 100% CPU quando o
  cliente morre; processo persiste e come toda a RAM; Bloop/Scala reparented a PID 1; JDTLS
  concorrentes na mesma `-data` → corrupção de índice; leak de subprocess quando o LS falha após
  spawn. mcpls **#458/#308/#318/#329 (CLOSED)** — requests in-flight penduram até timeout quando o
  loop sai; processo não sai em SIGTERM com cliente stdio conectado; sinal antes de registrar handler.
- **Probabilidade nossa: MÉDIA / parcial JÁ TRATADO.** Temos daemon failover + zombie reap (mesmo
  proxy). **Variantes a checar:** (i) fd leak (isaacphi #149) — abrimos e fechamos handles de disco?
  Lemos com `read_to_string` (fecha na hora), então baixo risco DE DISCO, mas cada LSP filho tem seus
  fds; N daemons/backends acumulam. (ii) SIGTERM: saímos limpo com cliente stdio conectado? (mcpls
  #308). (iii) órfão a 100% CPU quando o parent morre (serena #1683) — o daemon detecta parent morto?
- **Recomendação/teste.** Teste: matar o MCP e conferir que backends não viram órfão CPU-bound. Contar
  fds do processo antes/depois de N chamadas. Confirmar handler de SIGTERM/SIGINT.

### 8. Negociação de capabilities / tools anunciadas vs suportadas

- **Concorrentes.** mcpls **#461 (OPEN)** — `tools/list` sempre anuncia 20 tools independente das
  capabilities do LS conectado; **#432/#299 (CLOSED)** — `codeAction/resolve` anunciado mas não
  implementado → edits vazios silenciosos; **#412 (CLOSED)** (gating de capability duplicado em 14
  call sites). serena **#1938 (CLOSED)** — `read_only:true` ainda anuncia tools de edição em
  `tools/list` (14 anunciadas, 7 chamáveis); **#2019 (OPEN)** (`find_implementations` retorna `[]` sob
  pyright — fallback p/ type hierarchy). karellen **#33 (OPEN)** (exposição de tool Java/Kotlin).
- **Probabilidade nossa: MÉDIA.** Detectamos PULL vs PUSH diagnostics por capability
  (`lsp.rs:162`) — bom. **Mas:** anunciamos move/extract como suportados mesmo quando o backend não
  implementa aquele refactoring por linguagem? (Já temos "honest unsupported/move_no_op", então
  parcialmente coberto.) Anunciamos capabilities no `initialize` que não usamos (ex.: pedimos
  `codeAction resolveSupport` — se o server anuncia mas devolve edit vazio, tratamos? = mcpls #432).
- **Recomendação/teste.** Teste com backend que anuncia `codeActionProvider` mas devolve edit vazio →
  a tool deve reportar honesto, não "OK" com 0 edits.

### 9. Passthrough de erro cru do LSP / mensagens não sanitizadas

- **Concorrentes.** mcpls **#395 (CLOSED)** (`get_hover` vaza texto de erro interno do rust-analyzer),
  **#313 (CLOSED)** (encaminha mensagem de erro LSP ilimitada ao caller), **#294 (CLOSED)** —
  truncamento por byte-index da mensagem de erro pode dar **panic e abortar o processo**. serena
  **#1891 (OPEN)** (retorna traceback ValueError cru para paths ignorados). serena **#1941 (CLOSED)**
  (Log Injection CWE-117, CRLF não escapado nos logs).
- **Probabilidade nossa: MÉDIA.** `lsp.rs:182` faz `format!("LSP error em {method}: {err}")` —
  encaminha o objeto de erro LSP **cru e ilimitado** ao caller (igual mcpls #313). Sem cap de
  tamanho. Como somos Rust, truncar por byte-index numa string com multibyte daria panic (mcpls #294)
  — não truncamos hoje, mas se adicionarmos cap, cuidar do boundary UTF-8.
- **Recomendação/teste.** Capar mensagem de erro (por char, não byte) antes de devolver. Não vazar
  detalhe interno acionável. Teste com erro LSP gigante → resposta limitada, sem panic.

### 10. Quirks específicos de linguagem

- **Concorrentes.** serena — Python/basedpyright config (#1851, LANGUAGE-SETUP já cobre no nosso
  lado); C# novos arquivos não pegos pelo roslyn (**#1961 OPEN**), decompiled-source (#1983); Dart LS
  queima 1 core em idle no monorepo (**#2045 OPEN**); rust-analyzer 40GB RAM em workspace grande
  (**#1556 CLOSED**), `ContentModified -32801` como erro duro sem retry (**#1724 CLOSED**); símbolos
  com `/` (Erlang `foo/1`) colidem com separador de name_path (#1797). mcpls **#382/#392 (CLOSED)** —
  `ContentModified`/`ServerCancelled` (-32801/-32802) surgem como erro em vez de retry.
- **Probabilidade nossa: MÉDIA (retry) / JÁ TRATADO (Python config, name_path).** Já tratamos
  find_symbol composite name_path em SymbolInformation flat, e Python config está em
  LANGUAGE-SETUP.md. **GAP concreto:** `ContentModified (-32801)` — nós NÃO temos retry
  (grep confirmou 0 ocorrências de `32801`/`retry` em `mcp/src`). rust-analyzer e outros devolvem
  esse erro transiente durante indexação; hoje viraria erro duro para o usuário (exatamente serena
  #1724 e mcpls #382).
- **Recomendação/teste.** Adicionar retry com backoff para `-32801 ContentModified` e `-32802
  ServerCancelled` em todas as requests LSP (`lsp.rs:request`). Teste que injeta ContentModified e
  confirma retry transparente.

### 11. Performance / memória em repos gigantes

- **Concorrentes.** serena **#1556 (rust-analyzer 40GB)**, **#2045 (Dart 1 core idle)**, **#1991/#1628
  (walk lento 35s, sem progresso)**, **#2077 (os.stat por arquivo por chamada)**, **#2088 (retém 50kB
  por request HTTP)**. mcpls **#457 (OPEN)** — leitura de header de frame LSP ilimitada → exhaustion
  de memória a partir de um server malicioso; **#311/#309 (CLOSED)** (cache sem cap por-entry, config
  read ilimitado).
- **Probabilidade nossa: MÉDIA.** mcpls **#457** é diretamente aplicável: como lemos o frame LSP
  (`write_frame`/leitura de header Content-Length em `lsp.rs`)? Se confiamos no Content-Length sem
  bound, um LS defeituoso/malicioso pode nos fazer alocar demais. Não temos cap de tamanho em leituras
  de arquivo (`read_to_string` sem guard) — arquivo enorme pode estourar memória.
- **Recomendação/teste.** Bound no Content-Length do frame LSP e cap de tamanho de arquivo antes de
  `read_to_string`. Teste com Content-Length absurdo.

### 12. Conformidade de spec MCP / lifecycle do protocolo

- **Concorrentes.** serena **#1922 (OPEN)** (7 requisitos violados via mcp-spec-test), **#1932
  (CLOSED)** (incompatível com padrão "tool search"), **#1889 (CLOSED)** (version errada no
  serverInfo). isaacphi **#152 (OPEN)** (10 requisitos violados), **#79 (OPEN)** (anuncia logging
  capability sem implementar `logging/setLevel`). mcpls **#282 (CLOSED)** (não chama `validate()` em
  config).
- **Probabilidade nossa: MÉDIA-BAIXA.** Todos os concorrentes falharam em `@hasmcp/mcp-spec-test`.
  Vale rodar o mesmo suite contra nós. Baixo impacto funcional, mas afeta compatibilidade de cliente.
- **Recomendação/teste.** Rodar `@hasmcp/mcp-spec-test` contra o `code-intel-mcp` e corrigir violações
  de lifecycle/serverInfo.

### 13. Setup / onboarding / descoberta de binário do LS  (dor comum, já mitigada)

- **Concorrentes.** isaacphi #5/#41/#46/#61/#67 e jonrad #5 — enxurrada de "como configuro em Claude
  Code?", "onde acho o rust-analyzer", "não funciona". serena #1670 (falha all-or-nothing: runtime de
  UMA linguagem ausente bloqueia TODAS as tools), #1773 (help text mente sobre fallback).
- **Probabilidade nossa: JÁ TRATADO (parcial).** Temos `doctor` (setup check/fix) e LANGUAGE-SETUP.
  **Variante a checar (serena #1670):** se UM backend falha ao subir (ex.: dart sem `pub get`), isso
  derruba TODAS as tools ou degrada só aquela linguagem? Idealmente isolar por linguagem.
- **Recomendação/teste.** Teste: um backend indisponível não deve impedir tools de outra linguagem.

---

## Tabela RANKED (gaps de maior prioridade primeiro)

Prioridade = (probabilidade de termos) × (severidade: corrupção silenciosa/segurança > erro visível >
performance > cosmético). "Já tratado" no fim.

| # | Classe | Prob. nossa | Severidade | Evidência concorrente | Ação recomendada |
|---|---|---|---|---|---|
| 1 | **Encoding UTF‑16 de posição** | ALTA | Crítica (corrupção silenciosa de edit) | serena #2064; mcpls #290/#287/#413 | Negociar `positionEncodings`; converter colunas p/ UTF‑16 em `find_ident`/`pos_to_offset`; fixture com unicode antes do símbolo |
| 2 | **URI sem percent-encode + containment sem canonicalize** | MÉDIA‑ALTA | Alta (paths quebram; traversal) | mcpls #411/#417/#415; serena #1602 | Usar `Url::from/to_file_path`; `canonicalize`+comparar por componentes; teste `../`/symlink/espaço |
| 3 | **ContentModified (-32801) sem retry** | ALTA | Alta (erro duro transiente ao usuário) | serena #1724; mcpls #382/#392 | Retry+backoff p/ -32801/-32802 em `request()` |
| 4 | **Rename/edit cross-file omitindo arquivos não abertos + corrupção que passa no build** | MÉDIA | Alta (refs stale / código inválido silencioso) | serena #1956/#1952/#1744 | Aplicar edits em TODOS os arquivos com refs; snapshot-test em `export const`/whitespace |
| 5 | **Frame LSP / read de arquivo sem bound de tamanho** | MÉDIA | Alta (exhaustion de memória) | mcpls #457/#311; serena #2077 | Bound no Content-Length; cap antes de `read_to_string` |
| 6 | **Passthrough de erro LSP cru/ilimitado** | MÉDIA | Média-Alta (vaza interno; panic se truncar por byte) | mcpls #313/#294/#395; serena #1891 | Capar por char; sanitizar antes de devolver ao caller |
| 7 | **LSP morto/OOM → resposta vazia como sucesso** | MÉDIA (variante do warmup) | Alta (0 refs falso) | serena #1814/#1923 | Distinguir "server caiu" de "0 refs"; retornar erro |
| 8 | **Ciclo de vida: SIGTERM limpo + órfão CPU + fd leak** | MÉDIA | Média-Alta | isaacphi #149/#83; serena #1683/#1549; mcpls #308/#458 | Handler SIGTERM; detectar parent morto; contar fds em teste |
| 9 | **Capability anunciada mas edit vazio (codeAction/resolve)** | MÉDIA | Média | mcpls #432; serena #1938 | Reportar honesto quando edit vem vazio |
| 10 | **Monorepo/multi-root: rootUri único sub-reporta cross-package** | MÉDIA | Média | serena #1939/#1766/#1627 | Fixture monorepo c/ project refs; documentar limite |
| 11 | **Conformidade spec MCP (mcp-spec-test)** | MÉDIA‑BAIXA | Baixa-Média | serena #1922; isaacphi #152 | Rodar `@hasmcp/mcp-spec-test` |
| 12 | **Um backend down derruba tudo** | BAIXA-MÉDIA | Média | serena #1670 | Isolar falha por linguagem |
| — | Cold-index race (gate de warmup) | **Já tratado** | — | serena #1937/mcpls #420 | Confirmar gate em TODAS as tools |
| — | Freshness por mtime (didChange) | **Já tratado** | — | serena #1718/#2005 | Confirmar version monotônica; custo do stat |
| — | Workspace scoping / ruído substring | **Já tratado** | — | serena #1999 | — |
| — | Colisão de rename / new_name / noop | **Já tratado** | — | serena #1548 | — |
| — | move/extract unsupported honesto | **Já tratado** | — | isaacphi #104 | — |
| — | name_path composto em SymbolInformation flat | **Já tratado** | — | serena #1797 | — |
| — | decorator/annotation position | **Já tratado** | — | (nosso scan_ident) | — |
| — | Python basedpyright config incompleta | **Já tratado (docs)** | — | serena #1851 | LANGUAGE-SETUP.md |
| — | daemon failover + zombie reap | **Já tratado (parcial)** | — | serena #1683 | Ver #8 p/ variantes |
| — | Setup/onboarding | **Já tratado (doctor)** | — | isaacphi #5/#41 | Ver #12 |

---

## Fontes

Repositórios e issues consultados (via `gh` em 2026-09-20):

**oraios/serena** — https://github.com/oraios/serena/issues
- #2064 https://github.com/oraios/serena/issues/2064 (UTF‑16, OPEN)
- #2077 https://github.com/oraios/serena/issues/2077 (os.stat por arquivo, OPEN)
- #2076 https://github.com/oraios/serena/issues/2076 (find_symbol unbounded, CLOSED)
- #2045 https://github.com/oraios/serena/issues/2045 (Dart 1 core idle, OPEN)
- #2019 https://github.com/oraios/serena/issues/2019 (find_implementations [], OPEN)
- #2017 https://github.com/oraios/serena/issues/2017 (UnicodeDecodeError, OPEN)
- #2005 https://github.com/oraios/serena/issues/2005 (didChange reusa version, OPEN)
- #1961 https://github.com/oraios/serena/issues/1961 (roslyn novos arquivos, OPEN)
- #1956 https://github.com/oraios/serena/issues/1956 (export const duplicado, OPEN)
- #1952 https://github.com/oraios/serena/issues/1952 (whitespace GDScript, OPEN)
- #1944 https://github.com/oraios/serena/issues/1944 (JDTLS -data corrupção, OPEN)
- #1939 https://github.com/oraios/serena/issues/1939 (monorepo TS sub-reporta, OPEN)
- #1937 https://github.com/oraios/serena/issues/1937 (partial results indexing, OPEN)
- #1816 https://github.com/oraios/serena/issues/1816 (Bloop reparented PID 1, OPEN)
- #1814 https://github.com/oraios/serena/issues/1814 (tsserver OOM → {} como sucesso, CLOSED)
- #1766 https://github.com/oraios/serena/issues/1766 (Metals raiz do monorepo, CLOSED)
- #1744 https://github.com/oraios/serena/issues/1744 (zls rename omite arquivos não abertos, OPEN)
- #1724 https://github.com/oraios/serena/issues/1724 (rust-analyzer ContentModified erro duro, CLOSED)
- #1718 https://github.com/oraios/serena/issues/1718 (didChangeWatchedFiles nunca enviado, CLOSED)
- #1697 https://github.com/oraios/serena/issues/1697 (blank lines no delete, CLOSED)
- #1683 https://github.com/oraios/serena/issues/1683 (órfãos 100% CPU, CLOSED)
- #1670 https://github.com/oraios/serena/issues/1670 (one runtime missing blocks all, CLOSED)
- #1602 https://github.com/oraios/serena/issues/1602 (Windows forward slashes, CLOSED)
- #1593 https://github.com/oraios/serena/issues/1593 (find_symbol stale Clojure, OPEN)
- #1586 https://github.com/oraios/serena/issues/1586 (tsserver inferred project, CLOSED)
- #1585 https://github.com/oraios/serena/issues/1585 (COMMAND_INJECTION shell=True, CLOSED)
- #1569 https://github.com/oraios/serena/issues/1569 (subprocess shell=True, CLOSED)
- #1556 https://github.com/oraios/serena/issues/1556 (rust-analyzer 40GB, CLOSED)
- #1549 https://github.com/oraios/serena/issues/1549 (processo persiste, come RAM, CLOSED)
- #1548 https://github.com/oraios/serena/issues/1548 (renomear parâmetros, OPEN)
- #1941 https://github.com/oraios/serena/issues/1941 (Log Injection CWE-117, CLOSED)
- #1938 https://github.com/oraios/serena/issues/1938 (read_only anuncia edit tools, CLOSED)
- #1922 https://github.com/oraios/serena/issues/1922 (MCP spec 7 violações, OPEN)
- #1891 https://github.com/oraios/serena/issues/1891 (traceback cru, OPEN)
- #1851 https://github.com/oraios/serena/issues/1851 (BasedPyright default, CLOSED)
- #1797 https://github.com/oraios/serena/issues/1797 (Erlang foo/1 name_path, CLOSED)
- #1681 https://github.com/oraios/serena/issues/1681 (pyright 5s wait curto, CLOSED)
- #1628 https://github.com/oraios/serena/issues/1628 (first-index walk lento, CLOSED)
- #1627 https://github.com/oraios/serena/issues/1627 (sem escopo N sub-roots, CLOSED)
- #1999 https://github.com/oraios/serena/issues/1999 (C# discovery bypassa .gitignore, OPEN)
- #1991 https://github.com/oraios/serena/issues/1991 (ignored_paths não poda walk, OPEN)
- #1923 https://github.com/oraios/serena/issues/1923 (Vue indexing marcado completo, OPEN)
- #1889 https://github.com/oraios/serena/issues/1889 (serverInfo.version do SDK, CLOSED)

**bug-ops/mcpls** — https://github.com/bug-ops/mcpls/issues
- #461 https://github.com/bug-ops/mcpls/issues/461 (tools/list ignora capabilities, OPEN)
- #459 https://github.com/bug-ops/mcpls/issues/459 (output opaco string, OPEN)
- #458 https://github.com/bug-ops/mcpls/issues/458 (requests in-flight penduram, OPEN)
- #457 https://github.com/bug-ops/mcpls/issues/457 (frame header ilimitado → memory exhaustion, OPEN)
- #432 https://github.com/bug-ops/mcpls/issues/432 (codeAction/resolve edit vazio, CLOSED)
- #429 https://github.com/bug-ops/mcpls/issues/429 (ignora document_changes, CLOSED)
- #423 https://github.com/bug-ops/mcpls/issues/423 (call_hierarchy/workspace_symbol bypass gate, CLOSED)
- #420 https://github.com/bug-ops/mcpls/issues/420 (read-only responde de workspace não indexado, CLOSED)
- #417 https://github.com/bug-ops/mcpls/issues/417 (sandbox falha aberto com roots vazio, CLOSED)
- #415 https://github.com/bug-ops/mcpls/issues/415 (respostas LSP não filtradas por root, CLOSED)
- #413 https://github.com/bug-ops/mcpls/issues/413 (u32 overflow em position, CLOSED)
- #411 https://github.com/bug-ops/mcpls/issues/411 (URI sem percent-decode, CLOSED)
- #403 https://github.com/bug-ops/mcpls/issues/403 (shutdown params null, tsgo rejeita, CLOSED)
- #395 https://github.com/bug-ops/mcpls/issues/395 (vaza erro interno rust-analyzer, CLOSED)
- #392 https://github.com/bug-ops/mcpls/issues/392 (ContentModified log ERROR antes de retry, CLOSED)
- #382 https://github.com/bug-ops/mcpls/issues/382 (ContentModified sem retry, CLOSED)
- #359 https://github.com/bug-ops/mcpls/issues/359 (diagnostics push não retomam pós-respawn, CLOSED)
- #358 https://github.com/bug-ops/mcpls/issues/358 (DocumentTracker race com ensure_open, CLOSED)
- #353 https://github.com/bug-ops/mcpls/issues/353 (tool_prefix p/ múltiplas bridges, CLOSED)
- #329 https://github.com/bug-ops/mcpls/issues/329 (SIGTERM sem escalação, CLOSED)
- #318 https://github.com/bug-ops/mcpls/issues/318 (sinal antes do handler, CLOSED)
- #313 https://github.com/bug-ops/mcpls/issues/313 (erro LSP ilimitado ao caller, CLOSED)
- #311 https://github.com/bug-ops/mcpls/issues/311 (cache sem cap por-entry, CLOSED)
- #308 https://github.com/bug-ops/mcpls/issues/308 (não sai em SIGTERM com cliente stdio, CLOSED)
- #299 https://github.com/bug-ops/mcpls/issues/299 (LSP 3.18 capabilities não trackeadas, CLOSED)
- #294 https://github.com/bug-ops/mcpls/issues/294 (truncamento byte-index → panic, CLOSED)
- #290 https://github.com/bug-ops/mcpls/issues/290 (positionEncoding negociado não consumido, CLOSED)
- #287 https://github.com/bug-ops/mcpls/issues/287 (position_encodings parseado não consumido, CLOSED)
- #284 https://github.com/bug-ops/mcpls/issues/284 (PublishDiagnostics vazio consome budget, CLOSED)
- #422 https://github.com/bug-ops/mcpls/issues/422 ($/progress p/ não-rust-analyzer, CLOSED)
- #425 https://github.com/bug-ops/mcpls/issues/425 (respawn não re-adquire readiness, CLOSED)

**isaacphi/mcp-language-server** — https://github.com/isaacphi/mcp-language-server/issues
- #152 https://github.com/isaacphi/mcp-language-server/issues/152 (MCP spec 10 violações, OPEN)
- #149 https://github.com/isaacphi/mcp-language-server/issues/149 (fd leak 76k, OPEN)
- #130 https://github.com/isaacphi/mcp-language-server/issues/130 (security scan AIVSS 85/100, OPEN)
- #121 https://github.com/isaacphi/mcp-language-server/issues/121 (símbolos nunca achados, OPEN)
- #104 https://github.com/isaacphi/mcp-language-server/issues/104 (rename só um arquivo, OPEN)
- #86 https://github.com/isaacphi/mcp-language-server/issues/86 (references de classe falham, OPEN)
- #83 https://github.com/isaacphi/mcp-language-server/issues/83 (too many open files, OPEN)
- #79 https://github.com/isaacphi/mcp-language-server/issues/79 (logging capability não implementada, OPEN)
- #60 https://github.com/isaacphi/mcp-language-server/issues/60 (diagnostics não confiáveis TS, OPEN)
- #54 https://github.com/isaacphi/mcp-language-server/issues/54 (diagnostics cacheados, OPEN)
- #32 https://github.com/isaacphi/mcp-language-server/issues/32 (definition falha, didOpen não chamado, OPEN)

**jonrad/lsp-mcp** — https://github.com/jonrad/lsp-mcp/issues (#3 JSON schema, #5/#8 setup/linguagens)
**Tritlo/lsp-mcp** — https://github.com/Tritlo/lsp-mcp/issues (#2 capabilities faltando diag, #5 Dart timeout Windows)
**crepererum-oss/common-sense-coder** — https://github.com/crepererum-oss/common-sense-coder/issues (#22 roots, #21 diretórios, #19 Windows)
**karellen/karellen-lsp-mcp** — https://github.com/karellen/karellen-lsp-mcp/issues (#33 exposição Java/Kotlin, #6 JDK path)
**beixiyo/vsc-lsp-mcp** — https://github.com/beixiyo/vsc-lsp-mcp/issues (#3 um LS por janela, #1 connect failed)
**lostbean/agent-lsp** — issues DESABILITADAS (não acessível).
