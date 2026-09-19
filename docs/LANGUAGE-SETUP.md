# Setup por linguagem — requisitos para resultados CORRETOS

Cada language server tem pré-requisitos de projeto. **Se faltarem, o resultado pode sair
incompleto — às vezes em silêncio** (o gate de warmup protege contra índice-carregando, não
contra config errada). Requisitos abaixo vêm de testes em projetos reais.

## 🐍 Python (basedpyright) — ⚠️ o mais crítico

**Requer config de workspace**, senão o `find_references`/`rename` fica **incompleto e silencioso**:
o basedpyright roda em modo "só arquivos abertos" e só acha referências dentro do arquivo aberto.

Medido no projeto pachamama (~3.700 arquivos): `find_references` de uma classe deu **5** (1 arquivo)
sem config → **61** (20 arquivos) com config. O gate reportou `stable: true` nos dois casos, porque
5 era *estavelmente incompleto*.

**Correção — adicione ao `pyproject.toml`** (ou um `pyrightconfig.json` equivalente):
```toml
[tool.basedpyright]
include = ["src", "tests"]   # as raízes de código do projeto
venvPath = "."
venv = ".venv"               # para resolver imports do virtualenv
```

- **Custo:** cold-start ~9 s; warm ~1,7 s por `find_references` (Pyright re-varre o programa —
  bem mais que os ~40 ms de projetos pequenos); RAM ~376 MB.
- **Dica:** rode com `CODE_INTEL_DAEMON=1` para não pagar o warmup a cada sessão.

## 🟦 TypeScript (tsgo / vtsls)

- Precisa de `tsconfig.json`. Em **monorepo**, use **project references** (`composite`) — senão as
  referências cross-package ficam parciais.
- Navegação/rename via **tsgo** (rápido, não trunca). Extract/move via **vtsls** (tsgo não os tem).
- Sem config extra além do tsconfig; cold-start ~250 ms.

## 🎯 Dart (Dart Analysis Server)

- **Rode `dart pub get` (ou `flutter pub get`) antes** — o server precisa do
  `.dart_tool/package_config.json` para resolver imports.
- Não trunca (eager). Cold-start baixo em pacote puro; **alto em Flutter** (grafo SDK+deps).

## 🦀 Rust (rust-analyzer)

- Precisa de `Cargo.toml`. Cold-start **~30 s** (cargo metadata + `cargo check`) — o mais pesado;
  o gate de warmup espera até 60 s.
- **Limitação:** o `net_delta` em memória vê os diagnostics **nativos**, mas **não** os que só o
  `cargo check` (flycheck, que lê o disco) reporta. Para rename/extract em Rust, use
  **`verify_build: true`** (roda `cargo check` no disco e reverte se falhar). RAM pode ser alta.

## 🟪 C# (csharp-ls / Roslyn)

- Precisa do **.NET SDK** instalado e de `DOTNET_ROOT` no ambiente do MCP; o projeto carrega via
  **MSBuild** (cold-start ~24 s). Não trunca (bloqueia até carregar). `net_delta` confiável
  (Roslyn analisa em memória).

---

## Regra geral

- **Referência semântica só é confiável se o server "enxerga" o projeto inteiro.** Garanta a config
  de workspace da linguagem (acima) e confirme com um sanity check: compare o `count` do
  `find_references` com um `grep -rl` — se o semântico for muito menor, falta config.
- Freshness (edições externas) e cache entre sessões funcionam em todas as linguagens.
