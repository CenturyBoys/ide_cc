# python-degraded (fixture)

pyrightconfig presente (include=["."]) MAS a dependência importada NÃO está instalada
(sem venv). basedpyright emite reportMissingImports → ambiente degradado.
Testa doctor.env_resolution.ok=false e type_check_reliable=false no simulate_edit,
SEM instalar o pacote (a ausência é o ponto).
