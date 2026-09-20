# Changelog

Todas as mudanças notáveis deste projeto são documentadas aqui.
Formato baseado em [Keep a Changelog](https://keepachangelog.com/pt-BR/1.1.0/);
versionamento segue [SemVer](https://semver.org/lang/pt-BR/).

## [Unreleased]

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

[Unreleased]: https://github.com/CenturyBoys/ide_cc/compare/v0.6.0...HEAD
[0.6.0]: https://github.com/CenturyBoys/ide_cc/compare/v0.5.0...v0.6.0
[0.5.0]: https://github.com/CenturyBoys/ide_cc/compare/v0.4.1...v0.5.0
[0.4.1]: https://github.com/CenturyBoys/ide_cc/compare/v0.4.0...v0.4.1
[0.4.0]: https://github.com/CenturyBoys/ide_cc/compare/v0.3.0...v0.4.0
[0.3.0]: https://github.com/CenturyBoys/ide_cc/compare/v0.2.0...v0.3.0
[0.2.0]: https://github.com/CenturyBoys/ide_cc/compare/v0.1.0...v0.2.0
[0.1.0]: https://github.com/CenturyBoys/ide_cc/releases/tag/v0.1.0
