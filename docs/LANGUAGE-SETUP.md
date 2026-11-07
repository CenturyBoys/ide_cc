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
- **Ambiente degradado (detecção automática):** imports não-resolvidos (venv/deps ausentes) →
  basedpyright emite `reportMissingImports`/`reportMissingModuleSource`. O servidor marca
  `env_resolution.ok=false` (doctor) e `type_check_reliable=false` (edit-tools), excluindo esses
  erros do `net_delta`. **Fix:** `python -m venv .venv && . .venv/bin/activate && pip install -r
  requirements.txt` (e `doctor fix=true` cria o `pyrightconfig.json`).

## 🟦 TypeScript (tsgo / vtsls)

- Precisa de `tsconfig.json`. Em **monorepo**, use **project references** (`composite`) — senão as
  referências cross-package ficam parciais.
- Navegação/rename via **tsgo** (rápido, não trunca). Extract/move via **vtsls** (tsgo não os tem).
- Sem config extra além do tsconfig; cold-start ~250 ms.
- **Monorepo sem tsconfig na RAIZ (ex.: pnpm workspaces + tsconfig por pacote):** válido, mas o tsgo
  lançado na raiz pode cair em *inferred-project* e **não resolver `@types/node`/aliases** — aí ele
  flaga `process`/globals/aliases como erro (falso-positivo). Isso **degrada o net_delta como
  type-check**: ele acerta erro de tipo "puro" (`number = "x"`), mas não os que dependem de
  `@types`/tsconfig. O servidor **detecta e é honesto** nesse caso:
  - `doctor` devolve `env_resolution: {ok:false, evidence, cause, impact, fix}` (abre um `.ts` real e
    vê o erro de assinatura de config — códigos TS **2580/2591/2318/2688/6053**) em vez de adivinhar
    por presença de arquivo. `workspace_config.ok` continua `true` (monorepo é layout válido — vira
    WARN `env_resolution_degradada`, não bloqueio).
  - as edit-tools (`simulate_edit`/`safe_apply`/rename/extract/…) devolvem `type_check_reliable:
    false` + objeto **`reliability`** (cause/impact/fix.commands/evidence) e **excluem** esses erros
    de config do `net_delta` (senão uma edição legítima seria recusada por ruído). O sinal é por
    conjunto-de-arquivos-da-edição.
  - **Fix de verdade:** garanta que o tsconfig do pacote cobre o arquivo e que `@types/node` é
    resolvível; ou use `validate_build`/CI como juiz de tipos nesse repo.
- **`validate_build` em TS não tem default** (ao contrário de Rust/Dart/C#/Python) e exige o comando
  explícito via **`TYPESCRIPT_CHECK_CMD`** (ex.: `tsc --noEmit -p tsconfig.json`; `TS_CHECK_CMD` é
  alias legado). É de propósito: um `tsgo --noEmit` na raiz pode cair em *inferred-project* e checar
  nada em silêncio. Sem esse env, `validate_build` devolve erro acionável apontando o nome do var.

## 🎯 Dart (Dart Analysis Server)

- **Rode `dart pub get` (ou `flutter pub get`) antes** — o server precisa do
  `.dart_tool/package_config.json` para resolver imports.
- Não trunca (eager). Cold-start baixo em pacote puro; **alto em Flutter** (grafo SDK+deps).
- **Ambiente degradado (detecção automática):** sem `pub get` (ou dep ausente) → `uri_does_not_exist`/
  `uri_has_not_been_generated`. O servidor marca `env_resolution.ok=false`/`type_check_reliable=false`
  e exclui do `net_delta`. **Fix:** `dart pub get` (ou `flutter pub get`).

## 🦀 Rust (rust-analyzer)

- Precisa de `Cargo.toml`. Cold-start **~30 s** (cargo metadata + `cargo check`) — o mais pesado;
  o gate de warmup espera até 60 s.
- **Limitação:** o `net_delta` em memória vê os diagnostics **nativos**, mas **não** os que só o
  `cargo check` (flycheck, que lê o disco) reporta. Para rename/extract em Rust, use
  **`verify_build: true`** (roda `cargo check` no disco e reverte se falhar). RAM pode ser alta.
- **Ambiente degradado (detecção automática):** crate/dependência externa não-resolvida (deps não
  baixadas / índice frio) → `E0432`/`E0463`/`unresolved-import`/`unresolved-extern-crate` (o `E0583`
  de módulo faltando NÃO entra — é erro real). Marca `env_resolution.ok=false`/`type_check_reliable=
  false` e exclui do `net_delta`. **Fix:** `cargo fetch && cargo check` (e aumente
  `CODE_INTEL_WARMUP_MS` se o índice estiver frio); use `validate_build` como juiz confiável.

## 🟪 C# (csharp-ls / Roslyn)

- Precisa do **.NET SDK** instalado e de `DOTNET_ROOT` no ambiente do MCP; o projeto carrega via
  **MSBuild** (cold-start ~24 s). Não trunca (bloqueia até carregar). `net_delta` confiável
  (Roslyn analisa em memória).
- **Instalar o language server:** `dotnet tool install --global csharp-ls` (fica em
  `$DOTNET_ROOT/tools/csharp-ls`). No MCP: `DOTNET_ROOT=<sdk>` + `CSHARP_LS_BIN=<.../tools/csharp-ls>`
  (e garanta que `$DOTNET_ROOT` e `$DOTNET_ROOT/tools` estão no `PATH`). O TFM do projeto deve bater
  com o SDK (o fixture usa `net10.0`; ajuste `CS_TFM` se seu SDK for outro). Verificação e2e:
  `mcp/test-csharp.jsonl` (503 refs stable no `cs-demo`).
- **Ambiente degradado (detecção automática):** projeto/solução não carregada (sem `restore`) →
  `CS0246`/`CS0234` (tipo/namespace não encontrado). Marca `env_resolution.ok=false`/
  `type_check_reliable=false`. **Fix:** `dotnet restore` + `DOTNET_ROOT`/SDK no PATH. *(assinatura
  documentada; verificada no CI — o ambiente de dev local não tem dotnet.)*

---

## Atalho: a tool `doctor`

Em vez de checar à mão, rode a ferramenta **`doctor`** do MCP ao abrir um projeto: ela detecta as
linguagens, verifica se o language server está disponível e se a config de workspace está correta,
e com `fix=true` **corrige** o que dá (ex.: cria o `pyrightconfig.json` do Python, detectando `src/`
e o `.venv` automaticamente).

```
doctor(project="/caminho/do/projeto")            # só diagnóstico
doctor(project="/caminho/do/projeto", fix=true)  # aplica as correções
```

## Regra geral

- **Referência semântica só é confiável se o server "enxerga" o projeto inteiro.** Garanta a config
  de workspace da linguagem (acima) e confirme com um sanity check: compare o `count` do
  `find_references` com um `grep -rl` — se o semântico for muito menor, falta config.
- Freshness (edições externas) e cache entre sessões funcionam em todas as linguagens.
