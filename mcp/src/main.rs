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

// ---- LOG DE ERROS LOCAL (sem rede/telemetria) ---------------------------
// Registra falhas de tool + panics num arquivo JSONL, para DESCOBRIR erros na máquina do usuário
// sem depender de relatório manual. Path via CODE_INTEL_LOG (="off" desliga); default
// ~/.cache/code-intel-mcp/errors.jsonl. Best-effort: nunca afeta a operação.
fn log_file_path() -> Option<std::path::PathBuf> {
    match std::env::var("CODE_INTEL_LOG") {
        Ok(v) if v.eq_ignore_ascii_case("off") => None,
        Ok(v) if !v.is_empty() => Some(std::path::PathBuf::from(v)),
        _ => {
            let home = std::env::var("HOME").unwrap_or_else(|_| "/tmp".into());
            let base = std::env::var("XDG_CACHE_HOME")
                .ok()
                .filter(|s| !s.is_empty())
                .unwrap_or_else(|| format!("{home}/.cache"));
            Some(
                std::path::PathBuf::from(base)
                    .join("code-intel-mcp")
                    .join("errors.jsonl"),
            )
        }
    }
}

fn now_ms() -> u128 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0)
}

// ---- G1: GUIDANCE PORTÁTIL (server-side) — FONTE ÚNICA DA VERDADE --------
// A orientação de uso vive AQUI, no server, não na skill (que é específica do Claude Code). Assim
// TODO cliente MCP (Codex/Cursor/Cline/Zed…) herda a mesma utilização. Dois canais, uma fonte:
//  - `FULL_MANUAL`: o manual completo, servido sob demanda pela tool `instructions` (C11: fonte única).
//  - `SHORT_INSTRUCTIONS`: um EXCERTO ENXUTO derivado do mesmo manual, colocado no campo
//    `instructions` do `initialize` (enviado toda sessão → mantido curto, C13: bloat expulsa contexto).
// C12: é guidance (texto + notas), NUNCA bloqueio duro de workflow — as edit-tools seguem com
// apply=false default (preview numa chamada só).
mod guidance {
    // Manual COMPLETO (tool `instructions`). É a fonte da verdade; o SHORT é um recorte dele.
    pub const FULL_MANUAL: &str = r#"# code-intel — manual de uso (IDE na mão da LLM)

Este MCP dá operações SEMÂNTICAS de código (via language server), rápidas e com edição VERIFICADA.
Você decide O QUÊ; a tool faz a operação mecânica; o validador CONFERE (net_delta / build).

## 1. Roteamento: semântico vs. grep
- Para qualquer coisa sobre SÍMBOLOS (achar, contar refs, renomear, mover, extrair, deletar, mudar
  assinatura), use as tools do code-intel — NÃO grep/sed. Grep textual corrompe strings, comentários
  e homônimos, e muitas vezes AINDA compila (bug silencioso).
- Reserve grep/Grep para TEXTO LITERAL: uma string, um trecho de comentário, uma chave de config.
- find_symbol / document_symbols resolvem "método dentro de classe" por name_path ('Classe/metodo')
  — não chute linha por texto.

## 2. Confie no resultado (não releia para "conferir")
- As tools passam por um GATE DE WARMUP: nunca devolvem uma contagem parcial durante a indexação.
  Se o índice ainda aquece, você recebe um ERRO (index_not_ready) — não um resultado falso.
- Confirme que find_references veio `stable: true`. Vindo estável, NÃO releia os arquivos só para
  confirmar referências de código — a tool já garantiu o alcance.
- Contexto enxuto: prefira document_symbols → ler só o corpo do símbolo-alvo → expandir via
  find_references/call_hierarchy sob demanda; não "engula" o arquivo inteiro.

## 3. Edite com preview → simulação → apply
- As edit-tools têm apply=false por DEFAULT: uma chamada já te dá o PREVIEW (não toca o disco).
- Antes de um apply arriscado, use simulate_edit (roda net_delta em memória: erros introduzidos/
  resolvidos, veredito safe/unsafe) e/ou preview_edit (WorkspaceEdit + diff). safe_apply só persiste
  se net_delta<=0.
- Em Rust (ou quando quiser garantia de build), passe verify_build:true — aplica só se o build no
  disco passar; senão REVERTE. Fecha o buraco que a simulação em memória não vê (ex.: cargo check).

## 4. Meça o risco antes de uma edição ampla
- blast_radius (READ-ONLY, composto sobre find_references + call_hierarchy) mostra a superfície de
  risco — refs e chamadores particionados test vs. produção — ANTES de mexer num símbolo muito usado.
- change_signature atualiza declaração + TODOS os call-sites juntos (simula antes de aplicar).

## 5. Delete com segurança, não às cegas
- safe_delete apaga um símbolo APENAS se ele não tiver referência externa; se houver USO, RECUSA e
  lista quem referencia. Não delete texto à mão.

## 6. Varredura textual APÓS o rename (grep-sweep)
- find_references IGNORA de propósito ocorrências do nome em comentários/strings/docstrings/docs
  (.md)/config. Depois de um rename_symbol semântico, faça UMA varredura textual do nome ANTIGO só
  nesses lugares não-código e PERGUNTE antes de tocar (podem ser intencionais: changelog, histórico).
- Isso NÃO contradiz o item 2: código = confie no semântico; texto não-código = único alvo do grep.

## 7. Loop de diagnostics (nível-projeto)
- A tool já garante o build da EDIÇÃO (net_delta / verify_build). O loop de projeto é SEU: rodar
  validate_build / a suíte → ler diagnostics estruturados de OUTROS arquivos → corrigir a causa (via
  tool semântica, não Edit textual) → re-checar até limpo. Nunca silencie diagnostic (any/ignore/
  allow) para "fechar o loop".

## 8. Setup
- Rode `doctor` ao abrir um projeto novo: valida language server + config de workspace (sem ela,
  find_references pode sair incompleto EM SILÊNCIO — crítico em Python). doctor smoke=true roda um
  find_references real e alerta se a contagem parece baixa demais.
"#;

    // Excerto CURTO para o campo `instructions` do initialize (C13). Mesma fonte (FULL_MANUAL);
    // são as regras de ouro. Chame a tool `instructions` para o manual completo.
    pub const SHORT_INSTRUCTIONS: &str = r#"code-intel: operações SEMÂNTICAS de código com edição VERIFICADA (você decide o quê; a tool confere via net_delta/build). Regras de ouro:
- SÍMBOLOS (achar/contar refs/renomear/mover/extrair/deletar/mudar assinatura) → use as tools do code-intel, NUNCA grep/sed (grep corrompe strings/comentários/homônimos). Grep só para TEXTO LITERAL.
- Confie no resultado: as tools passam por um gate de warmup e devolvem ERRO (não contagem parcial) se o índice aquece. find_references estável (stable:true) → NÃO releia arquivos só para conferir.
- Edite com preview: as edit-tools têm apply=false por default (uma chamada já é preview). Antes de um apply arriscado, use simulate_edit (net_delta) e safe_apply (só aplica se net_delta<=0). Em Rust, verify_build:true (aplica só se o build passar; senão reverte).
- Antes de uma edição AMPLA, rode blast_radius (refs+callers, test vs. produção). Para apagar, use safe_delete (recusa se houver uso), nunca delete às cegas.
Chame a tool `instructions` para o manual completo (roteamento, grep-sweep pós-rename, loop de diagnostics, setup via doctor)."#;
}

fn log_event(kind: &str, tool: &str, args: &Value, msg: &str) {
    let Some(path) = log_file_path() else {
        return;
    };
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    let line = json!({
        "ts": now_ms(), "kind": kind, "tool": tool,
        "version": env!("CARGO_PKG_VERSION"), "msg": msg, "args": args,
    });
    if let Ok(mut f) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
    {
        let _ = writeln!(f, "{}", serde_json::to_string(&line).unwrap_or_default());
    }
}

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

// ---- I1: CONTRATO DE SAÍDA `path:line:content` + contexto ---------------
// Uma tool que devolve LOCALIZAÇÕES deve dar ao modelo `path:line:content` (a linha exata do
// código) + ~2 linhas de contexto acima/abaixo — para reduzir re-leituras de arquivo (medido: 15,2
// → 3,2 por tarefa). NUNCA devolvemos o payload LSP cru. Este helper é o ÚNICO ponto de formatação
// (compartilhado por find_references/find_symbol/workspace_symbols/document_symbols/call_hierarchy).

// Linhas de contexto (acima e abaixo) que acompanham cada localização.
const CONTEXT_LINES: usize = 2;

// Cache de conteúdo de arquivo por caminho ABSOLUTO. find_references pode devolver dezenas de
// locais no MESMO arquivo; sem cache reliríamos o disco por local. Split por '\n' UMA vez.
#[derive(Default)]
struct SourceCache {
    files: HashMap<String, Vec<String>>,
}

impl SourceCache {
    // Linhas do arquivo (lidas + memoizadas). Vazio se o arquivo não pôde ser lido.
    fn lines(&mut self, abs: &str) -> &Vec<String> {
        self.files.entry(abs.to_string()).or_insert_with(|| {
            std::fs::read_to_string(abs)
                .map(|t| t.split('\n').map(|s| s.to_string()).collect())
                .unwrap_or_default()
        })
    }
}

// Trunca uma linha em no MÁXIMO `max` chars, respeitando BORDA UTF-8 (nunca corta no meio de um
// char). Sufixo '…' quando truncada. Evita despejar linhas gigantes (minificadas/geradas) no
// contexto — mantém o payload enxuto sem quebrar unicode.
fn clip_line(line: &str, max: usize) -> String {
    if line.chars().count() <= max {
        return line.to_string();
    }
    let mut s: String = line.chars().take(max).collect();
    s.push('…');
    s
}

// Comprimento máximo (em chars) de uma linha de código/contexto reportada.
const MAX_LINE_CHARS: usize = 200;

// Constrói o objeto de localização padrão para `line0` (0-indexed) no arquivo `abs`:
//   { "at": "rel/path.ts:LINHA:COL", "content": "<a linha>", "context": ["<±2 linhas>", ...] }
// `col0` é a coluna 0-indexed (opcional; vira 1-indexed no `at`). As linhas de contexto vêm
// prefixadas com o número da linha ("42: código") para o modelo ancorar sem re-ler o arquivo.
// UTF-8-safe (clip_line respeita a borda de char); clampa nos limites do arquivo.
fn format_location(cache: &mut SourceCache, root: &str, abs: &str, line0: u64, col0: u64) -> Value {
    let lines = cache.lines(abs);
    let idx = line0 as usize;
    let content = lines.get(idx).map(|s| clip_line(s, MAX_LINE_CHARS));
    // janela de contexto [idx-CONTEXT_LINES, idx+CONTEXT_LINES], clampada nos limites do arquivo,
    // EXCLUINDO a própria linha (já está em `content`) — cada item prefixado com o nº da linha.
    let start = idx.saturating_sub(CONTEXT_LINES);
    let end = (idx + CONTEXT_LINES + 1).min(lines.len());
    let mut context: Vec<Value> = vec![];
    for (i, l) in lines
        .iter()
        .enumerate()
        .take(end)
        .skip(start)
        .filter(|(i, _)| *i != idx)
    {
        context.push(json!(format!(
            "{}: {}",
            i + 1,
            clip_line(l, MAX_LINE_CHARS)
        )));
    }
    let rel_path = rel(root, &path_to_uri(abs));
    json!({
        "at": format!("{}:{}:{}", rel_path, line0 + 1, col0 + 1),
        "content": content,
        "context": context,
    })
}

// Igual a format_location, mas partindo de uma URI LSP (converte p/ path absoluto). Conveniência
// para as tools que recebem `uri` do server (find_references, workspace_symbols, call_hierarchy).
fn format_location_uri(
    cache: &mut SourceCache,
    root: &str,
    uri: &str,
    line0: u64,
    col0: u64,
) -> Value {
    format_location(cache, root, &uri_to_path(uri), line0, col0)
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

// Range COMPLETO da declaração do símbolo ((sl,sc),(el,ec)) — o `range` (não `selectionRange`, que
// é só o identificador). Usado pelo safe_delete para apagar a declaração inteira. Fallback p/
// selectionRange/location quando o server não dá `range`.
fn sym_full_range(s: &Value) -> ((u64, u64), (u64, u64)) {
    let r = if s.get("range").is_some() {
        &s["range"]
    } else if s.get("location").is_some() {
        &s["location"]["range"]
    } else {
        &s["selectionRange"]
    };
    (
        (
            r["start"]["line"].as_u64().unwrap_or(0),
            r["start"]["character"].as_u64().unwrap_or(0),
        ),
        (
            r["end"]["line"].as_u64().unwrap_or(0),
            r["end"]["character"].as_u64().unwrap_or(0),
        ),
    )
}

// Achata a árvore de documentSymbol em (name_path, full_range) — o range COMPLETO da declaração
// (via sym_full_range). Compartilhado por safe_delete (apagar a decl inteira) e pelas edições por
// símbolo F4 (replace_symbol_body/insert_before/after). Mesma reconstrução de name_path do
// flatten_symbols (usa containerName no formato achatado do tsgo/csharp-ls).
fn flatten_ranges(
    symbols: &[Value],
    prefix: &str,
    out: &mut Vec<(String, ((u64, u64), (u64, u64)))>,
) {
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
        out.push((fp.clone(), sym_full_range(s)));
        if let Some(children) = s["children"].as_array() {
            flatten_ranges(children, &fp, out);
        }
    }
}

// Resolve a DECLARAÇÃO de um símbolo (por nome/name_path) para seu range completo ((sl,sc),(el,ec)).
// Prefere o símbolo cujo range CONTÉM a posição resolvida (l,c) — desambigua homônimos —; senão o
// 1º com o nome-base. Usado por F4 para editar por símbolo sem coordenadas cruas. Erro honesto se
// o símbolo não existe no documentSymbol.
fn find_decl_range(
    client: &LspClient,
    abs: &str,
    name_path: &str,
    l: u64,
) -> Result<((u64, u64), (u64, u64)), String> {
    let last = base_name(name_path.rsplit('/').next().unwrap_or(name_path));
    let syms = document_symbols(client, abs)?;
    let mut flat: Vec<(String, ((u64, u64), (u64, u64)))> = vec![];
    flatten_ranges(&syms, "", &mut flat);
    flat.iter()
        .filter(|(fp, r)| {
            base_name(fp.rsplit('/').next().unwrap_or(fp)) == last && ref_in_def(l, *r)
        })
        .min_by_key(|(_, ((sl, _), (el, _)))| el.saturating_sub(*sl))
        .map(|(_, r)| *r)
        .or_else(|| {
            flat.iter()
                .find(|(fp, _)| base_name(fp.rsplit('/').next().unwrap_or(fp)) == last)
                .map(|(_, r)| *r)
        })
        .ok_or_else(|| format!("símbolo '{name_path}' não encontrado em {abs} (documentSymbol)"))
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

// Inverso de pos_to_offset: byte-offset → (linha, coluna 0-indexed em UNIDADES UTF-16, como o LSP).
// Usado por change_signature (F5) para converter os spans de bytes calculados sobre o texto de volta
// em Position LSP. UTF-16 (não char count) para casar com a coluna que o resto do código usa.
fn offset_to_pos(text: &str, off: usize) -> (u64, u64) {
    let mut line = 0u64;
    let mut line_start = 0usize;
    for (i, ch) in text.char_indices() {
        if i >= off {
            break;
        }
        if ch == '\n' {
            line += 1;
            line_start = i + 1;
        }
    }
    let col = utf16_col(&text[line_start..off.min(text.len())], off - line_start);
    (line, col)
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

// ---- F1/C1: NÚCLEO ÚNICO de simulação + apply ---------------------------
// `verify_and_apply` (acima) é o ÚNICO caminho que simula em memória, mede net_delta e aplica/
// reverte. Para deixar o contrato explícito (e garantir que NINGUÉM abra um caminho de apply
// paralelo — C1), expomos dois pontos de entrada nomeados sobre ele. rename/extract/move/
// safe_delete/organize_imports E as tools novas (simulate_edit/preview_edit/safe_apply) chamam
// SEMPRE um destes (ou verify_and_apply direto com o `apply` em runtime) — nunca escrevem no disco
// por conta própria.

// SIMULA em memória: mede net_delta e SEMPRE reverte (nunca toca o disco). Usado por preview_edit,
// simulate_edit e por todo preview (apply=false) das demais tools.
fn simulate(client: &LspClient, edit: &Value, project: &str, lang: &str) -> Result<Value, String> {
    verify_and_apply(client, edit, false, false, project, lang)
}

// APLICA no disco SÓ SE net_delta<=0 (mesma simulação, depois persiste; com verify_build opcional
// roda o build e reverte se falhar). Usado por safe_apply e por todo apply=true das demais tools.
fn apply_if_safe(
    client: &LspClient,
    edit: &Value,
    verify_build: bool,
    project: &str,
    lang: &str,
) -> Result<Value, String> {
    verify_and_apply(client, edit, true, verify_build, project, lang)
}

// Constrói um WorkspaceEdit (o MESMO shape que o engine já consome — `changes` por URI) a partir da
// representação de edição que o agente fornece, para UM arquivo `abs`. Aceita, em ordem:
//  - `edit`/`workspace_edit`: um WorkspaceEdit LSP CRU (changes/documentChanges) — repassado como está;
//  - `new_content`: o conteúdo COMPLETO proposto do arquivo → vira um único edit que substitui o arquivo;
//  - `edits`: lista de {start_line,end_line (1-indexed), start_col,end_col (0-indexed, opcionais),
//             new_text} → convertida em TextEdits LSP (0-indexed).
// Assim o agente edita por conteúdo/range sem precisar montar o WorkspaceEdit à mão, mas o núcleo
// recebe exatamente a estrutura que rename/extract/move já produzem.
fn build_workspace_edit(a: &Value, abs: &str, uri: &str) -> Result<Value, String> {
    // (1) WorkspaceEdit cru — repassa direto (mesma representação interna).
    for key in ["edit", "workspace_edit"] {
        if let Some(e) = a.get(key).filter(|e| !e.is_null()) {
            if e.get("changes").is_some() || e.get("documentChanges").is_some() {
                return Ok(e.clone());
            }
        }
    }
    // (2) new_content: substitui o arquivo inteiro por um único TextEdit cobrindo todo o texto atual.
    if let Some(nc) = a.get("new_content").and_then(|v| v.as_str()) {
        let orig = std::fs::read_to_string(abs).unwrap_or_default();
        let lines: Vec<&str> = orig.split('\n').collect();
        let end_line = lines.len().saturating_sub(1) as u64;
        let end_col = lines.last().map(|l| l.chars().count() as u64).unwrap_or(0);
        let range =
            json!({"start":{"line":0,"character":0},"end":{"line":end_line,"character":end_col}});
        return Ok(json!({"changes": {uri: [{"range": range, "newText": nc}]}}));
    }
    // (3) edits: ranges 1-indexed (humano) → TextEdits LSP (0-indexed). Padrões pensados p/ o caso
    // comum "substituir estas linhas": start_col omitido = 0 (início da linha); end_col omitido =
    // FIM da end_line (substitui a linha inteira, não insere no começo). Assim {start_line,end_line,
    // new_text} vira um replace de bloco de linhas — o que o agente quase sempre quer.
    if let Some(arr) = a.get("edits").and_then(|v| v.as_array()) {
        if arr.is_empty() {
            return Err("'edits' vazio".into());
        }
        let orig = std::fs::read_to_string(abs).unwrap_or_default();
        let flines: Vec<&str> = orig.split('\n').collect();
        let mut tes = vec![];
        for e in arr {
            let sl = e["start_line"]
                .as_u64()
                .ok_or("edit sem 'start_line' (1-indexed)")?;
            let el = e["end_line"].as_u64().unwrap_or(sl);
            let sc = e["start_col"].as_u64().unwrap_or(0);
            // end_col ausente → fim (em chars) da end_line no disco = "substitui a linha inteira".
            let ec = e["end_col"].as_u64().unwrap_or_else(|| {
                flines
                    .get((el.saturating_sub(1)) as usize)
                    .map(|l| l.chars().count() as u64)
                    .unwrap_or(0)
            });
            let nt = e["new_text"].as_str().unwrap_or("");
            tes.push(json!({
                "range": {"start":{"line":sl.saturating_sub(1),"character":sc},
                          "end":{"line":el.saturating_sub(1),"character":ec}},
                "newText": nt
            }));
        }
        return Ok(json!({"changes": {uri: tes}}));
    }
    Err("informe a edição proposta via 'edit' (WorkspaceEdit), 'new_content' (arquivo inteiro) ou 'edits' (lista de ranges 1-indexed)".into())
}

// Diff unificado MÍNIMO (por linha) entre `old` e `new` de UM arquivo — apenas para o preview.
// Não é um diff LCS completo: emite um hunk simples (linhas removidas '-' seguidas das adicionadas
// '+') sobre o intervalo que difere no início/fim. Suficiente para o agente VER a mudança sem reabrir
// o arquivo; o WorkspaceEdit exato acompanha o preview para a verdade-fonte.
fn unified_diff(rel_path: &str, old: &str, new: &str) -> Vec<String> {
    if old == new {
        return vec![];
    }
    let a: Vec<&str> = old.split('\n').collect();
    let b: Vec<&str> = new.split('\n').collect();
    // prefixo comum
    let mut pre = 0usize;
    while pre < a.len() && pre < b.len() && a[pre] == b[pre] {
        pre += 1;
    }
    // sufixo comum (sem invadir o prefixo)
    let mut suf = 0usize;
    while suf < a.len() - pre && suf < b.len() - pre && a[a.len() - 1 - suf] == b[b.len() - 1 - suf]
    {
        suf += 1;
    }
    let mut out = vec![format!("--- {rel_path}"), format!("+++ {rel_path}")];
    out.push(format!(
        "@@ -{},{} +{},{} @@",
        pre + 1,
        a.len().saturating_sub(pre + suf),
        pre + 1,
        b.len().saturating_sub(pre + suf)
    ));
    for l in &a[pre..a.len() - suf] {
        out.push(format!("-{l}"));
    }
    for l in &b[pre..b.len() - suf] {
        out.push(format!("+{l}"));
    }
    out.truncate(200); // não estoura tokens em arquivos gerados/minificados
    out
}

// Resolve project/file/edit comuns às 3 tools de F1 e monta (client, abs, uri, edit). O backend é o
// de NAVEGAÇÃO (nav_backend): a edição vem pronta do agente (não é um refactoring do server), então
// só precisamos de um server que faça diagnostics do arquivo — o mesmo que já mede net_delta.
fn resolve_f1_edit<'s>(
    srv: &'s Server,
    a: &Value,
) -> Result<(Arc<LspClient>, String, String, Value, String), String> {
    let project = a["project"].as_str().ok_or("faltou 'project'")?;
    let file = a["file"].as_str().ok_or("faltou 'file'")?;
    let client = srv.client(project, nav_backend(file))?;
    let abs = safe_abs(project, file)?;
    let uri = path_to_uri(&abs);
    let edit = build_workspace_edit(a, &abs, &uri)?;
    Ok((client, abs, uri, edit, project.to_string()))
}

// F1 · simulate_edit — roda net_delta EM MEMÓRIA sobre a edição proposta, SEM tocar o disco.
// Retorna erros introduzidos/resolvidos + veredito safe/unsafe. Passa pelo mesmo núcleo (simulate →
// verify_and_apply(apply=false)), então o resultado é idêntico ao que safe_apply usaria para decidir.
fn tool_simulate_edit(srv: &Server, a: &Value) -> Result<Value, String> {
    let file = a["file"].as_str().unwrap_or("");
    let (client, abs, _uri, edit, project) = resolve_f1_edit(srv, a)?;
    // Freshness: reflete mudanças externas antes de simular (mesmo cuidado das demais tools).
    client.ensure_open(&abs)?;
    client.resync_all_changed();
    let mut result = simulate(&client, &edit, &project, build_lang(file))?;
    result["operation"] = json!("simulate_edit");
    result["file"] = json!(file);
    result["verdict"] = json!(if result["safe"].as_bool().unwrap_or(false) {
        "safe"
    } else {
        "unsafe"
    });
    Ok(result)
}

// F1 · preview_edit — mostra o que a edição MUDARIA: o WorkspaceEdit resolvido + um diff unificado +
// um resumo de blast (arquivos/edições). Read-only (não simula diagnostics nem toca o disco): é o
// "veja o diff antes"; use simulate_edit para o veredito de segurança e safe_apply para aplicar.
fn tool_preview_edit(srv: &Server, a: &Value) -> Result<Value, String> {
    let file = a["file"].as_str().unwrap_or("");
    let (client, _abs, _uri, edit, _project) = resolve_f1_edit(srv, a)?;
    let root = client.root().to_string();
    let by_file = edits_by_file(&edit);
    let (files_n, edits_n, per_file) = summarize_edit(&edit, &root);
    // Constrói o diff por arquivo aplicando os TextEdits EM MEMÓRIA sobre o texto do disco (não escreve).
    let mut diffs: Vec<Value> = vec![];
    let mut touched: Vec<String> = vec![];
    for (f, es) in &by_file {
        let orig = std::fs::read_to_string(f).unwrap_or_default();
        let newt = apply_text_edits(&orig, es);
        let rel_path = rel(&root, &path_to_uri(f));
        touched.push(rel_path.clone());
        let d = unified_diff(&rel_path, &orig, &newt);
        if !d.is_empty() {
            diffs.push(json!({"file": rel_path, "diff": d}));
        }
    }
    let creates: Vec<String> = creates_from(&edit)
        .iter()
        .map(|c| rel(&root, &path_to_uri(c)))
        .collect();
    Ok(json!({
        "operation": "preview_edit",
        "file": file,
        "workspace_edit": edit,
        "diffs": diffs,
        "blast_radius": {"files": files_n, "edits": edits_n, "touched": touched, "per_file": per_file},
        "creates": creates,
        "note": "read-only: nenhum diagnostic simulado e nada escrito no disco. Use simulate_edit p/ o veredito net_delta e safe_apply p/ aplicar.",
    }))
}

// F1 · safe_apply — aplica a edição proposta SÓ SE net_delta<=0 (nenhum erro novo); senão RECUSA e
// devolve os erros introduzidos, SEM tocar o disco. Mesmo núcleo das demais (apply_if_safe →
// verify_and_apply(apply=true)), com verify_build opcional (roda o build no disco e reverte se falhar).
fn tool_safe_apply(srv: &Server, a: &Value) -> Result<Value, String> {
    let file = a["file"].as_str().unwrap_or("");
    let (client, abs, _uri, edit, project) = resolve_f1_edit(srv, a)?;
    client.ensure_open(&abs)?;
    client.resync_all_changed();
    let verify_build = a["verify_build"].as_bool().unwrap_or(false);
    let mut result = apply_if_safe(&client, &edit, verify_build, &project, build_lang(file))?;
    result["operation"] = json!("safe_apply");
    result["file"] = json!(file);
    result["verdict"] = json!(if result["safe"].as_bool().unwrap_or(false) {
        "safe"
    } else {
        "unsafe"
    });
    Ok(result)
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
    // I1: cada referência vira `path:line:content` + ~2 linhas de contexto (via helper compartilhado),
    // agrupada por arquivo — para o modelo NÃO precisar reabrir o arquivo p/ ler a linha. Mantém
    // `references` (lista `path:linha:col`) para retrocompat; o novo `by_file` é o formato rico.
    let root = client.root().to_string();
    let mut cache = SourceCache::default();
    let mut locs: Vec<String> = vec![];
    let mut grouped: std::collections::BTreeMap<String, Vec<Value>> =
        std::collections::BTreeMap::new();
    for r in &refs {
        let u = r["uri"].as_str().unwrap_or("");
        let sl0 = r["range"]["start"]["line"].as_u64().unwrap_or(0);
        let sc0 = r["range"]["start"]["character"].as_u64().unwrap_or(0);
        let rel_path = rel(&root, u);
        locs.push(format!("{}:{}:{}", rel_path, sl0 + 1, sc0 + 1));
        grouped
            .entry(rel_path)
            .or_default()
            .push(format_location_uri(&mut cache, &root, u, sl0, sc0));
    }
    locs.sort();
    // ordena os locais de cada arquivo por linha (o helper já traz o "at" com a linha)
    for v in grouped.values_mut() {
        v.sort_by_key(|loc| loc["at"].as_str().unwrap_or("").to_string());
    }
    Ok(json!({
        "symbol": symbol,
        "count": refs.len(),
        "files": grouped.len(),
        "stable": stable,
        "warning": warning,
        "warmup_ms": warmup_ms,
        "polls": polls,
        "references": locs,
        "by_file": grouped,
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

// F2/C2: EXECUTOR DE CODE-ACTION `source.*` INTERNO (resolve `source.organizeImports` /
// `source.removeUnusedImports` → WorkspaceEdit). NÃO é exposto como tool crua ao modelo (superfície
// enxuta, C10): só as tools NOMEADAS (organize_imports) o usam. Difere de refactor_edit porque as
// ações `source.*` operam sobre o ARQUIVO INTEIRO (não uma seleção) e o range é o documento todo.
// Pede vários kinds `source.*`; casa a 1ª ação cujo `kind` bata (nem todo server rotula igual) e
// resolve o edit se for lazy. Retorna Err "unsupported" quando o backend não oferece a ação.
fn source_action_edit(client: &LspClient, uri: &str, kinds: &[&str]) -> Result<Value, String> {
    // range = documento inteiro (source actions são file-scoped). Usa um range grande e seguro.
    let range = json!({"start":{"line":0,"character":0},"end":{"line":u32::MAX,"character":0}});
    let start = Instant::now();
    let mut actions: Vec<Value> = vec![];
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
    // casa por prefixo de kind (source.organizeImports.ts do vtsls conta como source.organizeImports)
    let chosen = actions
        .iter()
        .find(|a| {
            a["kind"].as_str().map_or(false, |k| {
                kinds
                    .iter()
                    .any(|want| k == *want || k.starts_with(&format!("{want}.")))
            })
        })
        .or_else(|| actions.first())
        .cloned()
        .ok_or_else(|| format!("nenhuma source-action {kinds:?} disponível neste arquivo"))?;
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
        .ok_or_else(|| "a source-action não produziu edit".to_string())
}

fn tool_organize_imports(srv: &Server, a: &Value) -> Result<Value, String> {
    let project = a["project"].as_str().ok_or("faltou 'project'")?;
    let file = a["file"].as_str().ok_or("faltou 'file'")?;
    let apply = a["apply"].as_bool().unwrap_or(false); // false = preview (mede e reverte)
    let backend = refactor_backend(file); // C7: refactors de TS vão pro vtsls (tsgo não os tem)
    let mut client = srv.client(project, backend)?;
    let abs = safe_abs(project, file)?;
    client.ensure_open(&abs)?;
    let uri = path_to_uri(&abs);
    // Pede organizeImports E removeUnusedImports (onde o server oferecer). organizeImports já
    // reordena/dedup e remove NÃO-USADOS de forma segura — o LSP sabe o USO REAL, então NÃO remove
    // import de side-effect (`import "./polyfill"`) nem type-only usado (o que um sed textual erraria).
    let kinds: &[&str] = &["source.organizeImports", "source.removeUnusedImports"];
    let edit = match source_action_edit(&client, &uri, kinds) {
        Ok(e) => e,
        Err(e) if is_conn_dead(&e) => {
            client = srv.restart_client(project, backend)?;
            client.ensure_open(&abs)?;
            match source_action_edit(&client, &uri, kinds) {
                Ok(e) => e,
                Err(e2) => {
                    return Err(format!(
                        "backend '{backend}' fechou a conexão durante organize_imports e falhou após reinício: {e2}"
                    ))
                }
            }
        }
        // Honesto (como move/extract): backend não oferece a source-action → unsupported, não erro cru.
        Err(_e) => {
            return Ok(json!({
                "operation": "organize_imports", "file": file,
                "applied": false, "safe": false, "unsupported": true, "error": "organize_unsupported",
                "detail": format!("o backend '{}' ({}) não ofereceu 'source.organizeImports' neste arquivo — organize_imports é garantido em TypeScript (vtsls)", backend, build_lang(file))
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
    result["operation"] = json!("organize_imports");
    result["file"] = json!(file);
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

// F3: uma referência (start line) está DENTRO do range da declaração do símbolo? Assim distinguimos
// a própria definição (que find_references retorna com includeDeclaration:true) das referências de
// USO reais em outros lugares. Comparação por linha (o start de cada ref cai numa linha do range).
fn ref_in_def(ref_line: u64, def: ((u64, u64), (u64, u64))) -> bool {
    let ((sl, _), (el, _)) = def;
    ref_line >= sl && ref_line <= el
}

// F3: deleta um símbolo APENAS se ele não tiver referências fora da própria definição. Funde
// find_references (pós warmup gate, C4: índice frio → ERRO, nunca falso "0 refs") + net_delta +
// verify_build num único gate. Se houver USO externo, RECUSA e devolve os locais (formato I1).
fn tool_safe_delete(srv: &Server, a: &Value) -> Result<Value, String> {
    let project = a["project"].as_str().ok_or("faltou 'project'")?;
    let file = a["file"].as_str().ok_or("faltou 'file'")?;
    let symbol = a["symbol"].as_str().ok_or("faltou 'symbol'")?;
    let line = a["line"].as_u64();
    let apply = a["apply"].as_bool().unwrap_or(false); // false = preview (mede e reverte)

    // Nav backend (tsgo p/ TS): find_references é navegação, não refactor.
    let client = srv.client(project, nav_backend(file))?;
    let abs = safe_abs(project, file)?;
    client.ensure_open(&abs)?;
    client.resync_all_changed(); // Bug 3: freshness cross-file
    let (l, c) = resolve_pos(&client, &abs, symbol, line)?;
    let uri = path_to_uri(&abs);

    // Range COMPLETO da declaração — para (a) distinguir a def das refs de uso; (b) construir o edit
    // que apaga a declaração inteira. Vem do documentSymbol (semântico), não de heurística textual.
    // Range COMPLETO da declaração do alvo (helper compartilhado com F4): o símbolo cujo range
    // CONTÉM a posição resolvida (l,c), desambiguando homônimos.
    let def_range = find_decl_range(&client, &abs, symbol, l)?;

    // GATE de warmup (C4): índice quente ANTES de decidir. Índice FRIO/instável → ERRO, nunca um
    // falso "0 refs" que levaria a deletar um símbolo ainda referenciado.
    let (refs, stable, warmup_ms, _polls) = warmup_references(&client, &uri, l, c, None)?;
    if !stable {
        return Ok(json!({
            "operation": "safe_delete", "symbol": symbol,
            "applied": false, "safe": false, "error": "index_not_ready",
            "detail": index_not_ready_hint(),
        }));
    }

    // Separa referências de USO (fora da declaração) da própria definição. As de uso, se houver,
    // são o motivo da RECUSA — devolvidas no formato I1 (path:line:content + contexto).
    let root = client.root().to_string();
    let mut cache = SourceCache::default();
    let def_abs = uri_to_path(&uri);
    let mut external: Vec<Value> = vec![];
    for r in &refs {
        let u = r["uri"].as_str().unwrap_or("");
        let rl = r["range"]["start"]["line"].as_u64().unwrap_or(0);
        let rc = r["range"]["start"]["character"].as_u64().unwrap_or(0);
        // é a própria definição? (mesmo arquivo E linha dentro do range da declaração)
        if uri_to_path(u) == def_abs && ref_in_def(rl, def_range) {
            continue;
        }
        external.push(format_location_uri(&mut cache, &root, u, rl, rc));
    }
    if !external.is_empty() {
        return Ok(json!({
            "operation": "safe_delete", "symbol": symbol,
            "applied": false, "safe": false, "error": "has_references",
            "references_count": external.len(),
            "index_warmup_ms": warmup_ms,
            "detail": format!("'{symbol}' tem {} referência(s) FORA da própria definição — RECUSADO. Remova/atualize os usos antes, ou renomeie. Locais abaixo.", external.len()),
            "references": external,
        }));
    }

    // Zero refs externas: constrói o WorkspaceEdit que apaga a declaração inteira (do início do range
    // até o início da linha seguinte, para não deixar linha em branco) e passa pelo verify_and_apply.
    let ((sl, sc), (el, ec)) = def_range;
    let del_range = json!({
        "start": {"line": sl, "character": sc},
        "end": {"line": el + 1, "character": 0},
    });
    // se o range não termina no fim da linha, usa (el,ec) — evita comer a linha seguinte por engano.
    let del_range = {
        let text = std::fs::read_to_string(&abs).unwrap_or_default();
        let lines: Vec<&str> = text.split('\n').collect();
        let line_len = lines
            .get(el as usize)
            .map(|s| s.chars().count() as u64)
            .unwrap_or(ec);
        if ec >= line_len {
            del_range // termina no fim da linha → apaga até o começo da próxima (some a linha toda)
        } else {
            json!({"start":{"line":sl,"character":sc},"end":{"line":el,"character":ec}})
        }
    };
    let edit = json!({"changes": {uri.clone(): [{"range": del_range, "newText": ""}]}});

    let mut result = verify_and_apply(
        &client,
        &edit,
        apply,
        a["verify_build"].as_bool().unwrap_or(false),
        project,
        build_lang(file),
    )?;
    result["operation"] = json!("safe_delete");
    result["symbol"] = json!(symbol);
    result["index_warmup_ms"] = json!(warmup_ms);
    result["references_count"] = json!(0);
    Ok(result)
}

// ---- F4: edições POR SÍMBOLO (alvo por NOME/name_path, nunca coordenadas cruas) ----
// As três (replace_symbol_body / insert_before_symbol / insert_after_symbol) resolvem a declaração
// do símbolo SEMANTICAMENTE (documentSymbol → find_decl_range), montam um WorkspaceEdit e passam
// SEMPRE pelo núcleo verify_and_apply (net_delta, verify_build opcional, preview=apply=false). NÃO
// abrem caminho de apply/LSP novo — reusam sym_full_range (F3) e o edit-builder do F1.

// Modo da edição por símbolo (o range LSP é derivado do full_range da declaração).
enum SymEditMode {
    Replace,      // substitui o range completo da declaração pelo texto
    InsertBefore, // insere texto ANTES da declaração (no início da 1ª linha da decl)
    InsertAfter,  // insere texto DEPOIS da declaração (após a última linha da decl)
}

fn symbol_scoped_edit(srv: &Server, a: &Value, mode: SymEditMode) -> Result<Value, String> {
    let project = a["project"].as_str().ok_or("faltou 'project'")?;
    let file = a["file"].as_str().ok_or("faltou 'file'")?;
    let symbol = a["symbol"]
        .as_str()
        .ok_or("faltou 'symbol' (nome ou name_path)")?;
    let text = a["text"]
        .as_str()
        .ok_or("faltou 'text' (o conteúdo a inserir/substituir)")?;
    let line = a["line"].as_u64();
    let apply = a["apply"].as_bool().unwrap_or(false); // false = preview (mede e reverte)

    // Nav backend (tsgo p/ TS): resolvemos posição/decl por documentSymbol; a edição vem pronta.
    let client = srv.client(project, nav_backend(file))?;
    let abs = safe_abs(project, file)?;
    client.ensure_open(&abs)?;
    client.resync_all_changed(); // Bug 3: freshness cross-file
    let (l, _c) = resolve_pos(&client, &abs, symbol, line)?;
    let uri = path_to_uri(&abs);

    // Range COMPLETO da declaração (semântico) — desambigua homônimos pelo range que contém (l,c).
    let ((sl, sc), (el, ec)) = find_decl_range(&client, &abs, symbol, l)?;

    // Deriva o range LSP + o texto do TextEdit conforme o modo. Inserções são de largura zero.
    let (op_name, edit_range, new_text) = match mode {
        SymEditMode::Replace => (
            "replace_symbol_body",
            json!({"start":{"line":sl,"character":sc},"end":{"line":el,"character":ec}}),
            text.to_string(),
        ),
        // Insere no início da declaração; garante uma quebra de linha para não colar no símbolo.
        SymEditMode::InsertBefore => {
            let nt = if text.ends_with('\n') {
                text.to_string()
            } else {
                format!("{text}\n")
            };
            (
                "insert_before_symbol",
                json!({"start":{"line":sl,"character":0},"end":{"line":sl,"character":0}}),
                nt,
            )
        }
        // Insere logo após a última linha da declaração (coluna 0 da linha seguinte).
        SymEditMode::InsertAfter => {
            let nt = if text.starts_with('\n') {
                text.to_string()
            } else {
                format!("\n{text}")
            };
            (
                "insert_after_symbol",
                json!({"start":{"line":el,"character":ec},"end":{"line":el,"character":ec}}),
                nt,
            )
        }
    };
    let edit = json!({"changes": {uri.clone(): [{"range": edit_range, "newText": new_text}]}});

    let mut result = verify_and_apply(
        &client,
        &edit,
        apply,
        a["verify_build"].as_bool().unwrap_or(false),
        project,
        build_lang(file),
    )?;
    result["operation"] = json!(op_name);
    result["symbol"] = json!(symbol);
    Ok(result)
}

fn tool_replace_symbol_body(srv: &Server, a: &Value) -> Result<Value, String> {
    symbol_scoped_edit(srv, a, SymEditMode::Replace)
}
fn tool_insert_before_symbol(srv: &Server, a: &Value) -> Result<Value, String> {
    symbol_scoped_edit(srv, a, SymEditMode::InsertBefore)
}
fn tool_insert_after_symbol(srv: &Server, a: &Value) -> Result<Value, String> {
    symbol_scoped_edit(srv, a, SymEditMode::InsertAfter)
}

// ---- F6/C5: blast_radius — COMPOSTO sobre tools existentes (NENHUM caminho LSP novo) ----
// Read-only. Dado um símbolo, junta (a) find_references (pós warmup gate — índice frio → ERRO,
// nunca falso-vazio) e (b) call_hierarchy incomingCalls (chamadores). Particiona os locais em
// test vs não-test por heurística de path. Usa I1 format_location. Serve para o modelo VER a
// superfície de risco ANTES de editar. Não abre codeAction/refactor — só compõe refs + hierarchy.

// Heurística de "arquivo de teste" por path (cobre TS/JS/Py/Rust/Dart/C#): dir __tests__/tests/
// test, sufixos .test./.spec./_test./_spec, prefixo test_, ou nome terminando em Test/Tests/Spec.
fn is_test_path(rel_path: &str) -> bool {
    let p = rel_path.to_lowercase();
    let segs: Vec<&str> = p.split('/').collect();
    if segs
        .iter()
        .any(|s| matches!(*s, "test" | "tests" | "__tests__" | "spec" | "specs"))
    {
        return true;
    }
    let file = segs.last().copied().unwrap_or(&p);
    let stem = file.rsplit_once('.').map(|(s, _)| s).unwrap_or(file);
    // sufixos com delimitador explícito (.test/.spec/_test/_spec) e prefixo test_ — inequívocos.
    if file.starts_with("test_")
        || stem.ends_with(".test")
        || stem.ends_with(".spec")
        || stem.ends_with("_test")
        || stem.ends_with("_spec")
    {
        return true;
    }
    // Convenção PascalCase (C#): 'WidgetTests'/'WidgetSpec'. Casa 'test'/'tests'/'spec' no fim
    // do stem SÓ quando precedido de letra MAIÚSCULA (limite de palavra) — evita 'latest'/'manifest'.
    // (o file veio em lowercase de `p`, então usamos o nome ORIGINAL para checar a maiúscula.)
    for suf in ["tests", "test", "spec"] {
        if let Some(orig_file) = rel_path.rsplit('/').next() {
            let orig_stem = orig_file
                .rsplit_once('.')
                .map(|(s, _)| s)
                .unwrap_or(orig_file);
            if orig_stem.len() > suf.len()
                && orig_stem.to_lowercase().ends_with(suf)
                && orig_stem
                    .as_bytes()
                    .get(orig_stem.len() - suf.len())
                    .map(|b| b.is_ascii_uppercase())
                    .unwrap_or(false)
            {
                return true;
            }
        }
    }
    false
}

fn tool_blast_radius(srv: &Server, a: &Value) -> Result<Value, String> {
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

    // (a) GATE de warmup (C4): índice frio/instável → ERRO acionável, nunca um blast_radius vazio
    // enganoso que faria o modelo achar a edição "segura".
    let (refs, stable, warmup_ms, _polls) = warmup_references(&client, &uri, l, c, None)?;
    if !stable {
        return Ok(json!({
            "operation": "blast_radius", "symbol": symbol,
            "error": "index_not_ready", "detail": index_not_ready_hint(),
        }));
    }

    let root = client.root().to_string();
    let mut cache = SourceCache::default();
    let def_abs = uri_to_path(&uri);
    let ((dsl, _), (del, _)) =
        find_decl_range(&client, &abs, symbol, l).unwrap_or(((l, 0), (l, 0))); // fallback: só a linha resolvida

    // Particiona as REFERÊNCIAS (exclui a própria declaração) em test vs não-test (formato I1).
    let mut refs_test: Vec<Value> = vec![];
    let mut refs_prod: Vec<Value> = vec![];
    for r in &refs {
        let u = r["uri"].as_str().unwrap_or("");
        let rl = r["range"]["start"]["line"].as_u64().unwrap_or(0);
        let rc = r["range"]["start"]["character"].as_u64().unwrap_or(0);
        // pula a própria definição (mesmo arquivo E dentro do range da declaração)
        if uri_to_path(u) == def_abs && rl >= dsl && rl <= del {
            continue;
        }
        let rel_path = rel(&root, u);
        let loc = format_location_uri(&mut cache, &root, u, rl, rc);
        if is_test_path(&rel_path) {
            refs_test.push(loc);
        } else {
            refs_prod.push(loc);
        }
    }

    // (b) CHAMADORES via call_hierarchy incomingCalls — mesmo caminho da tool existente, sem LSP novo.
    let mut callers_test: Vec<Value> = vec![];
    let mut callers_prod: Vec<Value> = vec![];
    if let Ok(prep) = client.request(
        "textDocument/prepareCallHierarchy",
        json!({"textDocument":{"uri":uri},"position":{"line":l,"character":c}}),
        10_000,
    ) {
        if let Some(item) = prep.as_array().and_then(|a| a.first()).cloned() {
            if let Ok(incoming) =
                client.request("callHierarchy/incomingCalls", json!({"item": item}), 10_000)
            {
                for call in incoming.as_array().cloned().unwrap_or_default() {
                    let from = &call["from"];
                    let (fl, fc) = sym_pos(from);
                    let u = from["uri"].as_str().unwrap_or("");
                    let rel_path = rel(&root, u);
                    let loc = format_location_uri(&mut cache, &root, u, fl, fc);
                    let entry = json!({"caller": from["name"].as_str().unwrap_or(""),
                        "at": loc["at"].clone(), "content": loc["content"].clone(),
                        "context": loc["context"].clone()});
                    if is_test_path(&rel_path) {
                        callers_test.push(entry);
                    } else {
                        callers_prod.push(entry);
                    }
                }
            }
        }
    }

    // "exports afetados": arquivos DISTINTOS (não-test) tocados pelas referências — a superfície
    // pública que muda de comportamento se o símbolo mudar. Composição barata sobre as refs.
    let mut export_files: std::collections::BTreeSet<String> = std::collections::BTreeSet::new();
    for r in &refs {
        let u = r["uri"].as_str().unwrap_or("");
        let rp = rel(&root, u);
        if uri_to_path(u) != def_abs && !is_test_path(&rp) {
            export_files.insert(rp);
        }
    }

    Ok(json!({
        "operation": "blast_radius",
        "symbol": symbol,
        "stable": stable,
        "warmup_ms": warmup_ms,
        "summary": {
            "references": refs_prod.len() + refs_test.len(),
            "references_prod": refs_prod.len(),
            "references_test": refs_test.len(),
            "callers": callers_prod.len() + callers_test.len(),
            "callers_prod": callers_prod.len(),
            "callers_test": callers_test.len(),
            "affected_files": export_files.len(),
        },
        "affected_files": export_files.into_iter().collect::<Vec<_>>(),
        "references": {"non_test": refs_prod, "test": refs_test},
        "callers": {"non_test": callers_prod, "test": callers_test},
        "note": "read-only: composto sobre find_references (warmup-gated) + call_hierarchy. Use antes de editar para ver a superfície de risco.",
    }))
}

// ---- F7/C2: quick_fix DIRIGIDO — aplica UMA code-action de correção para um diagnóstico ----
// Usa o EXECUTOR INTERNO de code-action (irmão de source_action_edit): NÃO expõe um code_action
// cru/genérico ao modelo (C2/C10). Puxa os diagnósticos na LOCALIZAÇÃO dada, pede as ações
// `quickfix` COM esses diagnósticos no context, escolhe UMA (por título preferido, senão a 1ª),
// resolve o edit e passa pelo verify_and_apply. Sem correção casável → unsupported/none honesto.

// Coleta os diagnósticos LSP que INTERSECTAM a linha `line0` no arquivo `uri` (PULL p/ tsgo;
// PUSH p/ vtsls/pyright, via o store de diagnostics do client). Alimenta o context da codeAction —
// sem diagnósticos, muitos servers não oferecem quickfix. Como o PUSH é ASSÍNCRONO (o publish
// chega depois do didOpen), faz POLLING até achar um diagnóstico na linha ou estourar o budget —
// senão o 1º quick_fix num arquivo recém-aberto voltaria vazio por corrida (o publish ainda não
// chegou), como o 2º acertaria (flaky). Budget curto e dedicado.
fn on_line(all: &[Value], line0: u64) -> Vec<Value> {
    all.iter()
        .filter(|d| {
            let sl = d["range"]["start"]["line"].as_u64().unwrap_or(0);
            let el = d["range"]["end"]["line"].as_u64().unwrap_or(sl);
            line0 >= sl && line0 <= el
        })
        .cloned()
        .collect()
}
fn diagnostics_at(client: &LspClient, uri: &str, line0: u64) -> Vec<Value> {
    let abs = uri_to_path(uri);
    if client.supports_pull() {
        return on_line(&client.pull_diagnostics(&abs).unwrap_or_default(), line0);
    }
    // PUSH: faz polling até o publish chegar com um diagnóstico NA LINHA (ou o budget expirar).
    let start = Instant::now();
    loop {
        let hits = on_line(&client.pushed_diagnostics(&abs), line0);
        if !hits.is_empty() || start.elapsed().as_millis() >= 5000 {
            return hits;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
}

// Executor interno de quickfix: pede codeAction (only quickfix) com os diagnósticos no context,
// filtra por CodeActionKind quickfix, escolhe uma (título preferido opcional) e resolve o edit.
// Err("nenhum quick_fix ...") quando não há correção casável — o chamador transforma em unsupported.
fn quickfix_edit(
    client: &LspClient,
    uri: &str,
    line0: u64,
    diags: &[Value],
    prefer_title: Option<&str>,
) -> Result<Value, String> {
    let range =
        json!({"start":{"line":line0,"character":0},"end":{"line":line0,"character":u32::MAX}});
    let res = client.request(
        "textDocument/codeAction",
        json!({"textDocument":{"uri":uri},"range":range,
               "context":{"diagnostics":diags,"only":["quickfix"]}}),
        10_000,
    )?;
    let actions = res.as_array().cloned().unwrap_or_default();
    // fica só com CodeActions (não Commands) do kind quickfix (ou prefixo quickfix.*).
    let is_quickfix = |a: &Value| {
        a["kind"]
            .as_str()
            .map_or(false, |k| k == "quickfix" || k.starts_with("quickfix."))
    };
    let chosen = prefer_title
        .and_then(|t| {
            actions
                .iter()
                .find(|a| is_quickfix(a) && a["title"].as_str().map_or(false, |s| s.contains(t)))
        })
        .or_else(|| actions.iter().find(|a| is_quickfix(a)))
        .cloned()
        .ok_or_else(|| {
            "nenhum quick_fix disponível para o diagnóstico nesta posição".to_string()
        })?;
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
        .ok_or_else(|| "o quick_fix não produziu edit".to_string())
}

fn tool_quick_fix(srv: &Server, a: &Value) -> Result<Value, String> {
    let project = a["project"].as_str().ok_or("faltou 'project'")?;
    let file = a["file"].as_str().ok_or("faltou 'file'")?;
    let line = a["line"]
        .as_u64()
        .ok_or("faltou 'line' (1-indexed) do diagnóstico")?;
    let apply = a["apply"].as_bool().unwrap_or(false); // false = preview (mede e reverte)
    let prefer_title = a["prefer_title"].as_str(); // opcional: escolhe a ação por título
    let line0 = line.saturating_sub(1); // 1-indexed (humano) → 0-indexed (LSP)

    // Backend de REFACTOR (C7): quickfix é uma code-action → vtsls no TS (tsgo não faz refactor).
    let backend = refactor_backend(file);
    let mut client = srv.client(project, backend)?;
    let abs = safe_abs(project, file)?;
    client.ensure_open(&abs)?;
    let uri = path_to_uri(&abs);

    let diags = diagnostics_at(&client, &uri, line0);
    let edit = match quickfix_edit(&client, &uri, line0, &diags, prefer_title) {
        Ok(e) => e,
        Err(e) if is_conn_dead(&e) => {
            client = srv.restart_client(project, backend)?;
            client.ensure_open(&abs)?;
            let diags = diagnostics_at(&client, &uri, line0);
            match quickfix_edit(&client, &uri, line0, &diags, prefer_title) {
                Ok(e) => e,
                Err(e2) => {
                    return Err(format!(
                        "backend '{backend}' fechou a conexão durante quick_fix e falhou após reinício: {e2}"
                    ))
                }
            }
        }
        // Honesto (como organize/move/extract): sem correção casável → unsupported/none, não erro cru.
        Err(_e) => {
            return Ok(json!({
                "operation": "quick_fix", "file": file, "line": line,
                "applied": false, "safe": false, "unsupported": true, "error": "no_quick_fix",
                "diagnostics_found": diags.len(),
                "detail": format!("nenhum quick_fix disponível para um diagnóstico na linha {line} de {file} (backend '{backend}'). Confira se há de fato um diagnóstico ali (diagnostics_found={}).", diags.len())
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
    result["operation"] = json!("quick_fix");
    result["file"] = json!(file);
    result["line"] = json!(line);
    Ok(result)
}

// ---- F5: change_signature (add/remove/reorder de parâmetro) ----------------------------------
// Onde o LSP oferece um refactor NATIVO de "change signature" (não é o caso hoje de vtsls/tsgo,
// rust-analyzer nem pyright), usaríamos o code-action. Onde NÃO oferece — que é o caso da maioria
// e é o DIFERENCIAL — construímos o WorkspaceEdit À MÃO: reescrevemos a lista de parâmetros na
// declaração e, para CADA call-site (descoberto via call_hierarchy/fromRanges), reescrevemos a
// lista de argumentos com a MESMA operação. Depois SIMULAMOS net_delta + verify_build antes de
// aplicar (verify_and_apply). Índice frio => ERRO (C4), nunca callers faltando em silêncio.
//
// Spec de mudança (parâmetro `spec`), simples e explícita — UMA operação por chamada:
//   {"op":"add",     "index": <i>, "param": "<texto do parâmetro na decl>", "arg": "<texto do argumento no call-site>"}
//   {"op":"remove",  "index": <i>}
//   {"op":"reorder", "order": [<índices na nova ordem>]}   // permutação dos parâmetros existentes
// `index`/`order` são 0-indexed sobre a lista de parâmetros ATUAL. add exige `arg` (o valor a
// passar nos call-sites) — SÓ é seguro adicionar um parâmetro se soubermos o que os chamadores
// devem passar; sem `arg` recusamos (evita call-site que não compila).

// Encontra o span (byte range) da lista de argumentos/parâmetros ENTRE os parênteses cuja abertura
// vem logo após `open_from`. Respeita aninhamento de () [] {} <> e strings/char/template — para não
// quebrar em vírgulas dentro de genéricos, closures ou literais. Retorna (inicio_conteudo,
// fim_conteudo) EXCLUINDO os próprios parênteses. `open_from` é o offset onde procurar o '(' de
// abertura (ex.: logo após o identificador). None se não achar um par balanceado.
fn paren_span(s: &str, open_from: usize) -> Option<(usize, usize)> {
    let b = s.as_bytes();
    let mut i = open_from;
    while i < b.len() && b[i] != b'(' {
        // só espaços/identificador entre o nome e o '(' — se topar com algo estranho, aborta.
        if !(b[i] as char).is_whitespace()
            && b[i] != b'('
            && b[i].is_ascii_punctuation()
            && b[i] != b'_'
        {
            // permite '<...>' de genéricos entre nome e '(' (raro em call-sites; comum em decls TS)
            if b[i] == b'<' {
                if let Some(close) = balanced_close(s, i, b'<', b'>') {
                    i = close + 1;
                    continue;
                }
            }
            return None;
        }
        i += 1;
    }
    if i >= b.len() {
        return None;
    }
    let content_start = i + 1;
    let close = balanced_close(s, i, b'(', b')')?;
    Some((content_start, close))
}

// Dado o offset de um caractere de abertura `open` (== s[open_off]), acha o offset do fechamento
// balanceado correspondente, respeitando aninhamento de todos os pares e pulando strings. Retorna
// o offset do char de fechamento. None se desbalanceado.
fn balanced_close(s: &str, open_off: usize, open: u8, close: u8) -> Option<usize> {
    let b = s.as_bytes();
    let mut depth = 0i32;
    let mut i = open_off;
    let mut string: Option<u8> = None; // ", ', ` ativo
    while i < b.len() {
        let c = b[i];
        if let Some(q) = string {
            if c == b'\\' {
                i += 2;
                continue;
            }
            if c == q {
                string = None;
            }
            i += 1;
            continue;
        }
        match c {
            b'"' | b'\'' | b'`' => string = Some(c),
            _ if c == open => depth += 1,
            _ if c == close => {
                depth -= 1;
                if depth == 0 {
                    return Some(i);
                }
            }
            _ => {}
        }
        i += 1;
    }
    None
}

// Divide o conteúdo de uma lista (parâmetros ou argumentos) em itens de TOP-LEVEL, respeitando
// aninhamento de () [] {} <> e strings. Preserva o texto exato de cada item (com espaços). Lista
// vazia (só espaços) => vec vazio.
fn split_top_level(content: &str) -> Vec<String> {
    let b = content.as_bytes();
    let mut items = vec![];
    let mut depth = 0i32;
    let mut start = 0usize;
    let mut string: Option<u8> = None;
    let mut i = 0usize;
    let mut any = false;
    while i < b.len() {
        let c = b[i];
        if let Some(q) = string {
            if c == b'\\' {
                i += 2;
                continue;
            }
            if c == q {
                string = None;
            }
            i += 1;
            continue;
        }
        match c {
            b'"' | b'\'' | b'`' => string = Some(c),
            b'(' | b'[' | b'{' | b'<' => depth += 1,
            b')' | b']' | b'}' | b'>' => depth -= 1,
            b',' if depth == 0 => {
                items.push(content[start..i].to_string());
                start = i + 1;
                any = true;
            }
            _ => {}
        }
        if !c.is_ascii_whitespace() {
            any = true;
        }
        i += 1;
    }
    if any {
        items.push(content[start..].to_string());
    }
    // remove um eventual item final vazio (trailing comma)
    if items.last().map(|s| s.trim().is_empty()).unwrap_or(false) {
        items.pop();
    }
    items
}

// Aplica a operação da spec a uma lista de itens (parâmetros na decl OU argumentos no call-site).
// `is_decl` escolhe o texto: `param` para a declaração, `arg` para o call-site. Preserva o trim/
// espaçamento original re-juntando com ", ". Erros são honestos (índice fora do range, etc.).
fn apply_sig_op(items: &[String], spec: &Value, is_decl: bool) -> Result<Vec<String>, String> {
    let op = spec["op"]
        .as_str()
        .ok_or("spec sem 'op' (add|remove|reorder)")?;
    let mut out: Vec<String> = items.iter().map(|s| s.trim().to_string()).collect();
    match op {
        "add" => {
            let idx = spec["index"].as_u64().unwrap_or(out.len() as u64) as usize;
            if idx > out.len() {
                return Err(format!("add.index {idx} fora do range (0..={})", out.len()));
            }
            let text = if is_decl {
                spec["param"]
                    .as_str()
                    .ok_or("add exige 'param' (texto do parâmetro na declaração)")?
            } else {
                // add exige 'arg' — sem saber o que os callers passam, adicionar é inseguro.
                spec["arg"].as_str().ok_or("add exige 'arg' (o valor a passar nos call-sites) — sem ele o caller não compila")?
            };
            out.insert(idx, text.to_string());
        }
        "remove" => {
            let idx = spec["index"]
                .as_u64()
                .ok_or("remove exige 'index' (0-indexed)")? as usize;
            if idx >= out.len() {
                return Err(format!(
                    "remove.index {idx} fora do range (0..{})",
                    out.len()
                ));
            }
            out.remove(idx);
        }
        "reorder" => {
            let order: Vec<usize> = spec["order"]
                .as_array()
                .ok_or("reorder exige 'order' (lista de índices na nova ordem)")?
                .iter()
                .map(|v| v.as_u64().map(|n| n as usize))
                .collect::<Option<Vec<_>>>()
                .ok_or("reorder.order deve ser uma lista de inteiros")?;
            let mut sorted = order.clone();
            sorted.sort_unstable();
            let expected: Vec<usize> = (0..out.len()).collect();
            if sorted != expected {
                return Err(format!(
                    "reorder.order {order:?} não é uma permutação exata de 0..{} (a lista atual tem {} itens)",
                    out.len(),
                    out.len()
                ));
            }
            out = order.into_iter().map(|i| out[i].clone()).collect();
        }
        other => {
            return Err(format!(
                "op '{other}' desconhecida (use add|remove|reorder)"
            ))
        }
    }
    Ok(out)
}

// Reescreve UMA lista (params ou args) que começa logo após `ident_end` (offset do fim do
// identificador chamado/declarado) no texto `s`, aplicando a spec. Retorna (byte_start, byte_end,
// novo_texto) do CONTEÚDO entre parênteses. None se não achar a lista (posição não é uma chamada/
// declaração com parênteses balanceados) — o chamador trata como "não editável aqui".
fn rewrite_list_at(
    s: &str,
    ident_end: usize,
    spec: &Value,
    is_decl: bool,
) -> Option<(usize, usize, String)> {
    let (cs, ce) = paren_span(s, ident_end)?;
    let content = &s[cs..ce];
    let items = split_top_level(content);
    let new_items = apply_sig_op(&items, spec, is_decl).ok()?;
    Some((cs, ce, new_items.join(", ")))
}

fn tool_change_signature(srv: &Server, a: &Value) -> Result<Value, String> {
    let project = a["project"].as_str().ok_or("faltou 'project'")?;
    let file = a["file"].as_str().ok_or("faltou 'file'")?;
    let symbol = a["symbol"]
        .as_str()
        .ok_or("faltou 'symbol' (nome ou name_path da função/método)")?;
    let spec = a.get("spec").filter(|s| !s.is_null()).ok_or(
        "faltou 'spec' (a mudança: {op:add,index,param,arg} | {op:remove,index} | {op:reorder,order:[..]})",
    )?;
    let line = a["line"].as_u64();
    let apply = a["apply"].as_bool().unwrap_or(false); // false = preview (mede e reverte)

    // Validação básica da spec ANTES de qualquer trabalho pesado (falha honesta e barata).
    let op = spec["op"]
        .as_str()
        .ok_or("spec sem 'op' (add|remove|reorder)")?;
    if op == "add"
        && spec
            .get("arg")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .is_empty()
    {
        return Ok(json!({
            "operation": "change_signature", "symbol": symbol,
            "applied": false, "safe": false, "unsupported": true, "error": "unsafe_add",
            "detail": "op=add exige 'arg' (o valor que os call-sites devem passar). Sem ele os chamadores não compilariam — recusado em vez de gerar edição perigosa.",
        }));
    }

    // Backend de NAVEGAÇÃO (tsgo p/ TS): precisamos de call_hierarchy/references + diagnostics; a
    // edição é construída à mão (não é um refactoring do server). C7: nav != refactor aqui.
    let client = srv.client(project, nav_backend(file))?;
    let abs = safe_abs(project, file)?;
    client.ensure_open(&abs)?;
    client.resync_all_changed(); // Bug 3: freshness cross-file
    let (l, c) = resolve_pos(&client, &abs, symbol, line)?;
    let uri = path_to_uri(&abs);
    let last = base_name(symbol.rsplit('/').next().unwrap_or(symbol));

    // GATE de warmup (C4): índice quente ANTES de coletar call-sites. Frio/instável => ERRO — nunca
    // um WorkspaceEdit que atualiza a declaração mas PERDE chamadores em silêncio (quebra o build).
    let (_refs, stable, warmup_ms, _polls) = warmup_references(&client, &uri, l, c, None)?;
    if !stable {
        return Ok(json!({
            "operation": "change_signature", "symbol": symbol,
            "applied": false, "safe": false, "error": "index_not_ready",
            "detail": index_not_ready_hint(),
        }));
    }

    // Monta o WorkspaceEdit à mão: (1) a DECLARAÇÃO; (2) CADA call-site (via call_hierarchy).
    let root = client.root().to_string();
    let mut changes: HashMap<String, Vec<Value>> = HashMap::new();
    let mut sites = 0u64;

    // (1) Declaração: reescreve a lista de parâmetros logo após o identificador na posição resolvida.
    let decl_text = std::fs::read_to_string(&abs).map_err(|e| format!("ler {abs}: {e}"))?;
    let decl_ident_end = {
        let off = pos_to_offset(&decl_text, l, c);
        // o identificador começa em `off`; avança até o fim do nome (last).
        off + last.len()
    };
    match rewrite_list_at(&decl_text, decl_ident_end, spec, true) {
        Some((cs, ce, nt)) => {
            let (sl, sc) = offset_to_pos(&decl_text, cs);
            let (el, ec) = offset_to_pos(&decl_text, ce);
            changes.entry(uri.clone()).or_default().push(json!({
                "range": {"start":{"line":sl,"character":sc},"end":{"line":el,"character":ec}},
                "newText": nt
            }));
        }
        None => {
            return Ok(json!({
                "operation": "change_signature", "symbol": symbol,
                "applied": false, "safe": false, "unsupported": true, "error": "decl_not_editable",
                "detail": format!("não consegui localizar a lista de parâmetros da declaração de '{symbol}' na posição resolvida (linha {}). change_signature à mão exige uma declaração com '( ... )' após o nome.", l + 1),
            }));
        }
    }

    // (2) Call-sites via call_hierarchy incomingCalls → fromRanges (cada chamada). Reescreve a lista
    // de argumentos de cada uma. Se ALGUM call-site não for editável (não casa '(...)' balanceado),
    // ABORTA com unsupported: melhor recusar do que aplicar uma mudança parcial que quebra callers.
    let prep = client.request(
        "textDocument/prepareCallHierarchy",
        json!({"textDocument":{"uri":uri},"position":{"line":l,"character":c}}),
        10_000,
    )?;
    if let Some(item) = prep.as_array().and_then(|a| a.first()).cloned() {
        let incoming =
            client.request("callHierarchy/incomingCalls", json!({"item": item}), 10_000)?;
        for call in incoming.as_array().cloned().unwrap_or_default() {
            let from = &call["from"];
            let cu = from["uri"].as_str().unwrap_or("").to_string();
            let cabs = uri_to_path(&cu);
            let ctext = std::fs::read_to_string(&cabs).unwrap_or_default();
            for r in call["fromRanges"].as_array().cloned().unwrap_or_default() {
                let rl = r["start"]["line"].as_u64().unwrap_or(0);
                let rc = r["start"]["character"].as_u64().unwrap_or(0);
                // fromRanges aponta o identificador chamado; o '(' vem logo após o nome.
                let call_off = pos_to_offset(&ctext, rl, rc);
                // confirma que o texto no ponto é o identificador esperado (robustez: fromRanges
                // pode apontar o início da expressão de chamada). Avança até o fim de `last`.
                let ident_end = match ctext.get(call_off..) {
                    Some(rest) if rest.starts_with(last) => call_off + last.len(),
                    // fallback: procura o identificador na linha do call-site.
                    _ => match find_ident(ctext.lines().nth(rl as usize).unwrap_or(""), last) {
                        Some(col) => pos_to_offset(&ctext, rl, col as u64) + last.len(),
                        None => {
                            return Ok(json!({
                                "operation": "change_signature", "symbol": symbol,
                                "applied": false, "safe": false, "unsupported": true, "error": "callsite_not_editable",
                                "detail": format!("não consegui localizar a chamada de '{last}' em {}:{} — abortado para não gerar edição parcial que quebra chamadores.", rel(&root, &cu), rl + 1),
                            }))
                        }
                    },
                };
                match rewrite_list_at(&ctext, ident_end, spec, false) {
                    Some((cs, ce, nt)) => {
                        let (sl, sc) = offset_to_pos(&ctext, cs);
                        let (el, ec) = offset_to_pos(&ctext, ce);
                        changes.entry(cu.clone()).or_default().push(json!({
                            "range": {"start":{"line":sl,"character":sc},"end":{"line":el,"character":ec}},
                            "newText": nt
                        }));
                        sites += 1;
                    }
                    None => {
                        return Ok(json!({
                            "operation": "change_signature", "symbol": symbol,
                            "applied": false, "safe": false, "unsupported": true, "error": "callsite_not_editable",
                            "detail": format!("a chamada de '{last}' em {}:{} não tem uma lista de argumentos '( ... )' balanceada que eu saiba reescrever com segurança — abortado (change_signature à mão não aplica edição parcial).", rel(&root, &cu), rl + 1),
                        }))
                    }
                }
            }
        }
    }

    let edit = json!({ "changes": changes });
    // net_delta + verify_build são a REDE: se a reescrita à mão introduziu qualquer erro (aridade,
    // tipo, ordem), o net_delta pega e RECUSA em vez de aplicar. É o diferencial "constrói à mão,
    // mas confere semanticamente".
    let mut result = verify_and_apply(
        &client,
        &edit,
        apply,
        a["verify_build"].as_bool().unwrap_or(false),
        project,
        build_lang(file),
    )?;
    result["operation"] = json!("change_signature");
    result["symbol"] = json!(symbol);
    result["strategy"] = json!("hand_built"); // (nenhum LSP hoje oferece o refactor nativo p/ estes)
    result["call_sites_rewritten"] = json!(sites);
    result["index_warmup_ms"] = json!(warmup_ms);
    Ok(result)
}

// ---- F8: move_file (move/renomeia um arquivo inteiro + conserta importers) --------------------
// DIFERENTE de move_symbol: move_symbol tira UM símbolo de um arquivo e o põe em outro (novo);
// move_file move o ARQUIVO INTEIRO para um novo caminho e reescreve TODOS os importers/re-exports/
// barrels que apontavam pra ele. Usa workspace/willRenameFiles: o server devolve o WorkspaceEdit
// que conserta os imports; nós movemos o arquivo no disco e aplicamos esses edits via
// verify_and_apply. basedpyright é buggy aqui (#1888: willRenameFiles ignora diretório) → confiamos
// no verify_build (roda o build no disco e REVERTE se o move quebrar).

// Move/renomeia o arquivo no disco (cria dirs do destino). Retorna Err se o destino já existe.
fn do_move_file(src: &str, dest: &str) -> Result<(), String> {
    if Path::new(dest).exists() {
        return Err(format!("destino já existe: {dest}"));
    }
    if let Some(parent) = Path::new(dest).parent() {
        std::fs::create_dir_all(parent).map_err(|e| format!("criar dir do destino: {e}"))?;
    }
    std::fs::rename(src, dest).map_err(|e| format!("mover {src} -> {dest}: {e}"))
}

// F8 (TS): vtsls NÃO implementa workspace/willRenameFiles (só anuncia didRename). O caminho que
// funciona é o comando `typescript.tsserverRequest` → `getEditsForFileRename`, que devolve os
// fixups de import no formato do tsserver (fileName + textChanges com line/offset 1-indexed).
// Converte essa resposta num WorkspaceEdit LSP (changes por URI, ranges 0-indexed). Vazio => nenhum
// importer para consertar (WorkspaceEdit sem changes).
fn tsserver_rename_edit(client: &LspClient, old_abs: &str, new_abs: &str) -> Result<Value, String> {
    let res = client.request(
        "workspace/executeCommand",
        json!({"command":"typescript.tsserverRequest",
               "arguments":["getEditsForFileRename",{"oldFilePath":old_abs,"newFilePath":new_abs}]}),
        15_000,
    )?;
    // O comando pode voltar {body:[...]} (envelope do tsserver) ou já o array.
    let body = res
        .get("body")
        .cloned()
        .or_else(|| res.as_array().map(|_| res.clone()))
        .unwrap_or(Value::Null);
    let arr = body.as_array().cloned().unwrap_or_default();
    let mut changes = serde_json::Map::new();
    for file_edit in &arr {
        let fname = file_edit["fileName"].as_str().unwrap_or("");
        if fname.is_empty() {
            continue;
        }
        let uri = path_to_uri(fname);
        let tes: Vec<Value> = file_edit["textChanges"]
            .as_array()
            .cloned()
            .unwrap_or_default()
            .iter()
            .map(|tc| {
                // tsserver: line/offset são 1-indexed; LSP: line/character 0-indexed.
                let sl = tc["start"]["line"].as_u64().unwrap_or(1).saturating_sub(1);
                let sc = tc["start"]["offset"].as_u64().unwrap_or(1).saturating_sub(1);
                let el = tc["end"]["line"].as_u64().unwrap_or(1).saturating_sub(1);
                let ec = tc["end"]["offset"].as_u64().unwrap_or(1).saturating_sub(1);
                json!({"range":{"start":{"line":sl,"character":sc},"end":{"line":el,"character":ec}},
                       "newText": tc["newText"].as_str().unwrap_or("")})
            })
            .collect();
        if !tes.is_empty() {
            changes.insert(uri, json!(tes));
        }
    }
    Ok(json!({ "changes": Value::Object(changes) }))
}

fn tool_move_file(srv: &Server, a: &Value) -> Result<Value, String> {
    let project = a["project"].as_str().ok_or("faltou 'project'")?;
    let source = a["source"]
        .as_str()
        .ok_or("faltou 'source' (caminho relativo do arquivo a mover)")?;
    let dest = a["dest"]
        .as_str()
        .ok_or("faltou 'dest' (caminho relativo de destino)")?;
    let apply = a["apply"].as_bool().unwrap_or(false); // false = preview (não move nem escreve)
    let verify_build = a["verify_build"].as_bool().unwrap_or(false);

    let src_abs = safe_abs(project, source)?;
    let dest_abs = safe_abs(project, dest)?; // recusa traversal no destino também
    if !Path::new(&src_abs).exists() {
        return Err(format!("source não existe: {source}"));
    }
    if Path::new(&dest_abs).exists() {
        return Ok(json!({
            "operation": "move_file", "source": source, "dest": dest,
            "applied": false, "safe": false, "error": "dest_exists",
            "detail": format!("o destino '{dest}' já existe — recusado para não sobrescrever."),
        }));
    }

    // Backend de REFACTOR (C7): willRenameFiles é uma operação de workspace do server de refactor
    // (vtsls no TS; tsgo não implementa refactors/file-ops).
    let backend = refactor_backend(source);
    let mut client = srv.client(project, backend)?;
    client.ensure_open(&src_abs)?;
    client.resync_all_changed(); // Bug 3: freshness cross-file (importers)
    let src_uri = path_to_uri(&src_abs);
    let dest_uri = path_to_uri(&dest_abs);

    let is_ts = backend == "vtsls";
    // Pede ao server o WorkspaceEdit que conserta os importers ANTES de mover.
    //  - Caminho padrão LSP: workspace/willRenameFiles (usado por servers que o implementam, ex.:
    //    basedpyright — que é BUGGY, #1888; verify_build vira a rede).
    //  - TypeScript (vtsls): NÃO implementa willRenameFiles (só anuncia didRename). Usa o comando
    //    `typescript.tsserverRequest`/getEditsForFileRename, que dá os mesmos fixups de import.
    let params = json!({"files":[{"oldUri": src_uri, "newUri": dest_uri}]});
    let via_lsp = client.request("workspace/willRenameFiles", params.clone(), 15_000);
    let import_edit = match via_lsp {
        Ok(e) if e.get("changes").is_some() || e.get("documentChanges").is_some() => e,
        // LSP indisponível/vazio → fallback tsserver p/ TS. Se o backend caiu, reinicia e tenta.
        other => {
            if let Err(e) = &other {
                if is_conn_dead(e) {
                    client = srv.restart_client(project, backend)?;
                    client.ensure_open(&src_abs)?;
                }
            }
            if is_ts {
                match tsserver_rename_edit(&client, &src_abs, &dest_abs) {
                    Ok(e) => e,
                    Err(e) => {
                        return Err(format!(
                            "move_file: nem willRenameFiles nem getEditsForFileRename (vtsls) responderam: {e}"
                        ))
                    }
                }
            } else {
                // Server não suporta willRenameFiles e não é TS → não sabemos consertar os imports
                // com segurança. Honesto: unsupported (não move às cegas, que deixaria imports quebrados).
                return Ok(json!({
                    "operation": "move_file", "source": source, "dest": dest,
                    "applied": false, "safe": false, "unsupported": true, "error": "willrename_unsupported",
                    "detail": format!("o backend '{}' não respondeu workspace/willRenameFiles — não é seguro mover sem consertar os importers. Suportado hoje em TypeScript (vtsls).", backend),
                }));
            }
        }
    };

    // O WorkspaceEdit do willRenameFiles pode referenciar o oldUri (o server descreve edits COMO SE
    // o arquivo ainda estivesse no lugar antigo, mas às vezes já usa o novo). Coletamos os importers
    // (arquivos != source que serão editados) para reportar o blast.
    let root = client.root().to_string();
    let by_file = edits_by_file(&import_edit);
    let importers: Vec<String> = by_file
        .keys()
        .filter(|p| **p != src_abs && **p != dest_abs)
        .map(|p| rel(&root, &path_to_uri(p)))
        .collect();
    let (_bf, import_edits_n, _pf) = summarize_edit(&import_edit, &root);

    // PREVIEW (apply=false): simula os edits de import EM MEMÓRIA (sem mover o arquivo) e mostra o
    // blast. Não movemos no disco no preview — só medimos se os edits de import são seguros.
    if !apply {
        let sim = simulate(&client, &import_edit, project, build_lang(source))?;
        return Ok(json!({
            "operation": "move_file", "source": source, "dest": dest,
            "applied": false, "mode": "preview",
            "safe": sim["safe"].clone(),
            "net_delta": sim["net_delta"].clone(),
            "errors_introduced": sim["errors_introduced"].clone(),
            "importers": importers,
            "import_edits": import_edits_n,
            "note": "preview: o arquivo NÃO foi movido; medimos só os edits de import em memória. Use apply=true (idealmente com verify_build=true, essencial p/ pyright #1888) para mover e consertar os imports.",
        }));
    }

    // APPLY: move o arquivo no disco PRIMEIRO (rename), depois aplica os edits de import via o núcleo.
    // Se algo falhar (edits inseguros OU build quebrado com verify_build), REVERTEMOS o move também.
    do_move_file(&src_abs, &dest_abs)?;
    // O willRenameFiles descreveu os edits antes do move; reabrimos o arquivo no NOVO caminho para o
    // server ter o conteúdo. (verify_and_apply reabre os afetados; garantimos o novo aqui.)
    client.ensure_open(&dest_abs).ok();

    // Filtra do import_edit quaisquer edits sobre o PRÓPRIO arquivo movido no caminho ANTIGO (o
    // server pode ter incluído; o arquivo não está mais lá). Mantém só edits em arquivos existentes.
    let filtered = filter_edit_to_existing(&import_edit, &src_abs);

    let apply_res = apply_if_safe(
        &client,
        &filtered,
        verify_build,
        project,
        build_lang(source),
    );
    match apply_res {
        Ok(mut result) => {
            let applied_ok = result["applied"].as_bool().unwrap_or(false);
            if !applied_ok {
                // edits de import inseguros (net_delta>0) OU build quebrou e verify_and_apply reverteu
                // os arquivos-texto — mas o MOVE do arquivo é NOSSO, então revertemos aqui também.
                let _ = std::fs::rename(&dest_abs, &src_abs);
                client.ensure_open(&src_abs).ok();
                result["operation"] = json!("move_file");
                result["source"] = json!(source);
                result["dest"] = json!(dest);
                result["moved"] = json!(false);
                result["reverted"] = json!(true);
                result["importers"] = json!(importers);
                result["note"] = json!("REVERTIDO: o move foi desfeito porque consertar os importers introduziria erros (net_delta>0) ou o build falhou (ex.: pyright #1888 não ajusta o diretório — verify_build pegou).");
                return Ok(result);
            }
            result["operation"] = json!("move_file");
            result["source"] = json!(source);
            result["dest"] = json!(dest);
            result["moved"] = json!(true);
            result["importers"] = json!(importers);
            Ok(result)
        }
        Err(e) => {
            // erro duro no apply → reverte o move para deixar o disco consistente.
            let _ = std::fs::rename(&dest_abs, &src_abs);
            Err(format!(
                "move_file: falha ao aplicar os edits de import ({e}); o move foi revertido"
            ))
        }
    }
}

// F8: remove de um WorkspaceEdit quaisquer changes sobre `drop_abs` (o arquivo movido, no caminho
// antigo) e sobre arquivos que não existem mais no disco — mantendo só edits aplicáveis nos
// importers. Preserva o shape `changes` que o núcleo consome.
fn filter_edit_to_existing(edit: &Value, drop_abs: &str) -> Value {
    let by_file = edits_by_file(edit);
    let mut changes = serde_json::Map::new();
    for (path, edits) in by_file {
        if path == drop_abs || !Path::new(&path).exists() {
            continue;
        }
        changes.insert(path_to_uri(&path), json!(edits));
    }
    json!({ "changes": Value::Object(changes) })
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
    // I1: cada símbolo carrega `content` + ~2 linhas de contexto (via helper compartilhado) — assim
    // o modelo pega a assinatura sem reabrir o arquivo. `at` fica só linha:col (o arquivo já é o param).
    let root = client.root().to_string();
    let mut cache = SourceCache::default();
    let list: Vec<Value> = flat
        .into_iter()
        .map(|(fp, k, l, c)| {
            let (rl, rc) = refine_at(&flines, &fp, l, c);
            let mut loc = format_location(&mut cache, &root, &abs, rl, rc);
            // `at` local (só linha:col) para não repetir o path do arquivo já informado no topo.
            loc["at"] = json!(format!("{}:{}", rl + 1, rc + 1));
            json!({"name_path": fp, "kind": kind_label(k, &flines, rl),
                   "at": loc["at"].clone(), "content": loc["content"].clone(), "context": loc["context"].clone()})
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
    // I1: cada match traz `content` + contexto (helper compartilhado) — evita re-leitura do arquivo.
    let root = client.root().to_string();
    let mut cache = SourceCache::default();
    let matches: Vec<Value> = flat
        .iter()
        .filter(|(fp, ..)| name_path_matches(fp, name_path))
        .map(|(fp, k, l, c)| {
            let (rl, rc) = refine_at(&flines, fp, *l, *c);
            let mut loc = format_location(&mut cache, &root, &abs, rl, rc);
            loc["at"] = json!(format!("{}:{}", rl + 1, rc + 1));
            json!({"name_path": fp, "kind": kind_name(*k),
                   "at": loc["at"].clone(), "content": loc["content"].clone(), "context": loc["context"].clone()})
        })
        .collect();
    Ok(json!({"file": file, "query": name_path, "count": matches.len(), "matches": matches}))
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
    // I1: cada símbolo do workspace vira `at` (path:line:col) + `content` + contexto (helper
    // compartilhado). Como abrange VÁRIOS arquivos, o SourceCache evita reler o mesmo arquivo.
    let mut cache = SourceCache::default();
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
            let loc = format_location_uri(&mut cache, &root, uri, l, c);
            json!({"name": s["name"].as_str().unwrap_or(""),
                   "kind": kind_name(s["kind"].as_u64().unwrap_or(0)),
                   "at": loc["at"].clone(), "content": loc["content"].clone(), "context": loc["context"].clone()})
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
    // I1: cada chamador e cada call-site carrega `content` + contexto (helper compartilhado) — o
    // modelo vê a LINHA da chamada sem reabrir o arquivo do chamador.
    let mut cache = SourceCache::default();
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
            // devolvíamos a def do chamador (1), subcontando o blast radius. Agora expõe cada site
            // como `at`+`content`+`context` (não só a string path:linha:col).
            let sites: Vec<Value> = call["fromRanges"]
                .as_array()
                .map(|rs| {
                    rs.iter()
                        .map(|r| {
                            let sl0 = r["start"]["line"].as_u64().unwrap_or(0);
                            let sc0 = r["start"]["character"].as_u64().unwrap_or(0);
                            format_location_uri(&mut cache, &root, uri, sl0, sc0)
                        })
                        .collect()
                })
                .unwrap_or_default();
            call_site_total += sites.len().max(1) as u64;
            let caller_loc = format_location_uri(&mut cache, &root, uri, fl, fc);
            json!({"caller": from["name"].as_str().unwrap_or(""),
                   "at": caller_loc["at"].clone(), "content": caller_loc["content"].clone(),
                   "context": caller_loc["context"].clone(),
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
// I2: lacunas de config pyright que causam find_references INCOMPLETO em silêncio. `cfg_blob` é a
// concatenação de pyrightconfig.json + pyproject.toml (busca textual barata). Retorna os itens
// FALTANDO (vazio = ok). `venv_on_disk` só cobra venv/venvPath se existe um virtualenv no disco
// (sem venv no disco, não há imports de venv p/ resolver → não é lacuna).
fn py_config_gaps(cfg_blob: &str, venv_on_disk: bool) -> Vec<&'static str> {
    let mut gaps = vec![];
    if !cfg_blob.contains("include") {
        gaps.push("include (raízes de código, ex.: [\"src\",\"tests\"])");
    }
    // "venv" cobre tanto a chave venv quanto venvPath.
    if venv_on_disk && !cfg_blob.contains("venv") {
        gaps.push("venv/venvPath (resolver imports do virtualenv)");
    }
    gaps
}

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
    // Contexto p/ a sonda de contagem (I2): num projeto multi-arquivo, um símbolo referenciável
    // (classe/função exportada) que retorna 0-1 refs ESTÁVEIS é o assinatura do bug silencioso de
    // config (pachamama: 5 vs 61 refs, stable:true nos dois). Só alertamos se o projeto tem >=3
    // arquivos-fonte (evita falso-alarme em projeto minúsculo/1-arquivo, onde 1 ref é normal).
    let file_scale = find_source_files(project, ext, 3).len();
    let start = Instant::now();
    let mut attempts = 0u32;
    let mut tried = 0u32;
    let mut last_ident = String::new();
    let mut last_file = String::new();
    // Sonda de contagem: o 1º símbolo referenciável que resolveu ESTÁVEL com count baixo (0-1).
    // Guardamos p/ anexar como hint mesmo quando outro símbolo depois passa (count>=2).
    let mut low_probe: Option<Value> = None;
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
            // Sonda de contagem (I2): count>=2 num referenciável já descarta o bug de config —
            // retorna já com ref_probe:ok. count 0-1 estável em projeto multi-arquivo é suspeito:
            // guardamos o hint e continuamos tentando outros símbolos (podem ter mais refs).
            if stable && refs.len() >= 2 {
                return json!({"ran": true, "ok": true, "symbol": ident, "file": rel_file,
                    "count": refs.len(), "stable": true, "warmup_ms": ms, "polls": polls,
                    "ref_probe": {"ok": true, "count": refs.len()}});
            }
            if stable && file_scale >= 3 && low_probe.is_none() {
                low_probe = Some(
                    json!({"symbol": ident, "file": rel_file, "count": refs.len(),
                    "stable": true,
                    "hint": format!(
                        "HINT (não é erro): o referenciável '{ident}' retornou só {} ref(s) ESTÁVEL num projeto com múltiplos arquivos. Pode ser símbolo realmente sem uso — OU config de workspace faltando fazendo find_references sair INCOMPLETO em silêncio (ex.: Python sem [tool.basedpyright] include/venv: pachamama deu 5 vs 61 refs, stable:true nos dois). Confirme com um grep e veja docs/LANGUAGE-SETUP.md.",
                        refs.len())}),
                );
            }
        }
        if start.elapsed().as_millis() + 2_000 >= total_budget {
            break;
        }
    }
    // Chegou aqui: nenhum símbolo passou com count>=2. Se algum resolveu ESTÁVEL com 0-1 ref num
    // projeto multi-arquivo, o smoke "rodou" (o server respondeu), mas levantamos o hint de sonda
    // de contagem — é exatamente a assinatura do bug silencioso de config.
    if let Some(probe) = low_probe {
        return json!({"ran": true, "ok": true, "symbols_tried": tried,
            "count": probe["count"].clone(), "stable": true,
            "symbol": probe["symbol"].clone(), "file": probe["file"].clone(),
            "ref_probe": {"ok": false, "low_ref": true,
                "count": probe["count"].clone(), "hint": probe["hint"].clone()}});
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
                // Detecção do bug silencioso #1 (pachamama 5 vs 61 refs). Dois casos:
                //  (a) NENHUMA config pyright → basedpyright roda em openFilesOnly → refs incompletas.
                //  (b) config EXISTE mas sem include/venv/venvPath → ainda pode sair incompleto
                //      (sem include o server não sabe as raízes; sem venv não resolve imports do venv).
                let pyrightconfig =
                    std::fs::read_to_string(p.join("pyrightconfig.json")).unwrap_or_default();
                let has_cfg = !pyrightconfig.trim().is_empty();
                let pyproject =
                    std::fs::read_to_string(p.join("pyproject.toml")).unwrap_or_default();
                let has_tool = pyproject.contains("[tool.basedpyright]")
                    || pyproject.contains("[tool.pyright]");
                // Concatena as duas fontes p/ checar as chaves relevantes (busca textual barata: a
                // chave pode estar em qualquer uma; pyrightconfig.json usa "include"/"venv", o
                // pyproject.toml usa include/venv sob [tool.*]).
                let cfg_blob = format!("{pyrightconfig}\n{pyproject}");
                let venv_on_disk = [".venv", "venv", "env"]
                    .iter()
                    .find(|d| p.join(d).join("pyvenv.cfg").exists())
                    .copied();
                let cfg_gaps = py_config_gaps(&cfg_blob, venv_on_disk.is_some());
                if !has_cfg && !has_tool {
                    cfg_ok = false;
                    issue = json!("sem [tool.basedpyright]/pyrightconfig.json → find_references INCOMPLETO em SILÊNCIO (basedpyright cai em openFilesOnly; pachamama deu 5 vs 61 refs, stable:true nos dois)");
                    let src = if p.join("src").is_dir() { "src" } else { "." };
                    let venv = venv_on_disk;
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
                } else if !cfg_gaps.is_empty() {
                    // Caso (b): config existe mas INCOMPLETA. Não é falha dura (mantém cfg_ok=true,
                    // deixa o smoke rodar) — é um WARN, porque sem include/venv o resultado ainda
                    // pode sair parcial em silêncio. Sinalizamos via issue/fix sem bloquear.
                    let faltando = cfg_gaps.join(" e ");
                    issue = json!(format!(
                        "config pyright presente mas SEM {faltando} → risco de find_references INCOMPLETO em silêncio (pachamama: 5 vs 61 refs). WARN, não bloqueio."
                    ));
                    fix_desc = json!(format!(
                        "adicione {faltando} à config existente (ver docs/LANGUAGE-SETUP.md)"
                    ));
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
    // Warnings SOFT (I2): não contam como `problems` (não bloqueiam), mas são a assinatura do bug
    // silencioso #1 — juntamos aqui p/ ficarem visíveis sem escavar cada entrada do report:
    //  - config de workspace presente mas incompleta (cfg_ok=true + issue não-nula);
    //  - sonda de contagem: um referenciável retornou 0-1 ref estável (ref_probe.low_ref).
    let mut warnings = vec![];
    for e in &report {
        if e["workspace_config"]["ok"].as_bool().unwrap_or(true)
            && !e["workspace_config"]["issue"].is_null()
        {
            warnings.push(
                json!({"lang": e["lang"].clone(), "kind": "config_incompleta",
                "detail": e["workspace_config"]["issue"].clone()}),
            );
        }
        if e["smoke"]["ref_probe"]["low_ref"]
            .as_bool()
            .unwrap_or(false)
        {
            warnings.push(json!({"lang": e["lang"].clone(), "kind": "ref_count_baixa",
                "detail": e["smoke"]["ref_probe"]["hint"].clone()}));
        }
    }
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
        "warnings": warnings, // avisos soft (config incompleta / ref-count baixa) — não bloqueiam
        "report": report,
        "hint": hint,
        "error_log": log_file_path().map(|p| p.to_string_lossy().into_owned()),
    }))
}

// G1: manual completo de uso sob demanda. Devolve `guidance::FULL_MANUAL` (a FONTE ÚNICA da verdade
// — o campo `instructions` do initialize é um excerto do mesmo texto). Sem argumentos.
fn tool_instructions() -> Result<Value, String> {
    Ok(json!({
        "manual": guidance::FULL_MANUAL,
        "note": "Guidance server-side (portátil a qualquer cliente MCP). O campo `instructions` do initialize traz um excerto curto deste mesmo manual.",
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
            "name": "organize_imports",
            "description": "Organiza os imports de um arquivo (reordena, deduplica e remove NÃO-USADOS) via a source-action do language server (vtsls no TS). SEGURO onde um sed/grep erraria: o LSP conhece o USO REAL, então NÃO remove import de side-effect (`import \"./polyfill\"`) nem type-only ainda usado. Mesmo ciclo apply→verify com net_delta: apply=false (default) = preview; apply=true persiste só se net_delta<=0. Se o backend não oferecer a ação, retorna unsupported em vez de fingir sucesso.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "project": {"type": "string"}, "file": {"type": "string"},
                    "apply": {"type": "boolean", "description": "false=preview (default); true=aplica no disco se seguro"},
                    "verify_build": {"type": "boolean", "description": "com apply=true: roda o build e reverte se falhar"}
                },
                "required": ["project", "file"]
            }
        },
        {
            "name": "safe_delete",
            "description": "Deleta um símbolo (função/classe/método/tipo/variável) APENAS se ele NÃO tiver referências fora da própria definição. Funde find_references (gate de warmup: índice frio → ERRO, nunca falso '0 refs') + net_delta + verify_build num único gate. Se houver USO externo, RECUSA e lista os locais (path:linha:conteúdo). Se zero, apaga a declaração inteira via o mesmo apply→verify. apply=false (default) = preview; apply=true persiste só se seguro. 'symbol' aceita name_path ('Classe/metodo').",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "project": {"type": "string"}, "file": {"type": "string"},
                    "symbol": {"type": "string", "description": "nome ou name_path (ex.: 'unusedHelper' ou 'Widget/oldMethod')"},
                    "line": {"type": "integer", "description": "opcional: linha 1-indexed para desambiguar"},
                    "apply": {"type": "boolean", "description": "false=preview (default); true=aplica no disco se seguro"},
                    "verify_build": {"type": "boolean", "description": "com apply=true: roda o build e reverte se falhar"}
                },
                "required": ["project", "file", "symbol"]
            }
        },
        {
            "name": "replace_symbol_body",
            "description": "Substitui a declaração/corpo COMPLETO de um símbolo (função/classe/método/tipo/variável) — alvo por NOME/name_path, NUNCA por coordenadas cruas. Resolve o range da declaração via documentSymbol e passa pelo mesmo apply→verify com net_delta das demais edições: apply=false (default) = preview (mede e reverte); apply=true persiste só se net_delta<=0. 'symbol' aceita name_path ('Classe/metodo').",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "project": {"type": "string"}, "file": {"type": "string"},
                    "symbol": {"type": "string", "description": "nome ou name_path (ex.: 'compute' ou 'Widget/render')"},
                    "text": {"type": "string", "description": "o texto NOVO da declaração inteira (substitui o range completo do símbolo)"},
                    "line": {"type": "integer", "description": "opcional: linha 1-indexed para desambiguar homônimos"},
                    "apply": {"type": "boolean", "description": "false=preview (default); true=aplica no disco se seguro"},
                    "verify_build": {"type": "boolean", "description": "com apply=true: roda o build e reverte se falhar"}
                },
                "required": ["project", "file", "symbol", "text"]
            }
        },
        {
            "name": "insert_before_symbol",
            "description": "Insere texto IMEDIATAMENTE ANTES da declaração de um símbolo (alvo por NOME/name_path, sem coordenadas cruas). Útil para adicionar um decorator, comentário, overload ou uma nova declaração-irmã acima. Mesmo apply→verify com net_delta: apply=false (default) = preview; apply=true persiste só se seguro.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "project": {"type": "string"}, "file": {"type": "string"},
                    "symbol": {"type": "string", "description": "nome ou name_path"},
                    "text": {"type": "string", "description": "o texto a inserir antes do símbolo (uma quebra de linha é garantida)"},
                    "line": {"type": "integer", "description": "opcional: linha 1-indexed para desambiguar"},
                    "apply": {"type": "boolean", "description": "false=preview (default); true=aplica se seguro"},
                    "verify_build": {"type": "boolean"}
                },
                "required": ["project", "file", "symbol", "text"]
            }
        },
        {
            "name": "insert_after_symbol",
            "description": "Insere texto IMEDIATAMENTE DEPOIS da declaração de um símbolo (alvo por NOME/name_path, sem coordenadas cruas). Útil para adicionar uma nova declaração-irmã logo abaixo. Mesmo apply→verify com net_delta: apply=false (default) = preview; apply=true persiste só se seguro.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "project": {"type": "string"}, "file": {"type": "string"},
                    "symbol": {"type": "string", "description": "nome ou name_path"},
                    "text": {"type": "string", "description": "o texto a inserir depois do símbolo (uma quebra de linha é garantida)"},
                    "line": {"type": "integer", "description": "opcional: linha 1-indexed para desambiguar"},
                    "apply": {"type": "boolean", "description": "false=preview (default); true=aplica se seguro"},
                    "verify_build": {"type": "boolean"}
                },
                "required": ["project", "file", "symbol", "text"]
            }
        },
        {
            "name": "blast_radius",
            "description": "Superfície de RISCO de mexer num símbolo, ANTES de editar. READ-ONLY e COMPOSTO sobre as tools existentes (find_references + call_hierarchy) — não roda nenhuma operação nova. Retorna as referências e os chamadores PARTICIONADOS em test vs não-test (produção), os arquivos afetados e um resumo de contagens. Passa pelo gate de warmup: índice frio → ERRO, nunca um raio vazio enganoso. Locais no formato path:linha:conteúdo. 'symbol' aceita name_path.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "project": {"type": "string"}, "file": {"type": "string"},
                    "symbol": {"type": "string", "description": "nome ou name_path (ex.: 'compute' ou 'Widget/render')"},
                    "line": {"type": "integer", "description": "opcional: linha 1-indexed para desambiguar"}
                },
                "required": ["project", "file", "symbol"]
            }
        },
        {
            "name": "quick_fix",
            "description": "Aplica UMA correção (quick-fix) do language server para um diagnóstico numa LINHA específica (ex.: import faltando, remover não-usado, adicionar await). Usa o executor de code-action interno (kind 'quickfix'), NÃO um code_action cru/genérico. Escolhe a 1ª ação casável (ou a que bate 'prefer_title') e passa pelo mesmo apply→verify com net_delta: apply=false (default) = preview; apply=true persiste só se seguro. Se não houver correção para aquele diagnóstico, retorna unsupported/none honesto (não finge sucesso). No TS roteia p/ vtsls.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "project": {"type": "string"}, "file": {"type": "string"},
                    "line": {"type": "integer", "description": "linha 1-indexed do diagnóstico a corrigir"},
                    "prefer_title": {"type": "string", "description": "opcional: escolhe a ação cujo título contém este texto (ex.: 'Add import')"},
                    "apply": {"type": "boolean", "description": "false=preview (default); true=aplica no disco se seguro"},
                    "verify_build": {"type": "boolean", "description": "com apply=true: roda o build e reverte se falhar"}
                },
                "required": ["project", "file", "line"]
            }
        },
        {
            "name": "change_signature",
            "description": "Muda a assinatura de uma função/método (adiciona, remove ou reordena um parâmetro) E ATUALIZA TODOS os call-sites juntos. Onde nenhum language server oferece esse refactor nativo (TypeScript/vtsls, rust-analyzer, pyright — a maioria), constrói o WorkspaceEdit À MÃO: descobre os chamadores via call_hierarchy (gate de warmup: índice frio → ERRO, nunca callers faltando em silêncio), reescreve a declaração e cada chamada, e então SIMULA net_delta + verify_build antes de aplicar (apply=false=preview; apply=true persiste só se net_delta<=0). Se não der para reescrever com segurança um call-site (ou a declaração), retorna unsupported em vez de aplicar uma edição PARCIAL/perigosa. spec: op='add' exige 'index','param' (texto na decl) e 'arg' (valor nos call-sites); op='remove' exige 'index'; op='reorder' exige 'order' (permutação dos índices). Índices 0-based. 'symbol' aceita name_path.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "project": {"type": "string"}, "file": {"type": "string"},
                    "symbol": {"type": "string", "description": "nome ou name_path da função/método (ex.: 'compute' ou 'Widget/render')"},
                    "spec": {"type": "object", "description": "a mudança (UMA op): {\"op\":\"add\",\"index\":<i>,\"param\":\"x: number\",\"arg\":\"0\"} | {\"op\":\"remove\",\"index\":<i>} | {\"op\":\"reorder\",\"order\":[1,0]}"},
                    "line": {"type": "integer", "description": "opcional: linha 1-indexed para desambiguar homônimos"},
                    "apply": {"type": "boolean", "description": "false=preview (default); true=aplica no disco se net_delta<=0"},
                    "verify_build": {"type": "boolean", "description": "com apply=true: roda o build e reverte se falhar (pega erros de aridade/tipo que a simulação em memória pode não ver)"}
                },
                "required": ["project", "file", "symbol", "spec"]
            }
        },
        {
            "name": "move_file",
            "description": "Move/renomeia um ARQUIVO INTEIRO para um novo caminho e conserta TODOS os importers/re-exports/barrels que apontavam para ele (via workspace/willRenameFiles do language server). DIFERE de move_symbol: move_symbol tira UM símbolo de um arquivo e o põe em outro; move_file move o arquivo todo + o fixup de imports. apply=false (default) = preview (NÃO move; só mede os edits de import em memória e lista os importers); apply=true move no disco e aplica os edits via net_delta, com verify_build opcional. basedpyright tem bug conhecido aqui (#1888: willRenameFiles ignora o diretório) → use verify_build=true: se o move quebrar o build, o move é REVERTIDO (arquivo volta ao lugar). Se o backend não implementar willRenameFiles, retorna unsupported (não move às cegas). Confiável hoje em TypeScript (vtsls).",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "project": {"type": "string"},
                    "source": {"type": "string", "description": "caminho do arquivo a mover, RELATIVO ao project"},
                    "dest": {"type": "string", "description": "caminho de destino, RELATIVO ao project (recusa se já existir)"},
                    "apply": {"type": "boolean", "description": "false=preview sem mover (default); true=move e conserta os imports se seguro"},
                    "verify_build": {"type": "boolean", "description": "com apply=true: roda o build após mover e REVERTE o move se falhar (essencial p/ pyright #1888)"}
                },
                "required": ["project", "source", "dest"]
            }
        },
        {
            "name": "simulate_edit",
            "description": "Simula uma edição PROPOSTA por VOCÊ em memória (net_delta) SEM tocar o disco e retorna o veredito safe/unsafe + os erros introduzidos e resolvidos (errors_introduced/errors_resolved). É o MESMO motor que rename/extract/move/safe_delete usam para decidir. Informe a edição de UMA das formas: 'new_content' (o conteúdo COMPLETO proposto do arquivo), 'edits' (lista de {start_line,end_line 1-indexed, start_col,end_col 0-indexed opcionais, new_text}) ou 'edit' (um WorkspaceEdit LSP cru). Use ANTES de aplicar para saber se a mudança quebra o build.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "project": {"type": "string", "description": "caminho ABSOLUTO da raiz do projeto"},
                    "file": {"type": "string", "description": "arquivo RELATIVO ao project"},
                    "new_content": {"type": "string", "description": "conteúdo COMPLETO proposto do arquivo (substitui o arquivo inteiro)"},
                    "edits": {"type": "array", "description": "lista de edições por range: {start_line,end_line (1-indexed), start_col,end_col (0-indexed, opcionais), new_text}",
                        "items": {"type": "object"}},
                    "edit": {"type": "object", "description": "alternativa avançada: um WorkspaceEdit LSP cru (changes/documentChanges)"}
                },
                "required": ["project", "file"]
            }
        },
        {
            "name": "preview_edit",
            "description": "Mostra o que uma edição PROPOSTA por VOCÊ mudaria: o WorkspaceEdit resolvido + um diff unificado por arquivo + o blast (arquivos/edições tocados). READ-ONLY: não simula diagnostics nem escreve no disco (use simulate_edit p/ o veredito de segurança e safe_apply p/ aplicar). Mesma representação de edição do simulate_edit ('new_content' | 'edits' | 'edit').",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "project": {"type": "string"},
                    "file": {"type": "string"},
                    "new_content": {"type": "string", "description": "conteúdo COMPLETO proposto do arquivo"},
                    "edits": {"type": "array", "description": "lista de edições por range (ver simulate_edit)", "items": {"type": "object"}},
                    "edit": {"type": "object", "description": "WorkspaceEdit LSP cru (avançado)"}
                },
                "required": ["project", "file"]
            }
        },
        {
            "name": "safe_apply",
            "description": "Aplica uma edição PROPOSTA por VOCÊ no disco SÓ SE net_delta<=0 (não introduz erros novos); caso contrário RECUSA e devolve os erros introduzidos SEM tocar o disco. Mesmo motor (simular→gate→apply) das demais tools de edição. Com verify_build=true roda o build da linguagem após aplicar e REVERTE se falhar (pega erros que o net_delta em memória não vê, ex.: cargo check). Mesma representação de edição do simulate_edit ('new_content' | 'edits' | 'edit').",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "project": {"type": "string"},
                    "file": {"type": "string"},
                    "new_content": {"type": "string", "description": "conteúdo COMPLETO proposto do arquivo"},
                    "edits": {"type": "array", "description": "lista de edições por range (ver simulate_edit)", "items": {"type": "object"}},
                    "edit": {"type": "object", "description": "WorkspaceEdit LSP cru (avançado)"},
                    "verify_build": {"type": "boolean", "description": "roda o build da linguagem após aplicar e REVERTE se falhar"}
                },
                "required": ["project", "file"]
            }
        },
        {
            "name": "doctor",
            "description": "Verifica o setup do projeto por linguagem: language server disponível + config de workspace correta (senão find_references sai incompleto EM SILÊNCIO — crítico em Python: além de 'sem config', pega config presente mas SEM include/venv). Com fix=true, corrige o que dá (ex.: cria pyrightconfig.json). Com smoke=true, roda um find_references REAL end-to-end (gate de warmup) e, como SONDA DE CONTAGEM, alerta (warning soft, não bloqueio) se um símbolo referenciável retorna 0-1 ref estável num projeto multi-arquivo — assinatura do bug pachamama (5 vs 61 refs). Veja o campo 'warnings' na resposta. Rode uma vez ao abrir um projeto novo.",
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
        },
        {
            "name": "instructions",
            "description": "Manual COMPLETO de uso do code-intel (fonte única da verdade; o campo `instructions` do initialize é um excerto curto deste texto). Puxe sob demanda quando precisar do guia inteiro: roteamento semântico vs. grep, confiar no gate de warmup (não reler), preview/simulate antes de safe_apply, blast_radius antes de editar amplo, safe_delete, grep-sweep pós-rename, loop de diagnostics de nível-projeto e setup via doctor. Sem argumentos.",
            "inputSchema": {
                "type": "object",
                "properties": {},
                "required": []
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
        "organize_imports" => tool_organize_imports(srv, args),
        "safe_delete" => tool_safe_delete(srv, args),
        "replace_symbol_body" => tool_replace_symbol_body(srv, args),
        "insert_before_symbol" => tool_insert_before_symbol(srv, args),
        "insert_after_symbol" => tool_insert_after_symbol(srv, args),
        "blast_radius" => tool_blast_radius(srv, args),
        "quick_fix" => tool_quick_fix(srv, args),
        "change_signature" => tool_change_signature(srv, args),
        "move_file" => tool_move_file(srv, args),
        "simulate_edit" => tool_simulate_edit(srv, args),
        "preview_edit" => tool_preview_edit(srv, args),
        "safe_apply" => tool_safe_apply(srv, args),
        "validate_build" => tool_validate_build(srv, args),
        "doctor" => tool_doctor(srv, args),
        "instructions" => tool_instructions(),
        other => Err(format!("ferramenta desconhecida: {other}")),
    };
    match res {
        Ok(v) => {
            json!({"content":[{"type":"text","text": serde_json::to_string_pretty(&v).unwrap()}]})
        }
        Err(e) => {
            log_event("tool_error", name, args, &e); // descobre erros de campo (log local)
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
    // panics vão pro log local (além do stderr) — descobre crashes na máquina do usuário.
    std::panic::set_hook(Box::new(|info| {
        let msg = info.to_string();
        log_event("panic", "-", &Value::Null, &msg);
        eprintln!("code-intel-mcp panic: {msg}");
    }));
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
                    "serverInfo": {"name": "code-intel-mcp", "version": env!("CARGO_PKG_VERSION")},
                    // G1: guidance portátil (todo cliente MCP herda). Excerto CURTO (C13); manual
                    // completo via a tool `instructions` (mesma fonte, guidance::*).
                    "instructions": guidance::SHORT_INSTRUCTIONS
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

    // I1: clip_line NUNCA corta no meio de um char UTF-8 e anexa '…' quando trunca.
    #[test]
    fn clip_line_respects_utf8_boundary() {
        assert_eq!(clip_line("abc", 10), "abc"); // curta: intacta
        assert_eq!(clip_line("abcdef", 3), "abc…"); // truncada + reticências
                                                    // multibyte: 5 'é' (2 bytes cada) truncado em 3 chars não pode cortar no meio do byte
        let s = "ééééé";
        let clipped = clip_line(s, 3);
        assert_eq!(clipped, "ééé…");
        assert!(clipped.is_char_boundary(clipped.len())); // string válida
    }

    // I1: format_location produz `at` (path:linha:col 1-indexed), `content` e ~2 linhas de contexto
    // clampadas nos limites do arquivo, com prefixo de nº de linha e SEM a própria linha no contexto.
    #[test]
    fn format_location_content_and_context() {
        let dir = std::env::temp_dir().join(format!("fmtloc-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let f = dir.join("a.ts");
        std::fs::write(&f, "l0\nl1\nTARGET\nl3\nl4\n").unwrap();
        let abs = f.to_string_lossy().to_string();
        let root = dir.to_string_lossy().to_string();
        let mut cache = SourceCache::default();
        // linha 0-indexed 2 = "TARGET"; col 0
        let loc = format_location(&mut cache, &root, &abs, 2, 0);
        assert_eq!(loc["at"], json!("a.ts:3:1"));
        assert_eq!(loc["content"], json!("TARGET"));
        // contexto: linhas 1..=4 exceto a 3 → "1: l0","2: l1","4: l3","5: l4"
        let ctx: Vec<String> = loc["context"]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v.as_str().unwrap().to_string())
            .collect();
        assert_eq!(ctx, vec!["1: l0", "2: l1", "4: l3", "5: l4"]);
        std::fs::remove_dir_all(&dir).ok();
    }

    // I1: contexto clampa no TOPO do arquivo (linha 0 não tem 2 acima).
    #[test]
    fn format_location_clamps_at_file_start() {
        let dir = std::env::temp_dir().join(format!("fmtloc2-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let f = dir.join("b.ts");
        std::fs::write(&f, "first\nsecond\nthird\n").unwrap();
        let abs = f.to_string_lossy().to_string();
        let root = dir.to_string_lossy().to_string();
        let mut cache = SourceCache::default();
        let loc = format_location(&mut cache, &root, &abs, 0, 0);
        assert_eq!(loc["content"], json!("first"));
        let ctx = loc["context"].as_array().unwrap();
        assert_eq!(ctx.len(), 2); // só as 2 linhas abaixo (nada acima da 1ª)
        std::fs::remove_dir_all(&dir).ok();
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

    // I2: doctor detecta config pyright INCOMPLETA (bug silencioso pachamama 5-vs-61).
    #[test]
    fn py_config_gaps_flags_missing_keys() {
        // config completa (include + venv), com venv no disco → sem lacunas.
        assert!(
            py_config_gaps("{\"include\":[\"src\"],\"venv\":\".venv\"}", true).is_empty(),
            "config completa não deve gerar lacuna"
        );
        // sem include → lacuna de include, mesmo sem venv no disco.
        let g = py_config_gaps("{\"typeCheckingMode\":\"basic\"}", false);
        assert_eq!(g.len(), 1);
        assert!(g[0].starts_with("include"));
        // include presente mas venv AUSENTE com virtualenv no disco → lacuna de venv.
        let g = py_config_gaps("{\"include\":[\"src\"]}", true);
        assert_eq!(g.len(), 1);
        assert!(g[0].starts_with("venv"));
        // include presente e SEM venv no disco → não cobra venv (não há imports de venv a resolver).
        assert!(py_config_gaps("{\"include\":[\"src\"]}", false).is_empty());
        // venvPath conta como venv (busca por "venv" cobre venv e venvPath).
        assert!(py_config_gaps("{\"include\":[\".\"],\"venvPath\":\".\"}", true).is_empty());
        // config vazia com venv no disco → as DUAS lacunas.
        assert_eq!(py_config_gaps("", true).len(), 2);
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

    // F3: ref_in_def separa a própria definição (linha dentro do range da declaração) das refs de uso.
    #[test]
    fn ref_in_def_distinguishes_definition_from_uses() {
        let def = ((10u64, 0u64), (14u64, 1u64)); // declaração ocupa as linhas 10..=14
        assert!(ref_in_def(10, def)); // linha do identificador (a própria def)
        assert!(ref_in_def(12, def)); // dentro do corpo da declaração
        assert!(ref_in_def(14, def)); // última linha da declaração
        assert!(!ref_in_def(20, def)); // uso externo, abaixo
        assert!(!ref_in_def(3, def)); // uso externo, acima
    }

    // F6: is_test_path particiona refs/callers em test vs produção (cobre TS/JS/Py/Rust/Dart/C#).
    #[test]
    fn is_test_path_partitions_test_vs_prod() {
        // dir de teste
        assert!(is_test_path("src/__tests__/widget.ts"));
        assert!(is_test_path("packages/core/test/main.dart"));
        assert!(is_test_path("tests/test_utils.py"));
        // sufixos por arquivo
        assert!(is_test_path("src/widget.test.ts"));
        assert!(is_test_path("src/widget.spec.ts"));
        assert!(is_test_path("mod_test.rs"));
        assert!(is_test_path("WidgetTests.cs"));
        assert!(is_test_path("test_helpers.py"));
        // produção NÃO casa
        assert!(!is_test_path("src/widget.ts"));
        assert!(!is_test_path("packages/core/src/index.ts"));
        assert!(!is_test_path("src/latest.ts")); // 'latest' não é 'test' (word-ish, mas stem não termina em 'test' isolado)
    }

    // F3: sym_full_range pega o `range` (declaração inteira), não o selectionRange (só o nome).
    #[test]
    fn sym_full_range_prefers_range_over_selection() {
        let s = json!({
            "name": "foo",
            "range": {"start": {"line": 5, "character": 0}, "end": {"line": 9, "character": 1}},
            "selectionRange": {"start": {"line": 5, "character": 9}, "end": {"line": 5, "character": 12}}
        });
        assert_eq!(sym_full_range(&s), ((5, 0), (9, 1)));
        // fallback: SymbolInformation (só location.range)
        let si = json!({"name": "bar",
            "location": {"range": {"start": {"line": 2, "character": 0}, "end": {"line": 2, "character": 20}}}});
        assert_eq!(sym_full_range(&si), ((2, 0), (2, 20)));
    }

    // F1: build_workspace_edit aceita as 3 formas de edição e produz o MESMO shape (changes por URI).
    #[test]
    fn build_workspace_edit_accepts_all_forms() {
        let dir = std::env::temp_dir().join(format!("bwe-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let f = dir.join("x.ts");
        std::fs::write(&f, "const a = 1;\nconst b = 2;\n").unwrap();
        let abs = f.to_string_lossy().to_string();
        let uri = path_to_uri(&abs);

        // (1) WorkspaceEdit cru é repassado como está.
        let raw = json!({"edit": {"changes": {uri.clone(): [{"range": {"start":{"line":0,"character":0},"end":{"line":0,"character":1}}, "newText": "X"}]}}});
        let e1 = build_workspace_edit(&raw, &abs, &uri).unwrap();
        assert!(e1["changes"][&uri].is_array());

        // (2) new_content vira um único edit cobrindo o arquivo inteiro.
        let nc = json!({"new_content": "const a = 42;\n"});
        let e2 = build_workspace_edit(&nc, &abs, &uri).unwrap();
        assert_eq!(e2["changes"][&uri][0]["newText"], json!("const a = 42;\n"));

        // (3) edits 1-indexed → TextEdit 0-indexed.
        let ed = json!({"edits": [{"start_line": 2, "end_line": 2, "start_col": 6, "end_col": 7, "new_text": "b2"}]});
        let e3 = build_workspace_edit(&ed, &abs, &uri).unwrap();
        assert_eq!(e3["changes"][&uri][0]["range"]["start"]["line"], json!(1));
        assert_eq!(e3["changes"][&uri][0]["newText"], json!("b2"));

        // (3b) end_col omitido → substitui a LINHA INTEIRA (fim = nº de chars da linha), não insere.
        let ed2 = json!({"edits": [{"start_line": 1, "new_text": "const a = 9;"}]});
        let e4 = build_workspace_edit(&ed2, &abs, &uri).unwrap();
        let r = &e4["changes"][&uri][0]["range"];
        assert_eq!(r["start"]["character"], json!(0));
        assert_eq!(r["end"]["line"], json!(0));
        assert_eq!(r["end"]["character"], json!(12)); // "const a = 1;" = 12 chars → end exclusivo = 12

        // sem nenhuma forma → erro acionável.
        assert!(build_workspace_edit(&json!({}), &abs, &uri).is_err());
        std::fs::remove_dir_all(&dir).ok();
    }

    // F1: unified_diff emite hunk só sobre a região que difere (prefixo/sufixo comuns preservados).
    #[test]
    fn unified_diff_emits_minimal_hunk() {
        assert!(unified_diff("a.ts", "same\n", "same\n").is_empty()); // igual → vazio
        let d = unified_diff("a.ts", "l0\nOLD\nl2\n", "l0\nNEW\nl2\n");
        assert!(d.iter().any(|l| l == "-OLD"));
        assert!(d.iter().any(|l| l == "+NEW"));
        // prefixo/sufixo comuns NÃO aparecem como +/-
        assert!(!d
            .iter()
            .any(|l| l == "-l0" || l == "+l0" || l == "-l2" || l == "+l2"));
    }

    // F5: split_top_level respeita aninhamento e strings — não quebra em vírgula dentro de
    // genérico/objeto/string.
    #[test]
    fn split_top_level_respects_nesting() {
        assert_eq!(split_top_level("a, b, c"), vec!["a", " b", " c"]);
        assert_eq!(split_top_level(""), Vec::<String>::new());
        assert_eq!(split_top_level("  "), Vec::<String>::new());
        // vírgula dentro de genérico não separa
        assert_eq!(
            split_top_level("x: Map<string, number>, y: number"),
            vec!["x: Map<string, number>", " y: number"]
        );
        // vírgula dentro de string/objeto não separa
        assert_eq!(
            split_top_level("\"a, b\", { k: 1, j: 2 }"),
            vec!["\"a, b\"", " { k: 1, j: 2 }"]
        );
        // trailing comma não vira item vazio
        assert_eq!(split_top_level("a, b,"), vec!["a", " b"]);
    }

    // F5: paren_span acha o conteúdo entre os parênteses balanceados após o identificador.
    #[test]
    fn paren_span_finds_balanced_content() {
        let s = "foo(a, b)";
        let ie = "foo".len();
        assert_eq!(paren_span(s, ie), Some((4, 8))); // "a, b"
        assert_eq!(&s[4..8], "a, b");
        // aninhado: para no ')' externo, não no interno
        let s2 = "call(x, nested(1, 2), y)";
        let (cs, ce) = paren_span(s2, "call".len()).unwrap();
        assert_eq!(&s2[cs..ce], "x, nested(1, 2), y");
        // sem parênteses → None
        assert_eq!(paren_span("foo bar", 3), None);
    }

    // F5: apply_sig_op — add/remove/reorder sobre a lista, com erros honestos.
    #[test]
    fn apply_sig_op_add_remove_reorder() {
        let items: Vec<String> = vec!["a: number".into(), "b: number".into()];
        // add na decl usa 'param'; no call-site usa 'arg'
        let add = json!({"op":"add","index":2,"param":"c: number","arg":"0"});
        assert_eq!(
            apply_sig_op(&items, &add, true).unwrap(),
            vec!["a: number", "b: number", "c: number"]
        );
        let args: Vec<String> = vec!["1".into(), "2".into()];
        assert_eq!(
            apply_sig_op(&args, &add, false).unwrap(),
            vec!["1", "2", "0"]
        );
        // remove
        let rm = json!({"op":"remove","index":0});
        assert_eq!(apply_sig_op(&items, &rm, true).unwrap(), vec!["b: number"]);
        // reorder (permutação válida)
        let ro = json!({"op":"reorder","order":[1,0]});
        assert_eq!(
            apply_sig_op(&items, &ro, true).unwrap(),
            vec!["b: number", "a: number"]
        );
        // reorder inválido (não é permutação) → erro
        assert!(apply_sig_op(&items, &json!({"op":"reorder","order":[0,0]}), true).is_err());
        // remove fora do range → erro
        assert!(apply_sig_op(&items, &json!({"op":"remove","index":9}), true).is_err());
        // add no call-site sem 'arg' → erro (inseguro)
        assert!(apply_sig_op(
            &args,
            &json!({"op":"add","index":0,"param":"c: number"}),
            false
        )
        .is_err());
    }

    // F5: rewrite_list_at reescreve a lista completa (decl e call-site) end-to-end.
    #[test]
    fn rewrite_list_at_rewrites_decl_and_call() {
        let decl = "function compute(a: number, b: number): number {";
        let ie = "function compute".len();
        let spec = json!({"op":"remove","index":1});
        let (cs, ce, nt) = rewrite_list_at(decl, ie, &spec, true).unwrap();
        assert_eq!(&decl[cs..ce], "a: number, b: number");
        assert_eq!(nt, "a: number");
        // call-site: compute(2, 3) -> compute(2)
        let call = "return compute(2, 3);";
        let ie2 = "return compute".len();
        let (cs2, ce2, nt2) = rewrite_list_at(call, ie2, &spec, false).unwrap();
        assert_eq!(&call[cs2..ce2], "2, 3");
        assert_eq!(nt2, "2");
    }

    // F5: offset_to_pos é inverso de pos_to_offset (UTF-16 nas colunas).
    #[test]
    fn offset_to_pos_roundtrips() {
        let text = "l0\nsecond line\ncafé x\n";
        for &(l, c) in &[(0u64, 0u64), (1, 7), (2, 5)] {
            let off = pos_to_offset(text, l, c);
            assert_eq!(offset_to_pos(text, off), (l, c), "roundtrip @ {l}:{c}");
        }
    }

    // F8: filter_edit_to_existing descarta edits sobre o arquivo movido (caminho antigo) e sobre
    // arquivos inexistentes, mantendo os importers.
    #[test]
    fn filter_edit_drops_moved_and_missing() {
        let dir = std::env::temp_dir().join(format!("mvf-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let importer = dir.join("importer.ts");
        std::fs::write(&importer, "import x from './old';\n").unwrap();
        let moved = dir.join("old.ts"); // NÃO existe no disco (simula já movido)
        let importer_abs = importer.to_string_lossy().to_string();
        let moved_abs = moved.to_string_lossy().to_string();
        let edit = json!({"changes": {
            path_to_uri(&importer_abs): [{"range": {"start":{"line":0,"character":0},"end":{"line":0,"character":1}}, "newText":"X"}],
            path_to_uri(&moved_abs): [{"range": {"start":{"line":0,"character":0},"end":{"line":0,"character":1}}, "newText":"Y"}],
        }});
        let filtered = filter_edit_to_existing(&edit, &moved_abs);
        let by = edits_by_file(&filtered);
        assert!(by.contains_key(&importer_abs), "importer mantido");
        assert!(!by.contains_key(&moved_abs), "arquivo movido descartado");
        std::fs::remove_dir_all(&dir).ok();
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

    // G1: o campo `instructions` do initialize é um EXCERTO ENXUTO do MESMO manual servido pela tool
    // `instructions` (fonte única — C11) e fica CURTO (C13). Também garante que a tool devolve o
    // manual completo.
    #[test]
    fn instructions_short_is_trimmed_excerpt_of_full() {
        let short = guidance::SHORT_INSTRUCTIONS;
        let full = guidance::FULL_MANUAL;
        assert!(!short.is_empty(), "excerto curto não pode ser vazio");
        assert!(!full.is_empty(), "manual completo não pode ser vazio");
        // C13: o excerto do initialize é bem menor que o manual completo (enviado toda sessão).
        assert!(
            short.len() < full.len(),
            "SHORT ({}) deve ser menor que FULL ({})",
            short.len(),
            full.len()
        );
        // aponta para a tool que serve o manual completo (mesma fonte da verdade).
        assert!(
            short.contains("instructions"),
            "excerto aponta para a tool `instructions`"
        );
        // ambos carregam as regras de ouro (mesmo conteúdo, um é recorte do outro). Case-insensitive
        // porque o manual usa "WARMUP" em caixa alta ("GATE DE WARMUP").
        let (slow, flow) = (short.to_lowercase(), full.to_lowercase());
        for kw in ["grep", "warmup", "net_delta", "blast_radius", "safe_delete"] {
            assert!(slow.contains(kw), "excerto curto deve mencionar '{kw}'");
            assert!(flow.contains(kw), "manual completo deve mencionar '{kw}'");
        }
    }

    // G1: a tool `instructions` devolve o manual completo (fonte única) + a nota de excerto.
    #[test]
    fn instructions_tool_returns_full_manual() {
        let v = tool_instructions().expect("instructions ok");
        assert_eq!(v["manual"].as_str().unwrap(), guidance::FULL_MANUAL);
        assert!(v["note"].as_str().unwrap().contains("excerto"));
    }

    // G1: a superfície é de 23 tools (22 anteriores + `instructions`), todas com nome+schema.
    #[test]
    fn tools_schema_has_23_tools_incl_instructions() {
        let schema = tools_schema();
        let arr = schema.as_array().expect("schema é array");
        assert_eq!(arr.len(), 23, "23 tools no total (22 + instructions)");
        assert!(
            arr.iter().any(|t| t["name"] == "instructions"),
            "a tool `instructions` está no schema"
        );
    }
}
