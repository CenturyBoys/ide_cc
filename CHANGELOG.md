# Changelog

Todas as mudanças notáveis deste projeto são documentadas aqui.
Formato baseado em [Keep a Changelog](https://keepachangelog.com/pt-BR/1.1.0/);
versionamento segue [SemVer](https://semver.org/lang/pt-BR/).

## [Unreleased]

## [0.7.7] - 2026-09-20

### Added
- **Log de erros local** (para descobrir erros na máquina do usuário sem relatório manual): toda
  falha de tool (`isError`) e todo `panic` são registrados como JSONL (`ts, kind, tool, msg, args,
  version`) — **sem rede/telemetria**. Path via `CODE_INTEL_LOG` (`=off` desliga); default
  `~/.cache/code-intel-mcp/errors.jsonl`. O `doctor` reporta o caminho em `error_log`. Basta
  inspecionar o arquivo (ou pedir ao usuário) para diagnosticar problemas de campo.

## [0.7.6] - 2026-09-20

### Added
- **Detecção de OVER-REACH no rename** (Bug 1 do relatório rename, C#, **reproduzido localmente**):
  o csharp-ls desambigua overload no `references` mas NÃO no `rename` — o WorkspaceEdit vaza pros
  homônimos com `safe:true` silencioso (bug UPSTREAM do csharp-ls). Como já temos as referências do
  símbolo (warmup), comparamos: se o rename toca mais edições que as refs, marca `over_reach` +
  `warning` (preview) e **RECUSA no `apply=true`** quando é claramente over-reach (≥ 2×). Torna o bug
  upstream visível e seguro. Fixture C# de overloads + caso e2e. Inclui `references_count` no retorno.
- **Cobertura e2e de MONOREPO** (`fixtures/ts-monorepo`, 2 packages + alias `@core`): `find_references`
  e `rename_symbol` **cruzam packages** com segurança. Confirma e guarda o suporte a monorepo.

### Fixed
- **`document_symbols`/`find_symbol` estouravam timeout no cold start de servers pesados** (csharp-ls
  ~24s): o timeout fixo de 10s não passava pelo gate de warmup. Agora reintenta dentro do budget
  (`CODE_INTEL_WARMUP_MS`) em timeout/ContentModified — cobrindo a lacuna "o gate cobre todas as tools?".

## [0.7.5] - 2026-09-20

Endurecimento proativo a partir da varredura de concorrentes (`docs/COMPETITOR-ISSUE-SCAN.md`).

### Fixed
- **Gap #2 — URIs sem percent-encode + containment frágil**: `path_to_uri`/`uri_to_path` agora
  fazem percent-encode/decode (paths com espaço/acento/`#`/`%` — mcpls #411); `workspace_symbols`
  usa containment **por boundary** (`/root2` não conta como dentro de `/root`).
- **Gap #3 — `ContentModified` (-32801) sem retry**: `request()` reintenta o erro transiente que o
  rust-analyzer (e outros) devolvem durante a indexação, em vez de propagar erro duro (serena #1724).

### Security
- **Gap #2 — path traversal**: `safe_abs` normaliza `..`/`.` e **recusa** paths que escapam a raiz
  do projeto (ex.: `file="../../etc/passwd"`), por comparação de componentes.
- **Gap #5 — frame LSP ilimitado**: `read_frame` limita `Content-Length` a 64 MiB (evita
  alocação/OOM com header malicioso — mcpls #457).

### Notes
- **Gap #4 — rename com corrupção sintaticamente VÁLIDA**: fica como limitação conhecida — o
  `net_delta`+`verify_build` garante "não quebra o build", não "faz o que você quis dizer"; detectar
  edição válida-mas-errada exige entender a intenção. Documentado.

## [0.7.4] - 2026-09-20

### Added
- **`call_hierarchy` expõe call-sites** (P14, issue #5): além de `incoming_count`, agora traz
  `call_site_count` e, por chamador, `call_sites` (cada chamada via `fromRanges`) — antes 4 chamadas
  no mesmo `wrapper` viravam "1" e subcontavam o blast radius.
- **`docs/COMPETITOR-ISSUE-SCAN.md`**: varredura das issues (abertas+fechadas) de bridges LSP↔MCP
  concorrentes (serena, mcpls, agent-lsp…), com ranking de classes de problema que podemos ter.

### Fixed
- **Coluna de posição usava offset de BYTE em vez de UTF-16** (achado #1 da pesquisa competitiva —
  serena #2064, mcpls #290): `locate`/`scan_ident` mandavam índice de byte como `character` do LSP;
  qualquer unicode antes do símbolo na linha (acento/emoji em comentário/string) deslocava a coluna
  → edit na posição errada em silêncio. Agora converte para unidades UTF-16 (`utf16_col`). Coberto
  por teste. `scan_ident` também passou a casar por identificador inteiro (não substring).
- **P13 — `validate_build`/`verify_build` revertia rename SEGURO em Python** (issue #5): o parser lia
  o resumo do basedpyright `"0 errors, 0 warnings, 0 notes"` como erro (regressão do meu P9 — o
  filtro só excluía `"N Error(s)"` do dotnet). Agora `is_count_summary` ignora qualquer resumo de
  contagem `<n> error(s)`. Coberto por teste unit + e2e (build verde → `build_ok:true`).
- **N1/N2 (Dart) — extract/move usavam o kind do TS e/ou faziam raw-throw** (issue #4): `refactor_edit`
  pede o kind PAI (`refactor.extract`/`refactor.move`, hierarquia LSP) e, se o backend não oferece,
  retorna `unsupported` GRACIOSO (não erro cru) — como a descrição promete. Casos e2e Dart.
- **N3 — `workspace_symbols` não era project-scoped** (issue #4): resultados de `.pub-cache`/SDK/deps
  (e substring) poluíam a busca. Agora filtra ao projeto por default (`project_only`, exclui
  `.pub-cache`/`node_modules`/SDK/etc.); reporta `filtered_out`.
- **Bug 3 — índice stale após revert externo (`git checkout`)** (relatório rename): find_references/
  call_hierarchy/rename agora `resync_all_changed()` — re-sincronizam TODOS os arquivos abertos com
  mtime mudado (não só o consultado), refletindo mudanças feitas fora da ferramenta.
- **`rename`/`find_references`/`call_hierarchy` atingiam o símbolo ERRADO por match de substring**
  (Bug 2 do relatório rename, C#): `locate` usava `row.find(symbol)`, então `"Result"` casava DENTRO
  de `"RefundResult"` e a operação mirava o tipo em vez do parâmetro — com `safe:true` silencioso.
  Agora casa **identificador completo** (word boundary) via `find_ident`. Coberto por teste unitário
  **e** por um caso e2e que asserta o ALVO da resolução (não só o flag de segurança) — a lacuna de
  teste que deixou isso passar.

### Added
- **Job CI `e2e-oss` (nightly + manual)**: roda o `--real` contra projetos OSS grandes e reais, um
  por linguagem (flask/Python, zod/TS, ripgrep/Rust, http/Dart) — reprodutível (os repos de campo
  são privados) e pega casos que os fixtures não têm. Foi rodando o `--real` em OSS que achamos o
  bug do `doctor smoke` (v0.7.3).

## [0.7.3] - 2026-09-20

### Fixed
- **`doctor smoke` escolhia símbolo de arquivo scratch/0-refs → `index_not_ready` enganoso + budget
  desperdiçado** (achado rodando `--real` em OSS: `zod/play.ts`, símbolo `transform` com 0 refs
  girou 715 polls/180s). Agora o smoke: (1) pula arquivos não-biblioteca (`play`/`example`/`test`/
  `*.g.dart`/`*.d.ts`…) e **prefere `src/`|`lib/`**; (2) tenta **vários arquivos × candidatos** com
  budget curto por tentativa (a 1ª absorve o cold start), parando no 1º símbolo com refs estáveis;
  (3) `warmup_references` ganhou budget por-chamada. Um símbolo folha não consome mais o teto todo.

### Added
- **Camada ADVERSARIAL de testes e2e + `docs/TEST-STRATEGY.md`**: além dos casos positivos, agora há
  casos que exigem o **bloqueio** de operações destrutivas (rename com colisão, para keyword, noop,
  extract/move não suportado, move_no_op, build verde). Inclui o fixture Python com
  `typeCheckingMode=off` (a config que quebrou o P8) provando que a rede pega a colisão **sem depender
  de diagnósticos**. Princípio: *testar a garantia (recusa quebrar), não só a feature*.

### Fixed
- **P8 — `rename_symbol` reportava `safe:true` num rename que QUEBRA (colisão de nome)** (issue #3,
  Python): com `typeCheckingMode=off` o basedpyright não emite diagnósticos → `net_delta` sempre 0 →
  tudo "safe". Agora há **detecção explícita de colisão de escopo** (independente de diagnósticos):
  se `new_name` já existe como irmão no mesmo container, o rename é barrado (`name_collision`).
- **P12 — `rename_symbol` não validava `new_name`**: nome inválido/keyword agora é **rejeitado**
  (`invalid_new_name`) em vez de virar noop silencioso; `new==old` é **noop** explícito.
- **P9 — `validate_build`/`verify_build` no-op em Python**: agora há default (`basedpyright`); env key
  corrigido para `PYTHON_CHECK_CMD` (batia com a mensagem de erro).
- **P11 — extract/move em Python**: erro **explícito** "indisponível para <lang>" em vez do
  enigmático "nenhum refactoring disponível".
- **P10 — `workspace_symbols` com lang sem fontes**: retorna **rápido** com aviso (antes esperava
  60s e dava `index_not_ready` enganoso).
- **`validate_build` C# falso-negativo** (relatório suite-completa): `build_ok:false` num build VERDE
  porque o parser casava a linha de resumo `"0 Error(s)"`. Agora ignora `N Error(s)` e só conta erros
  reais (`error CS...`/`error[E...]`) — evita reverter um apply seguro via `verify_build`.
- **`document_symbols` rotulava `record` (não-struct) como `Class`**: heurístico relabela p/ `Record`
  (cosmético; `record struct` já vinha `Struct`).

## [0.7.1] - 2026-09-20

### Added
- **Suíte e2e** (`mcp/e2e/`, job CI dedicado): roda as ferramentas REAIS contra os 5 language
  servers REAIS sobre fixtures que reproduzem cada armadilha dos relatórios de campo (decorator,
  método em C#, `workspace_symbols` Dart, move no-op, extract). Modo opt-in `--real` aponta pra um
  repo grande de verdade (valida escala/warmup via `doctor smoke=true`). Cobre o que o `cargo test`
  (lógica pura) não vê — e já pegou 2 bugs (abaixo).

### Fixed
- **`find_symbol`/`document_symbols` — name_path composto vazio em servers ACHATADOS** (pego pela
  suíte e2e): tsgo/basedpyright/csharp-ls retornam `SymbolInformation[]` (achatado, com
  `containerName`), não a árvore hierárquica; o `flatten_symbols` só olhava `children`, então
  métodos vinham como `metodo` (sem a classe) e `find_symbol("Classe/metodo")` dava `count:0` (e
  regrediu com o tightening de precisão). Agora o flatten reconstrói `Classe/metodo` via
  `containerName` — precisão entre classes homônimas nos DOIS formatos. E quando o server é
  achatado E **sem** `containerName` (csharp-ls), a query composta `Classe/metodo` casa por último
  segmento (best-effort, sem info de classe) — mantendo precisão onde há hierarquia.
- **`workspace_symbols` — "silent empty" no cold index** (pego pela suíte e2e, Dart): o servidor
  devolvia `[]` VÁLIDO enquanto indexava e o retry (P5) só re-tentava em erro. Agora reintenta também
  em VAZIO até o budget; se seguir vazio, sinaliza `warning` (pode não estar pronto) em vez de afirmar
  "não existe". `find_source_file` (do doctor smoke) agora varre de forma determinística (ordenada).
- **`move_symbol` reportava `safe:true` em C# sem mover nada (no-op silencioso)** (relatório
  extract/move): o csharp-ls devolvia uma ação `refactor.move` trivial (sem criar arquivo), contada
  como sucesso. Agora, se "mover para novo arquivo" **não cria arquivo** (`creates` vazio), retorna
  `unsupported`/`move_no_op` honesto em vez de fingir sucesso. Coberto por teste (`is_conn_dead`).
- **`extract_function`/`move_symbol` — recuperação de backend morto** (relatório extract/move: vtsls
  fechava a conexão no TS → `ERRO: Broken pipe` cru): agora, ao detectar a conexão caída, **reinicia
  o language server e tenta mais uma vez**; se persistir, dá erro acionável (provável crash do
  backend) em vez do "Broken pipe" cru. Descrições das duas tools ajustadas (não são mais
  "via vtsls" genérico). NB: se o vtsls crashar deterministicamente no extract, ainda falha — mas
  agora com mensagem clara e sem deixar o client quebrado.
- **Daemon sem failover + vazamento de zumbi** (issue #2, relatório Dart): quando o daemon morria, o
  proxy devolvia `ERRO: falha ao ler do daemon` cru (sem recuperação) e o processo morto ficava
  `<defunct>` (zumbi) sob o proxy. Agora o `forward_call` detecta a conexão quebrada, **respawna o
  daemon e tenta mais uma vez** antes de errar; o handle do daemon é guardado e **reapado** (`wait()`)
  no respawn — sem zumbi para o proxy que o subiu.

## [0.7.0] - 2026-09-20

### Added
- **`doctor smoke=true`** (P2): teste END-TO-END por linguagem — descobre um símbolo referenciável,
  roda um `find_references` real e exige `count>0 && stable`; senão reporta `index_not_ready`/
  `resolved_zero` acionável. Pega o que os checks estáticos (binário+config) não veem (posição,
  warmup, escala). Sem `smoke`, o `hint` do doctor deixa claro que foi só checagem estática.
- **`find_references summary=true`** (P7): saída resumida `{count, files, by_file}` sem cada
  `path:linha:col` — evita estourar o limite de tokens do cliente em símbolos muito usados.
- **`CODE_INTEL_WS_TIMEOUT_MS`** (P5): timeout por request do `workspace_symbols` (default 10s). No
  cold index ele agora **reintenta** dentro do budget (`CODE_INTEL_WARMUP_MS`) e devolve
  `index_not_ready` acionável em vez de timeout SECO.
- **`CODE_INTEL_WARMUP_MS`** (P3): teto de warmup do `find_references`/rename configurável (default
  60000ms) para repos grandes em cold start. O aviso `index_not_ready` agora é **acionável** — sugere
  ligar `CODE_INTEL_DAEMON=1` (sem daemon o warmup reinicia a cada chamada) e/ou esticar o teto.

### Fixed
- **`workspace_symbols` silenciosamente quebrado fora de TS/Python** (relatório Dart): só roteava
  `python→basedpyright`; **todo o resto caía no tsgo**, então `dart`/`rust`/`csharp` consultavam o
  servidor de TypeScript → `count:0` em silêncio. Agora roteia por linguagem para os 5 backends e
  **erra alto** em `lang` desconhecida (em vez de retornar vazio). Descrição/param `lang` atualizados.
  Coberto por teste.
- **`doctor` — `hint` não insiste mais em `fix=true` quando `problems: 0`** (relatório Dart): reporta
  "nenhum problema encontrado". (O check do `.dart_tool/package_config.json` já existia; o
  `smoke=true` agora também cobre Dart end-to-end.)
- **`INSTALL_LSP=1` não instalava o basedpyright** (P4): só testava `pip`. Agora tenta
  `uv → pipx → pip → pip3 → python3 -m pip → npm` (`install.sh` e `install.ps1`) — o relatório
  pachamama usava `uv` e ficava sem backend Python, caindo direto em `index_not_ready`.

### Changed
- **Descrição do `find_references` agnóstica de linguagem** (P6): não diz mais "via tsgo"; lista o
  backend por linguagem (tsgo/basedpyright/rust-analyzer/csharp-ls/dart).
- **Símbolos decorados resolviam no `@decorator`, não no identificador → `find_references` dava
  `count:0` em silêncio** (P1, relatório pachamama/Python). O basedpyright reporta a posição de uma
  classe/método decorado na linha do `@dataclass`/`@classmethod`; a resolução implícita de posição
  perguntava `textDocument/references` em cima do `@` e recebia zero. Agora a refinação **varre pra
  frente** (até 16 linhas, cobrindo decorators empilhados) até o token do identificador — em
  `resolve_pos` (afeta find_references/rename/move/call_hierarchy sem `line`) e no `at` de
  `document_symbols`/`find_symbol` (agora alinhado ao identificador, como o `workspace_symbols`).
- **`find_symbol`/`resolve_pos` — precisão do name_path composto**: uma query `Classe/metodo` não
  casa mais um método homônimo de OUTRA classe (o fallback por último segmento agora só vale quando
  a query não qualifica a classe). Inofensivo em arquivo de 1 classe (por isso o TS/viva-bff passava),
  mas evita over-match em arquivos com várias classes. Coberto por teste (cenário viva-bff).
- **`find_symbol` não resolvia métodos em C#** (`count: 0` para `kind: Method`, embora `Class`/`Field`
  funcionassem). Causa: o `csharp-ls` anexa a assinatura ao nome do método no `documentSymbol`
  (ex.: `HandleAsync(string x, Guid y)`), e o casamento por `name_path` comparava contra o nome cru.
  Agora o nome é normalizado (descarta a assinatura antes do `(`) tanto no `find_symbol` quanto no
  `resolve_pos` — então rename/find_references/call_hierarchy também resolvem métodos C# sem `line`
  explícito. Idempotente para TS/Rust/etc. Coberto por testes unitários.

### Changed
- **Setup Python (docs+install)**: `install.sh`/`install.ps1` passam a avisar, ao final, que o
  instalador **não** configura o workspace por projeto — é preciso rodar `doctor(fix=true)` dentro
  de cada projeto (crítico em Python: sem a config, `find_references` sai incompleto em silêncio).
  README (EN+PT) documenta o mesmo, com a cobertura de venv do `doctor` (`.venv`/`venv`/`env` →
  cobre uv e venv nativo) e a pegadinha do poetry (venv fora do projeto por padrão → não detectado).
- **CI**: bump das GitHub Actions para runtime Node 24 — `actions/checkout@v4→v5` (ci + release) e
  `softprops/action-gh-release@v2→v3`, resolvendo o aviso de deprecação do Node 20.
- **CI**: passa a rodar `cargo test --release` (step bloqueante) — guarda os testes unitários de
  regressão (ex.: o casamento de `name_path` para métodos C#).

## [0.6.0] - 2026-09-20

### Added
- **Suporte a Windows** (x86_64): release passa a publicar `code-intel-mcp-x86_64-pc-windows-msvc.zip`
  (workflow com runner `windows-latest`); novo instalador PowerShell [`install.ps1`]. O core do MCP
  e todas as ferramentas semânticas funcionam; o daemon de cache entre sessões (`CODE_INTEL_DAEMON`)
  fica atrás de `#[cfg(unix)]` — indisponível no Windows nativo (avisa via stderr), disponível via WSL.

### Changed
- README agora é **English-first** (mercado internacional), com o pitch "you use an IDE, why don't
  your agents?" e ângulo de posicionamento em **segurança/garantia** (apply→verify→auto-rollback).
  Adicionada tabela de comparação honesta vs. Serena / agent-lsp / bridges LSP→MCP genéricos
  (pesquisa competitiva). README em português preservado em [`README.pt-BR.md`](README.pt-BR.md),
  com seletor de idioma em ambos.

## [0.5.0] - 2026-09-19

### Added
- **Tool `doctor`**: verifica o setup do projeto por linguagem (LSP disponível + config de
  workspace) e, com `fix=true`, corrige o que dá — ex.: cria `pyrightconfig.json` p/ Python
  (detecta `src/` e `.venv`). Fecha o gap descoberto no teste do pachamama.
- **`install.sh`**: opções `INSTALL_LSP=1` (instala os language servers) e `REGISTER=1` (registra
  global no Claude Code) — instalação "tudo em um comando".

### Changed
- README reformulado (banner, prova de valor, catálogo de 10 ferramentas × 5 linguagens).

## [0.4.1] - 2026-09-19

### Fixed
- Workflow de release: alvo `x86_64-apple-darwin` migrado de `macos-13` (runner escasso — travava a
  fila por horas) para `macos-latest` com cross-compile. Os 4 binários passam a sair de forma confiável.

### Added
- **`docs/LANGUAGE-SETUP.md`**: requisitos de config por linguagem para resultados corretos
  (validado em projeto real; Python exige `[tool.basedpyright]` include/venv, senão referências
  saem incompletas em silêncio).
- **`benchmarks/harness/measure-project.mjs`**: medidor reutilizável (cold-start, warm p50/p95,
  RAM, rename blast radius) para apontar em qualquer projeto/símbolo.

## [0.4.0] - 2026-09-19

### Added
- **`install.sh`**: instalador que detecta a plataforma, baixa o binário do release e verifica os
  language servers (`curl … | bash`; opções `BIN_DIR`, `VERSION`, `WRITE_MCP`).
- Flag `--version` no binário.

### Fixed
- `serverInfo.version` agora reflete a versão do crate (`env!("CARGO_PKG_VERSION")`) em vez de um
  literal fixo.

## [0.3.0] - 2026-09-19

### Added
- **Validação de build** (Fase 5): tool `validate_build` + opção `verify_build` em
  rename/extract/move — 2ª camada de segurança que roda o build da linguagem no disco e reverte se
  falhar (pega erros que a simulação em memória não vê, ex.: `cargo check` do Rust).
- **Frescor**: `ensure_open` re-sincroniza por mtime — edições feitas fora do agente são refletidas.
- **Cache entre sessões** (opt-in `CODE_INTEL_DAEMON=1`): daemon mantém os LSPs quentes entre
  reinícios do MCP (~23× na 2ª sessão em projeto Rust).
- **Experimento A/B** (prova de valor): fixture-armadilha + `ab-rename.mjs` (mecânico),
  `ab-refs.mjs` (delete/impacto) e `ab-agent.sh` (agente real com/sem a camada); skill
  `semantic-refactor`.
- **Distribuição**: `README.md`, `LICENSE` (MIT), CI (`fmt`+`clippy`+`build`) e workflow de
  **release multiplataforma** por tag (`x86_64`/`aarch64` linux e macOS).

### Changed
- Código Rust formatado com `rustfmt`.

## [0.2.0] - 2026-09-19

### Added
- **Fase 4 — 5 linguagens**: Python (basedpyright), Dart (Dart Analysis Server), Rust
  (rust-analyzer) e C# (csharp-ls), além de TypeScript.
- Roteamento por **linguagem × operação** (`nav_backend`/`refactor_backend`).
- **Diagnostics híbrido** (PULL/PUSH, detectado por capability) — necessário porque servers
  diferem (tsgo pull; vtsls/basedpyright/rust-analyzer push).
- `rename_symbol` devolve `rejected_by_server` quando o server valida e recusa a operação (ex.: Dart).

## [0.1.0] - 2026-09-19

### Added
- **Governança IA-first**: `CLAUDE.md`, `docs/WORKFLOW.md`, `docs/ROADMAP.md`,
  `docs/RELATORIO-LEVANTAMENTO.md` (levantamento LSP/MCP).
- **Benchmark de latência LSP** (Fase 0): harness reproduzível; **tsgo** eleito backend TS
  (rápido e não trunca em monorepo).
- **code-intel-mcp** (Fases 1–2c, TypeScript): **gate de warmup** (nunca retorna contagem parcial),
  **apply→verify com `net_delta`**, e 8 ferramentas — navegação (`find_references`, `document_symbols`,
  `find_symbol`, `workspace_symbols`, `call_hierarchy`), `rename_symbol`, `extract_function`,
  `move_symbol`.

[Unreleased]: https://github.com/CenturyBoys/ide_cc/compare/v0.7.7...HEAD
[0.7.7]: https://github.com/CenturyBoys/ide_cc/compare/v0.7.6...v0.7.7
[0.7.6]: https://github.com/CenturyBoys/ide_cc/compare/v0.7.5...v0.7.6
[0.7.5]: https://github.com/CenturyBoys/ide_cc/compare/v0.7.4...v0.7.5
[0.7.4]: https://github.com/CenturyBoys/ide_cc/compare/v0.7.3...v0.7.4
[0.7.3]: https://github.com/CenturyBoys/ide_cc/compare/v0.7.2...v0.7.3
[0.7.2]: https://github.com/CenturyBoys/ide_cc/compare/v0.7.1...v0.7.2
[0.7.1]: https://github.com/CenturyBoys/ide_cc/compare/v0.7.0...v0.7.1
[0.7.0]: https://github.com/CenturyBoys/ide_cc/compare/v0.6.0...v0.7.0
[0.6.0]: https://github.com/CenturyBoys/ide_cc/compare/v0.5.0...v0.6.0
[0.5.0]: https://github.com/CenturyBoys/ide_cc/compare/v0.4.1...v0.5.0
[0.4.1]: https://github.com/CenturyBoys/ide_cc/compare/v0.4.0...v0.4.1
[0.4.0]: https://github.com/CenturyBoys/ide_cc/compare/v0.3.0...v0.4.0
[0.3.0]: https://github.com/CenturyBoys/ide_cc/compare/v0.2.0...v0.3.0
[0.2.0]: https://github.com/CenturyBoys/ide_cc/compare/v0.1.0...v0.2.0
[0.1.0]: https://github.com/CenturyBoys/ide_cc/releases/tag/v0.1.0
