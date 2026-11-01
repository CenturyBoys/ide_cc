# Eval de refactoring behavior-preserving (Camada 2 do #16)

> A pergunta-mãe do POC: *o rename pela camada semântica chega à mudança **correta** com menos
> risco que o rename textual?* Aqui medimos **preservação de comportamento** num refactor real,
> usando o **build/análise da linguagem como oráculo** (à la RefactorPlatform, micro-escala,
> reproduzível) — o degrau acima do A/B de build-clean (`docs/AB-EXPERIMENT.md`) e da Camada 1
> (contagem de refs em repo real).

## Método

`benchmarks/scripts/eval_refactor.py <lang> <project> <file> <symbol> <new_name> [--src-dir DIR]`
no mesmo alvo mede os DOIS mecanismos:

- **Semântico** = `rename_symbol` do code-intel (via LSP), `apply=true`.
- **Textual** = `sed \bsymbol\b → new_name` **só no diretório de fonte** (`--src-dir`), que é o que
  um agente sem tools faria (não sabe atualizar imports/testes cross-file).

Oráculo = `validate_build` (build/análise real). O fixture é um clone git pinado → cada medição
reverte com `git checkout`/`clean` (medições isoladas, árvore limpa ao fim). A tese se confirma
quando o **semântico mantém o build limpo** (atualizou TODAS as refs, inclusive testes) e o
**textual quebra** (deixou os outros arquivos apontando para o nome antigo e/ou corrompeu literais).

## Resultado — repo REAL (reprodutível)

Fixture: `dart-lang/collection` (pinado em `fixtures/real/VERSIONS.txt`), oráculo `dart analyze`.

| lang | refactor | semântico (code-intel) | textual (sed no fonte) | veredito |
|---|---|---|---|---|
| dart | `QueueList`→`QueueListX` | `applied=true safe=true build_ok=true` (tocou **2 arquivos, incl. `test/`**) | `build_ok=false` (**20 erros**: `test/` aponta pro nome antigo) | **SEMÂNTICO preserva; TEXTUAL quebra** |

Reproduzir:
```bash
bash benchmarks/scripts/setup-real-fixtures.sh dart
DART_BIN=dart python3 benchmarks/scripts/eval_refactor.py \
  dart "$PWD/fixtures/real/dart" lib/src/queue_list.dart QueueList QueueListX --src-dir lib
```

## Leitura

O rename semântico atualizou `lib/` **e** `test/` num passo e o `dart analyze` ficou limpo — o
comportamento (a suíte ainda compila/resolve contra o símbolo renomeado) foi **preservado**. O
textual, editando só o fonte, deixou as 38 referências nos testes órfãs → **20 erros de análise**. É
o "bug silencioso" do `docs/AB-EXPERIMENT.md` (ali o oráculo era `tsc --noEmit` no fixture-armadilha
`ab-rename`), agora demonstrado **cross-file, incluindo os testes, num repo público de verdade**.

## Companheiro (Nível 1, TS) e follow-up

- **TS build-clean A/B** já existente: `docs/AB-EXPERIMENT.md` (`ab-rename`, `tsgo`/`vtsls` + `tsc`).
- O harness é genérico: roda para Python/Rust/C# apontando `--src-dir`/`lang` e o `validate_build`
  da linguagem (Rust/C# pedem daemon quente/CI pelo cold-start, como na Camada 1).
- **Follow-up (fora do escopo desta rodada):** rodar os benchmarks EXTERNOS de refactoring —
  **SWE-Refactor** (1.099 refactorings Java), **SWE-Bench ProMax** (170, 7 langs), **RefactorPlatform**
  — como oráculos em larga escala. São Java/JVM + container-pesados; exigem infra dedicada.
