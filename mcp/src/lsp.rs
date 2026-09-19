// Cliente LSP mínimo e persistente para o tsgo (`tsgo --lsp -stdio`).
// Padrão (endossado pela pesquisa / Serena): processo long-lived + thread leitora dedicada
// roteando respostas por id; requests síncronos bloqueantes. Sem runtime async.
use serde_json::{json, Value};
use std::collections::{HashMap, HashSet};
use std::io::{BufRead, BufReader, Write};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::atomic::{AtomicBool, AtomicI64, AtomicU64, Ordering};
use std::sync::mpsc::{channel, Sender};
use std::sync::{Arc, Mutex};
use std::time::Duration;

pub struct LspClient {
    stdin: Arc<Mutex<ChildStdin>>,
    pending: Arc<Mutex<HashMap<i64, Sender<Value>>>>,
    id: AtomicI64,
    opened: Mutex<HashSet<String>>,
    versions: Mutex<HashMap<String, i64>>,
    // diagnostics via PUSH (publishDiagnostics) — usado quando o server não suporta PULL
    diagnostics: Arc<Mutex<HashMap<String, Value>>>,
    diag_gen: Arc<AtomicU64>,
    supports_pull: AtomicBool,
    root: String,
    _child: Child,
}

fn write_frame(w: &mut ChildStdin, v: &Value) -> std::io::Result<()> {
    let s = serde_json::to_string(v).unwrap();
    write!(w, "Content-Length: {}\r\n\r\n{}", s.len(), s)?;
    w.flush()
}

fn read_frame<R: BufRead>(r: &mut R) -> Option<Value> {
    let mut len = 0usize;
    loop {
        let mut line = String::new();
        if r.read_line(&mut line).ok()? == 0 {
            return None;
        }
        let t = line.trim_end();
        if t.is_empty() {
            break;
        }
        if let Some(v) = t.strip_prefix("Content-Length:") {
            len = v.trim().parse().ok()?;
        }
    }
    let mut buf = vec![0u8; len];
    r.read_exact(&mut buf).ok()?;
    serde_json::from_slice(&buf).ok()
}

impl LspClient {
    /// Sobe um language server (`cmd args`) para um projeto e completa initialize/initialized.
    pub fn start(cmd: &str, args: &[&str], root_abs: &str) -> Result<Arc<Self>, String> {
        let mut child = Command::new(cmd)
            .args(args)
            .current_dir(root_abs)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .map_err(|e| format!("falha ao spawnar '{cmd}': {e}"))?;

        let stdin = Arc::new(Mutex::new(child.stdin.take().unwrap()));
        let stdout = child.stdout.take().unwrap();
        let pending: Arc<Mutex<HashMap<i64, Sender<Value>>>> = Arc::new(Mutex::new(HashMap::new()));
        let diagnostics: Arc<Mutex<HashMap<String, Value>>> = Arc::new(Mutex::new(HashMap::new()));
        let diag_gen = Arc::new(AtomicU64::new(0));

        // thread leitora: roteia respostas por id, RESPONDE requests server-initiated
        // (sem isso o workspace nunca carrega) e captura publishDiagnostics (modo PUSH).
        {
            let pending = pending.clone();
            let stdin_w = stdin.clone();
            let diagnostics = diagnostics.clone();
            let diag_gen = diag_gen.clone();
            std::thread::spawn(move || {
                let mut r = BufReader::new(stdout);
                while let Some(msg) = read_frame(&mut r) {
                    let has_id = msg.get("id").map(|i| !i.is_null()).unwrap_or(false);
                    let is_method = msg.get("method").is_some();
                    if has_id && !is_method {
                        // resposta a um request nosso
                        if let Some(id) = msg.get("id").and_then(|i| i.as_i64()) {
                            if let Some(tx) = pending.lock().unwrap().remove(&id) {
                                let _ = tx.send(msg);
                            }
                        }
                    } else if has_id && is_method {
                        // request server->client: precisa de resposta
                        let id = msg.get("id").cloned().unwrap();
                        let method = msg.get("method").and_then(|m| m.as_str()).unwrap_or("");
                        let result = match method {
                            "workspace/configuration" => {
                                let n = msg["params"]["items"].as_array().map(|a| a.len()).unwrap_or(1);
                                Value::Array(vec![json!({}); n])
                            }
                            _ => Value::Null, // registerCapability, workDoneProgress/create, etc.
                        };
                        let resp = json!({"jsonrpc":"2.0","id":id,"result":result});
                        let _ = write_frame(&mut stdin_w.lock().unwrap(), &resp);
                    } else if is_method {
                        // notificação server->client: captura diagnostics via PUSH
                        if msg.get("method").and_then(|m| m.as_str()) == Some("textDocument/publishDiagnostics") {
                            if let Some(uri) = msg["params"]["uri"].as_str() {
                                diagnostics.lock().unwrap().insert(uri.to_string(), msg["params"]["diagnostics"].clone());
                                diag_gen.fetch_add(1, Ordering::SeqCst);
                            }
                        }
                    }
                }
            });
        }

        let client = Arc::new(LspClient {
            stdin,
            pending,
            id: AtomicI64::new(1),
            opened: Mutex::new(HashSet::new()),
            versions: Mutex::new(HashMap::new()),
            diagnostics,
            diag_gen,
            supports_pull: AtomicBool::new(false),
            root: root_abs.to_string(),
            _child: child,
        });

        let root_uri = path_to_uri(root_abs);
        let init = client.request(
            "initialize",
            json!({
                "processId": std::process::id(),
                "rootUri": root_uri,
                "workspaceFolders": [{"uri": root_uri, "name": "root"}],
                "capabilities": {
                    "textDocument": {
                        "references": {}, "definition": {},
                        "rename": {"prepareSupport": true},
                        "synchronization": {"dynamicRegistration": true},
                        "publishDiagnostics": {},
                        "codeAction": {
                            "codeActionLiteralSupport": {"codeActionKind": {"valueSet": ["refactor","refactor.extract","refactor.move"]}},
                            "resolveSupport": {"properties": ["edit"]},
                            "dataSupport": true
                        }
                    },
                    "workspace": {"workspaceFolders": true, "configuration": true, "applyEdit": true}
                }
            }),
            10_000,
        )?;
        // o server suporta PULL diagnostics? (senão, usamos PUSH)
        let pull = !init["capabilities"]["diagnosticProvider"].is_null();
        client.supports_pull.store(pull, Ordering::SeqCst);
        client.notify("initialized", json!({}));
        Ok(client)
    }

    pub fn supports_pull(&self) -> bool {
        self.supports_pull.load(Ordering::SeqCst)
    }

    pub fn request(&self, method: &str, params: Value, timeout_ms: u64) -> Result<Value, String> {
        let id = self.id.fetch_add(1, Ordering::SeqCst);
        let (tx, rx) = channel();
        self.pending.lock().unwrap().insert(id, tx);
        let msg = json!({"jsonrpc":"2.0","id":id,"method":method,"params":params});
        write_frame(&mut self.stdin.lock().unwrap(), &msg).map_err(|e| e.to_string())?;
        match rx.recv_timeout(Duration::from_millis(timeout_ms)) {
            Ok(resp) => {
                if let Some(err) = resp.get("error") {
                    if !err.is_null() {
                        return Err(format!("LSP error em {method}: {err}"));
                    }
                }
                Ok(resp.get("result").cloned().unwrap_or(Value::Null))
            }
            Err(_) => {
                self.pending.lock().unwrap().remove(&id);
                Err(format!("timeout ({timeout_ms}ms) em {method}"))
            }
        }
    }

    pub fn notify(&self, method: &str, params: Value) {
        let msg = json!({"jsonrpc":"2.0","method":method,"params":params});
        let _ = write_frame(&mut self.stdin.lock().unwrap(), &msg);
    }

    /// Garante didOpen do arquivo (idempotente). Necessário antes de operações semânticas.
    pub fn ensure_open(&self, abs_file: &str) -> Result<(), String> {
        {
            if self.opened.lock().unwrap().contains(abs_file) {
                return Ok(());
            }
        }
        let text = std::fs::read_to_string(abs_file).map_err(|e| format!("ler {abs_file}: {e}"))?;
        let lang = lang_id(abs_file);
        self.notify(
            "textDocument/didOpen",
            json!({"textDocument":{"uri":path_to_uri(abs_file),"languageId":lang,"version":1,"text":text}}),
        );
        self.opened.lock().unwrap().insert(abs_file.to_string());
        self.versions.lock().unwrap().insert(abs_file.to_string(), 1);
        Ok(())
    }

    /// Sincroniza o conteúdo (full sync) do documento com o server — SEM tocar o disco.
    /// É a base do "simulate in-memory": trocamos o texto, medimos diagnostics, e podemos voltar.
    pub fn did_change(&self, abs_file: &str, new_text: &str) {
        let ver = {
            let mut v = self.versions.lock().unwrap();
            let n = v.get(abs_file).copied().unwrap_or(1) + 1;
            v.insert(abs_file.to_string(), n);
            n
        };
        self.notify(
            "textDocument/didChange",
            json!({"textDocument":{"uri":path_to_uri(abs_file),"version":ver},
                   "contentChanges":[{"text":new_text}]}),
        );
    }

    /// Abre um documento com um TEXTO explícito (para arquivos novos que ainda não existem
    /// em disco — ex.: 'move to new file'). Assim o server analisa o conteúdo simulado.
    pub fn open_with_text(&self, abs_file: &str, text: &str) {
        let lang = lang_id(abs_file);
        self.notify(
            "textDocument/didOpen",
            json!({"textDocument":{"uri":path_to_uri(abs_file),"languageId":lang,"version":1,"text":text}}),
        );
        self.opened.lock().unwrap().insert(abs_file.to_string());
        self.versions.lock().unwrap().insert(abs_file.to_string(), 1);
    }

    pub fn close(&self, abs_file: &str) {
        self.notify("textDocument/didClose", json!({"textDocument":{"uri":path_to_uri(abs_file)}}));
        self.opened.lock().unwrap().remove(abs_file);
    }

    /// PULL diagnostics (LSP 3.17 textDocument/diagnostic). Determinístico quando suportado.
    pub fn pull_diagnostics(&self, abs_file: &str) -> Result<Vec<Value>, String> {
        let res = self.request(
            "textDocument/diagnostic",
            json!({"textDocument":{"uri":path_to_uri(abs_file)}}),
            10_000,
        )?;
        Ok(res.get("items").and_then(|i| i.as_array()).cloned().unwrap_or_default())
    }

    /// Diagnostics via PUSH (do store preenchido por publishDiagnostics).
    pub fn pushed_diagnostics(&self, abs_file: &str) -> Vec<Value> {
        let uri = path_to_uri(abs_file);
        self.diagnostics.lock().unwrap().get(&uri).and_then(|v| v.as_array().cloned()).unwrap_or_default()
    }

    pub fn diag_gen(&self) -> u64 {
        self.diag_gen.load(Ordering::SeqCst)
    }

    pub fn root(&self) -> &str {
        &self.root
    }
}

pub fn lang_id(abs_file: &str) -> &'static str {
    if abs_file.ends_with(".tsx") { "typescriptreact" }
    else if abs_file.ends_with(".jsx") { "javascriptreact" }
    else if abs_file.ends_with(".js") || abs_file.ends_with(".mjs") || abs_file.ends_with(".cjs") { "javascript" }
    else if abs_file.ends_with(".py") { "python" }
    else if abs_file.ends_with(".dart") { "dart" }
    else if abs_file.ends_with(".rs") { "rust" }
    else if abs_file.ends_with(".cs") { "csharp" }
    else { "typescript" }
}

pub fn path_to_uri(p: &str) -> String {
    // suficiente para caminhos absolutos Unix nesta POC
    format!("file://{}", p)
}

pub fn uri_to_path(uri: &str) -> String {
    uri.strip_prefix("file://").unwrap_or(uri).to_string()
}
