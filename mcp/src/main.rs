// code-intel-mcp — POC "IDE na mão da LLM".
// Servidor MCP (stdio, JSON-RPC por linha) que expõe operações semânticas rápidas sobre o
// tsgo, com a defesa central: um GATE DE WARMUP que nunca devolve uma contagem de referências
// parcial durante a indexação (o modo de falha #76870, medido em benchmarks/results/RESULTS.md).
mod lsp;
use lsp::{path_to_uri, uri_to_path, LspClient};
use serde_json::{json, Value};
use std::collections::{BTreeSet, HashMap};
use std::io::{BufRead, Write};
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

// Roteamento por LINGUAGEM × OPERAÇÃO:
// - TypeScript: tsgo (nav/rename, rápido e correto) + vtsls (refactorings — tsgo não os tem).
// - Python: basedpyright para tudo.
// A camada é agnóstica: adicionar linguagem = adicionar um backend + um match aqui.
fn nav_backend(file: &str) -> &'static str {
    if file.ends_with(".py") {
        "basedpyright"
    } else if file.ends_with(".dart") {
        "dart"
    } else if file.ends_with(".rs") {
        "rust-analyzer"
    } else if file.ends_with(".cs") {
        "csharp-ls"
    } else {
        "tsgo"
    }
}
fn refactor_backend(file: &str) -> &'static str {
    if file.ends_with(".py") {
        "basedpyright"
    } else if file.ends_with(".dart") {
        "dart"
    } else if file.ends_with(".rs") {
        "rust-analyzer"
    } else if file.ends_with(".cs") {
        "csharp-ls"
    } else {
        "vtsls"
    }
}

struct Server {
    clients: Mutex<HashMap<String, Arc<LspClient>>>, // chave: "project\0backend"
    tsgo_bin: String,
    vtsls_bin: String,
    basedpyright_bin: String,
    dart_bin: String,
    rust_analyzer_bin: String,
    csharp_ls_bin: String,
}

impl Server {
    fn client(&self, project: &str, backend: &str) -> Result<Arc<LspClient>, String> {
        let key = format!("{project}\u{0}{backend}");
        let mut map = self.clients.lock().unwrap();
        if let Some(c) = map.get(&key) {
            return Ok(c.clone());
        }
        let (cmd, args): (&str, Vec<&str>) = match backend {
            "vtsls" => (&self.vtsls_bin, vec!["--stdio"]),
            "basedpyright" => (&self.basedpyright_bin, vec!["--stdio"]),
            "dart" => (&self.dart_bin, vec!["language-server"]),
            "rust-analyzer" => (&self.rust_analyzer_bin, vec![]),
            "csharp-ls" => (&self.csharp_ls_bin, vec![]),
            _ => (&self.tsgo_bin, vec!["--lsp", "-stdio"]),
        };
        let c = LspClient::start(cmd, &args, project)?;
        map.insert(key, c.clone());
        Ok(c)
    }
}

// Localiza a posição (LSP 0-indexed) do símbolo no arquivo. `line` opcional é 1-indexed (humano).
fn locate(abs_file: &str, symbol: &str, line: Option<u64>) -> Result<(u64, u64), String> {
    let text = std::fs::read_to_string(abs_file).map_err(|e| format!("ler {abs_file}: {e}"))?;
    let lines: Vec<&str> = text.split('\n').collect();
    if let Some(l) = line {
        let idx = (l as usize).saturating_sub(1);
        if let Some(row) = lines.get(idx) {
            if let Some(c) = row.find(symbol) {
                return Ok((idx as u64, c as u64));
            }
        }
        return Err(format!("símbolo '{symbol}' não achado na linha {l}"));
    }
    for (i, row) in lines.iter().enumerate() {
        if let Some(c) = row.find(symbol) {
            return Ok((i as u64, c as u64));
        }
    }
    Err(format!("símbolo '{symbol}' não achado em {abs_file}"))
}

// Varre `lines` a partir de `start` (0-based) até `max` linhas à frente procurando o token
// `symbol`. Pula decorators (@...) e trivia à esquerda que alguns language servers (basedpyright)
// incluem no range de símbolos decorados — reportando a posição no `@dataclass`/`@classmethod` em
// vez do identificador. Retorna (linha0, col0) do identificador.
fn scan_ident(lines: &[&str], symbol: &str, start: usize, max: usize) -> Option<(u64, u64)> {
    let end = (start + max).min(lines.len());
    for (off, row) in lines.get(start..end)?.iter().enumerate() {
        if let Some(c) = row.find(symbol) {
            return Some(((start + off) as u64, c as u64));
        }
    }
    None
}

// Igual a scan_ident, mas lendo o arquivo do disco. Usado na resolução implícita de posição.
fn locate_ident_from(
    abs_file: &str,
    symbol: &str,
    start_line: u64,
    max: usize,
) -> Option<(u64, u64)> {
    let text = std::fs::read_to_string(abs_file).ok()?;
    let lines: Vec<&str> = text.split('\n').collect();
    scan_ident(&lines, symbol, start_line as usize, max)
}

// Janela de varredura à frente (cobre decorators empilhados) ao refinar posição p/ o identificador.
const IDENT_SCAN_LINES: usize = 16;

// Refina (l,c) de um símbolo achatado para o token do identificador (pula decorators). Fallback (l,c).
fn refine_at(lines: &[&str], name_path: &str, l: u64, c: u64) -> (u64, u64) {
    let ident = base_name(name_path.rsplit('/').next().unwrap_or(name_path));
    scan_ident(lines, ident, l as usize, IDENT_SCAN_LINES).unwrap_or((l, c))
}

fn rel(root: &str, uri: &str) -> String {
    let p = uri_to_path(uri);
    p.strip_prefix(root)
        .map(|s| s.trim_start_matches('/').to_string())
        .unwrap_or(p)
}

// GATE DE WARMUP: repete find_references até a contagem estabilizar (N iguais seguidas).
// Retorna (locations, stable, warmup_ms, polls). `stable=false` => resultado NÃO confiável.
fn warmup_references(
    client: &LspClient,
    uri: &str,
    line: u64,
    ch: u64,
) -> Result<(Vec<Value>, bool, u128, u32), String> {
    let start = Instant::now();
    let mut last: i64 = -1;
    let mut stable_hits = 0u32;
    let mut polls = 0u32;
    let mut refs: Vec<Value> = vec![];
    // 60s: rust-analyzer roda cargo metadata + check no cold start (~30s no fixture medido).
    while start.elapsed().as_millis() < 60_000 {
        polls += 1;
        // rust-analyzer LANÇA erro ('No references found at position') enquanto indexa;
        // tratamos como "ainda não pronto" e re-tentamos, em vez de propagar.
        let res = match client.request(
            "textDocument/references",
            json!({"textDocument":{"uri":uri},"position":{"line":line,"character":ch},
                   "context":{"includeDeclaration":true}}),
            15_000,
        ) {
            Ok(r) => r,
            Err(_) => {
                std::thread::sleep(Duration::from_millis(300));
                continue;
            }
        };
        refs = res.as_array().cloned().unwrap_or_default();
        let count = refs.len() as i64;
        if count == last && count > 0 {
            stable_hits += 1;
            if stable_hits >= 3 {
                return Ok((refs, true, start.elapsed().as_millis(), polls));
            }
        } else {
            stable_hits = 0;
        }
        last = count;
        std::thread::sleep(std::time::Duration::from_millis(250));
    }
    Ok((refs, false, start.elapsed().as_millis(), polls))
}

// ---- resolução SEMÂNTICA de símbolo via textDocument/documentSymbol ----

fn kind_name(k: u64) -> &'static str {
    match k {
        1 => "File",
        2 => "Module",
        3 => "Namespace",
        4 => "Package",
        5 => "Class",
        6 => "Method",
        7 => "Property",
        8 => "Field",
        9 => "Constructor",
        10 => "Enum",
        11 => "Interface",
        12 => "Function",
        13 => "Variable",
        14 => "Constant",
        23 => "Struct",
        26 => "TypeParameter",
        _ => "Symbol",
    }
}

fn sym_pos(s: &Value) -> (u64, u64) {
    let r = if s.get("selectionRange").is_some() {
        &s["selectionRange"]
    } else if s.get("range").is_some() {
        &s["range"]
    } else {
        &s["location"]["range"]
    };
    (
        r["start"]["line"].as_u64().unwrap_or(0),
        r["start"]["character"].as_u64().unwrap_or(0),
    )
}

// achata a árvore de documentSymbol em (name_path, kind, line, char)
fn flatten_symbols(symbols: &[Value], prefix: &str, out: &mut Vec<(String, u64, u64, u64)>) {
    for s in symbols {
        let name = s["name"].as_str().unwrap_or("");
        let fp = if prefix.is_empty() {
            name.to_string()
        } else {
            format!("{prefix}/{name}")
        };
        let (l, c) = sym_pos(s);
        let kind = s["kind"].as_u64().unwrap_or(0);
        out.push((fp.clone(), kind, l, c));
        if let Some(children) = s["children"].as_array() {
            flatten_symbols(children, &fp, out);
        }
    }
}

// Alguns language servers (notadamente csharp-ls) anexam a assinatura ao nome do método no
// documentSymbol (ex.: "HandleAsync(string x, int y)"). Para casar por name_path, comparamos o
// nome "base" (antes do '('). Idempotente para nomes sem assinatura (TS/Rust/etc.).
fn base_name(seg: &str) -> &str {
    match seg.find('(') {
        Some(i) => seg[..i].trim_end(),
        None => seg,
    }
}

// Normaliza um name_path inteiro removendo a assinatura de cada segmento.
fn strip_sigs(path: &str) -> String {
    path.split('/').map(base_name).collect::<Vec<_>>().join("/")
}

// Casa um name_path achatado `fp` (possivelmente com assinatura de método, ex.: csharp-ls)
// contra a `query` do usuário (sem assinatura). Normaliza `fp` antes de comparar (então métodos
// C# resolvem). Critérios: igualdade exata, sufixo "/query" ou — SÓ quando a query não qualifica
// a classe (sem '/') — último segmento igual. Assim uma query composta "A/foo" não casa "B/foo".
fn name_path_matches(fp: &str, query: &str) -> bool {
    let nfp = strip_sigs(fp);
    if nfp == query || nfp.ends_with(&format!("/{query}")) {
        return true;
    }
    !query.contains('/') && nfp.rsplit('/').next() == Some(base_name(query))
}

fn document_symbols(client: &LspClient, abs: &str) -> Result<Vec<Value>, String> {
    client.ensure_open(abs)?;
    let res = client.request(
        "textDocument/documentSymbol",
        json!({"textDocument":{"uri":path_to_uri(abs)}}),
        10_000,
    )?;
    Ok(res.as_array().cloned().unwrap_or_default())
}

// Resolve a posição do símbolo SEMANTICAMENTE (documentSymbol); fallback textual (locate).
// `name_path` aceita "Classe/metodo" além de "Simbolo".
fn resolve_pos(
    client: &LspClient,
    abs: &str,
    name_path: &str,
    line: Option<u64>,
) -> Result<(u64, u64), String> {
    if line.is_none() {
        if let Ok(syms) = document_symbols(client, abs) {
            let mut flat = vec![];
            flatten_symbols(&syms, "", &mut flat);
            let last = base_name(name_path.rsplit('/').next().unwrap_or(name_path));
            let suffix = format!("/{name_path}");
            // Casa contra o name_path normalizado (sem assinatura), cobrindo métodos do csharp-ls.
            let hit = flat
                .iter()
                .find(|(fp, ..)| strip_sigs(fp) == name_path)
                .or_else(|| {
                    flat.iter()
                        .find(|(fp, ..)| strip_sigs(fp).ends_with(&suffix))
                })
                .or_else(|| {
                    // fallback por último segmento só quando a query não qualifica a classe
                    if name_path.contains('/') {
                        None
                    } else {
                        flat.iter()
                            .find(|(fp, ..)| strip_sigs(fp).rsplit('/').next() == Some(last))
                    }
                });
            if let Some((_, _, l, c)) = hit {
                // O documentSymbol às vezes aponta pro início da declaração — em símbolos DECORADOS
                // (basedpyright) isso é a linha do @decorator, não o identificador. Refina varrendo
                // pra frente até o token do nome (pula decorators); senão find_references pergunta
                // em cima do '@' e retorna 0 refs em silêncio (P1).
                let last = base_name(name_path.rsplit('/').next().unwrap_or(name_path));
                if let Some((rl, rc)) = locate_ident_from(abs, last, *l, IDENT_SCAN_LINES) {
                    return Ok((rl, rc));
                }
                return Ok((*l, *c));
            }
        }
    }
    let sym = name_path.rsplit('/').next().unwrap_or(name_path);
    locate(abs, sym, line)
}

// ---- helpers de edição (apply → verify) --------------------------------

fn pos_to_offset(text: &str, line: u64, ch: u64) -> usize {
    let mut off = 0usize;
    for (i, l) in text.split_inclusive('\n').enumerate() {
        if i as u64 == line {
            let bidx = l
                .char_indices()
                .nth(ch as usize)
                .map(|(b, _)| b)
                .unwrap_or(l.len());
            return off + bidx;
        }
        off += l.len();
    }
    off
}

fn apply_text_edits(text: &str, edits: &[Value]) -> String {
    let mut spans: Vec<(usize, usize, String)> = edits
        .iter()
        .map(|e| {
            let s = pos_to_offset(
                text,
                e["range"]["start"]["line"].as_u64().unwrap_or(0),
                e["range"]["start"]["character"].as_u64().unwrap_or(0),
            );
            let en = pos_to_offset(
                text,
                e["range"]["end"]["line"].as_u64().unwrap_or(0),
                e["range"]["end"]["character"].as_u64().unwrap_or(0),
            );
            (s, en, e["newText"].as_str().unwrap_or("").to_string())
        })
        .collect();
    spans.sort_by(|a, b| b.0.cmp(&a.0)); // do fim pro começo
    let mut out = text.to_string();
    for (s, en, nt) in spans {
        if s <= en && en <= out.len() {
            out.replace_range(s..en, &nt);
        }
    }
    out
}

fn edits_by_file(edit: &Value) -> HashMap<String, Vec<Value>> {
    let mut map: HashMap<String, Vec<Value>> = HashMap::new();
    if let Some(changes) = edit.get("changes").and_then(|c| c.as_object()) {
        for (uri, arr) in changes {
            map.entry(uri_to_path(uri))
                .or_default()
                .extend(arr.as_array().cloned().unwrap_or_default());
        }
    }
    if let Some(dc) = edit.get("documentChanges").and_then(|c| c.as_array()) {
        for change in dc {
            if let Some(arr) = change.get("edits").and_then(|e| e.as_array()) {
                let uri = change["textDocument"]["uri"].as_str().unwrap_or("");
                map.entry(uri_to_path(uri))
                    .or_default()
                    .extend(arr.iter().cloned());
            }
        }
    }
    map
}

fn err_key(file: &str, diag: &Value) -> Option<String> {
    if diag["severity"].as_u64().unwrap_or(1) == 1 {
        let line = diag["range"]["start"]["line"].as_u64().unwrap_or(0);
        let msg = diag["message"].as_str().unwrap_or("");
        Some(format!("{file}|{line}|{msg}"))
    } else {
        None
    }
}

// Conjunto de ERROS nos arquivos. HÍBRIDO: usa PULL (determinístico, tsgo) se suportado;
// senão PUSH (publishDiagnostics, vtsls/pyright) com espera de estabilização após min_gen.
fn collect_errors(client: &LspClient, files: &[String], min_gen: u64) -> BTreeSet<String> {
    let mut s = BTreeSet::new();
    if client.supports_pull() {
        for f in files {
            for d in client.pull_diagnostics(f).unwrap_or_default() {
                if let Some(k) = err_key(f, &d) {
                    s.insert(k);
                }
            }
        }
        return s;
    }
    // PUSH: espera um publish após min_gen, depois a estabilidade do conjunto de erros.
    // NOTA: captura diagnostics NATIVOS do server (que veem o didChange em memória). Erros que só
    // aparecem via build externo (ex.: rust-analyzer/`cargo check`, que lê o DISCO) NÃO são vistos
    // na simulação em memória — para esses, a Fase de validação (run build/test) é necessária.
    let start = Instant::now();
    while client.diag_gen() <= min_gen && start.elapsed().as_millis() < 3000 {
        std::thread::sleep(Duration::from_millis(80));
    }
    let read = |c: &LspClient| {
        let mut ss = BTreeSet::new();
        for f in files {
            for d in c.pushed_diagnostics(f) {
                if let Some(k) = err_key(f, &d) {
                    ss.insert(k);
                }
            }
        }
        ss
    };
    let mut prev = read(client);
    loop {
        std::thread::sleep(Duration::from_millis(350));
        let cur = read(client);
        if cur == prev || start.elapsed().as_millis() > 8000 {
            return cur;
        }
        prev = cur;
    }
}

// resource ops de criação de arquivo no WorkspaceEdit (ex.: 'move to new file')
fn creates_from(edit: &Value) -> Vec<String> {
    let mut v = vec![];
    if let Some(dc) = edit.get("documentChanges").and_then(|c| c.as_array()) {
        for change in dc {
            if change.get("kind").and_then(|k| k.as_str()) == Some("create") {
                if let Some(uri) = change.get("uri").and_then(|u| u.as_str()) {
                    let p = uri_to_path(uri);
                    if !Path::new(&p).exists() {
                        v.push(p);
                    }
                }
            }
        }
    }
    v
}

// NÚCLEO da segurança: dado um WorkspaceEdit, simula EM MEMÓRIA, mede net_delta e aplica/reverte.
// Compartilhado por rename, extract_function e move_symbol. Suporta arquivos novos (CreateFile).
// ---- validação de BUILD (Fase 5) ---------------------------------------
// Fecha o buraco do net_delta em memória: erros que só o build externo pega (ex.: rust-analyzer
// não vê `cargo check`, que lê o disco). Roda o checker da linguagem NO DISCO após aplicar.

fn build_lang(file: &str) -> &'static str {
    if file.ends_with(".rs") {
        "rust"
    } else if file.ends_with(".dart") {
        "dart"
    } else if file.ends_with(".cs") {
        "csharp"
    } else if file.ends_with(".py") {
        "python"
    } else {
        "typescript"
    }
}

// comando de check por linguagem (override por env <LANG>_CHECK_CMD, ex.: RUST_CHECK_CMD)
fn build_cmd(lang: &str) -> Option<(String, Vec<String>)> {
    let (env_key, default): (&str, Option<(&str, Vec<&str>)>) = match lang {
        "rust" => (
            "RUST_CHECK_CMD",
            Some(("cargo", vec!["check", "--quiet", "--message-format=short"])),
        ),
        "dart" => ("DART_CHECK_CMD", Some(("dart", vec!["analyze"]))),
        "csharp" => (
            "CSHARP_CHECK_CMD",
            Some(("dotnet", vec!["build", "--nologo", "-v", "q"])),
        ),
        "typescript" => ("TS_CHECK_CMD", None),
        "python" => ("PY_CHECK_CMD", None),
        _ => ("", None),
    };
    if let Ok(s) = std::env::var(env_key) {
        let parts: Vec<String> = s.split_whitespace().map(|x| x.to_string()).collect();
        if !parts.is_empty() {
            return Some((parts[0].clone(), parts[1..].to_vec()));
        }
    }
    default.map(|(c, a)| {
        (
            c.to_string(),
            a.into_iter().map(|x| x.to_string()).collect(),
        )
    })
}

// roda o checker no diretório do projeto; retorna (ok, amostra de linhas de erro)
fn build_check(project: &str, lang: &str) -> Result<(bool, Vec<String>), String> {
    let (cmd, args) = build_cmd(lang).ok_or_else(|| {
        format!(
            "sem comando de build p/ '{lang}' (defina {}_CHECK_CMD)",
            lang.to_uppercase()
        )
    })?;
    let out = std::process::Command::new(&cmd)
        .args(&args)
        .current_dir(project)
        .output()
        .map_err(|e| format!("falha ao rodar '{cmd}': {e}"))?;
    let combined = format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    let errors: Vec<String> = combined
        .lines()
        .filter(|l| l.to_lowercase().contains("error"))
        .take(20)
        .map(|l| l.trim().to_string())
        .collect();
    Ok((out.status.success() && errors.is_empty(), errors))
}

fn verify_and_apply(
    client: &LspClient,
    edit: &Value,
    apply: bool,
    verify_build: bool,
    project: &str,
    lang: &str,
) -> Result<Value, String> {
    let by_file = edits_by_file(edit);
    let creates = creates_from(edit);
    let mut affected: Vec<String> = by_file.keys().cloned().collect();
    for c in &creates {
        if !affected.contains(c) {
            affected.push(c.clone());
        }
    }
    if affected.is_empty() {
        return Ok(json!({"applied": false, "mode": "noop", "detail": "sem edições"}));
    }
    let (files_n, edits_n, per_file) = summarize_edit(edit, client.root());

    // textos original (vazio p/ arquivos novos) e novo
    let mut originals: HashMap<String, String> = HashMap::new();
    let mut news: HashMap<String, String> = HashMap::new();
    for f in &affected {
        let orig = std::fs::read_to_string(f).unwrap_or_default();
        let newt = match by_file.get(f) {
            Some(es) if !es.is_empty() => apply_text_edits(&orig, es),
            _ => orig.clone(),
        };
        originals.insert(f.clone(), orig);
        news.insert(f.clone(), newt);
    }
    let existing: Vec<String> = affected
        .iter()
        .filter(|f| Path::new(f).exists())
        .cloned()
        .collect();

    // diagnostics ANTES — força o server ao baseline do DISCO (did_change com o texto original),
    // evitando estado stale de uma operação anterior no mesmo client (ex.: preview não revertido a tempo).
    let g0 = client.diag_gen();
    for f in &existing {
        client.ensure_open(f)?;
        client.did_change(f, &originals[f]);
    }
    let before = collect_errors(client, &existing, g0);

    // SIMULA em memória: existentes via didChange; novos via didOpen com o texto novo
    let g1 = client.diag_gen();
    for f in &affected {
        let nt = &news[f];
        if Path::new(f).exists() {
            client.did_change(f, nt);
        } else {
            client.open_with_text(f, nt);
        }
    }
    let after = collect_errors(client, &affected, g1);

    let introduced: Vec<String> = after.difference(&before).cloned().collect();
    let resolved: Vec<String> = before.difference(&after).cloned().collect();
    let net_delta = introduced.len() as i64 - resolved.len() as i64;
    let safe = net_delta <= 0;

    let creates_set: std::collections::HashSet<String> = creates.iter().cloned().collect();
    let mut applied;
    let mut note;
    let mut build_ok = Value::Null;
    let mut build_errors: Vec<String> = vec![];
    if apply && safe {
        for (f, nt) in &news {
            if let Some(parent) = Path::new(f).parent() {
                std::fs::create_dir_all(parent).ok();
            }
            std::fs::write(f, nt).map_err(|e| format!("escrever {f}: {e}"))?;
        }
        applied = true;
        note = "aplicado no disco (net_delta<=0)";

        // Fase 5: validação de BUILD no disco (pega erros que a simulação em memória não vê).
        if verify_build {
            match build_check(project, lang) {
                Ok((ok, errs)) => {
                    build_ok = json!(ok);
                    build_errors = errs;
                    if !ok {
                        // build quebrou -> REVERTE o disco (restaura existentes, remove criados)
                        for f in &affected {
                            if creates_set.contains(f) {
                                let _ = std::fs::remove_file(f);
                            } else {
                                let _ = std::fs::write(f, &originals[f]);
                            }
                            if Path::new(f).exists() {
                                client.did_change(f, &originals[f]);
                            } else {
                                client.close(f);
                            }
                        }
                        applied = false;
                        note = "REVERTIDO: net_delta passou mas o build falhou (ex.: erro que só o cargo check vê)";
                    }
                }
                Err(e) => {
                    build_ok = json!(null);
                    build_errors = vec![format!("build-check indisponível: {e}")];
                }
            }
        }
    } else {
        // reverte o estado do server (não escreve disco): existentes voltam; novos fecham
        for f in &affected {
            if Path::new(f).exists() {
                client.did_change(f, &originals[f]);
            } else {
                client.close(f);
            }
        }
        applied = false;
        note = if apply {
            "NÃO aplicado: introduziria erros (net_delta>0); revertido"
        } else {
            "preview: medido em memória e revertido (use apply=true para persistir se seguro)"
        };
    }

    let root = client.root().to_string();
    Ok(json!({
        "applied": applied,
        "safe": safe,
        "net_delta": net_delta,
        "errors_introduced": introduced.iter().map(|k| rel_diag(&root, k)).collect::<Vec<_>>(),
        "errors_resolved": resolved.iter().map(|k| rel_diag(&root, k)).collect::<Vec<_>>(),
        "blast_radius": {"files": files_n, "edits": edits_n},
        "creates": creates.iter().map(|c| rel(&root, &path_to_uri(c))).collect::<Vec<_>>(),
        "changes": per_file,
        "build_ok": build_ok,
        "build_errors": build_errors,
        "note": note,
    }))
}

fn tool_find_references(srv: &Server, a: &Value) -> Result<Value, String> {
    let project = a["project"]
        .as_str()
        .ok_or("faltou 'project' (caminho absoluto)")?;
    let file = a["file"]
        .as_str()
        .ok_or("faltou 'file' (relativo ao project)")?;
    let symbol = a["symbol"].as_str().ok_or("faltou 'symbol'")?;
    let line = a["line"].as_u64();
    let client = srv.client(project, nav_backend(file))?;
    let abs = format!("{}/{}", project.trim_end_matches('/'), file);
    client.ensure_open(&abs)?;
    let (l, c) = resolve_pos(&client, &abs, symbol, line)?;
    let uri = path_to_uri(&abs);
    let (refs, stable, warmup_ms, polls) = warmup_references(&client, &uri, l, c)?;
    let mut locs: Vec<String> = refs
        .iter()
        .map(|r| {
            let u = r["uri"].as_str().unwrap_or("");
            let sl = r["range"]["start"]["line"].as_u64().unwrap_or(0) + 1;
            let sc = r["range"]["start"]["character"].as_u64().unwrap_or(0) + 1;
            format!("{}:{}:{}", rel(client.root(), u), sl, sc)
        })
        .collect();
    locs.sort();
    Ok(json!({
        "symbol": symbol,
        "count": refs.len(),
        "stable": stable,
        "warning": if stable { Value::Null } else { json!("index_not_ready: contagem AINDA mudando; NÃO use para rename/delete") },
        "warmup_ms": warmup_ms,
        "polls": polls,
        "references": locs,
    }))
}

fn summarize_edit(edit: &Value, root: &str) -> (u64, u64, Vec<Value>) {
    let mut files = 0u64;
    let mut edits = 0u64;
    let mut per_file = vec![];
    if let Some(changes) = edit.get("changes").and_then(|c| c.as_object()) {
        for (uri, arr) in changes {
            let n = arr.as_array().map(|a| a.len()).unwrap_or(0) as u64;
            files += 1;
            edits += n;
            per_file.push(json!({"file": rel(root, uri), "edits": n}));
        }
    }
    if let Some(dc) = edit.get("documentChanges").and_then(|c| c.as_array()) {
        for change in dc {
            if let Some(arr) = change.get("edits").and_then(|e| e.as_array()) {
                let uri = change["textDocument"]["uri"].as_str().unwrap_or("");
                files += 1;
                edits += arr.len() as u64;
                per_file.push(json!({"file": rel(root, uri), "edits": arr.len()}));
            }
        }
    }
    (files, edits, per_file)
}

fn rel_diag(root: &str, key: &str) -> String {
    let mut it = key.splitn(3, '|');
    let u = it.next().unwrap_or("");
    let line = it.next().unwrap_or("");
    let msg = it.next().unwrap_or("");
    format!("{}:{}  {}", rel(root, u), line, msg)
}

fn tool_rename_symbol(srv: &Server, a: &Value) -> Result<Value, String> {
    let project = a["project"].as_str().ok_or("faltou 'project'")?;
    let file = a["file"].as_str().ok_or("faltou 'file'")?;
    let symbol = a["symbol"].as_str().ok_or("faltou 'symbol'")?;
    let new_name = a["new_name"].as_str().ok_or("faltou 'new_name'")?;
    let line = a["line"].as_u64();
    let apply = a["apply"].as_bool().unwrap_or(false); // false = preview (mede e reverte)
    let client = srv.client(project, nav_backend(file))?;
    let abs = format!("{}/{}", project.trim_end_matches('/'), file);
    client.ensure_open(&abs)?;
    let (l, c) = resolve_pos(&client, &abs, symbol, line)?;
    let uri = path_to_uri(&abs);

    // GATE de warmup: índice quente ANTES de renomear (senão o WorkspaceEdit é incompleto).
    let (_refs, stable, warmup_ms, _polls) = warmup_references(&client, &uri, l, c)?;
    if !stable {
        return Ok(json!({
            "applied": false, "error": "index_not_ready",
            "detail": "índice instável; rename abortado para evitar edição parcial destrutiva"
        }));
    }

    // alguns servers (ex.: Dart) VALIDAM e recusam o rename na origem (colisão de nome) —
    // devolvemos isso de forma estruturada, não como erro genérico.
    let edit = match client.request(
        "textDocument/rename",
        json!({"textDocument":{"uri":uri},"position":{"line":l,"character":c},"newName":new_name}),
        15_000,
    ) {
        Ok(e) => e,
        Err(reason) => {
            return Ok(json!({
                "operation": "rename_symbol", "applied": false, "safe": false,
                "rejected_by_server": true, "reason": reason,
                "symbol": symbol, "new_name": new_name
            }))
        }
    };
    let mut result = verify_and_apply(
        &client,
        &edit,
        apply,
        a["verify_build"].as_bool().unwrap_or(false),
        project,
        build_lang(file),
    )?;
    result["operation"] = json!("rename_symbol");
    result["symbol"] = json!(symbol);
    result["new_name"] = json!(new_name);
    result["index_warmup_ms"] = json!(warmup_ms);
    Ok(result)
}

// pega o edit de um refactoring (codeAction -> resolve se lazy). Faz warmup até aparecerem ações.
fn refactor_edit(
    client: &LspClient,
    uri: &str,
    range: &Value,
    kind: &str,
    prefer_title: Option<&str>,
) -> Result<Value, String> {
    let start = Instant::now();
    let mut actions: Vec<Value> = vec![];
    while start.elapsed().as_millis() < 10_000 {
        let res = client.request(
            "textDocument/codeAction",
            json!({"textDocument":{"uri":uri},"range":range,"context":{"diagnostics":[],"only":[kind]}}),
            10_000,
        )?;
        actions = res.as_array().cloned().unwrap_or_default();
        if !actions.is_empty() {
            break;
        }
        std::thread::sleep(Duration::from_millis(300));
    }
    if actions.is_empty() {
        return Err(format!(
            "nenhum refactoring '{kind}' disponível nesta posição/seleção"
        ));
    }
    // escolhe por título preferido, senão a 1ª
    let chosen = prefer_title
        .and_then(|t| {
            actions
                .iter()
                .find(|a| a["title"].as_str().map(|s| s.contains(t)).unwrap_or(false))
        })
        .or_else(|| actions.first())
        .cloned()
        .unwrap();
    // resolve se o edit for lazy (data sem edit)
    let action = if chosen.get("edit").map(|e| !e.is_null()).unwrap_or(false) {
        chosen
    } else {
        client.request("codeAction/resolve", chosen, 10_000)?
    };
    action
        .get("edit")
        .cloned()
        .filter(|e| !e.is_null())
        .ok_or_else(|| "refactoring não produziu edit".to_string())
}

fn tool_extract_function(srv: &Server, a: &Value) -> Result<Value, String> {
    let project = a["project"].as_str().ok_or("faltou 'project'")?;
    let file = a["file"].as_str().ok_or("faltou 'file'")?;
    let start_line = a["start_line"]
        .as_u64()
        .ok_or("faltou 'start_line' (1-indexed)")?;
    let end_line = a["end_line"]
        .as_u64()
        .ok_or("faltou 'end_line' (1-indexed)")?;
    let apply = a["apply"].as_bool().unwrap_or(false);
    let client = srv.client(project, refactor_backend(file))?; // vtsls tem os refactorings
    let abs = format!("{}/{}", project.trim_end_matches('/'), file);
    client.ensure_open(&abs)?;
    let text = std::fs::read_to_string(&abs).map_err(|e| format!("ler {abs}: {e}"))?;
    let lines: Vec<&str> = text.split('\n').collect();
    let end_col = a["end_col"].as_u64().unwrap_or_else(|| {
        lines
            .get((end_line - 1) as usize)
            .map(|l| l.chars().count() as u64)
            .unwrap_or(0)
    });
    let start_col = a["start_col"].as_u64().unwrap_or(0);
    let range = json!({"start":{"line":start_line-1,"character":start_col},"end":{"line":end_line-1,"character":end_col}});
    let uri = path_to_uri(&abs);
    // prefere extração para o escopo do módulo (função nomeada no topo)
    let edit = refactor_edit(
        &client,
        &uri,
        &range,
        "refactor.extract.function",
        Some("module scope"),
    )?;
    let mut result = verify_and_apply(
        &client,
        &edit,
        apply,
        a["verify_build"].as_bool().unwrap_or(false),
        project,
        build_lang(file),
    )?;
    result["operation"] = json!("extract_function");
    Ok(result)
}

fn tool_move_symbol(srv: &Server, a: &Value) -> Result<Value, String> {
    let project = a["project"].as_str().ok_or("faltou 'project'")?;
    let file = a["file"].as_str().ok_or("faltou 'file'")?;
    let symbol = a["symbol"].as_str().ok_or("faltou 'symbol'")?;
    let line = a["line"].as_u64();
    let apply = a["apply"].as_bool().unwrap_or(false);
    let client = srv.client(project, refactor_backend(file))?;
    let abs = format!("{}/{}", project.trim_end_matches('/'), file);
    client.ensure_open(&abs)?;
    let (l, c) = resolve_pos(&client, &abs, symbol, line)?;
    let range = json!({"start":{"line":l,"character":c},"end":{"line":l,"character":c}});
    let uri = path_to_uri(&abs);
    let edit = refactor_edit(&client, &uri, &range, "refactor.move", Some("new file"))?;
    let mut result = verify_and_apply(
        &client,
        &edit,
        apply,
        a["verify_build"].as_bool().unwrap_or(false),
        project,
        build_lang(file),
    )?;
    result["operation"] = json!("move_symbol");
    result["symbol"] = json!(symbol);
    Ok(result)
}

fn tool_document_symbols(srv: &Server, a: &Value) -> Result<Value, String> {
    let project = a["project"].as_str().ok_or("faltou 'project'")?;
    let file = a["file"].as_str().ok_or("faltou 'file'")?;
    let client = srv.client(project, nav_backend(file))?;
    let abs = format!("{}/{}", project.trim_end_matches('/'), file);
    let syms = document_symbols(&client, &abs)?;
    let mut flat = vec![];
    flatten_symbols(&syms, "", &mut flat);
    // Alinha o `at` ao token do identificador (pula decorators), como o workspace_symbols já faz.
    let txt = std::fs::read_to_string(&abs).unwrap_or_default();
    let flines: Vec<&str> = txt.split('\n').collect();
    let list: Vec<Value> = flat
        .into_iter()
        .map(|(fp, k, l, c)| {
            let (rl, rc) = refine_at(&flines, &fp, l, c);
            json!({"name_path": fp, "kind": kind_name(k), "at": format!("{}:{}", rl + 1, rc + 1)})
        })
        .collect();
    Ok(json!({"file": file, "count": list.len(), "symbols": list}))
}

fn tool_find_symbol(srv: &Server, a: &Value) -> Result<Value, String> {
    let project = a["project"].as_str().ok_or("faltou 'project'")?;
    let file = a["file"].as_str().ok_or("faltou 'file'")?;
    let name_path = a["name_path"]
        .as_str()
        .ok_or("faltou 'name_path' (ex.: 'Widget' ou 'Widget/render')")?;
    let client = srv.client(project, nav_backend(file))?;
    let abs = format!("{}/{}", project.trim_end_matches('/'), file);
    let syms = document_symbols(&client, &abs)?;
    let mut flat = vec![];
    flatten_symbols(&syms, "", &mut flat);
    let txt = std::fs::read_to_string(&abs).unwrap_or_default();
    let flines: Vec<&str> = txt.split('\n').collect();
    let matches: Vec<Value> = flat
        .iter()
        .filter(|(fp, ..)| name_path_matches(fp, name_path))
        .map(|(fp, k, l, c)| {
            let (rl, rc) = refine_at(&flines, fp, *l, *c);
            json!({"name_path": fp, "kind": kind_name(*k), "at": format!("{}:{}", rl + 1, rc + 1)})
        })
        .collect();
    Ok(json!({"query": name_path, "count": matches.len(), "matches": matches}))
}

fn tool_workspace_symbols(srv: &Server, a: &Value) -> Result<Value, String> {
    let project = a["project"].as_str().ok_or("faltou 'project'")?;
    let query = a["query"].as_str().ok_or("faltou 'query'")?;
    // workspace_symbols opera no projeto inteiro (sem arquivo); backend por 'lang' (default ts)
    let backend = if a["lang"].as_str() == Some("python") {
        "basedpyright"
    } else {
        "tsgo"
    };
    let client = srv.client(project, backend)?;
    let res = client.request("workspace/symbol", json!({"query": query}), 10_000)?;
    let root = client.root().to_string();
    let list: Vec<Value> = res
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|s| {
            let (l, c) = sym_pos(s);
            let uri = s["location"]["uri"].as_str().unwrap_or("");
            json!({"name": s["name"].as_str().unwrap_or(""),
                   "kind": kind_name(s["kind"].as_u64().unwrap_or(0)),
                   "at": format!("{}:{}:{}", rel(&root, uri), l + 1, c + 1)})
        })
        .collect();
    Ok(json!({"query": query, "count": list.len(), "symbols": list}))
}

fn tool_call_hierarchy(srv: &Server, a: &Value) -> Result<Value, String> {
    let project = a["project"].as_str().ok_or("faltou 'project'")?;
    let file = a["file"].as_str().ok_or("faltou 'file'")?;
    let symbol = a["symbol"].as_str().ok_or("faltou 'symbol'")?;
    let line = a["line"].as_u64();
    let client = srv.client(project, nav_backend(file))?;
    let abs = format!("{}/{}", project.trim_end_matches('/'), file);
    client.ensure_open(&abs)?;
    let (l, c) = resolve_pos(&client, &abs, symbol, line)?;
    let uri = path_to_uri(&abs);
    let prep = client.request(
        "textDocument/prepareCallHierarchy",
        json!({"textDocument":{"uri":uri},"position":{"line":l,"character":c}}),
        10_000,
    )?;
    let item = prep.as_array().and_then(|a| a.first()).cloned();
    let Some(item) = item else {
        return Ok(
            json!({"symbol": symbol, "incoming": [], "detail": "sem item de call hierarchy nesta posição"}),
        );
    };
    let incoming = client.request("callHierarchy/incomingCalls", json!({"item": item}), 10_000)?;
    let root = client.root().to_string();
    let callers: Vec<Value> = incoming
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|call| {
            let from = &call["from"];
            let (fl, fc) = sym_pos(from);
            let uri = from["uri"].as_str().unwrap_or("");
            json!({"caller": from["name"].as_str().unwrap_or(""),
                   "at": format!("{}:{}:{}", rel(&root, uri), fl + 1, fc + 1)})
        })
        .collect();
    Ok(json!({"symbol": symbol, "incoming_count": callers.len(), "incoming": callers}))
}

// Fase 5: roda o build/check da linguagem NO DISCO e reporta erros (pega o que a simulação
// em memória não vê — ex.: erros de `cargo check` no Rust). Standalone: chame após um apply.
fn tool_validate_build(_srv: &Server, a: &Value) -> Result<Value, String> {
    let project = a["project"].as_str().ok_or("faltou 'project'")?;
    let lang = a["lang"]
        .as_str()
        .map(|s| s.to_string())
        .or_else(|| a["file"].as_str().map(|f| build_lang(f).to_string()))
        .ok_or("faltou 'lang' ou 'file'")?;
    let (ok, errors) = build_check(project, &lang)?;
    Ok(json!({"lang": lang, "build_ok": ok, "errors": errors}))
}

// ---- doctor: verifica/corrige o setup do LSP por linguagem no projeto ----

fn which(bin: &str) -> bool {
    if bin.contains('/') {
        return Path::new(bin).exists();
    }
    std::env::var("PATH")
        .map(|p| p.split(':').any(|d| Path::new(d).join(bin).exists()))
        .unwrap_or(false)
}

fn dir_has_ext(dir: &Path, ext: &str) -> bool {
    std::fs::read_dir(dir)
        .map(|rd| {
            rd.flatten()
                .any(|e| e.path().extension().map(|x| x == ext).unwrap_or(false))
        })
        .unwrap_or(false)
}

fn detect_langs(project: &str) -> Vec<&'static str> {
    let p = Path::new(project);
    let mut v = vec![];
    if p.join("pyproject.toml").exists()
        || p.join("setup.py").exists()
        || p.join("requirements.txt").exists()
    {
        v.push("python");
    }
    if p.join("tsconfig.json").exists() || p.join("package.json").exists() {
        v.push("typescript");
    }
    if p.join("Cargo.toml").exists() {
        v.push("rust");
    }
    if p.join("pubspec.yaml").exists() {
        v.push("dart");
    }
    if dir_has_ext(p, "csproj") || dir_has_ext(p, "sln") {
        v.push("csharp");
    }
    v
}

// Verifica (e opcionalmente corrige com fix=true) o setup por linguagem: language server disponível
// + config de workspace correta (senão as referências saem incompletas — ver docs/LANGUAGE-SETUP.md).
fn tool_doctor(srv: &Server, a: &Value) -> Result<Value, String> {
    let project = a["project"].as_str().ok_or("faltou 'project'")?;
    let fix = a["fix"].as_bool().unwrap_or(false);
    let p = Path::new(project);
    let langs = detect_langs(project);
    let mut report = vec![];

    for lang in &langs {
        let (server_bin, server) = match *lang {
            "python" => (srv.basedpyright_bin.clone(), "basedpyright"),
            "typescript" => (srv.tsgo_bin.clone(), "tsgo/vtsls"),
            "rust" => (srv.rust_analyzer_bin.clone(), "rust-analyzer"),
            "dart" => (srv.dart_bin.clone(), "dart language-server"),
            "csharp" => (srv.csharp_ls_bin.clone(), "csharp-ls"),
            _ => (String::new(), ""),
        };
        let available = which(&server_bin) || (*lang == "typescript" && which(&srv.vtsls_bin));
        let install_hint = match *lang {
            "python" => "pip install basedpyright  (ou: npm i -g basedpyright)",
            "typescript" => "npm i -g @typescript/native-preview @vtsls/language-server",
            "rust" => "rustup component add rust-analyzer",
            "dart" => "instale o Dart/Flutter SDK",
            "csharp" => "dotnet tool install --global csharp-ls",
            _ => "",
        };
        let mut cfg_ok = true;
        let mut issue = Value::Null;
        let mut fix_desc = Value::Null;
        let mut applied = Value::Null;

        match *lang {
            "python" => {
                let has_cfg = p.join("pyrightconfig.json").exists();
                let pyproject =
                    std::fs::read_to_string(p.join("pyproject.toml")).unwrap_or_default();
                let has_tool = pyproject.contains("[tool.basedpyright]")
                    || pyproject.contains("[tool.pyright]");
                if !has_cfg && !has_tool {
                    cfg_ok = false;
                    issue = json!("sem [tool.basedpyright]/pyrightconfig.json → find_references INCOMPLETO (modo openFilesOnly)");
                    let src = if p.join("src").is_dir() { "src" } else { "." };
                    let venv = [".venv", "venv", "env"]
                        .iter()
                        .find(|d| p.join(d).join("pyvenv.cfg").exists())
                        .copied();
                    fix_desc = json!(format!(
                        "criar pyrightconfig.json (include=[\"{src}\"]{})",
                        venv.map(|v| format!(", venv=\"{v}\"")).unwrap_or_default()
                    ));
                    if fix {
                        let mut cfg = json!({"include": [src], "useLibraryCodeForTypes": false});
                        if let Some(v) = venv {
                            cfg["venvPath"] = json!(".");
                            cfg["venv"] = json!(v);
                        }
                        std::fs::write(
                            p.join("pyrightconfig.json"),
                            serde_json::to_string_pretty(&cfg).unwrap(),
                        )
                        .map_err(|e| format!("escrever pyrightconfig.json: {e}"))?;
                        applied = json!(true);
                        cfg_ok = true;
                    }
                }
            }
            "dart" => {
                if !p.join(".dart_tool").join("package_config.json").exists() {
                    cfg_ok = false;
                    issue = json!("sem .dart_tool/package_config.json");
                    fix_desc = json!("rode: dart pub get (ou flutter pub get)");
                }
            }
            "typescript" => {
                if !p.join("tsconfig.json").exists() {
                    cfg_ok = false;
                    issue = json!("sem tsconfig.json");
                    fix_desc = json!("crie um tsconfig.json (monorepo: use project references)");
                }
            }
            "csharp" => {
                if !which("dotnet") {
                    cfg_ok = false;
                    issue = json!("dotnet SDK não encontrado no PATH");
                    fix_desc = json!("instale o .NET SDK e defina DOTNET_ROOT");
                }
            }
            _ => {}
        }

        report.push(json!({
            "lang": lang, "server": server, "server_bin": server_bin,
            "server_available": available,
            "install": if available { Value::Null } else { json!(install_hint) },
            "workspace_config": {"ok": cfg_ok, "issue": issue, "fix": fix_desc, "applied": applied},
        }));
    }

    let problems = report
        .iter()
        .filter(|e| {
            !e["server_available"].as_bool().unwrap_or(true)
                || !e["workspace_config"]["ok"].as_bool().unwrap_or(true)
        })
        .count();
    Ok(json!({
        "project": project,
        "languages_detected": langs,
        "problems": problems,
        "report": report,
        "hint": if fix { "correções aplicadas onde possível" } else { "rode com fix=true para corrigir as configs automaticamente" },
    }))
}

fn tools_schema() -> Value {
    json!([
        {
            "name": "find_references",
            "description": "Encontra TODAS as referências semânticas a um símbolo (via tsgo). Aguarda o índice estabilizar (gate de warmup) e sinaliza se o resultado ainda não é confiável. Use isto em vez de grep para rename/delete.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "project": {"type": "string", "description": "caminho ABSOLUTO da raiz do projeto"},
                    "file": {"type": "string", "description": "caminho do arquivo RELATIVO ao project"},
                    "symbol": {"type": "string", "description": "nome do símbolo (ex.: 'ZodType')"},
                    "line": {"type": "integer", "description": "opcional: linha 1-indexed para desambiguar"}
                },
                "required": ["project", "file", "symbol"]
            }
        },
        {
            "name": "rename_symbol",
            "description": "Rename semântico com verificação. Gate de warmup + simula a edição EM MEMÓRIA e mede net_delta (erros introduzidos - resolvidos). Com apply=false (default) é preview (mede e reverte). Com apply=true persiste no disco APENAS se net_delta<=0; senão reverte e reporta os erros que introduziria. 'symbol' aceita name_path ('Classe/metodo').",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "project": {"type": "string"},
                    "file": {"type": "string"},
                    "symbol": {"type": "string", "description": "nome ou name_path (ex.: 'ZodType' ou 'Widget/render')"},
                    "new_name": {"type": "string"},
                    "apply": {"type": "boolean", "description": "false=preview (default); true=aplica no disco se seguro"},
                    "verify_build": {"type": "boolean", "description": "com apply=true: roda o build da linguagem após aplicar e REVERTE se falhar (pega erros que o net_delta em memória não vê, ex.: cargo check)"},
                    "line": {"type": "integer", "description": "opcional: linha 1-indexed"}
                },
                "required": ["project", "file", "symbol", "new_name"]
            }
        },
        {
            "name": "document_symbols",
            "description": "Árvore de símbolos de um arquivo (classes → métodos/propriedades), com name_path e posição. Base para 'achar método dentro de classe'.",
            "inputSchema": {
                "type": "object",
                "properties": {"project": {"type": "string"}, "file": {"type": "string"}},
                "required": ["project", "file"]
            }
        },
        {
            "name": "find_symbol",
            "description": "Acha um símbolo por name_path dentro de um arquivo (ex.: 'Widget/render'), com posição exata. Resolução semântica (não textual).",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "project": {"type": "string"}, "file": {"type": "string"},
                    "name_path": {"type": "string", "description": "'Classe', 'metodo' ou 'Classe/metodo'"}
                },
                "required": ["project", "file", "name_path"]
            }
        },
        {
            "name": "workspace_symbols",
            "description": "Busca símbolos por nome em TODO o projeto (workspace/symbol). Use lang='python' para projetos Python.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "project": {"type": "string"}, "query": {"type": "string"},
                    "lang": {"type": "string", "description": "'python' ou 'typescript' (default)"}
                },
                "required": ["project", "query"]
            }
        },
        {
            "name": "call_hierarchy",
            "description": "Quem chama este símbolo (incoming calls). Útil para refactoring seguro. 'symbol' aceita name_path.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "project": {"type": "string"}, "file": {"type": "string"},
                    "symbol": {"type": "string"},
                    "line": {"type": "integer", "description": "opcional: linha 1-indexed"}
                },
                "required": ["project", "file", "symbol"]
            }
        },
        {
            "name": "extract_function",
            "description": "Extrai um intervalo de linhas para uma nova função (escopo do módulo), via refactoring semântico (vtsls). Mesmo ciclo apply→verify com net_delta: apply=false=preview; apply=true persiste só se seguro.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "project": {"type": "string"}, "file": {"type": "string"},
                    "start_line": {"type": "integer", "description": "1-indexed"},
                    "end_line": {"type": "integer", "description": "1-indexed"},
                    "start_col": {"type": "integer", "description": "opcional, 0-indexed"},
                    "end_col": {"type": "integer", "description": "opcional, 0-indexed (default: fim da linha)"},
                    "apply": {"type": "boolean"},
                    "verify_build": {"type": "boolean", "description": "com apply=true: roda o build e reverte se falhar"}
                },
                "required": ["project", "file", "start_line", "end_line"]
            }
        },
        {
            "name": "move_symbol",
            "description": "Move um símbolo (top-level) para um novo arquivo, via refactoring semântico (vtsls), atualizando os imports. Mesmo ciclo apply→verify com net_delta (suporta criação de arquivo). 'symbol' aceita name_path.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "project": {"type": "string"}, "file": {"type": "string"},
                    "symbol": {"type": "string"},
                    "line": {"type": "integer", "description": "opcional: linha 1-indexed"},
                    "apply": {"type": "boolean"},
                    "verify_build": {"type": "boolean", "description": "com apply=true: roda o build e reverte se falhar"}
                },
                "required": ["project", "file", "symbol"]
            }
        },
        {
            "name": "doctor",
            "description": "Verifica o setup do projeto por linguagem: language server disponível + config de workspace correta (senão find_references sai incompleto EM SILÊNCIO — crítico em Python). Com fix=true, corrige o que dá (ex.: cria pyrightconfig.json). Rode uma vez ao abrir um projeto novo.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "project": {"type": "string", "description": "caminho absoluto da raiz do projeto"},
                    "fix": {"type": "boolean", "description": "true = aplica as correções possíveis (escreve configs)"}
                },
                "required": ["project"]
            }
        },
        {
            "name": "validate_build",
            "description": "Roda o build/check da linguagem NO DISCO e reporta erros. Fecha o buraco do net_delta em memória (ex.: erros que só o `cargo check` do Rust pega). Chame após um apply. Comando por linguagem, override via env <LANG>_CHECK_CMD.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "project": {"type": "string"},
                    "file": {"type": "string", "description": "para inferir a linguagem"},
                    "lang": {"type": "string", "description": "rust|dart|csharp|typescript|python (alternativa a 'file')"}
                },
                "required": ["project"]
            }
        }
    ])
}

fn call_tool(srv: &Server, name: &str, args: &Value) -> Value {
    let res = match name {
        "find_references" => tool_find_references(srv, args),
        "rename_symbol" => tool_rename_symbol(srv, args),
        "document_symbols" => tool_document_symbols(srv, args),
        "find_symbol" => tool_find_symbol(srv, args),
        "workspace_symbols" => tool_workspace_symbols(srv, args),
        "call_hierarchy" => tool_call_hierarchy(srv, args),
        "extract_function" => tool_extract_function(srv, args),
        "move_symbol" => tool_move_symbol(srv, args),
        "validate_build" => tool_validate_build(srv, args),
        "doctor" => tool_doctor(srv, args),
        other => Err(format!("ferramenta desconhecida: {other}")),
    };
    match res {
        Ok(v) => {
            json!({"content":[{"type":"text","text": serde_json::to_string_pretty(&v).unwrap()}]})
        }
        Err(e) => {
            json!({"content":[{"type":"text","text": format!("ERRO: {e}")}], "isError": true})
        }
    }
}

fn build_server() -> Server {
    Server {
        clients: Mutex::new(HashMap::new()),
        tsgo_bin: std::env::var("TSGO_BIN").unwrap_or_else(|_| "tsgo".to_string()),
        vtsls_bin: std::env::var("VTSLS_BIN").unwrap_or_else(|_| "vtsls".to_string()),
        basedpyright_bin: std::env::var("BASEDPYRIGHT_BIN")
            .unwrap_or_else(|_| "basedpyright-langserver".to_string()),
        dart_bin: std::env::var("DART_BIN").unwrap_or_else(|_| "dart".to_string()),
        rust_analyzer_bin: std::env::var("RUST_ANALYZER_BIN")
            .unwrap_or_else(|_| "rust-analyzer".to_string()),
        csharp_ls_bin: std::env::var("CSHARP_LS_BIN").unwrap_or_else(|_| "csharp-ls".to_string()),
    }
}

// ---- CACHE ENTRE SESSÕES (Fase 5, opt-in via CODE_INTEL_DAEMON=1) -------
// Problema: o Claude Code recria o processo MCP a cada sessão, matando os LSPs quentes -> paga o
// cold-start (rust-analyzer ~30s, csharp-ls ~24s) de novo. Solução: um DAEMON separado, dono dos
// LSPs, que sobrevive ao restart do MCP. O MCP vira um proxy fino sobre um Unix socket.
// (Seguro porque o freshness re-sincroniza arquivos mudados no disco entre sessões.)
// NB: usa Unix domain sockets -> disponível só em unix. No Windows o MCP roda sem o daemon
// (cache entre sessões indisponível); todo o resto funciona normalmente.

#[cfg(unix)]
fn sock_path() -> String {
    std::env::var("CODE_INTEL_SOCK").unwrap_or_else(|_| {
        let home = std::env::var("HOME").unwrap_or_else(|_| "tmp".into());
        format!("/tmp/code-intel-mcp{}.sock", home.replace('/', "_"))
    })
}

#[cfg(unix)]
fn run_daemon() {
    let srv = Arc::new(build_server());
    let path = sock_path();
    let _ = std::fs::remove_file(&path);
    let listener = std::os::unix::net::UnixListener::bind(&path).expect("bind unix socket");
    // watchdog: encerra após 30min ocioso (sem conexões)
    let active = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let last = Arc::new(Mutex::new(Instant::now()));
    {
        let (active, last) = (active.clone(), last.clone());
        std::thread::spawn(move || loop {
            std::thread::sleep(Duration::from_secs(60));
            if active.load(std::sync::atomic::Ordering::SeqCst) == 0
                && last.lock().unwrap().elapsed() > Duration::from_secs(1800)
            {
                std::process::exit(0);
            }
        });
    }
    for stream in listener.incoming() {
        let Ok(stream) = stream else { continue };
        let (srv, active, last) = (srv.clone(), active.clone(), last.clone());
        std::thread::spawn(move || {
            active.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            handle_daemon_conn(stream, &srv);
            active.fetch_sub(1, std::sync::atomic::Ordering::SeqCst);
            *last.lock().unwrap() = Instant::now();
        });
    }
}

#[cfg(unix)]
fn handle_daemon_conn(stream: std::os::unix::net::UnixStream, srv: &Server) {
    let reader = std::io::BufReader::new(match stream.try_clone() {
        Ok(s) => s,
        Err(_) => return,
    });
    let mut w = stream;
    for line in reader.lines() {
        let Ok(line) = line else { break };
        if line.trim().is_empty() {
            continue;
        }
        let Ok(msg) = serde_json::from_str::<Value>(&line) else {
            continue;
        };
        let id = msg.get("id").cloned();
        let name = msg["params"]["name"].as_str().unwrap_or("").to_string();
        let args = msg["params"]["arguments"].clone();
        let result = call_tool(srv, &name, &args);
        let resp = json!({"jsonrpc":"2.0","id":id,"result":result});
        if writeln!(w, "{}", serde_json::to_string(&resp).unwrap()).is_err() {
            break;
        }
        let _ = w.flush();
    }
}

// no MCP: encaminha um tools/call ao daemon (sobe o daemon se necessário)
#[cfg(unix)]
fn forward_call(name: &str, args: &Value) -> Value {
    let path = sock_path();
    if std::os::unix::net::UnixStream::connect(&path).is_err() {
        if let Ok(exe) = std::env::current_exe() {
            let _ = std::process::Command::new(exe)
                .arg("--daemon")
                .stdin(std::process::Stdio::null())
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .spawn();
        }
        let start = Instant::now();
        while std::os::unix::net::UnixStream::connect(&path).is_err()
            && start.elapsed() < Duration::from_secs(5)
        {
            std::thread::sleep(Duration::from_millis(100));
        }
    }
    let err = |m: &str| json!({"content":[{"type":"text","text": format!("ERRO: {m}")}], "isError": true});
    let Ok(mut stream) = std::os::unix::net::UnixStream::connect(&path) else {
        return err("daemon indisponível");
    };
    let req = json!({"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":name,"arguments":args}});
    if writeln!(stream, "{}", serde_json::to_string(&req).unwrap()).is_err() {
        return err("falha ao enviar ao daemon");
    }
    let _ = stream.flush();
    let mut reader = std::io::BufReader::new(stream);
    let mut line = String::new();
    if reader.read_line(&mut line).is_ok() {
        if let Ok(v) = serde_json::from_str::<Value>(&line) {
            return v
                .get("result")
                .cloned()
                .unwrap_or_else(|| err("resposta vazia do daemon"));
        }
    }
    err("falha ao ler do daemon")
}

fn main() {
    let argv: Vec<String> = std::env::args().collect();
    if argv.iter().any(|a| a == "--version" || a == "-V") {
        println!("code-intel-mcp {}", env!("CARGO_PKG_VERSION"));
        return;
    }
    if argv.iter().any(|a| a == "--daemon") {
        #[cfg(unix)]
        {
            run_daemon();
        }
        #[cfg(not(unix))]
        {
            eprintln!("code-intel-mcp: --daemon (cache entre sessões) não é suportado no Windows");
        }
        return;
    }
    // opt-in: encaminha as operações ao daemon (índice quente sobrevive entre sessões)
    #[cfg(unix)]
    let use_daemon = std::env::var("CODE_INTEL_DAEMON").is_ok();
    #[cfg(not(unix))]
    let use_daemon = {
        if std::env::var("CODE_INTEL_DAEMON").is_ok() {
            eprintln!("code-intel-mcp: CODE_INTEL_DAEMON ignorado no Windows (cache entre sessões indisponível)");
        }
        false
    };
    let srv = build_server();

    let stdin = std::io::stdin();
    let stdout = std::io::stdout();
    let mut out = stdout.lock();

    for line in stdin.lock().lines() {
        let line = match line {
            Ok(l) => l,
            Err(_) => break,
        };
        if line.trim().is_empty() {
            continue;
        }
        let msg: Value = match serde_json::from_str(&line) {
            Ok(v) => v,
            Err(_) => continue,
        };
        let id = msg.get("id").cloned();
        let method = msg.get("method").and_then(|m| m.as_str()).unwrap_or("");

        // notificações (sem id) — não respondem
        let is_notification = id.is_none() || id.as_ref().map(|i| i.is_null()).unwrap_or(true);

        let result: Option<Value> = match method {
            "initialize" => {
                let pv = msg["params"]["protocolVersion"]
                    .as_str()
                    .unwrap_or("2024-11-05")
                    .to_string();
                Some(json!({
                    "protocolVersion": pv,
                    "capabilities": {"tools": {}},
                    "serverInfo": {"name": "code-intel-mcp", "version": env!("CARGO_PKG_VERSION")}
                }))
            }
            "tools/list" => Some(json!({"tools": tools_schema()})),
            "tools/call" => {
                let name = msg["params"]["name"].as_str().unwrap_or("");
                let args = msg["params"]["arguments"].clone();
                #[cfg(unix)]
                let r = if use_daemon {
                    forward_call(name, &args)
                } else {
                    call_tool(&srv, name, &args)
                };
                #[cfg(not(unix))]
                let r = {
                    let _ = use_daemon;
                    call_tool(&srv, name, &args)
                };
                Some(r)
            }
            "ping" => Some(json!({})),
            _ => None,
        };

        if is_notification {
            continue; // ex.: notifications/initialized
        }
        let resp = match result {
            Some(r) => json!({"jsonrpc":"2.0","id":id,"result":r}),
            None => {
                json!({"jsonrpc":"2.0","id":id,"error":{"code":-32601,"message":format!("método não suportado: {method}")}})
            }
        };
        let _ = writeln!(out, "{}", serde_json::to_string(&resp).unwrap());
        let _ = out.flush();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn base_name_strips_method_signature() {
        // csharp-ls anexa a assinatura ao nome do método
        assert_eq!(base_name("HandleAsync(string x, int y)"), "HandleAsync");
        assert_eq!(
            base_name("AddWalletModule(this IServiceCollection services)"),
            "AddWalletModule"
        );
        // idempotente para nomes sem assinatura (TS/Rust/etc.)
        assert_eq!(base_name("Reasons"), "Reasons");
        assert_eq!(
            base_name("RefundRedemptionHandler"),
            "RefundRedemptionHandler"
        );
    }

    #[test]
    fn strip_sigs_normalizes_full_path() {
        assert_eq!(
            strip_sigs("RefundRedemptionHandler/HandleAsync(string x, Guid y)"),
            "RefundRedemptionHandler/HandleAsync"
        );
        assert_eq!(strip_sigs("Widget/render"), "Widget/render");
    }

    // Regressão do bug: find_symbol devolvia count 0 para métodos em C# porque o csharp-ls
    // inclui a assinatura no nome do símbolo (ex.: "HandleAsync(...)").
    #[test]
    fn name_path_matches_csharp_method_with_signature() {
        let fp = "RefundRedemptionHandler/HandleAsync(string publicRef, Guid partnerId)";
        assert!(name_path_matches(fp, "HandleAsync"));
        assert!(name_path_matches(fp, "RefundRedemptionHandler/HandleAsync"));

        let fp2 = "WalletModule/AddWalletModule(this IServiceCollection services)";
        assert!(name_path_matches(fp2, "AddWalletModule"));
    }

    // Regressão do P1 (relatório pachamama, Python): basedpyright reporta símbolos DECORADOS na
    // linha do @decorator, não do identificador. A varredura pra frente deve achar o nome.
    #[test]
    fn scan_ident_skips_decorator_class() {
        let src =
            "x = 1\n@dataclass(slots=True, frozen=True)\nclass ResponseModel(Kobject):\n    pass\n";
        let lines: Vec<&str> = src.split('\n').collect();
        // símbolo reportado na linha do decorator (idx 1) -> identificador na idx 2, col 6
        assert_eq!(scan_ident(&lines, "ResponseModel", 1, 16), Some((2, 6)));
    }

    #[test]
    fn scan_ident_skips_stacked_decorators_method() {
        let src = "class C:\n    @classmethod\n    @wraps(f)\n    def from_exception(cls):\n        ...\n";
        let lines: Vec<&str> = src.split('\n').collect();
        // reportado no @classmethod (idx 1) -> def na idx 3, col 8
        assert_eq!(scan_ident(&lines, "from_exception", 1, 16), Some((3, 8)));
    }

    #[test]
    fn scan_ident_no_decorator_same_line() {
        let src = "class ResponseCode(IntEnum):\n    A = 1\n";
        let lines: Vec<&str> = src.split('\n').collect();
        assert_eq!(scan_ident(&lines, "ResponseCode", 0, 16), Some((0, 6)));
    }

    #[test]
    fn scan_ident_not_found_returns_none() {
        let src = "def foo():\n    pass\n";
        let lines: Vec<&str> = src.split('\n').collect();
        assert_eq!(scan_ident(&lines, "Bar", 0, 16), None);
    }

    // Garantia cross-linguagem (relatório viva-bff, TypeScript/tsgo): nomes crus, sem assinatura,
    // com name_path composto ("Classe/metodo") — o mesmo padrão que sempre funcionou no TS deve
    // continuar funcionando após a normalização (idempotente), sem regressão.
    #[test]
    fn name_path_matches_typescript_composite_path_no_regression() {
        let fp = "GetBalanceResolver/resolve";
        assert!(name_path_matches(fp, "GetBalanceResolver/resolve"));
        assert!(name_path_matches(fp, "resolve"));
        // classe homônima de método em outro resolver não deve casar quando a classe é especificada
        assert!(!name_path_matches(
            "GetProfileResolver/resolve",
            "GetBalanceResolver/resolve"
        ));
    }

    #[test]
    fn name_path_matches_class_and_field_unaffected() {
        assert!(name_path_matches(
            "RefundRedemptionHandler",
            "RefundRedemptionHandler"
        ));
        assert!(name_path_matches(
            "RefundRedemptionHandler/Reasons",
            "Reasons"
        ));
        // não casa símbolo diferente
        assert!(!name_path_matches(
            "RefundRedemptionHandler/HandleAsync(string x)",
            "ExecuteRefundAsync"
        ));
    }
}
