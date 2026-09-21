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

    // Descarta o client em cache e sobe um novo — usado quando o backend cai (pipe quebrado)
    // durante um refactoring, para recuperar sem exigir restart do MCP.
    fn restart_client(&self, project: &str, backend: &str) -> Result<Arc<LspClient>, String> {
        let key = format!("{project}\u{0}{backend}");
        self.clients.lock().unwrap().remove(&key);
        self.client(project, backend)
    }
}

// Heurística: o erro indica que o backend fechou a conexão (processo morto)?
fn is_conn_dead(e: &str) -> bool {
    let e = e.to_lowercase();
    e.contains("pipe") || e.contains("broken") || e.contains("os error 32")
}

// Converte um offset de BYTE numa linha para a coluna em UNIDADES UTF-16 (encoding default do LSP).
// Sem isso, uma linha com unicode ANTES do símbolo (ex.: acento, emoji num comentário/string) faz a
// coluna sair errada e o edit atingir a posição errada em silêncio (achado #1 da pesquisa competitiva).
fn utf16_col(row: &str, byte_off: usize) -> u64 {
    row.get(..byte_off)
        .map(|s| s.encode_utf16().count() as u64)
        .unwrap_or(byte_off as u64)
}

// Acha `symbol` como IDENTIFICADOR COMPLETO em `row` (word boundary): o char antes e depois não
// pode ser [A-Za-z0-9_]. Sem isso, "Result" casaria DENTRO de "RefundResult" e o rename atingiria
// o símbolo errado (Bug 2 do relatório). Retorna o byte-offset (= coluna p/ ASCII).
fn find_ident(row: &str, symbol: &str) -> Option<usize> {
    if symbol.is_empty() {
        return None;
    }
    let bytes = row.as_bytes();
    let slen = symbol.len();
    let is_ident = |b: u8| b.is_ascii_alphanumeric() || b == b'_';
    let mut start = 0usize;
    while let Some(rel) = row[start..].find(symbol) {
        let i = start + rel;
        let before_ok = i == 0 || !is_ident(bytes[i - 1]);
        let after = i + slen;
        let after_ok = after >= bytes.len() || !is_ident(bytes[after]);
        if before_ok && after_ok {
            return Some(i);
        }
        start = i + 1;
    }
    None
}

// Junta project+file de forma SEGURA: normaliza componentes (resolve '..'/'.') e recusa paths que
// ESCAPAM a raiz do projeto (gap #2 da pesquisa: traversal via '../', symlink, prefixo parcial).
// Comparação por COMPONENTES (não string), então '/root2' não "começa com" '/root'.
fn safe_abs(project: &str, file: &str) -> Result<String, String> {
    use std::path::{Component, PathBuf};
    let root = std::fs::canonicalize(project)
        .unwrap_or_else(|_| PathBuf::from(project.trim_end_matches('/')));
    let joined = root.join(file);
    let mut norm = PathBuf::new();
    for comp in joined.components() {
        match comp {
            Component::ParentDir => {
                norm.pop();
            }
            Component::CurDir => {}
            c => norm.push(c.as_os_str()),
        }
    }
    if !norm.starts_with(&root) {
        return Err(format!(
            "path '{file}' escapa a raiz do projeto ('{}') — recusado",
            root.display()
        ));
    }
    Ok(norm.to_string_lossy().into_owned())
}

// Localiza a posição (LSP 0-indexed) do símbolo no arquivo. `line` opcional é 1-indexed (humano).
// Casa por IDENTIFICADOR COMPLETO (não substring) — ver find_ident.
fn locate(abs_file: &str, symbol: &str, line: Option<u64>) -> Result<(u64, u64), String> {
    let text = std::fs::read_to_string(abs_file).map_err(|e| format!("ler {abs_file}: {e}"))?;
    let lines: Vec<&str> = text.split('\n').collect();
    if let Some(l) = line {
        let idx = (l as usize).saturating_sub(1);
        if let Some(row) = lines.get(idx) {
            if let Some(c) = find_ident(row, symbol) {
                return Ok((idx as u64, utf16_col(row, c)));
            }
        }
        return Err(format!(
            "identificador '{symbol}' não achado na linha {l} (match por palavra inteira)"
        ));
    }
    for (i, row) in lines.iter().enumerate() {
        if let Some(c) = find_ident(row, symbol) {
            return Ok((i as u64, utf16_col(row, c)));
        }
    }
    Err(format!("identificador '{symbol}' não achado em {abs_file}"))
}

// Varre `lines` a partir de `start` (0-based) até `max` linhas à frente procurando o token
// `symbol`. Pula decorators (@...) e trivia à esquerda que alguns language servers (basedpyright)
// incluem no range de símbolos decorados — reportando a posição no `@dataclass`/`@classmethod` em
// vez do identificador. Retorna (linha0, col0) do identificador.
fn scan_ident(lines: &[&str], symbol: &str, start: usize, max: usize) -> Option<(u64, u64)> {
    let end = (start + max).min(lines.len());
    for (off, row) in lines.get(start..end)?.iter().enumerate() {
        // find_ident (palavra inteira) + coluna UTF-16 — consistente com locate.
        if let Some(c) = find_ident(row, symbol) {
            return Some(((start + off) as u64, utf16_col(row, c)));
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
// Aviso acionável quando o índice não estabiliza: sugere daemon (se desligado) e/ou esticar o teto.
fn index_not_ready_hint() -> String {
    let base = "index_not_ready: índice ainda indexando; NÃO use para rename/delete";
    #[cfg(unix)]
    {
        if std::env::var("CODE_INTEL_DAEMON").is_ok() {
            format!("{base} — aumente CODE_INTEL_WARMUP_MS se persistir")
        } else {
            format!("{base} — ligue CODE_INTEL_DAEMON=1 (sem daemon o warmup reinicia a cada chamada) e/ou aumente CODE_INTEL_WARMUP_MS")
        }
    }
    #[cfg(not(unix))]
    {
        format!("{base} — aumente CODE_INTEL_WARMUP_MS")
    }
}

fn warmup_references(
    client: &LspClient,
    uri: &str,
    line: u64,
    ch: u64,
    budget_override: Option<u128>,
) -> Result<(Vec<Value>, bool, u128, u32), String> {
    let start = Instant::now();
    let mut last: i64 = -1;
    let mut stable_hits = 0u32;
    let mut polls = 0u32;
    let mut refs: Vec<Value> = vec![];
    // Teto de warmup. Default 60s (rust-analyzer roda cargo metadata + check no cold start, ~30s
    // no fixture medido). Repos grandes em cold start podem precisar de mais — CODE_INTEL_WARMUP_MS.
    // `budget_override` permite um teto curto por tentativa (ex.: smoke tentando vários símbolos).
    let budget_ms: u128 = budget_override.unwrap_or_else(|| {
        std::env::var("CODE_INTEL_WARMUP_MS")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(60_000)
    });
    while start.elapsed().as_millis() < budget_ms {
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

// Relatório suite-completa: csharp-ls rotula `record` (tipo referência) como kind Class (o LSP não
// tem kind "Record"; `record struct` já vem como Struct). Heurístico barato: se é Class e a linha
// do identificador tem o token `record`, rotula "Record". As linhas já estão carregadas p/ refine_at.
fn kind_label(k: u64, lines: &[&str], line0: u64) -> &'static str {
    if k == 5 {
        if let Some(src) = lines.get(line0 as usize) {
            if src
                .split(|c: char| !c.is_alphanumeric() && c != '_')
                .any(|w| w == "record")
            {
                return "Record";
            }
        }
    }
    kind_name(k)
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

// achata a árvore de documentSymbol em (name_path, kind, line, char).
// Suporta os DOIS formatos do LSP:
//  - DocumentSymbol[] (hierárquico): usa `children` para o name_path "Classe/metodo".
//  - SymbolInformation[] (achatado, ex.: tsgo/basedpyright/csharp-ls): reconstrói o name_path
//    via `containerName` — senão métodos viriam como "metodo" (sem a classe) e queries compostas
//    "Classe/metodo" não resolveriam / não teriam precisão entre classes homônimas.
fn flatten_symbols(symbols: &[Value], prefix: &str, out: &mut Vec<(String, u64, u64, u64)>) {
    for s in symbols {
        let name = s["name"].as_str().unwrap_or("");
        let fp = if !prefix.is_empty() {
            format!("{prefix}/{name}")
        } else if let Some(cn) = s
            .get("containerName")
            .and_then(|v| v.as_str())
            .filter(|c| !c.is_empty())
        {
            format!("{cn}/{name}")
        } else {
            name.to_string()
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
// contra a `query` do usuário (sem assinatura). Normaliza `fp` antes de comparar (métodos C#
// resolvem). Critérios:
//  - igualdade exata ou sufixo "/query" (fp hierárquico ou com containerName);
//  - último segmento igual, permitido quando a query NÃO qualifica a classe (sem '/') OU quando o
//    `fp` é ACHATADO (sem '/': o server não deu info de classe — ex.: csharp-ls sem containerName).
//    Assim "A/foo" NÃO casa "B/foo" quando há hierarquia, mas casa um "foo" achatado (best-effort).
fn name_path_matches(fp: &str, query: &str) -> bool {
    let nfp = strip_sigs(fp);
    if nfp == query || nfp.ends_with(&format!("/{query}")) {
        return true;
    }
    let q_last = base_name(query.rsplit('/').next().unwrap_or(query));
    let last_matches = nfp.rsplit('/').next() == Some(q_last);
    let fp_flat = !nfp.contains('/');
    last_matches && (!query.contains('/') || fp_flat)
}

// P12: `new_name` precisa ser um identificador válido e não uma keyword (senão o rename ou vira
// noop silencioso, ou gera código que não compila). Denylist ampla (cobre TS/Python/Rust/C#/Dart).
fn is_reserved_keyword(s: &str) -> bool {
    matches!(
        s,
        "abstract"
            | "async"
            | "await"
            | "break"
            | "case"
            | "catch"
            | "class"
            | "const"
            | "continue"
            | "def"
            | "default"
            | "del"
            | "do"
            | "elif"
            | "else"
            | "enum"
            | "export"
            | "extends"
            | "false"
            | "final"
            | "finally"
            | "fn"
            | "for"
            | "from"
            | "function"
            | "if"
            | "impl"
            | "import"
            | "in"
            | "instanceof"
            | "interface"
            | "is"
            | "lambda"
            | "let"
            | "match"
            | "mod"
            | "move"
            | "mut"
            | "new"
            | "none"
            | "not"
            | "null"
            | "or"
            | "pass"
            | "priv"
            | "pub"
            | "raise"
            | "return"
            | "self"
            | "static"
            | "struct"
            | "super"
            | "switch"
            | "this"
            | "throw"
            | "trait"
            | "true"
            | "try"
            | "type"
            | "typeof"
            | "use"
            | "var"
            | "void"
            | "while"
            | "with"
            | "yield"
    )
}

fn validate_new_name(new_name: &str) -> Result<(), String> {
    if new_name.is_empty() {
        return Err("new_name vazio".into());
    }
    let mut chars = new_name.chars();
    let first = chars.next().unwrap();
    if !(first.is_alphabetic() || first == '_') {
        return Err(format!(
            "'{new_name}' não é um identificador válido (começa com caractere inválido)"
        ));
    }
    if !new_name.chars().all(|c| c.is_alphanumeric() || c == '_') {
        return Err(format!(
            "'{new_name}' não é um identificador válido (caracteres não permitidos)"
        ));
    }
    if is_reserved_keyword(new_name) {
        return Err(format!("'{new_name}' é uma palavra reservada"));
    }
    Ok(())
}

// P8: colisão de nome no MESMO escopo. Independe de diagnósticos (net_delta fica inerte quando o
// server não emite diagnósticos — ex.: basedpyright com typeCheckingMode=off). Acha o símbolo alvo
// (por nome, mais próximo da linha resolvida), pega seu container e vê se já há um IRMÃO com o
// novo nome. Retorna o name_path do irmão colidente, se houver. Cobre same-file/same-scope.
fn same_scope_collision(
    flat: &[(String, u64, u64, u64)],
    target_line: u64,
    old_name: &str,
    new_name: &str,
) -> Option<String> {
    let last_seg =
        |fp: &str| -> String { base_name(fp.rsplit('/').next().unwrap_or(fp)).to_string() };
    let container = |fp: &str| -> String {
        match fp.rsplit_once('/') {
            Some((c, _)) => c.to_string(),
            None => String::new(),
        }
    };
    let ti = flat
        .iter()
        .enumerate()
        .filter(|(_, (fp, ..))| last_seg(fp) == old_name)
        .min_by_key(|(_, (_, _, l, _))| (*l as i64 - target_line as i64).abs())
        .map(|(i, _)| i)?;
    let tcont = container(&flat[ti].0);
    for (i, (fp, k, _, _)) in flat.iter().enumerate() {
        if i == ti {
            continue;
        }
        // só membros "declaráveis" (evita falso-positivo com locais): Class/Method/Property/Field/
        // Constructor/Enum/Interface/Function/Constant/Struct
        let is_member = matches!(k, 5 | 6 | 7 | 8 | 9 | 10 | 11 | 12 | 14 | 23);
        if is_member && container(fp) == tcont && last_seg(fp) == new_name {
            return Some(fp.clone());
        }
    }
    None
}

fn document_symbols(client: &LspClient, abs: &str) -> Result<Vec<Value>, String> {
    client.ensure_open(abs)?;
    // Cold start de servers pesados (csharp-ls ~24s) estoura um timeout fixo de 10s. Reintenta
    // dentro do budget de warmup (o gate não cobria document_symbols — variante do achado #3/cold).
    let budget: u128 = std::env::var("CODE_INTEL_WARMUP_MS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(60_000);
    let start = Instant::now();
    loop {
        match client.request(
            "textDocument/documentSymbol",
            json!({"textDocument":{"uri":path_to_uri(abs)}}),
            10_000,
        ) {
            Ok(res) => return Ok(res.as_array().cloned().unwrap_or_default()),
            Err(e) => {
                // timeout/ContentModified durante indexação → re-tenta até o budget
                let retriable = e.contains("timeout") || e.contains("-32801");
                if start.elapsed().as_millis() >= budget || !retriable {
                    return Err(e);
                }
                std::thread::sleep(Duration::from_millis(300));
            }
        }
    }
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
        // P9: default sensato pra Python (antes: no-op silencioso). basedpyright lê o disco e o
        // pyrightconfig; override via PYTHON_CHECK_CMD (ex.: "python -m py_compile ...").
        "python" => ("PYTHON_CHECK_CMD", Some(("basedpyright", vec!["."]))),
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

// Uma linha é RESUMO de contagem (não um erro real)? Cobre:
//  - dotnet/msbuild: "0 Error(s)"  → contém "error(s)"
//  - basedpyright:   "0 errors, 0 warnings, 0 notes"  → dígito antes de "error(s)"
// Erros reais dizem "error CS1234"/"error:"/"error[E...]" — o char antes de "error" NÃO é dígito.
fn is_count_summary(line: &str) -> bool {
    let low = line.to_lowercase();
    if low.contains("error(s)") {
        return true;
    }
    let b = low.as_bytes();
    let mut i = 0usize;
    while let Some(rel) = low[i..].find("error") {
        let pos = i + rel;
        let mut j = pos;
        while j > 0 && b[j - 1] == b' ' {
            j -= 1;
        }
        if j > 0 && b[j - 1].is_ascii_digit() {
            return true; // "<n> error(s)" → contagem, não erro
        }
        i = pos + 5;
    }
    false
}

// Extrai linhas de ERRO REAL da saída do build, ignorando as linhas de RESUMO de contagem (P13:
// o basedpyright imprime "0 errors, 0 warnings, 0 notes" num build verde — sem esse filtro virava
// build_ok:false e revertia um rename SEGURO via verify_build).
fn parse_build_errors(output: &str) -> Vec<String> {
    output
        .lines()
        .filter(|l| l.to_lowercase().contains("error") && !is_count_summary(l))
        .take(20)
        .map(|l| l.trim().to_string())
        .collect()
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
    let errors = parse_build_errors(&combined);
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
    let abs = safe_abs(project, file)?;
    client.ensure_open(&abs)?;
    client.resync_all_changed(); // Bug 3: reflete mudanças externas (git checkout) nos OUTROS arquivos
    let (l, c) = resolve_pos(&client, &abs, symbol, line)?;
    let uri = path_to_uri(&abs);
    let (refs, stable, warmup_ms, polls) = warmup_references(&client, &uri, l, c, None)?;
    let warning = if stable {
        Value::Null
    } else {
        json!(index_not_ready_hint())
    };
    // P7: modo resumido — só contagem + arquivos distintos (+ por-arquivo). Evita estourar o limite
    // de tokens do cliente em símbolos muito usados (resultado grande vira dezenas de KB).
    if a["summary"].as_bool().unwrap_or(false) {
        let mut by_file: std::collections::BTreeMap<String, u64> =
            std::collections::BTreeMap::new();
        for r in &refs {
            let u = r["uri"].as_str().unwrap_or("");
            *by_file.entry(rel(client.root(), u)).or_insert(0) += 1;
        }
        return Ok(json!({
            "symbol": symbol,
            "count": refs.len(),
            "files": by_file.len(),
            "stable": stable,
            "warning": warning,
            "warmup_ms": warmup_ms,
            "polls": polls,
            "by_file": by_file,
        }));
    }
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
        "warning": warning,
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

    // P12: valida new_name (identificador válido, não-keyword) ANTES de qualquer trabalho.
    if let Err(reason) = validate_new_name(new_name) {
        return Ok(json!({
            "operation": "rename_symbol", "applied": false, "safe": false,
            "error": "invalid_new_name", "detail": reason,
            "symbol": symbol, "new_name": new_name
        }));
    }
    // P12: renomear para o MESMO nome é noop explícito (não um "rename real" com blast_radius).
    let old_last = base_name(symbol.rsplit('/').next().unwrap_or(symbol));
    if new_name == old_last {
        return Ok(json!({
            "operation": "rename_symbol", "applied": false, "mode": "noop", "safe": true,
            "detail": "new_name == nome atual — nada a fazer", "symbol": symbol, "new_name": new_name
        }));
    }

    let client = srv.client(project, nav_backend(file))?;
    let abs = safe_abs(project, file)?;
    client.ensure_open(&abs)?;
    client.resync_all_changed(); // Bug 3: freshness cross-file
    let (l, c) = resolve_pos(&client, &abs, symbol, line)?;
    let uri = path_to_uri(&abs);

    // P8: colisão de nome no mesmo escopo — rede INDEPENDENTE de diagnósticos (net_delta fica
    // inerte com typeCheckingMode=off). Se o novo nome já existe como irmão, o rename quebra.
    if let Ok(syms) = document_symbols(&client, &abs) {
        let mut flat = vec![];
        flatten_symbols(&syms, "", &mut flat);
        if let Some(colide) = same_scope_collision(&flat, l, old_last, new_name) {
            return Ok(json!({
                "operation": "rename_symbol", "applied": false, "safe": false,
                "error": "name_collision",
                "detail": format!("'{new_name}' já existe no mesmo escopo ('{colide}') — o rename criaria colisão/shadow e quebraria o código (rede independente de diagnósticos)"),
                "symbol": symbol, "new_name": new_name
            }));
        }
    }

    // GATE de warmup: índice quente ANTES de renomear (senão o WorkspaceEdit é incompleto).
    let (refs, stable, warmup_ms, _polls) = warmup_references(&client, &uri, l, c, None)?;
    if !stable {
        return Ok(json!({
            "applied": false, "error": "index_not_ready",
            "detail": "índice instável; rename abortado para evitar edição parcial destrutiva"
        }));
    }
    let ref_count = refs.len() as u64;

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

    // Bug 1 (relatório rename, C#): o csharp-ls desambigua overload no `references` mas NÃO no
    // `rename` — o WorkspaceEdit vaza pros overloads homônimos, com safe:true silencioso. Como já
    // temos as REFERÊNCIAS do símbolo (warmup), detectamos o over-reach: se o rename toca MUITO mais
    // que as refs do símbolo, provavelmente renomeia homônimos. Torna o bug (upstream) VISÍVEL e, se
    // for claramente over-reach (>= 2x), RECUSA no apply em vez de aplicar errado em silêncio.
    let (_bf, blast_edits, _pf) = summarize_edit(&edit, client.root());
    let over_reach = ref_count > 0 && blast_edits > ref_count;
    if apply && over_reach && blast_edits >= ref_count.saturating_mul(2) {
        return Ok(json!({
            "operation": "rename_symbol", "applied": false, "safe": false, "error": "over_reach",
            "symbol": symbol, "new_name": new_name,
            "references_count": ref_count, "blast_edits": blast_edits,
            "detail": format!("o rename tocaria {blast_edits} edições, mas o símbolo tem só {ref_count} referências — o backend pode estar renomeando homônimos/overloads (bug conhecido do csharp-ls no rename). RECUSADO; revise com find_references e renomeie por posição.")
        }));
    }

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
    result["references_count"] = json!(ref_count);
    if over_reach {
        result["over_reach"] = json!(true);
        result["warning"] = json!(format!(
            "blast_radius ({blast_edits} edições) excede as referências do símbolo ({ref_count}) — possível rename de homônimos/overloads (csharp-ls). REVISE o diff."
        ));
    }
    Ok(result)
}

// pega o edit de um refactoring (codeAction -> resolve se lazy). Faz warmup até aparecerem ações.
fn refactor_edit(
    client: &LspClient,
    uri: &str,
    range: &Value,
    kinds: &[&str],
    prefer_title: Option<&str>,
) -> Result<Value, String> {
    let start = Instant::now();
    let mut actions: Vec<Value> = vec![];
    // N1: aceita VÁRIOS kinds — o nome do refactoring difere por server (vtsls:
    // 'refactor.extract.function'; Dart: 'refactor.extract.method'). Pede todos, pega o 1º que casar.
    while start.elapsed().as_millis() < 10_000 {
        let res = client.request(
            "textDocument/codeAction",
            json!({"textDocument":{"uri":uri},"range":range,"context":{"diagnostics":[],"only":kinds}}),
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
            "nenhum refactoring {kinds:?} disponível nesta posição/seleção"
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
    let backend = refactor_backend(file); // vtsls tem os refactorings de TS
    let mut client = srv.client(project, backend)?;
    let abs = safe_abs(project, file)?;
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
    // N1: pede o kind PAI 'refactor.extract' (hierarquia LSP) — cobre .function (vtsls) e .method
    // (Dart) sem depender do nome exato. Recuperação: se o backend cair, reinicia e tenta 1x.
    let kinds: &[&str] = &["refactor.extract"];
    let edit = match refactor_edit(&client, &uri, &range, kinds, Some("module scope")) {
        Ok(e) => e,
        Err(e) if is_conn_dead(&e) => {
            client = srv.restart_client(project, backend)?;
            client.ensure_open(&abs)?;
            match refactor_edit(&client, &uri, &range, kinds, Some("module scope")) {
                Ok(e) => e,
                Err(e2) => {
                    return Err(format!(
                        "backend '{backend}' fechou a conexão durante extract e falhou após reinício: {e2}"
                    ))
                }
            }
        }
        // N2/N1: se o backend não oferece extract nesta seleção, retorna unsupported GRACIOSO.
        Err(_e) => {
            return Ok(json!({
                "operation": "extract_function",
                "applied": false, "safe": false, "unsupported": true, "error": "extract_unsupported",
                "detail": format!("o backend '{}' ({}) não ofereceu extract nesta seleção — selecione statements completos; extract é garantido em TypeScript (vtsls) e C#", backend, build_lang(file))
            }));
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
    result["operation"] = json!("extract_function");
    Ok(result)
}

fn tool_move_symbol(srv: &Server, a: &Value) -> Result<Value, String> {
    let project = a["project"].as_str().ok_or("faltou 'project'")?;
    let file = a["file"].as_str().ok_or("faltou 'file'")?;
    let symbol = a["symbol"].as_str().ok_or("faltou 'symbol'")?;
    let line = a["line"].as_u64();
    let apply = a["apply"].as_bool().unwrap_or(false);
    let backend = refactor_backend(file);
    let mut client = srv.client(project, backend)?;
    let abs = safe_abs(project, file)?;
    client.ensure_open(&abs)?;
    let (l, c) = resolve_pos(&client, &abs, symbol, line)?;
    let range = json!({"start":{"line":l,"character":c},"end":{"line":l,"character":c}});
    let uri = path_to_uri(&abs);
    // N1: tenta kinds de move (vtsls: refactor.move; Dart: refactor.move.file / refactor.move).
    // Recuperação: se o backend cair no meio, reinicia e tenta 1x.
    let mkinds: &[&str] = &["refactor.move"];
    let edit = match refactor_edit(&client, &uri, &range, mkinds, Some("file")) {
        Ok(e) => e,
        Err(e) if is_conn_dead(&e) => {
            client = srv.restart_client(project, backend)?;
            client.ensure_open(&abs)?;
            match refactor_edit(&client, &uri, &range, mkinds, Some("file")) {
                Ok(e) => e,
                Err(e2) => {
                    return Err(format!(
                        "backend '{backend}' fechou a conexão durante move e falhou após reinício: {e2}"
                    ))
                }
            }
        }
        // N2: se o backend não oferece move-para-arquivo, retorna unsupported GRACIOSO (não erro cru),
        // como a descrição promete.
        Err(_e) => {
            return Ok(json!({
                "operation": "move_symbol", "symbol": symbol,
                "applied": false, "safe": false, "unsupported": true, "error": "move_unsupported",
                "detail": format!("o backend '{}' ({}) não oferece 'mover para novo arquivo' nesta posição — move via novo arquivo é garantido em TypeScript (vtsls)", backend, build_lang(file))
            }));
        }
    };
    // Achado 2 (relatório): "mover para novo arquivo" que NÃO cria arquivo é no-op — alguns backends
    // (ex.: csharp-ls) devolvem uma ação refactor.move trivial. Reporta honestamente em vez de safe:true.
    if creates_from(&edit).is_empty() {
        return Ok(json!({
            "operation": "move_symbol", "symbol": symbol,
            "applied": false, "safe": false, "unsupported": true, "error": "move_no_op",
            "detail": format!("o backend '{backend}' não produziu um 'mover para novo arquivo' (nenhum arquivo criado) — seria no-op. move_symbol via novo arquivo é suportado hoje em TypeScript (vtsls)."),
        }));
    }
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
    let abs = safe_abs(project, file)?;
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
            json!({"name_path": fp, "kind": kind_label(k, &flines, rl), "at": format!("{}:{}", rl + 1, rc + 1)})
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
    let abs = safe_abs(project, file)?;
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

// Backend do workspace_symbols por 'lang' (não há arquivo p/ auto-detectar). ERRA em lang
// desconhecida em vez de cair silenciosamente no tsgo (o bug do relatório Dart: lang="dart"
// virava consulta no servidor de TS → count:0 em silêncio).
fn ws_backend(lang: Option<&str>) -> Result<&'static str, String> {
    match lang {
        None | Some("typescript") | Some("ts") | Some("javascript") | Some("js") => Ok("tsgo"),
        Some("python") | Some("py") => Ok("basedpyright"),
        Some("dart") => Ok("dart"),
        Some("rust") | Some("rs") => Ok("rust-analyzer"),
        Some("csharp") | Some("c#") | Some("cs") => Ok("csharp-ls"),
        Some(other) => Err(format!(
            "lang '{other}' não suportado em workspace_symbols; use: typescript|python|dart|rust|csharp"
        )),
    }
}

fn tool_workspace_symbols(srv: &Server, a: &Value) -> Result<Value, String> {
    let project = a["project"].as_str().ok_or("faltou 'project'")?;
    let query = a["query"].as_str().ok_or("faltou 'query'")?;
    // workspace_symbols opera no projeto inteiro (sem arquivo); backend por 'lang' (default ts).
    let backend = ws_backend(a["lang"].as_str())?;
    // P10: se o projeto não tem fontes da linguagem pedida, retorna RÁPIDO — em vez de subir o LSP
    // e esperar os 60s de warmup pra devolver vazio com msg enganosa de "index_not_ready".
    let exts: &[&str] = match backend {
        "tsgo" => &["ts", "tsx", "js", "jsx", "mts", "cts"],
        "basedpyright" => &["py", "pyi"],
        "dart" => &["dart"],
        "rust-analyzer" => &["rs"],
        "csharp-ls" => &["cs"],
        _ => &[],
    };
    if !exts.is_empty() && !exts.iter().any(|e| find_source_file(project, e).is_some()) {
        return Ok(json!({
            "query": query, "count": 0, "symbols": [],
            "warning": format!("nenhum arquivo-fonte da linguagem no projeto (ext: {}) — verifique o 'lang'", exts.join("/")),
        }));
    }
    let client = srv.client(project, backend)?;
    // P5: no cold index o workspace/symbol estourava timeout SECO. Agora reintenta dentro de um
    // budget (CODE_INTEL_WARMUP_MS, default 60s) e, se não vier, devolve index_not_ready ACIONÁVEL
    // em vez de erro cru. Timeout por request via CODE_INTEL_WS_TIMEOUT_MS (default 10s).
    let per_req: u64 = std::env::var("CODE_INTEL_WS_TIMEOUT_MS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(10_000);
    let budget: u128 = std::env::var("CODE_INTEL_WARMUP_MS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(60_000);
    let start = Instant::now();
    // No cold index, alguns servers (Dart) devolvem [] VÁLIDO enquanto ainda indexam — não é erro.
    // Reintenta tanto em erro quanto em VAZIO até o budget; senão o "silent empty" volta (bug Dart).
    let mut res = json!([]);
    let mut timed_out = false;
    loop {
        match client.request("workspace/symbol", json!({"query": query}), per_req) {
            Ok(r) => {
                let empty = r.as_array().map(|a| a.is_empty()).unwrap_or(true);
                if !empty {
                    res = r;
                    break;
                }
                if start.elapsed().as_millis() >= budget {
                    res = r; // aceita o vazio após o budget (símbolo pode realmente não existir)
                    timed_out = true;
                    break;
                }
                std::thread::sleep(Duration::from_millis(400));
            }
            Err(e) => {
                if start.elapsed().as_millis() >= budget {
                    return Ok(json!({
                        "query": query, "count": 0, "stable": false,
                        "warning": index_not_ready_hint(),
                        "detail": format!("workspace/symbol não respondeu em {budget}ms (cold index): {e}"),
                    }));
                }
                std::thread::sleep(Duration::from_millis(500));
            }
        }
    }
    let root = client.root().to_string();
    // N3: escopo do PROJETO. workspace/symbol de alguns servers (Dart) despeja resultados de
    // dependências (.pub-cache, SDK) e casa substring → ruído. Mantém só o que está DENTRO do
    // projeto e fora de dirs de dependência. project_only=false desliga o filtro.
    let project_only = a["project_only"].as_bool().unwrap_or(true);
    let dep_markers = [
        ".pub-cache",
        "/flutter/",
        "site-packages",
        "node_modules",
        "/.cargo/",
        "/target/",
        "/.venv/",
        ".nuget",
        "/usr/lib/",
        "/usr/share/",
    ];
    let root_prefix = format!("{}/", root.trim_end_matches('/'));
    let in_scope = |uri: &str| -> bool {
        let p = uri_to_path(uri);
        // boundary-aware: '/root2' NÃO conta como dentro de '/root' (gap #2).
        if project_only && p != root && !p.starts_with(&root_prefix) {
            return false;
        }
        !dep_markers.iter().any(|m| p.contains(m))
    };
    let mut dropped = 0u64;
    let list: Vec<Value> = res
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .filter(|s| {
            let keep = in_scope(s["location"]["uri"].as_str().unwrap_or(""));
            if !keep {
                dropped += 1;
            }
            keep
        })
        .map(|s| {
            let (l, c) = sym_pos(s);
            let uri = s["location"]["uri"].as_str().unwrap_or("");
            json!({"name": s["name"].as_str().unwrap_or(""),
                   "kind": kind_name(s["kind"].as_u64().unwrap_or(0)),
                   "at": format!("{}:{}:{}", rel(&root, uri), l + 1, c + 1)})
        })
        .collect();
    // vazio após o budget: sinaliza que PODE ser índice não-pronto (não afirma "não existe").
    let warning = if timed_out {
        json!(format!(
            "resultado vazio após {budget}ms — {}",
            index_not_ready_hint()
        ))
    } else {
        Value::Null
    };
    Ok(
        json!({"query": query, "count": list.len(), "symbols": list, "warning": warning,
              "filtered_out": dropped, "project_only": project_only}),
    )
}

fn tool_call_hierarchy(srv: &Server, a: &Value) -> Result<Value, String> {
    let project = a["project"].as_str().ok_or("faltou 'project'")?;
    let file = a["file"].as_str().ok_or("faltou 'file'")?;
    let symbol = a["symbol"].as_str().ok_or("faltou 'symbol'")?;
    let line = a["line"].as_u64();
    let client = srv.client(project, nav_backend(file))?;
    let abs = safe_abs(project, file)?;
    client.ensure_open(&abs)?;
    client.resync_all_changed(); // Bug 3: freshness cross-file
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
    let mut call_site_total = 0u64;
    let callers: Vec<Value> = incoming
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|call| {
            let from = &call["from"];
            let (fl, fc) = sym_pos(from);
            let uri = from["uri"].as_str().unwrap_or("");
            // P14: o LSP dá `fromRanges` = TODOS os call-sites daquele chamador. Antes só
            // devolvíamos a def do chamador (1), subcontando o blast radius. Agora expõe cada site.
            let sites: Vec<Value> = call["fromRanges"]
                .as_array()
                .map(|rs| {
                    rs.iter()
                        .map(|r| {
                            let sl = r["start"]["line"].as_u64().unwrap_or(0) + 1;
                            let sc = r["start"]["character"].as_u64().unwrap_or(0) + 1;
                            json!(format!("{}:{}:{}", rel(&root, uri), sl, sc))
                        })
                        .collect()
                })
                .unwrap_or_default();
            call_site_total += sites.len().max(1) as u64;
            json!({"caller": from["name"].as_str().unwrap_or(""),
                   "at": format!("{}:{}:{}", rel(&root, uri), fl + 1, fc + 1),
                   "call_sites": sites, "call_site_count": sites.len()})
        })
        .collect();
    Ok(json!({"symbol": symbol, "incoming_count": callers.len(),
              "call_site_count": call_site_total, "incoming": callers}))
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

// Diretórios ignorados na busca de fontes.
fn is_skip_dir(name: &str) -> bool {
    matches!(
        name,
        ".git"
            | ".venv"
            | "venv"
            | "env"
            | "node_modules"
            | "target"
            | "__pycache__"
            | "bin"
            | "obj"
            | ".dart_tool"
            | "dist"
            | "build"
            | "examples"
            | "example"
            | "samples"
            | "tests"
            | "test"
            | "__tests__"
            | "benches"
            | "benchmark"
            | "benchmarks"
            | "e2e"
    )
}

// Arquivos "não-biblioteca" que o smoke deve evitar: playgrounds, exemplos, testes, gerados —
// costumam ter símbolos SEM referências (ex.: zod/play.ts), o que confunde o smoke.
fn is_scratch_name(name: &str) -> bool {
    let n = name.to_lowercase();
    if n.ends_with(".g.dart") || n.ends_with(".freezed.dart") || n.ends_with(".d.ts") {
        return true;
    }
    if n.contains(".test.") || n.contains(".spec.") || n.contains("_test.") || n.contains("_spec.")
    {
        return true;
    }
    let stem = n.split('.').next().unwrap_or(&n);
    matches!(
        stem,
        "play"
            | "playground"
            | "scratch"
            | "demo"
            | "example"
            | "examples"
            | "sample"
            | "samples"
            | "bench"
            | "benchmark"
            | "benchmarks"
            | "conftest"
    )
}

// Coleta até `max` arquivos-fonte da linguagem, pulando build/vcs/test/scratch, PREFERINDO os que
// estão sob src/ ou lib/ (mais provável conter símbolos de biblioteca referenciados).
fn find_source_files(project: &str, ext: &str, max: usize) -> Vec<String> {
    fn walk(dir: &Path, ext: &str, root: &Path, budget: &mut u32, out: &mut Vec<String>) {
        if *budget == 0 {
            return;
        }
        let Ok(rd) = std::fs::read_dir(dir) else {
            return;
        };
        let mut entries: Vec<_> = rd.flatten().collect();
        entries.sort_by_key(|e| e.file_name());
        let mut subdirs = vec![];
        for e in entries {
            if *budget == 0 {
                return;
            }
            *budget -= 1;
            let path = e.path();
            let name = e.file_name();
            let name = name.to_string_lossy();
            if path.is_dir() {
                if !is_skip_dir(name.as_ref()) {
                    subdirs.push(path);
                }
            } else if path.extension().map(|x| x == ext).unwrap_or(false)
                && !is_scratch_name(name.as_ref())
            {
                if let Ok(r) = path.strip_prefix(root) {
                    out.push(r.to_string_lossy().replace('\\', "/"));
                }
            }
        }
        for d in subdirs {
            walk(&d, ext, root, budget, out);
        }
    }
    let root = Path::new(project);
    let mut budget = 20_000u32;
    let mut all = vec![];
    walk(root, ext, root, &mut budget, &mut all);
    // preferência: caminhos sob src/ ou lib/ primeiro (ordenação estável mantém determinismo)
    all.sort_by_key(|p| {
        let pref = p.contains("/src/")
            || p.starts_with("src/")
            || p.contains("/lib/")
            || p.starts_with("lib/");
        !pref // false (=0) antes de true
    });
    all.truncate(max);
    all
}

fn find_source_file(project: &str, ext: &str) -> Option<String> {
    find_source_files(project, ext, 1).into_iter().next()
}

// P2: smoke test END-TO-END — roda um find_references REAL num símbolo descoberto e exige
// count>0 && stable. Pega o que os checks de binário+config NÃO pegam (posição, warmup, escala).
fn doctor_smoke(srv: &Server, project: &str, lang: &str) -> Value {
    let ext = match lang {
        "python" => "py",
        "typescript" => "ts",
        "rust" => "rs",
        "dart" => "dart",
        "csharp" => "cs",
        _ => return json!({"ran": false, "reason": "linguagem sem smoke test"}),
    };
    let files = find_source_files(project, ext, 6);
    if files.is_empty() {
        return json!({"ran": false, "reason": format!("nenhum arquivo .{ext} encontrado")});
    }
    // Teto total do smoke (= budget de warmup); primeira tentativa absorve o cold start, as demais
    // usam budget curto (um símbolo de 0 refs não deve consumir tudo — foi o que o zod/play.ts expôs).
    let total_budget: u128 = std::env::var("CODE_INTEL_WARMUP_MS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(60_000);
    let start = Instant::now();
    let mut attempts = 0u32;
    let mut tried = 0u32;
    let mut last_ident = String::new();
    let mut last_file = String::new();
    for rel_file in &files {
        let Ok(client) = srv.client(project, nav_backend(rel_file)) else {
            continue;
        };
        let abs = format!("{}/{}", project.trim_end_matches('/'), rel_file);
        if client.ensure_open(&abs).is_err() {
            continue;
        }
        let Ok(syms) = document_symbols(&client, &abs) else {
            continue;
        };
        let mut flat = vec![];
        flatten_symbols(&syms, "", &mut flat);
        // símbolos referenciáveis: Class(5), Method(6), Interface(11), Function(12), Struct(23)
        let candidates: Vec<_> = flat
            .iter()
            .filter(|(_, k, ..)| matches!(k, 5 | 6 | 11 | 12 | 23))
            .take(4)
            .cloned()
            .collect();
        let uri = path_to_uri(&abs);
        for (fp, _k, _l, _c) in &candidates {
            let elapsed = start.elapsed().as_millis();
            if elapsed + 2_000 >= total_budget {
                break;
            }
            let ident = base_name(fp.rsplit('/').next().unwrap_or(fp));
            let Ok((rl, rc)) = resolve_pos(&client, &abs, ident, None) else {
                continue;
            };
            // 1ª tentativa: budget generoso (cold start). Demais: curto (rejeita 0-ref rápido).
            let per = if attempts == 0 {
                total_budget.min(90_000)
            } else {
                (total_budget - elapsed).min(15_000)
            };
            attempts += 1;
            tried += 1;
            last_ident = ident.to_string();
            last_file = rel_file.clone();
            let Ok((refs, stable, ms, polls)) = warmup_references(&client, &uri, rl, rc, Some(per))
            else {
                continue;
            };
            if stable && !refs.is_empty() {
                return json!({"ran": true, "ok": true, "symbol": ident, "file": rel_file,
                    "count": refs.len(), "stable": true, "warmup_ms": ms, "polls": polls});
            }
        }
        if start.elapsed().as_millis() + 2_000 >= total_budget {
            break;
        }
    }
    // Nenhum símbolo com referências estáveis — pode ser índice frio OU símbolos-folha nos arquivos
    // testados. Mensagem acionável, distinguindo dos casos "não rodou".
    json!({"ran": true, "ok": false, "symbols_tried": tried, "last_symbol": last_ident,
        "last_file": last_file,
        "issue": format!("nenhum símbolo com referências estáveis em {tried} tentativa(s) — índice pode estar frio (ligue CODE_INTEL_DAEMON=1 e/ou aumente CODE_INTEL_WARMUP_MS) ou os símbolos testados são folha/sem uso")})
}

// Verifica (e opcionalmente corrige com fix=true) o setup por linguagem: language server disponível
// + config de workspace correta (senão as referências saem incompletas — ver docs/LANGUAGE-SETUP.md).
// Com smoke=true, roda também um find_references REAL end-to-end por linguagem (pega o que os checks
// estáticos não pegam — P2 do relatório pachamama).
fn tool_doctor(srv: &Server, a: &Value) -> Result<Value, String> {
    let project = a["project"].as_str().ok_or("faltou 'project'")?;
    let fix = a["fix"].as_bool().unwrap_or(false);
    let smoke = a["smoke"].as_bool().unwrap_or(false);
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

        // P2: só roda o smoke se binário+config estiverem ok (senão o resultado seria óbvio).
        let smoke_res = if smoke && available && cfg_ok {
            doctor_smoke(srv, project, lang)
        } else if smoke {
            json!({"ran": false, "reason": "pré-requisito falhou (server/config)"})
        } else {
            Value::Null
        };

        report.push(json!({
            "lang": lang, "server": server, "server_bin": server_bin,
            "server_available": available,
            "install": if available { Value::Null } else { json!(install_hint) },
            "workspace_config": {"ok": cfg_ok, "issue": issue, "fix": fix_desc, "applied": applied},
            "smoke": smoke_res,
        }));
    }

    let problems = report
        .iter()
        .filter(|e| {
            !e["server_available"].as_bool().unwrap_or(true)
                || !e["workspace_config"]["ok"].as_bool().unwrap_or(true)
                // smoke conta como problema só quando rodou e falhou
                || (e["smoke"]["ran"].as_bool().unwrap_or(false)
                    && !e["smoke"]["ok"].as_bool().unwrap_or(true))
        })
        .count();
    let hint = if problems == 0 && smoke {
        "nenhum problema encontrado (inclui smoke test end-to-end)"
    } else if problems == 0 {
        "nenhum problema na checagem estática; rode com smoke=true p/ o teste end-to-end (find_references real)"
    } else if !smoke {
        "checagem ESTÁTICA (binário+config). Rode com smoke=true p/ o teste end-to-end (find_references real) — pega posição/warmup/escala que os checks estáticos não veem."
    } else if fix {
        "correções aplicadas onde possível; smoke test end-to-end executado"
    } else {
        "smoke test end-to-end executado; rode com fix=true para corrigir configs automaticamente"
    };
    Ok(json!({
        "project": project,
        "languages_detected": langs,
        "problems": problems,
        "report": report,
        "hint": hint,
    }))
}

fn tools_schema() -> Value {
    json!([
        {
            "name": "find_references",
            "description": "Encontra TODAS as referências semânticas a um símbolo (via o language server da linguagem detectada: tsgo p/ TS, basedpyright p/ Python, rust-analyzer, csharp-ls, dart). Aguarda o índice estabilizar (gate de warmup) e sinaliza se o resultado ainda não é confiável. Use isto em vez de grep para rename/delete.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "project": {"type": "string", "description": "caminho ABSOLUTO da raiz do projeto"},
                    "file": {"type": "string", "description": "caminho do arquivo RELATIVO ao project"},
                    "symbol": {"type": "string", "description": "nome do símbolo (ex.: 'ZodType')"},
                    "line": {"type": "integer", "description": "opcional: linha 1-indexed para desambiguar"},
                    "summary": {"type": "boolean", "description": "opcional: true = só {count, files, by_file} (sem cada path:linha:col) — evita estourar o limite de tokens em símbolos muito usados"}
                },
                "required": ["project", "file", "symbol"]
            }
        },
        {
            "name": "rename_symbol",
            "description": "Rename semântico com verificação. Valida new_name (identificador válido, não-keyword) e trata new==old como noop. Detecta COLISÃO de nome no mesmo escopo (rede independente de diagnósticos — pega o caso em que o net_delta fica inerte, ex.: Python typeCheckingMode=off). Gate de warmup + simula a edição EM MEMÓRIA e mede net_delta. apply=false (default) = preview; apply=true persiste só se net_delta<=0. 'symbol' aceita name_path ('Classe/metodo').",
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
            "description": "Busca símbolos por nome em TODO o projeto (workspace/symbol). Informe 'lang' conforme o projeto (typescript default, python, dart, rust, csharp) — lang desconhecida ERRA (não retorna vazio em silêncio). Escopo do PROJETO por default (exclui .pub-cache/SDK/deps); project_only=false inclui tudo.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "project": {"type": "string"}, "query": {"type": "string"},
                    "lang": {"type": "string", "description": "'typescript' (default), 'python', 'dart', 'rust' ou 'csharp'"},
                    "project_only": {"type": "boolean", "description": "default true: só símbolos dentro do projeto (fora de .pub-cache/SDK/node_modules/etc.)"}
                },
                "required": ["project", "query"]
            }
        },
        {
            "name": "call_hierarchy",
            "description": "Quem chama este símbolo (incoming calls), com call_sites (cada chamada via fromRanges) e call_site_count além do incoming_count. 'symbol' aceita name_path.",
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
            "description": "Extrai um intervalo de linhas para uma nova função (escopo do módulo), via o refactoring do language server (vtsls no TS; o próprio server nas demais linguagens). Mesmo ciclo apply→verify com net_delta: apply=false=preview; apply=true persiste só se seguro. Se o backend cair, reinicia e tenta 1x.",
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
            "description": "Move um símbolo (top-level) para um NOVO arquivo, via o refactoring do language server, atualizando os imports. Mesmo ciclo apply→verify com net_delta (cria arquivo). Confiável hoje em TypeScript (vtsls); se o backend não implementar 'mover para novo arquivo' (ex.: csharp-ls), retorna unsupported/move_no_op em vez de fingir sucesso. 'symbol' aceita name_path.",
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
            "description": "Verifica o setup do projeto por linguagem: language server disponível + config de workspace correta (senão find_references sai incompleto EM SILÊNCIO — crítico em Python). Com fix=true, corrige o que dá (ex.: cria pyrightconfig.json). Com smoke=true, roda um find_references REAL end-to-end e exige count>0 && stable (pega posição/warmup/escala que os checks estáticos não veem). Rode uma vez ao abrir um projeto novo.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "project": {"type": "string", "description": "caminho absoluto da raiz do projeto"},
                    "fix": {"type": "boolean", "description": "true = aplica as correções possíveis (escreve configs)"},
                    "smoke": {"type": "boolean", "description": "true = roda um find_references real por linguagem (teste end-to-end); pode demorar no cold start (ligue CODE_INTEL_DAEMON=1)"}
                },
                "required": ["project"]
            }
        },
        {
            "name": "validate_build",
            "description": "Roda o build/check da linguagem NO DISCO e reporta erros. Fecha o buraco do net_delta em memória (ex.: erros que só o `cargo check` do Rust pega). Chame após um apply. Defaults: rust=cargo check, dart=dart analyze, csharp=dotnet build, python=basedpyright. Override via env <LANG>_CHECK_CMD (ex.: PYTHON_CHECK_CMD).",
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

// Handle do daemon que ESTE processo subiu — mantido para reap (evita zumbi <defunct> quando ele
// morre: o proxy é o pai, então precisa dar wait() no filho morto).
#[cfg(unix)]
static DAEMON_CHILD: Mutex<Option<std::process::Child>> = Mutex::new(None);

// Reap do daemon anterior (se morto) e sobe um novo, guardando o handle para reap futuro.
#[cfg(unix)]
fn spawn_daemon() {
    let mut guard = DAEMON_CHILD.lock().unwrap_or_else(|p| p.into_inner());
    if let Some(mut old) = guard.take() {
        let _ = old.kill(); // idempotente se já morto
        let _ = old.wait(); // reap → sem processo <defunct>
    }
    if let Ok(exe) = std::env::current_exe() {
        if let Ok(child) = std::process::Command::new(exe)
            .arg("--daemon")
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
        {
            *guard = Some(child);
        }
    }
}

// Espera o socket do daemon aceitar conexão, até `ms`.
#[cfg(unix)]
fn wait_socket(path: &str, ms: u64) {
    let start = Instant::now();
    while std::os::unix::net::UnixStream::connect(path).is_err()
        && start.elapsed() < Duration::from_millis(ms)
    {
        std::thread::sleep(Duration::from_millis(100));
    }
}

// UMA tentativa de forward: conecta, envia, lê a resposta. Err em qualquer falha de I/O (usado
// para disparar o failover).
#[cfg(unix)]
fn try_forward(path: &str, name: &str, args: &Value) -> Result<Value, String> {
    let mut stream =
        std::os::unix::net::UnixStream::connect(path).map_err(|e| format!("connect: {e}"))?;
    let req = json!({"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":name,"arguments":args}});
    writeln!(stream, "{}", serde_json::to_string(&req).unwrap())
        .map_err(|e| format!("write: {e}"))?;
    stream.flush().ok();
    let mut reader = std::io::BufReader::new(stream);
    let mut line = String::new();
    let n = reader
        .read_line(&mut line)
        .map_err(|e| format!("read: {e}"))?;
    if n == 0 {
        return Err("conexão fechada pelo daemon (EOF)".into());
    }
    let v: Value = serde_json::from_str(&line).map_err(|e| format!("parse: {e}"))?;
    v.get("result")
        .cloned()
        .ok_or_else(|| "resposta sem 'result'".into())
}

// no MCP: encaminha um tools/call ao daemon (sobe se necessário) com FAILOVER — se a conexão
// quebrar (daemon morto no meio), respawna (reapando o zumbi) e tenta MAIS UMA vez antes de errar.
#[cfg(unix)]
fn forward_call(name: &str, args: &Value) -> Value {
    let path = sock_path();
    if std::os::unix::net::UnixStream::connect(&path).is_err() {
        spawn_daemon();
        wait_socket(&path, 5000);
    }
    match try_forward(&path, name, args) {
        Ok(v) => v,
        Err(_) => {
            // daemon indisponível/morto → failover: respawna e tenta 1x
            spawn_daemon();
            wait_socket(&path, 8000);
            match try_forward(&path, name, args) {
                Ok(v) => v,
                Err(e) => json!({
                    "content": [{"type":"text","text": format!("ERRO: daemon indisponível após failover: {e}")}],
                    "isError": true
                }),
            }
        }
    }
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

    // Relatório extract/move: detecção de backend morto (dispara restart+retry do refactoring).
    #[test]
    fn is_conn_dead_detects_broken_pipe() {
        assert!(is_conn_dead("Broken pipe (os error 32)"));
        assert!(is_conn_dead("write: Broken pipe"));
        assert!(!is_conn_dead(
            "nenhum refactoring 'refactor.move' disponível nesta posição/seleção"
        ));
        assert!(!is_conn_dead(
            "timeout (10000ms) em textDocument/codeAction"
        ));
    }

    // Relatório suite-completa: validate_build C# dava build_ok:false num build VERDE porque o
    // filtro casava a linha de resumo "0 Error(s)". O parser deve ignorá-la e pegar erros reais.
    #[test]
    fn parse_build_errors_ignores_summary_line() {
        let green = "Build succeeded.\n    0 Warning(s)\n    0 Error(s)\nTime Elapsed 00:00:08";
        assert!(parse_build_errors(green).is_empty());
        let broken = "Handler.cs(12,5): error CS1002: ; expected\n    1 Error(s)";
        let e = parse_build_errors(broken);
        assert_eq!(e.len(), 1);
        assert!(e[0].contains("CS1002"));
        // cargo/rust-style
        let rustish = "error[E0308]: mismatched types\n  --> src/x.rs:3:5";
        assert_eq!(parse_build_errors(rustish).len(), 1);
        // P13: resumo do basedpyright num build VERDE não é erro
        let pyright_green = "/x/models.py\n0 errors, 0 warnings, 0 notes ";
        assert!(parse_build_errors(pyright_green).is_empty());
        // basedpyright com erro real: a linha do erro conta, o resumo "1 error" não
        let pyright_bad =
            "models.py:12:5 - error: \"x\" is not defined\n1 error, 0 warnings, 0 notes";
        assert_eq!(parse_build_errors(pyright_bad).len(), 1);
    }

    // Relatório suite-completa: record (não-struct) rotulado Class. kind_label relabela p/ Record.
    #[test]
    fn kind_label_relabels_record() {
        let lines = vec![
            "public sealed record ProtectedCpf(string Value)",
            "public class Handler",
            "public readonly record struct Tn",
        ];
        assert_eq!(kind_label(5, &lines, 0), "Record"); // record de classe
        assert_eq!(kind_label(5, &lines, 1), "Class"); // class comum
        assert_eq!(kind_label(23, &lines, 2), "Struct"); // record struct já é kind Struct
        assert_eq!(kind_label(6, &lines, 1), "Method"); // não-Class inalterado
    }

    // Gap #2 da pesquisa: URI percent-encode (espaço/acento) com roundtrip, e safe_abs contra traversal.
    #[test]
    fn uri_roundtrip_percent() {
        let p = "/home/u/my project/café.rs";
        let uri = path_to_uri(p);
        assert!(uri.contains("my%20project"));
        assert_eq!(uri_to_path(&uri), p);
    }

    #[test]
    fn safe_abs_rejects_traversal() {
        assert!(safe_abs("/tmp", "../etc/passwd").is_err());
        assert!(safe_abs("/tmp", "a/../../etc").is_err());
        assert!(safe_abs("/tmp", "sub/ok.txt").is_ok());
        assert!(safe_abs("/tmp", "sub/../ok.txt").is_ok());
    }

    // Achado #1 da pesquisa: coluna LSP é UTF-16, não byte. Unicode antes do símbolo desloca.
    #[test]
    fn utf16_col_handles_unicode() {
        let row = "café x"; // 'é' = 2 bytes / 1 unidade UTF-16
        assert_eq!(utf16_col(row, row.find('x').unwrap()), 5);
        let row2 = "🚀ab"; // 🚀 = 4 bytes / 2 unidades UTF-16
        assert_eq!(utf16_col(row2, row2.find('a').unwrap()), 2);
        assert_eq!(utf16_col("hello world", 6), 6); // ASCII: byte == utf16
    }

    // Bug 2 (relatório rename): locate deve casar IDENTIFICADOR COMPLETO, não substring —
    // "Result" NÃO pode casar dentro de "RefundResult".
    #[test]
    fn find_ident_whole_word_not_substring() {
        let row = "    RefundResult Result, Guid? RefundId";
        assert_eq!(find_ident(row, "Result"), Some(17)); // o parâmetro, não o tipo
        assert_eq!(find_ident(row, "RefundResult"), Some(4)); // o tipo, inteiro
        assert_eq!(find_ident("foobar baz", "foo"), None); // sem match inteiro
        assert_eq!(find_ident("a.render()", "render"), Some(2)); // após '.' é boundary
        assert_eq!(find_ident("value_id = 1", "value"), None); // 'value' dentro de 'value_id'
    }

    // P9: Python passa a ter comando de build/check default (antes: no-op silencioso).
    #[test]
    fn build_cmd_python_has_default() {
        assert!(build_cmd("python").is_some());
        assert!(build_cmd("rust").is_some());
        assert!(build_cmd("dart").is_some());
        assert!(build_cmd("csharp").is_some());
    }

    // P12: validação de new_name.
    #[test]
    fn validate_new_name_rules() {
        assert!(validate_new_name("Gadget").is_ok());
        assert!(validate_new_name("_private2").is_ok());
        assert!(validate_new_name("").is_err());
        assert!(validate_new_name("2foo").is_err()); // começa com dígito
        assert!(validate_new_name("has space").is_err());
        assert!(validate_new_name("has-dash").is_err());
        assert!(validate_new_name("class").is_err()); // keyword
        assert!(validate_new_name("return").is_err());
    }

    // P8: colisão de nome no mesmo escopo (independe de diagnósticos).
    #[test]
    fn same_scope_collision_detects_sibling() {
        // ResponseModel tem for_human (linha 21) e from_exception (linha 32), ambos métodos.
        let flat = vec![
            ("ResponseModel".to_string(), 5u64, 15, 7),
            ("ResponseModel/for_human".to_string(), 6, 21, 9),
            ("ResponseModel/from_exception".to_string(), 6, 32, 9),
        ];
        // renomear for_human -> from_exception COLIDE (irmão já existe)
        assert_eq!(
            same_scope_collision(&flat, 21, "for_human", "from_exception").as_deref(),
            Some("ResponseModel/from_exception")
        );
        // renomear for_human -> for_display NÃO colide
        assert_eq!(
            same_scope_collision(&flat, 21, "for_human", "for_display"),
            None
        );
        // colisão só no MESMO escopo: um método homônimo em OUTRA classe não conta
        let flat2 = vec![
            ("A/foo".to_string(), 6u64, 3, 5),
            ("B/bar".to_string(), 6, 10, 5),
        ];
        assert_eq!(same_scope_collision(&flat2, 3, "foo", "bar"), None);
    }

    // Relatório e2e: servers ACHATADOS sem containerName (csharp-ls) → fp sem classe. A query
    // composta deve casar por último segmento (best-effort), mas hierarquia mantém precisão.
    #[test]
    fn name_path_matches_flat_server_composite_query() {
        assert!(name_path_matches("DoWork(int x)", "Handler/DoWork"));
        assert!(name_path_matches("DoWork", "Handler/DoWork"));
        // com hierarquia (containerName), a classe importa:
        assert!(!name_path_matches("Gadget/render", "Widget/render"));
        assert!(name_path_matches("Widget/render", "Widget/render"));
    }

    // Regressão do relatório Dart: workspace_symbols roteava lang!=python p/ tsgo em silêncio.
    #[test]
    fn ws_backend_routes_all_languages() {
        assert_eq!(ws_backend(Some("dart")).unwrap(), "dart");
        assert_eq!(ws_backend(Some("python")).unwrap(), "basedpyright");
        assert_eq!(ws_backend(Some("rust")).unwrap(), "rust-analyzer");
        assert_eq!(ws_backend(Some("csharp")).unwrap(), "csharp-ls");
        assert_eq!(ws_backend(Some("typescript")).unwrap(), "tsgo");
        assert_eq!(ws_backend(None).unwrap(), "tsgo");
        // lang desconhecida ERRA (não cai silenciosamente no tsgo)
        assert!(ws_backend(Some("cobol")).is_err());
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
