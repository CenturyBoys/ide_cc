<p align="center">
  <img src="docs/assets/banner.jpeg" alt="CRITICAL — code intelligence" width="460">
</p>

<h1 align="center">ide_<code>cc</code> · code intelligence <strong>CRITICAL</strong></h1>

<p align="center">
  <b>Uma IDE na mão da LLM.</b><br>
  Um servidor <b>MCP</b> que dá ao seu agente (Claude Code &amp; afins) operações <b>semânticas</b> de
  código — navegar, renomear, mover, extrair — <b>rápidas</b> e <b>seguras</b>, sobre language servers reais.
</p>

<p align="center">
  <a href="https://github.com/CenturyBoys/ide_cc/actions/workflows/ci.yml"><img src="https://github.com/CenturyBoys/ide_cc/actions/workflows/ci.yml/badge.svg" alt="CI"></a>
  <a href="https://github.com/CenturyBoys/ide_cc/releases"><img src="https://img.shields.io/github/v/release/CenturyBoys/ide_cc?sort=semver" alt="Release"></a>
  <img src="https://img.shields.io/badge/linguagens-5-blue" alt="5 linguagens">
  <img src="https://img.shields.io/badge/ferramentas-23-blueviolet" alt="23 ferramentas">
  <a href="LICENSE"><img src="https://img.shields.io/badge/license-MIT-green.svg" alt="MIT"></a>
</p>

<p align="center">
  <a href="README.md">English</a> · <b>Português (BR)</b>
</p>

---

## Por que isto é CRITICAL

Peça a um agente *"renomeie a classe `Widget` para `Gadget`"*. Sem uma camada semântica, ele cai no
`grep`/`sed` — e **corrompe em silêncio**: troca a string `"Widget"`, o comentário, e até uma
constante `Widget` **não-relacionada** noutro arquivo. E o pior: **compila**. Um bug silencioso que
ninguém vê. Isso, num refactor grande, é **crítico**.

> **A tese:** o LLM decide **o quê**; a ferramenta faz a operação **mecânica** (via LSP); o
> validador **confere** (`net_delta` + build). A mudança é **correta por construção** — não
> "torcendo pra ter raciocinado certo".

### Prova de valor (medida)

Renomear a classe `Widget` num projeto com armadilhas (const homônima, strings, substrings):

| | Correto? | O que aconteceu |
|---|---|---|
| **texto-cru** (`\bWidget\b`→sed) | **NÃO** ❌ | trocou strings e a const alheia — **e compilou** (bug silencioso) |
| **`rename_symbol`** (semântico) | **SIM** ✅ | só a classe e suas refs; armadilhas intactas; build limpo |

Num agente forte, **sem** a camada acerta ~2/3 (raciocina, mas falha); **com** a camada = **3/3
com garantia**. Detalhes: [`docs/AB-EXPERIMENT.md`](docs/AB-EXPERIMENT.md).

## O que torna isto diferente

- 🔥 **Gate de warmup** — nunca retorna referências **parciais** durante a indexação (o
  *cold-index race*, um bug real de agentes). Provado transversal: 5 linguagens.
- 🛡️ **apply → verify (`net_delta`)** — simula a edição **em memória**, mede erros antes/depois e
  **só aplica se não introduzir erros**; `verify_build` roda o build no disco e reverte se falhar.
- 🔄 **Frescor** — reflete edições feitas **fora** do agente (re-sync por mtime).
- ⚡ **Cache entre sessões** (opt-in) — daemon mantém os LSPs quentes entre reinícios do MCP
  (**~23×** na 2ª sessão em projetos pesados como Rust/C#).
- 🩺 **`doctor`** — verifica e **corrige** o setup por linguagem (ex.: cria o `pyrightconfig.json`
  do Python detectando `src/` e o `.venv`).

## Ferramentas (23)

**Navegação** — localizações retornam `path:linha:conteúdo` + ~2 linhas de contexto (menos re-leituras):

| Tool | O que faz |
|---|---|
| `find_references` | todas as referências semânticas a um símbolo (com gate de warmup) |
| `find_symbol` | acha símbolo por `Classe/metodo`, com posição exata |
| `workspace_symbols` | busca símbolo em todo o projeto |
| `document_symbols` | árvore de símbolos (classes → métodos) de um arquivo |
| `call_hierarchy` | quem chama este símbolo (incoming calls) |
| `blast_radius` | superfície de risco de um símbolo (refs + callers, test vs produção) antes de editar |

**Edição verificada** — `apply=false` é preview; edições passam por `net_delta` (+ `verify_build` opcional):

| Tool | O que faz |
|---|---|
| `rename_symbol` | rename semântico com apply→verify (`net_delta` + `verify_build` opcional) |
| `move_symbol` | move símbolo para novo arquivo, atualizando imports |
| `move_file` | move um arquivo inteiro e conserta os importers; reverte se o build quebrar |
| `extract_function` | extrai um trecho para uma nova função |
| `change_signature` | add/remove/reordena um parâmetro na declaração + todos os call-sites |
| `replace_symbol_body` · `insert_before_symbol` · `insert_after_symbol` | edita pelo **nome** do símbolo (sem coordenadas cruas) |
| `organize_imports` | organiza imports (preserva side-effect + type-only usado) |
| `quick_fix` | aplica UMA code-action de correção para o diagnóstico de uma linha |
| `safe_delete` | deleta um símbolo só se não houver referência externa (senão recusa) |
| `simulate_edit` · `preview_edit` · `safe_apply` | simula `net_delta` em memória / mostra diff+blast / aplica só se `net_delta ≤ 0` |

**Build & meta:**

| Tool | O que faz |
|---|---|
| `validate_build` | roda o build da linguagem e reporta erros (2ª camada de segurança) |
| `doctor` | verifica/corrige o setup por linguagem; sonda refs silenciosamente incompletas |
| `instructions` | manual de uso completo e portátil (herdado por qualquer cliente MCP) |

## Linguagens (5)

| Linguagem | Navegação / rename | Refactorings | Setup requerido |
|---|---|---|---|
| TypeScript/JS | **tsgo** (rápido, não trunca) | **vtsls** | `tsconfig.json` |
| Python | **basedpyright** | basedpyright | ⚠️ `[tool.basedpyright]` (use `doctor`) |
| Dart | **dart language-server** | dart | `dart pub get` |
| Rust | **rust-analyzer** | rust-analyzer | `Cargo.toml` |
| C# | **csharp-ls** (Roslyn) | csharp-ls | .NET SDK + `DOTNET_ROOT` |

Requisitos e gotchas por linguagem: [`docs/LANGUAGE-SETUP.md`](docs/LANGUAGE-SETUP.md).

## Instalação

Binários prontos para **Linux** (x86_64 / arm64), **macOS** (Apple Silicon / Intel) e
**Windows** (x86_64).

### Linux / macOS

**Tudo em um comando** (baixa o binário, instala os language servers e registra global no Claude Code):
```bash
curl -fsSL https://raw.githubusercontent.com/CenturyBoys/ide_cc/main/install.sh | INSTALL_LSP=1 REGISTER=1 bash
```

Ou só o binário (e você cuida do resto):
```bash
curl -fsSL https://raw.githubusercontent.com/CenturyBoys/ide_cc/main/install.sh | bash
```

### Windows (PowerShell)

**Tudo em um comando** (binário + language servers + registro global no Claude Code):
```powershell
$env:INSTALL_LSP=1; $env:REGISTER=1; irm https://raw.githubusercontent.com/CenturyBoys/ide_cc/main/install.ps1 | iex
```

Ou só o binário:
```powershell
irm https://raw.githubusercontent.com/CenturyBoys/ide_cc/main/install.ps1 | iex
```

> **Nota Windows:** todas as ferramentas semânticas funcionam. O único recurso indisponível é o
> cache entre sessões opt-in (`CODE_INTEL_DAEMON`), que depende de Unix sockets. Alternativa: rode o
> binário Linux via **WSL** para ter o daemon também.

Registrar **global** no Claude Code (todos os projetos, sem `.mcp.json` por pasta):
```bash
claude mcp add code-intel --scope user -- "$HOME/.local/bin/code-intel-mcp"
```
Ou `.mcp.json` **por projeto** — exemplo:
```json
{
  "mcpServers": {
    "code-intel": {
      "command": "/caminho/para/code-intel-mcp",
      "env": { "TSGO_BIN": "tsgo", "VTSLS_BIN": "vtsls", "BASEDPYRIGHT_BIN": "basedpyright-langserver",
               "DART_BIN": "dart", "RUST_ANALYZER_BIN": "rust-analyzer", "CSHARP_LS_BIN": "csharp-ls" }
    }
  }
}
```
Cache entre sessões: adicione `"CODE_INTEL_DAEMON": "1"` ao `env`.

## Uso

Num projeto novo, rode `doctor` uma vez (checa o setup; `fix=true` corrige). Depois é natural:
> *"quantas referências a classe `Widget` tem?"* · *"renomeie a classe `Widget` para `Gadget`"*

> **Importante — o instalador NÃO configura o workspace por projeto.** Isso é papel do `doctor`,
> rodado **dentro de cada projeto**. Crítico em **Python**: sem `[tool.basedpyright]`/
> `pyrightconfig.json`, o basedpyright cai no modo *openFilesOnly* e o `find_references` volta
> **incompleto EM SILÊNCIO**. O `doctor fix=true` cria a config e, se achar um virtualenv local
> (`.venv`/`venv`/`env` — cobre **uv** e venv nativo), preenche o `venvPath`. **Pegadinha do
> poetry:** por padrão o poetry cria o venv *fora* do projeto, então ele não é detectado — use
> `poetry config virtualenvs.in-project true` ou adicione `venvPath` manualmente.

O agente usa as ferramentas semânticas (a skill [`semantic-refactor`](.claude/skills/semantic-refactor/SKILL.md)
o orienta a preferir isso ao grep/sed). Você decide **o quê**; a ferramenta garante a precisão mecânica.

## Documentação

- [`docs/LANGUAGE-SETUP.md`](docs/LANGUAGE-SETUP.md) — requisitos por linguagem (⚠️ Python)
- [`docs/AB-EXPERIMENT.md`](docs/AB-EXPERIMENT.md) — a prova de valor (com/sem a camada)
- [`docs/ROADMAP.md`](docs/ROADMAP.md) · [`docs/RELATORIO-LEVANTAMENTO.md`](docs/RELATORIO-LEVANTAMENTO.md) · [`docs/WORKFLOW.md`](docs/WORKFLOW.md)
- [`CHANGELOG.md`](CHANGELOG.md) · [`CLAUDE.md`](CLAUDE.md) (guia IA-first)
- [`mcp/README.md`](mcp/README.md) — detalhes do servidor · [`benchmarks/README.md`](benchmarks/README.md) — latência dos 5 servers

## Licença

MIT — ver [LICENSE](LICENSE).
