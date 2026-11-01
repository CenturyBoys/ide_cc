# Suíte e2e — ferramentas reais × language servers reais

Os testes unitários (`cargo test`) cobrem a **lógica pura** (casamento de name_path, `scan_ident`,
`ws_backend`, `is_conn_dead`). Mas os bugs de campo (decorator → `count:0`, método em C#,
`workspace_symbols` do Dart, move no-op, extract Broken pipe) **só aparecem contra um language
server real sobre código real**. Esta suíte fecha esse buraco.

Cada fixture em `fixtures/<lang>/` reproduz uma armadilha específica dos relatórios; `cases.json`
declara as chamadas de ferramenta e as asserções. Linguagem cujo LSP não está instalado é **PULADA**
(não quebra). Sai `!= 0` se qualquer caso **FALHA**.

## Rodar (fixtures)

```bash
cargo build --release --manifest-path mcp/Cargo.toml
python3 mcp/e2e/run.py                 # todas as linguagens disponíveis
python3 mcp/e2e/run.py python csharp   # só essas
```

Aponte os binários dos LSPs por env se não estiverem no PATH: `TSGO_BIN`, `VTSLS_BIN`,
`BASEDPYRIGHT_BIN`, `DART_BIN`, `RUST_ANALYZER_BIN`, `CSHARP_LS_BIN`. Timeouts:
`CODE_INTEL_E2E_TIMEOUT` (sessão), `CODE_INTEL_E2E_SETUP_TIMEOUT` (setup como `dart pub get`).

## Rodar (repo REAL grande — opt-in, valida escala/warmup)

Fixtures pequenos não pegam escala/warmup de repo grande. Para isso, aponte pra um repo real:

```bash
CODE_INTEL_E2E_REPO=/caminho/do/repo CODE_INTEL_E2E_LANG=python \
  python3 mcp/e2e/run.py --real
```

Roda `doctor smoke=true` (descobre um símbolo e roda um `find_references` real) contra o repo —
exige `smoke.ok==true`. Bom como validação pré-release local.

## Rodar (OSS famoso PINADO por linguagem — Camada 1 do #16)

Em vez de apontar pra um repo da sua máquina, baixe um conjunto **curado e pinado** de OSS famoso
(1 por linguagem) e rode asserções de **escala/correção** com símbolos-oráculo:

```bash
bash benchmarks/scripts/setup-real-fixtures.sh          # clona em fixtures/real/<lang> (GITIGNORED)
CODE_INTEL_E2E_CASES=cases-real-fixtures.json \
  python3 mcp/e2e/run.py python dart                    # leves: passam out-of-the-box
```

Repos (pinados em `fixtures/real/VERSIONS.txt`): **zod** (TS v3.23.8), **requests** (Py v2.32.3),
**ripgrep** (Rust 14.1.1), **collection** (Dart), **Newtonsoft.Json** (C# 13.0.3). O setup roda o
pré-requisito de cada linguagem (npm / venv+pip / cargo fetch / pub get / dotnet restore) — senão o
LSP não resolve o projeto inteiro e `find_references` sai parcial.

**Oráculos** (contagem de refs de um símbolo conhecido, estável no tag pinado; cross-check com o
`verification.grep` do resultado): `Response` (requests) ≥3, `QueueList` (collection) ~104,
`ZodError` (zod) =98, `Glob` (ripgrep) e `JsonConvert` (Newtonsoft).

**Custo / o que roda onde:**
- **Python + Dart**: leves → verdes localmente e no CI.
- **TS (zod) + Rust (ripgrep)**: cold-start pesado (zod estoura o budget da sessão; ripgrep é um
  workspace de 13 crates) → marcados **`xfail`** no catálogo; rodam de verdade com
  `CODE_INTEL_DAEMON=1` quente ou num runner de CI com mais budget/recursos.
- **C# (Newtonsoft)**: **PULA** sem `dotnet`/`csharp-ls` (roda no CI que os instala).

## O que cada caso pega

| Linguagem | Caso | Armadilha |
|---|---|---|
| python | find_references em `@dataclass` sem `line` | posição no decorator → `count:0` (P1) |
| python | find_symbol `Classe/metodo` | método em classe decorada + servers achatados |
| python/dart | doctor `smoke=true` | falso-OK do doctor (checa só binário+config) |
| typescript | find_symbol `Widget/render` == 1 | precisão do name_path composto (homônimos) |
| typescript | move_symbol cria arquivo | move para novo arquivo (vtsls) |
| typescript | extract_function (`xfail`) | Broken pipe do vtsls (bug conhecido/ambiental) |
| dart | workspace_symbols `lang=dart` > 0 | roteamento por linguagem (silent-empty) |
| csharp | find_symbol `Handler/DoWork` == 1 | assinatura no nome do método → `count:0` |
| csharp | move_symbol == `move_no_op` | no-op honesto (csharp-ls não move p/ novo arquivo) |
| **typescript** (`ts-monorepo-degraded`) | doctor `env_resolution.ok=false` + simulate `type_check_reliable=false` | **ambiente degradado**: monorepo pnpm sem tsconfig raiz → `process` sem `@types/node`; net_delta não-confiável (erro de config excluído), mas doctor NÃO bloqueia (monorepo válido) |
| **python** (`python-degraded`) | doctor `env_resolution.ok=false` + simulate `type_check_reliable=false` | **ambiente degradado**: dep importada não instalada (`reportMissingImports`) |
| typescript/python (saudável) | **GUARDA** `env_resolution.ok=true` | anti-falso-positivo: projeto bem configurado NÃO é sinalizado |
| dart (`lib/ext.dart`) | **N5** (issue #4): `extension`/`mixin` → kind "Extension"/"Mixin" consistente em `find_symbol` **e** `workspace_symbols` | kind divergente entre document/workspace symbols (Namespace vs Class) |

`xfail` = bug conhecido/ambiental: se falhar é XFAIL (não quebra a suíte); se passar é XPASS (avisa).

### Fixtures de ambiente degradado

- `fixtures/ts-monorepo-degraded/` — monorepo pnpm **sem** `tsconfig.json` na raiz; o pacote usa
  `process` sem `@types/node`. **Não instale `node_modules`** aqui — a ausência de `@types/node` é o
  ponto (reproduz o `inferred-project` do tsgo).
- `fixtures/python-degraded/` — `pyrightconfig.json` presente, mas a dependência importada **não**
  está instalada (sem venv). **Não instale o pacote** — a ausência é o ponto (`reportMissingImports`).

Esses casos provam o contrato **auto-explicativo** consumido por agents: `env_resolution`/`reliability`
trazem `cause`, `impact`, `evidence` e `fix.commands` acionáveis.

## Matriz de capacidade por linguagem

O que cada backend suporta hoje (validado pelos casos e2e; o não-suportado retorna erro **explícito**,
não silencioso). `?` = ainda não coberto por caso e2e.

| Operação | TS (tsgo/vtsls) | Python (basedpyright) | Dart | Rust (rust-analyzer) | C# (csharp-ls) |
|---|---|---|---|---|---|
| find_references | ✅ | ✅ | ✅ | ✅ | ✅ |
| find_symbol (`Classe/metodo`) | ✅ | ✅ | ✅ | ✅ | ✅ |
| document_symbols | ✅ | ✅ | ✅ | ✅ | ✅ (`record`→`Record`) |
| workspace_symbols | ✅ | ✅ | ✅ | ? | ✅ |
| call_hierarchy | ✅ | ✅ | ✅ | ✅ | ✅ |
| rename_symbol (+ colisão) | ✅ | ✅ | ✅ | ✅ | ✅ |
| extract_function | ✅ (vtsls) | ❌ `unsupported` | ? | ? | ✅ |
| move_symbol (→ novo arquivo) | ✅ (vtsls) | ❌ `unsupported` | ? | ? | ⚠️ `move_no_op` (csharp-ls não cria arquivo) |
| validate_build | via `TS_CHECK_CMD` | ✅ basedpyright | ✅ dart analyze | ✅ cargo check | ✅ dotnet build |

Regras de segurança **transversais** (independentes de linguagem, cobertas por casos adversariais):
`rename` barra colisão de escopo (mesmo com diagnósticos desligados), valida `new_name`
(keyword/inválido → rejeita; `new==old` → noop); `move` que não cria arquivo → `move_no_op`;
`workspace_symbols` sem fontes da lang → retorno rápido; `validate_build` não confunde `"0 Error(s)"`.

## CI

`.github/workflows/e2e.yml` instala os 5 language servers e roda a suíte a cada push/PR.
Instalação best-effort — o que não instalar é pulado.
