# WORKFLOW.md — Git, Commits e PRs (repo IA-first)

Regras para manter a árvore git legível e o trabalho auditável — por humanos e por agentes.

## 1. Gitflow (modelo de branches)

```
main        ●──────────────●────────────────●         (produção; sempre estável, taggeada)
             \            / \              /
release       \          /   release/x.y /            (estabilização de release; só fixes)
               \        /               /
develop  ●──────●──────●───────●───────●──────●        (integração; base das features)
          \    /        \     /
feature    ●──●          ●───●                         (feature/<fase>-<slug>)  1 feature = 1 PR
```

| Branch | Origem | Merge para | Propósito |
|---|---|---|---|
| `main` | — | — | produção; cada merge é uma versão taggeada (`vX.Y.Z`) |
| `develop` | `main` | `main` (via release) | integração contínua; sempre buildável |
| `feature/<slug>` | `develop` | `develop` | uma unidade de trabalho (ex.: `feature/phase-4-python`) |
| `release/<x.y>` | `develop` | `main` + `develop` | congela features, só correções/documentação |
| `hotfix/<slug>` | `main` | `main` + `develop` | correção urgente em produção |

**Regras:**
- Nunca commitar direto em `main`. `develop` recebe merges de `feature/*` via PR.
- Nomes de feature ligados ao roadmap: `feature/phase-4-python`, `feature/move-extract`.
- Branch de vida curta: abra o PR cedo, faça merge e delete a branch.

## 2. Conventional Commits

Formato: `type(scope): subject` (imperativo, minúsculo, sem ponto final, ≤ 72 col).

| type | uso |
|---|---|
| `feat` | nova capacidade (ex.: `feat(mcp): add move_symbol via codeAction`) |
| `fix` | correção de bug (ex.: `fix(mcp): use pull diagnostics so net_delta detects errors`) |
| `docs` | documentação (README, ROADMAP, relatórios) |
| `bench` | benchmarks e seus resultados |
| `refactor` | mudança de código sem alterar comportamento |
| `test` | testes (ex.: `test-*.jsonl`) |
| `chore` | scaffolding, config, deps, gitignore |

Scopes comuns: `mcp`, `lsp`, `benchmarks`, `fixtures`, `docs`, `repo`.

**Corpo** (opcional): o *porquê* e o *impacto*, não o *como*. Referencie a fase/decisão.

**Trailer obrigatório** em todo commit (política do repo):
```
Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>
```

Exemplo:
```
feat(mcp): add apply→verify with net_delta guard

Simula a edição em memória (didChange), mede diagnostics antes/depois e só
persiste se net_delta<=0. Defesa contra rename/extract/move destrutivo.

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>
```

## 3. Pull Requests

- **Alvo:** `develop` (features) ou `main` (release/hotfix).
- **Título:** um Conventional Commit resumindo o PR.
- **Descrição** deve conter:
  - **O quê / Por quê** — objetivo e contexto (link para a fase no ROADMAP).
  - **Como verificar** — comandos reproduzíveis (script + versões) e resultado esperado.
  - **Docs atualizados** — quais README/RESULTS/ROADMAP mudaram.
  - **Limitações/pendências** — honestidade técnica.
- **Checks antes do merge:** `cargo build --release` limpo (zero warnings); testes `test-*.jsonl`
  passando; docs atualizados; fixtures restaurados (nenhuma mutação acidental commitada).
- **Merge:** squash para features pequenas (história limpa em `develop`); merge-commit para
  releases (preserva a topologia gitflow). Delete a branch após o merge.

## 4. Específico IA-first

- **Toda mudança de comportamento** vem com um teste reproduzível versionado e docs atualizados
  no MESMO PR — senão o PR está incompleto.
- **Resultados numéricos** citam script + versões (ver `benchmarks/README.md`).
- **Decisões arquiteturais** entram no `CLAUDE.md` (resumo) e no relatório/ROADMAP (detalhe), pra
  o próximo agente não re-derivar.
- **Trabalho por fase**: uma fase do ROADMAP ≈ uma `feature/phase-N-*` ≈ um PR.
