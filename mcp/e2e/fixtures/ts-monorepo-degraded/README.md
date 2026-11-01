# ts-monorepo-degraded (fixture)

Monorepo pnpm SEM tsconfig.json na raiz; o pacote usa `process` sem @types/node.
Reproduz o ambiente TS degradado: tsgo cai em inferred-project e flaga `process`
(TS2580). Usado para testar que doctor.env_resolution.ok=false e que as edit-tools
devolvem type_check_reliable=false (erros de config excluídos do net_delta).
NÃO instale node_modules aqui — a ausência de @types/node É o ponto.
