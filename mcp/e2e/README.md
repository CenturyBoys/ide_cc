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

`xfail` = bug conhecido/ambiental: se falhar é XFAIL (não quebra a suíte); se passar é XPASS (avisa).

## CI

`.github/workflows/e2e.yml` instala os 5 language servers e roda a suíte a cada push/PR.
Instalação best-effort — o que não instalar é pulado.
