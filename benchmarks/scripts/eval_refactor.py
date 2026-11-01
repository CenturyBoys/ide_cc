#!/usr/bin/env python3
"""
Camada 2 do #16 — eval de REFACTORING behavior-preserving (semântico vs textual), oráculo = build/
check real da linguagem (à la RefactorPlatform, micro-escala, reproduzível).

A tese do POC: o rename SEMÂNTICO (code-intel, via LSP) atualiza TODAS as referências cross-file —
inclusive nos testes — então o build/análise continua limpo (comportamento preservado). O rename
TEXTUAL (sed no código-fonte, o que um agente sem tools faria) NÃO atualiza os outros arquivos
(testes/imports) e/ou corrompe strings/comentários → o build QUEBRA. Medimos os dois no MESMO alvo.

Uso:
  python3 benchmarks/scripts/eval_refactor.py <lang> <project_abs> <file_rel> <symbol> <new_name> \
     [--src-dir lib] [--out benchmarks/results/REFACTOR-EVAL.md]
Requer: git no project (p/ revert), o binário code-intel (CODE_INTEL_BIN ou target/release) e o LSP.
"""
import json, os, subprocess, sys, shutil
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
BIN = os.environ.get("CODE_INTEL_BIN", str(ROOT / "mcp" / "target" / "release" / "code-intel-mcp"))


def mcp(project, calls):
    reqs = [{"jsonrpc": "2.0", "id": 0, "method": "initialize", "params": {}}]
    for i, (name, args) in enumerate(calls, 1):
        a = dict(args); a["project"] = project
        reqs.append({"jsonrpc": "2.0", "id": i, "method": "tools/call",
                     "params": {"name": name, "arguments": a}})
    stdin = "\n".join(json.dumps(r) for r in reqs) + "\n"
    env = dict(os.environ); env.pop("CODE_INTEL_DAEMON", None)
    p = subprocess.run([BIN], input=stdin, capture_output=True, text=True, env=env,
                       timeout=int(os.environ.get("CODE_INTEL_WARMUP_MS", "90000")) // 1000 + 60)
    out = {}
    for ln in p.stdout.splitlines():
        ln = ln.strip()
        if not ln:
            continue
        try:
            m = json.loads(ln)
        except Exception:
            continue
        if m.get("id"):
            out[m["id"]] = m.get("result", {}).get("structuredContent")
    return out


def git(project, *args):
    return subprocess.run(["git", "-C", project, *args], capture_output=True, text=True)


def revert(project):
    git(project, "checkout", "--", ".")
    git(project, "clean", "-fdq")


def build_ok(project, lang):
    r = mcp(project, [("validate_build", {"lang": lang})])
    v = r.get(1) or {}
    return bool(v.get("build_ok")), v.get("errors", [])


def changed_files(project):
    r = git(project, "diff", "--name-only")
    return [f for f in r.stdout.splitlines() if f.strip()]


def main():
    if len(sys.argv) < 6:
        print(__doc__); sys.exit(2)
    lang, project, file, symbol, new_name = sys.argv[1:6]
    src_dir = "lib"
    out = str(ROOT / "benchmarks" / "results" / "REFACTOR-EVAL.md")
    rest = sys.argv[6:]
    for i, a in enumerate(rest):
        if a == "--src-dir":
            src_dir = rest[i + 1]
        if a == "--out":
            out = rest[i + 1]
    project = str(Path(project).resolve())

    print(f"== eval-refactor {lang}: rename {symbol} -> {new_name} em {project} ==")
    revert(project)
    base_ok, _ = build_ok(project, lang)
    print(f"baseline build_ok={base_ok}")
    if not base_ok:
        print("!! baseline não compila — abortando (não dá pra medir preservação)"); sys.exit(1)

    # (A) SEMÂNTICO via code-intel (apply=true)
    r = mcp(project, [("rename_symbol", {"file": file, "symbol": symbol, "new_name": new_name, "apply": True})])
    sem = r.get(1) or {}
    sem_applied = bool(sem.get("applied")); sem_safe = sem.get("safe")
    sem_ok, sem_errs = build_ok(project, lang)
    sem_changed = changed_files(project)
    sem_touched_tests = any(("test" in f.lower()) for f in sem_changed)
    revert(project)
    print(f"SEMÂNTICO: applied={sem_applied} net_delta.safe={sem_safe} build_ok={sem_ok} "
          f"arquivos={len(sem_changed)} tocou_testes={sem_touched_tests}")

    # (B) TEXTUAL (sed no src_dir apenas — o que um agente sem tools faria)
    sed_n = 0
    for p in Path(project, src_dir).rglob("*"):
        if p.is_file() and p.suffix in (".dart", ".py", ".ts", ".rs", ".cs"):
            txt = p.read_text(errors="ignore")
            import re
            new = re.sub(rf"\b{re.escape(symbol)}\b", new_name, txt)
            if new != txt:
                p.write_text(new); sed_n += 1
    txt_ok, txt_errs = build_ok(project, lang)
    txt_changed = changed_files(project)
    revert(project)
    print(f"TEXTUAL (sed em {src_dir}/): arquivos_editados={sed_n} build_ok={txt_ok} "
          f"erros={len(txt_errs)}")

    verdict = "SEMÂNTICO preserva (build limpo, atualiza cross-file/testes); TEXTUAL quebra" \
        if (sem_ok and not txt_ok) else \
        ("ambos limpos — alvo sem armadilha cross-file (escolha outro símbolo usado em testes)"
         if (sem_ok and txt_ok) else "inconclusivo")

    row = (f"| {lang} | {symbol}→{new_name} | semântico: applied={sem_applied} safe={sem_safe} "
           f"build_ok={sem_ok} testes={sem_touched_tests} | textual: build_ok={txt_ok} "
           f"erros={len(txt_errs)} | **{verdict}** |")
    print("\nRESULT ROW:\n" + row)
    Path(out).parent.mkdir(parents=True, exist_ok=True)
    with open(out, "a") as f:
        f.write(row + "\n")
    print(f"\nanexado em {out}")
    sys.exit(0 if sem_ok and not txt_ok else 1)


if __name__ == "__main__":
    main()
