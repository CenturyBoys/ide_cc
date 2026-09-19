# Changelog

Todas as mudanças notáveis deste projeto são documentadas aqui.
Formato baseado em [Keep a Changelog](https://keepachangelog.com/pt-BR/1.1.0/);
versionamento segue [SemVer](https://semver.org/lang/pt-BR/).

## [Unreleased]

### Added
- **Tool `doctor`**: verifica o setup do projeto por linguagem (LSP disponível + config de
  workspace) e, com `fix=true`, corrige o que dá — ex.: cria `pyrightconfig.json` p/ Python
  (detecta `src/` e `.venv`). Fecha o gap descoberto no teste do pachamama.

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

[Unreleased]: https://github.com/CenturyBoys/ide_cc/compare/v0.4.1...HEAD
[0.4.1]: https://github.com/CenturyBoys/ide_cc/compare/v0.4.0...v0.4.1
[0.4.0]: https://github.com/CenturyBoys/ide_cc/compare/v0.3.0...v0.4.0
[0.3.0]: https://github.com/CenturyBoys/ide_cc/compare/v0.2.0...v0.3.0
[0.2.0]: https://github.com/CenturyBoys/ide_cc/compare/v0.1.0...v0.2.0
[0.1.0]: https://github.com/CenturyBoys/ide_cc/releases/tag/v0.1.0
