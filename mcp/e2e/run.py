#!/usr/bin/env python3
"""
Harness e2e do code-intel-mcp: roda as ferramentas REAIS contra language servers REAIS sobre
fixtures que reproduzem cada armadilha dos relatórios de campo (decorator, método C#, workspace
Dart, move no-op, etc.). Os testes unitários (`cargo test`) cobrem a lógica pura; ISTO cobre o
comportamento ponta-a-ponta com o LSP — que é onde aqueles bugs viviam.

Modos:
  - fixtures (default): projetos pequenos e reprodutíveis em mcp/e2e/fixtures/<lang>.
  - repo real (opt-in): CODE_INTEL_E2E_REPO=/caminho + CODE_INTEL_E2E_LANG=<lang> roda um smoke
    de escala (find_references + workspace_symbols + doctor smoke) contra um repo grande de verdade.

Cada linguagem cujo LSP não está instalado é PULADA (não quebra). Sai != 0 se qualquer caso FALHA.

Uso:
  python3 mcp/e2e/run.py                # todas as linguagens disponíveis (fixtures)
  python3 mcp/e2e/run.py python csharp  # só essas
  CODE_INTEL_E2E_REPO=/x CODE_INTEL_E2E_LANG=python python3 mcp/e2e/run.py --real
"""
import json
import os
import shutil
import subprocess
import sys
from pathlib import Path

HERE = Path(__file__).resolve().parent
MCP = HERE.parent
BIN = os.environ.get("CODE_INTEL_BIN", str(MCP / "target" / "release" / "code-intel-mcp"))

# nome do binário do language server por linguagem (mesmos env vars que o servidor usa)
LSP_BIN = {
    "python": os.environ.get("BASEDPYRIGHT_BIN", "basedpyright-langserver"),
    "typescript": os.environ.get("TSGO_BIN", "tsgo"),
    "dart": os.environ.get("DART_BIN", "dart"),
    "rust": os.environ.get("RUST_ANALYZER_BIN", "rust-analyzer"),
    "csharp": os.environ.get("CSHARP_LS_BIN", "csharp-ls"),
}
# refactorings de TS exigem vtsls além do tsgo
EXTRA_BIN = {"typescript": [os.environ.get("VTSLS_BIN", "vtsls")]}


def have(bin_name: str) -> bool:
    return shutil.which(bin_name) is not None or Path(bin_name).exists()


def resolve(target, obj):
    """Resolve um path pontilhado ('report.0.smoke.ok') no objeto; None se ausente."""
    cur = obj
    for seg in target.split("."):
        if isinstance(cur, list):
            try:
                cur = cur[int(seg)]
            except (ValueError, IndexError):
                return None
        elif isinstance(cur, dict):
            if seg not in cur:
                return None
            cur = cur[seg]
        else:
            return None
    return cur


def check_assert(a, result):
    """Retorna (ok, descricao). Ops: > >= < == != nonempty empty present absent contains."""
    op = a["op"]
    path = a.get("path", "")
    val = resolve(path, result) if path else result
    want = a.get("value")
    if op == "present":
        return (val is not None, f"{path} present -> {val!r}")
    if op == "absent":
        return (val is None, f"{path} absent -> {val!r}")
    if op == "nonempty":
        return (bool(val), f"{path} nonempty -> {val!r}")
    if op == "empty":
        return (not val, f"{path} empty -> {val!r}")
    if op == "contains":
        return (val is not None and want in val, f"{path} contains {want!r} -> {val!r}")
    if val is None:
        return (False, f"{path} {op} {want!r} -> AUSENTE")
    try:
        if op == "==":
            return (val == want, f"{path} == {want!r} -> {val!r}")
        if op == "!=":
            return (val != want, f"{path} != {want!r} -> {val!r}")
        if op == ">":
            return (val > want, f"{path} > {want} -> {val}")
        if op == ">=":
            return (val >= want, f"{path} >= {want} -> {val}")
        if op == "<":
            return (val < want, f"{path} < {want} -> {val}")
    except TypeError:
        return (False, f"{path} {op} {want!r} -> tipo incompatível ({val!r})")
    return (False, f"op desconhecido: {op}")


def run_session(project, cases):
    """Roda uma sessão do binário: initialize + um tools/call por caso. Retorna id->result(dict|erro)."""
    reqs = [
        {"jsonrpc": "2.0", "id": 0, "method": "initialize",
         "params": {"protocolVersion": "2024-11-05"}},
        {"jsonrpc": "2.0", "method": "notifications/initialized", "params": {}},
    ]
    for i, c in enumerate(cases, start=1):
        args = dict(c["args"])
        args["project"] = project
        reqs.append({"jsonrpc": "2.0", "id": i, "method": "tools/call",
                     "params": {"name": c["tool"], "arguments": args}})
    stdin = "\n".join(json.dumps(r) for r in reqs) + "\n"
    env = dict(os.environ)
    env.pop("CODE_INTEL_DAEMON", None)  # teste direto, sem daemon (isolamento)
    proc = subprocess.run([BIN], input=stdin, capture_output=True, text=True,
                          env=env, timeout=int(os.environ.get("CODE_INTEL_E2E_TIMEOUT", "240")))
    out = {}
    for line in proc.stdout.splitlines():
        line = line.strip()
        if not line:
            continue
        try:
            msg = json.loads(line)
        except json.JSONDecodeError:
            continue
        if "id" not in msg or msg.get("id") is None:
            continue
        res = msg.get("result", {})
        text = ""
        try:
            text = res["content"][0]["text"]
        except (KeyError, IndexError, TypeError):
            out[msg["id"]] = {"__error__": "resposta sem content"}
            continue
        if text.startswith("ERRO:"):
            out[msg["id"]] = {"__error__": text}
        else:
            try:
                out[msg["id"]] = json.loads(text)
            except json.JSONDecodeError:
                out[msg["id"]] = {"__error__": f"texto não-JSON: {text[:120]}"}
    return out


def run_lang(block):
    lang = block["lang"]
    reqs_bins = [LSP_BIN[lang]] + EXTRA_BIN.get(lang, [])
    missing = [b for b in reqs_bins if not have(b)]
    if missing:
        print(f"\n=== {lang}: SKIP (LSP ausente: {', '.join(missing)})")
        return {"skip": 1}
    project = str(HERE / block["project"])
    print(f"\n=== {lang}: {project}")
    # setup (ex.: dart pub get, dotnet restore)
    for cmd in block.get("setup", []):
        print(f"  setup: {' '.join(cmd)}")
        r = subprocess.run(cmd, cwd=project, capture_output=True, text=True,
                           timeout=int(os.environ.get("CODE_INTEL_E2E_SETUP_TIMEOUT", "300")))
        if r.returncode != 0:
            print(f"  ✗ setup falhou: {r.stderr.strip()[:300]}")
            return {"fail": 1}
    cases = block["cases"]
    try:
        results = run_session(project, cases)
    except subprocess.TimeoutExpired:
        print(f"  ✗ TIMEOUT na sessão ({lang})")
        return {"fail": len(cases)}
    tally = {"pass": 0, "fail": 0, "xfail": 0, "xpass": 0}
    for i, c in enumerate(cases, start=1):
        res = results.get(i, {"__error__": "sem resposta"})
        xfail = c.get("xfail", False)
        if "__error__" in res and not any(a["op"] in ("present",) and a.get("path") == "__error__" for a in c["assert"]):
            ok, detail = False, res["__error__"]
        else:
            oks = [check_assert(a, res) for a in c["assert"]]
            ok = all(o for o, _ in oks)
            detail = "; ".join(d for _, d in oks)
        if ok and xfail:
            tally["xpass"] += 1
            print(f"  ⚠ XPASS {c['name']} (marcado xfail mas passou — remova o xfail?) [{detail}]")
        elif ok:
            tally["pass"] += 1
            print(f"  ✓ {c['name']} [{detail}]")
        elif xfail:
            tally["xfail"] += 1
            print(f"  ~ XFAIL {c['name']} (bug conhecido) [{detail}]")
        else:
            tally["fail"] += 1
            print(f"  ✗ {c['name']} [{detail}]")
    return tally


def main():
    argv = [a for a in sys.argv[1:] if not a.startswith("--")]
    real = "--real" in sys.argv
    if not Path(BIN).exists():
        print(f"!! binário não encontrado: {BIN}\n   build: cargo build --release --manifest-path mcp/Cargo.toml")
        sys.exit(2)
    cases_file = HERE / ("cases-real.json" if real else "cases.json")
    blocks = json.loads(cases_file.read_text())
    if real:
        repo = os.environ.get("CODE_INTEL_E2E_REPO")
        lang = os.environ.get("CODE_INTEL_E2E_LANG")
        if not repo or not lang:
            print("!! modo --real exige CODE_INTEL_E2E_REPO e CODE_INTEL_E2E_LANG")
            sys.exit(2)
        blocks = [b for b in blocks if b["lang"] == lang]
        for b in blocks:
            b["project"] = repo  # sobrescreve com o repo real (path absoluto)
    if argv:
        blocks = [b for b in blocks if b["lang"] in argv]
    totals = {"pass": 0, "fail": 0, "xfail": 0, "xpass": 0, "skip": 0}
    for b in blocks:
        if real:
            # no modo real o project já é absoluto; run_lang usa HERE/project — ajustamos
            project = b["project"]
            print(f"\n=== {b['lang']} (REAL): {project}")
            reqs_bins = [LSP_BIN[b['lang']]] + EXTRA_BIN.get(b['lang'], [])
            missing = [x for x in reqs_bins if not have(x)]
            if missing:
                print(f"  SKIP (LSP ausente: {', '.join(missing)})")
                totals["skip"] += 1
                continue
            try:
                results = run_session(project, b["cases"])
            except subprocess.TimeoutExpired:
                print("  ✗ TIMEOUT"); totals["fail"] += 1; continue
            for i, c in enumerate(b["cases"], start=1):
                res = results.get(i, {"__error__": "sem resposta"})
                oks = [check_assert(a, res) for a in c["assert"]] if "__error__" not in res else [(False, res["__error__"])]
                ok = all(o for o, _ in oks)
                totals["pass" if ok else "fail"] += 1
                print(("  ✓ " if ok else "  ✗ ") + c["name"] + " [" + "; ".join(d for _, d in oks) + "]")
        else:
            t = run_lang(b)
            for k in totals:
                totals[k] += t.get(k, 0)
    print(f"\n==== TOTAL: {totals['pass']} pass, {totals['fail']} fail, "
          f"{totals['xfail']} xfail, {totals['xpass']} xpass, {totals['skip']} skip ====")
    sys.exit(1 if totals["fail"] > 0 else 0)


if __name__ == "__main__":
    main()
