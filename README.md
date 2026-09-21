<p align="center">
  <img src="docs/assets/banner.jpeg" alt="CRITICAL — code intelligence" width="460">
</p>

<h1 align="center">ide_<code>cc</code> · code intelligence <strong>CRITICAL</strong></h1>

<p align="center">
  <b>The refactoring layer that refuses to break your build.</b><br>
  An <b>MCP</b> server that gives your agent (Claude Code &amp; friends) <b>semantic</b> code
  operations — navigate, rename, move, extract — that are <b>fast</b> and, above all,
  <b>verified</b>: an edit is either <b>correct by construction</b> or it is <b>not applied</b>.
</p>

<p align="center">
  <a href="https://github.com/CenturyBoys/ide_cc/actions/workflows/ci.yml"><img src="https://github.com/CenturyBoys/ide_cc/actions/workflows/ci.yml/badge.svg" alt="CI"></a>
  <a href="https://github.com/CenturyBoys/ide_cc/releases"><img src="https://img.shields.io/github/v/release/CenturyBoys/ide_cc?sort=semver" alt="Release"></a>
  <img src="https://img.shields.io/badge/languages-5-blue" alt="5 languages">
  <img src="https://img.shields.io/badge/tools-10-blueviolet" alt="10 tools">
  <a href="LICENSE"><img src="https://img.shields.io/badge/license-MIT-green.svg" alt="MIT"></a>
</p>

<p align="center">
  <b>English</b> · <a href="README.pt-BR.md">Português (BR)</a>
</p>

---

## You use an IDE. Why don't your agents?

When *you* rename a class, you don't `grep`/`sed` it — you let the IDE do it, because the IDE
understands the code. Your agent, by default, has none of that. Ask it *"rename the class `Widget`
to `Gadget`"* and, without a semantic layer, it falls back to text substitution — and **corrupts
things silently**: it swaps the string `"Widget"`, a comment, and even an **unrelated** `Widget`
constant in another file. Worst of all: **it still compiles.** A silent bug nobody sees. In a large
refactor, that's **critical**.

> **The thesis:** the LLM decides **what**; the tool performs the **mechanical** operation (via LSP);
> the validator **checks** the result (`net_delta` + build). The change is **correct by
> construction** — not "hoping the model reasoned correctly."

So yes — this gives your agent an IDE-grade semantic layer. But navigation isn't the point; the
native LSP in Claude Code already does that. **Our point is the guarantee on the *edit*.**

### Proof of value (measured)

Renaming the class `Widget` in a project full of traps (a same-named constant, string literals,
substrings like `WidgetFactory`):

| | Correct? | What happened |
|---|---|---|
| **raw text** (`\bWidget\b`→sed) | **NO** ❌ | swapped string literals and the unrelated const — **and it compiled** (silent bug) |
| **`rename_symbol`** (semantic) | **YES** ✅ | only the class and its refs; traps untouched; clean build |

With a strong agent, **without** the layer it gets it right ~2/3 of the time (it reasons, but slips);
**with** the layer it's **3/3 with a guarantee**. Details: [`docs/AB-EXPERIMENT.md`](docs/AB-EXPERIMENT.md).

## How we're different

There are already good LSP→MCP bridges. What no one else ships as a **guarantee** is the safety on
the write path:

- 🛡️ **apply → verify → auto-rollback** — `net_delta` simulates the edit **in memory**, measures
  errors before/after, and **applies only if it introduces none**; `verify_build` then runs the real
  build **on disk** and **reverts** if it fails. Other tools hand you diagnostics and let *you* run
  the build; here the tool **refuses to leave your tree broken**.
- 🔥 **Warmup gate** — never returns **partial** references while the index is still building (the
  *cold-index race*, a real agent bug). Proven across all 5 languages.
- 🩺 **`doctor`** — checks **and fixes** per-language setup (e.g. generates Python's
  `pyrightconfig.json` by detecting `src/` and the `.venv`). This catches the *silent-incomplete*
  failure mode — where `find_references` quietly returns too few results because the workspace was
  misconfigured (measured: **5 vs 61** refs on a real project).
- 🔄 **Freshness** — reflects edits made **outside** the agent (mtime re-sync).
- ⚡ **Cross-session cache** (opt-in) — a daemon keeps the language servers warm across MCP restarts
  (**~23×** faster on the 2nd session for heavy projects like Rust/C#).

## How we compare

Honest table. We cover fewer languages than the generalists on purpose — the bet is **depth and
safety on the edit**, not breadth of navigation.

| | **ide_cc** | Serena | agent-lsp | generic LSP→MCP bridges |
|---|---|---|---|---|
| Semantic rename / navigation | ✅ | ✅ | ✅ | ✅ |
| In-memory pre-check (`net_delta`) | ✅ | ❌ | ✅ | ❌ |
| **Real build run + auto-rollback** | ✅ **(tool guarantee)** | ❌ (you run it) | ⚠️ (opt-in skill) | ❌ |
| Warmup / cold-index protection | ✅ | ✅ | ✅ | ❌ (often per-request cold start) |
| Setup **auto-fix** (`doctor`) | ✅ | ❌ | ❌ | ❌ |
| Cross-session warm cache | ✅ | ❌ | ⚠️ (persistent session) | ❌ |
| Languages | 5 | 40+ | 30 | many |

> If you want the widest language coverage, **Serena** and **agent-lsp** are excellent. If your
> priority is that a refactor **never silently breaks the build**, that's what we optimize for.

## Tools (10)

| Tool | What it does |
|---|---|
| `find_references` | all semantic references to a symbol (with warmup gate) |
| `rename_symbol` | semantic rename with apply→verify (`net_delta` + optional `verify_build`) |
| `move_symbol` | moves a symbol to a new file, updating imports |
| `extract_function` | extracts a snippet into a new function |
| `document_symbols` | symbol tree (classes → methods) of a file |
| `find_symbol` | finds a symbol by `Class/method`, with exact position |
| `workspace_symbols` | searches for a symbol across the whole project |
| `call_hierarchy` | who calls this symbol (incoming calls) |
| `validate_build` | runs the language build and reports errors (2nd safety layer) |
| `doctor` | checks/fixes per-language project setup |

## Languages (5)

| Language | Navigation / rename | Refactorings | Setup required |
|---|---|---|---|
| TypeScript/JS | **tsgo** (fast, doesn't truncate) | **vtsls** | `tsconfig.json` |
| Python | **basedpyright** | basedpyright | ⚠️ `[tool.basedpyright]` (use `doctor`) |
| Dart | **dart language-server** | dart | `dart pub get` |
| Rust | **rust-analyzer** | rust-analyzer | `Cargo.toml` |
| C# | **csharp-ls** (Roslyn) | csharp-ls | .NET SDK + `DOTNET_ROOT` |

Per-language requirements and gotchas: [`docs/LANGUAGE-SETUP.md`](docs/LANGUAGE-SETUP.md).

## Install

Prebuilt binaries for **Linux** (x86_64 / arm64), **macOS** (Apple Silicon / Intel) and
**Windows** (x86_64).

### Linux / macOS

**One command** (downloads the binary, installs the language servers, registers it globally in
Claude Code):
```bash
curl -fsSL https://raw.githubusercontent.com/CenturyBoys/ide_cc/main/install.sh | INSTALL_LSP=1 REGISTER=1 bash
```

Or just the binary (you handle the rest):
```bash
curl -fsSL https://raw.githubusercontent.com/CenturyBoys/ide_cc/main/install.sh | bash
```

### Windows (PowerShell)

**One command** (binary + language servers + global registration in Claude Code):
```powershell
$env:INSTALL_LSP=1; $env:REGISTER=1; irm https://raw.githubusercontent.com/CenturyBoys/ide_cc/main/install.ps1 | iex
```

Or just the binary:
```powershell
irm https://raw.githubusercontent.com/CenturyBoys/ide_cc/main/install.ps1 | iex
```

> **Windows note:** every semantic tool works. The only feature not available is the opt-in
> cross-session cache (`CODE_INTEL_DAEMON`), which relies on Unix sockets. Alternatively, run the
> Linux binary under **WSL** to get the daemon too.

Register it **globally** in Claude Code (all projects, no per-folder `.mcp.json`):
```bash
claude mcp add code-intel --scope user -- "$HOME/.local/bin/code-intel-mcp"
```
Or a **per-project** `.mcp.json` — example:
```json
{
  "mcpServers": {
    "code-intel": {
      "command": "/path/to/code-intel-mcp",
      "env": { "TSGO_BIN": "tsgo", "VTSLS_BIN": "vtsls", "BASEDPYRIGHT_BIN": "basedpyright-langserver",
               "DART_BIN": "dart", "RUST_ANALYZER_BIN": "rust-analyzer", "CSHARP_LS_BIN": "csharp-ls" }
    }
  }
}
```
Cross-session cache: add `"CODE_INTEL_DAEMON": "1"` to `env`.

## Usage

On a new project, run `doctor` once (checks setup; `fix=true` repairs it). After that it's natural:
> *"how many references does the class `Widget` have?"* · *"rename the class `Widget` to `Gadget`"*

> **Important — the installer does not configure the per-project workspace.** That's what `doctor`
> does, run **inside each project**. This matters most in **Python**: without a
> `[tool.basedpyright]`/`pyrightconfig.json`, basedpyright falls back to *openFilesOnly* mode and
> `find_references` comes back **silently incomplete**. `doctor fix=true` writes the config and, if
> it finds a local virtualenv (`.venv`/`venv`/`env` — this covers **uv** and native venvs), wires
> up `venvPath`. **Poetry caveat:** by default Poetry creates the venv *outside* the project, so it
> isn't auto-detected — either set `poetry config virtualenvs.in-project true` or add `venvPath`
> manually.

The agent uses the semantic tools (the [`semantic-refactor`](.claude/skills/semantic-refactor/SKILL.md)
skill nudges it to prefer these over grep/sed). You decide **what**; the tool guarantees the
mechanical precision.

### Error log (field diagnostics)

Every tool failure and panic is appended as JSON to a **local** log (no network/telemetry) — so
problems on a user's machine are discoverable without a manual report. Default:
`~/.cache/code-intel-mcp/errors.jsonl` (override with `CODE_INTEL_LOG=/path`, disable with
`CODE_INTEL_LOG=off`). `doctor` reports the path in `error_log`. To diagnose, inspect the file (or
ask the user to send it).

## Documentation

- [`docs/LANGUAGE-SETUP.md`](docs/LANGUAGE-SETUP.md) — per-language requirements (⚠️ Python)
- [`docs/AB-EXPERIMENT.md`](docs/AB-EXPERIMENT.md) — the proof of value (with/without the layer)
- [`docs/ROADMAP.md`](docs/ROADMAP.md) · [`docs/RELATORIO-LEVANTAMENTO.md`](docs/RELATORIO-LEVANTAMENTO.md) · [`docs/WORKFLOW.md`](docs/WORKFLOW.md)
- [`CHANGELOG.md`](CHANGELOG.md) · [`CLAUDE.md`](CLAUDE.md) (AI-first guide)
- [`mcp/README.md`](mcp/README.md) — server details · [`benchmarks/README.md`](benchmarks/README.md) — latency of the 5 servers

## License

MIT — see [LICENSE](LICENSE).
