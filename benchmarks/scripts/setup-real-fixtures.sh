#!/usr/bin/env bash
# Baixa OSS famoso (pinado) por linguagem para servir de fixture REAL (escala/correção) — Camada 1
# do #16. Clona em fixtures/real/<lang> (GITIGNORED), roda o setup que faz o LSP RESOLVER o projeto
# (npm/venv/pub get/cargo/dotnet restore) e grava os SHAs resolvidos em fixtures/real/VERSIONS.txt.
#
# Uso: bash benchmarks/scripts/setup-real-fixtures.sh [lang ...]   (sem args = todas)
# Best-effort: linguagem cujo toolchain falta é PULADA (não quebra). C# precisa de dotnet.
set -uo pipefail
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
FIX="$ROOT/fixtures/real"
mkdir -p "$FIX"
VLOCK="$FIX/VERSIONS.txt"
: > "$VLOCK"
WANT="${*:-typescript python rust dart csharp}"

have() { command -v "$1" >/dev/null 2>&1; }
want() { echo " $WANT " | grep -q " $1 "; }
record() { # record <lang> <dir>
  local sha; sha="$(git -C "$2" rev-parse HEAD 2>/dev/null || echo '?')"
  echo "$1  $(git -C "$2" describe --tags --always 2>/dev/null || echo '?')  $sha  $2" >> "$VLOCK"
}
# clone_pin <dir> <url> <tag>  — shallow no tag; se falhar, shallow do default
clone_pin() {
  local dir="$1" url="$2" tag="$3"
  [ -d "$dir/.git" ] && { echo ">> $dir já existe ($(git -C "$dir" describe --tags --always 2>/dev/null))"; return 0; }
  git clone --depth 1 --branch "$tag" "$url" "$dir" 2>/dev/null \
    || git clone --depth 1 "$url" "$dir" \
    || { echo "!! clone falhou: $url"; return 1; }
}

if want typescript && have git && have npm; then
  echo "== TypeScript: zod =="
  D="$FIX/typescript"
  if clone_pin "$D" https://github.com/colinhacks/zod.git v3.23.8; then
    ( cd "$D" && npm install --no-audit --no-fund >/dev/null 2>&1 ) || echo "  (npm install parcial — tsconfig já resolve zod)"
    record typescript "$D"
  fi
fi

if want python && have git && have python3; then
  echo "== Python: requests =="
  D="$FIX/python"
  if clone_pin "$D" https://github.com/psf/requests.git v2.32.3; then
    ( cd "$D" \
      && python3 -m venv .venv \
      && ./.venv/bin/python -m pip install --quiet --upgrade pip \
      && ./.venv/bin/python -m pip install --quiet -e . \
      && ./.venv/bin/python -m pip install --quiet chardet cryptography ) || echo "  (venv/pip parcial)"
      # chardet/cryptography são deps OPCIONAIS do requests (import em try/except); sem elas o
      # basedpyright flaga reportMissingImports e o doctor marca env_resolution degradado.
    cat > "$D/pyrightconfig.json" <<'JSON'
{ "include": ["src"], "venvPath": ".", "venv": ".venv", "typeCheckingMode": "basic" }
JSON
    record python "$D"
  fi
fi

if want rust && have git && have cargo; then
  echo "== Rust: ripgrep =="
  D="$FIX/rust"
  if clone_pin "$D" https://github.com/BurntSushi/ripgrep.git 14.1.1; then
    ( cd "$D" && cargo fetch >/dev/null 2>&1 ) || echo "  (cargo fetch parcial)"
    record rust "$D"
  fi
fi

if want dart && have git && have dart; then
  echo "== Dart: collection =="
  D="$FIX/dart"
  if clone_pin "$D" https://github.com/dart-lang/collection.git v1.19.1; then
    # acha o dir com pubspec.yaml (repo pode ser monorepo) e roda pub get nele
    PUB="$(find "$D" -maxdepth 2 -name pubspec.yaml -not -path '*/.*' | head -1)"
    [ -n "$PUB" ] && ( cd "$(dirname "$PUB")" && dart pub get >/dev/null 2>&1 ) || echo "  (pub get parcial)"
    record dart "$D"
  fi
fi

if want csharp && have git; then
  echo "== C#: Newtonsoft.Json =="
  D="$FIX/csharp"
  if clone_pin "$D" https://github.com/JamesNK/Newtonsoft.Json.git 13.0.3; then
    if have dotnet; then ( cd "$D" && dotnet restore --nologo -v q >/dev/null 2>&1 ) || echo "  (dotnet restore parcial)";
    else echo "  (dotnet AUSENTE — fixture clonada; o teste C# PULA local, roda no CI)"; fi
    record csharp "$D"
  fi
fi

echo; echo "== VERSIONS (reprodutibilidade) =="; cat "$VLOCK"
