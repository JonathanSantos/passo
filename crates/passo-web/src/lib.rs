//! Passo web: the live dashboard (`passo ui`) and the single-file report. One page serves both: live it polls
//! `/api/state`; as a report it reads the snapshot embedded in the HTML.
//!
//! The server binds to 127.0.0.1 only, checks the Host header (DNS rebinding) and needs a per-session token for
//! anything that changes a run.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use passo_app::{App, DevAction, Reporter, Snapshot, Source};
use passo_core::Size;
use serde::Deserialize;
use serde_json::json;
use tiny_http::{Header, Method, Request, Response, Server};

const HTML: &str = include_str!(concat!(env!("CARGO_MANIFEST_DIR"), "/../../assets/web/index.html"));
const CSS: &str = include_str!(concat!(env!("CARGO_MANIFEST_DIR"), "/../../assets/web/app.css"));
const JS: &str = include_str!(concat!(env!("CARGO_MANIFEST_DIR"), "/../../assets/web/app.js"));
const ICON: &str = include_str!(concat!(env!("CARGO_MANIFEST_DIR"), "/../../assets/brand/mark.svg"));

/// The page with styles and script inlined, and `boot` as the page's starting state.
fn page(boot: &serde_json::Value) -> String {
    let boot = boot.to_string().replace("</", "<\\/");
    let icon = format!(
        "data:image/svg+xml,{}",
        ICON.replace('"', "'").replace('#', "%23").replace('\n', "")
    );
    HTML.replace("/*CSS*/", CSS)
        .replace("/*JS*/", &JS.replace("</script", "<\\/script"))
        .replace("\"/*BOOT*/\"", &boot)
        .replace("{{ICON}}", &icon)
}

/// Writes the delivery report: the dashboard frozen with the run's data inside.
pub struct HtmlReporter;

impl Reporter for HtmlReporter {
    fn write(&self, snapshot: &Snapshot, path: &Path) -> Result<()> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let html = page(&json!({ "mode": "report", "snapshot": snapshot }));
        std::fs::write(path, html).with_context(|| format!("escrevendo {}", path.display()))
    }
}

#[derive(Deserialize)]
struct Decision {
    action: String,
    #[serde(default)]
    answers: Vec<String>,
    #[serde(default)]
    text: String,
}

pub fn serve(root: PathBuf, port: u16, open: bool) -> Result<()> {
    let (server, port) = bind(port)?;
    let token = token();
    let url = format!("http://127.0.0.1:{port}/");
    println!("Passo · painel em {url}  (Ctrl+C para sair)");
    if open {
        open_browser(&url);
    }
    for request in server.incoming_requests() {
        if let Err(e) = handle(request, &root, &token, port) {
            eprintln!("passo ui: {e:#}");
        }
    }
    Ok(())
}

fn bind(port: u16) -> Result<(Server, u16)> {
    for candidate in port..port.saturating_add(20) {
        if let Ok(server) = Server::http(("127.0.0.1", candidate)) {
            return Ok((server, candidate));
        }
    }
    anyhow::bail!("nenhuma porta livre entre {port} e {}", port.saturating_add(20))
}

fn token() -> String {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    format!("{:x}{:x}", nanos, std::process::id() as u128 * 2654435761)
}

fn header(name: &str, value: &str) -> Header {
    Header::from_bytes(name.as_bytes(), value.as_bytes()).expect("valid header")
}

fn handle(mut request: Request, root: &Path, token: &str, port: u16) -> Result<()> {
    let host = request
        .headers()
        .iter()
        .find(|h| h.field.equiv("Host"))
        .map(|h| h.value.to_string());
    let allowed = [format!("127.0.0.1:{port}"), format!("localhost:{port}")];
    if !host.is_some_and(|h| allowed.contains(&h)) {
        return Ok(request.respond(Response::from_string("host não permitido").with_status_code(403))?);
    }
    let url = request.url().to_string();
    let (path, query) = url.split_once('?').unwrap_or((&url, ""));
    match (request.method(), path) {
        (Method::Get, "/") => {
            let html = page(&json!({ "mode": "live", "token": token }));
            Ok(request
                .respond(Response::from_string(html).with_header(header("Content-Type", "text/html; charset=utf-8")))?)
        }
        (Method::Get, "/api/state") => {
            let run = query
                .split('&')
                .find_map(|kv| kv.strip_prefix("run="))
                .filter(|s| !s.is_empty());
            let body = match App::open(root).and_then(|app| app.snapshot(run)) {
                Ok(snapshot) => serde_json::to_string(&snapshot)?,
                Err(e) => json!({ "error": format!("{e:#}") }).to_string(),
            };
            Ok(request.respond(json_response(body))?)
        }
        (Method::Post, "/api/decide") => {
            let sent = request
                .headers()
                .iter()
                .find(|h| h.field.equiv("X-Passo-Token"))
                .map(|h| h.value.to_string());
            if sent.as_deref() != Some(token) {
                return Ok(request.respond(Response::from_string("token inválido").with_status_code(403))?);
            }
            let mut body = String::new();
            request.as_reader().read_to_string(&mut body)?;
            let result = serde_json::from_str::<Decision>(&body)
                .map_err(anyhow::Error::from)
                .and_then(|d| decide(root, d));
            let reply = match result {
                Ok(()) => json!({ "ok": true }),
                Err(e) => json!({ "ok": false, "error": format!("{e:#}") }),
            };
            Ok(request.respond(json_response(reply.to_string()))?)
        }
        _ => Ok(request.respond(Response::from_string("não encontrado").with_status_code(404))?),
    }
}

fn json_response(body: String) -> Response<std::io::Cursor<Vec<u8>>> {
    Response::from_string(body)
        .with_header(header("Content-Type", "application/json; charset=utf-8"))
        .with_header(header("Cache-Control", "no-store"))
}

fn decide(root: &Path, decision: Decision) -> Result<()> {
    let app = App::open(root)?.with_reporter(std::sync::Arc::new(HtmlReporter));
    let action = match decision.action.as_str() {
        "ok" => DevAction::Approve(decision.answers),
        "no" => DevAction::Reject(decision.text),
        "change" => DevAction::Change(decision.text),
        "stop" => DevAction::Stop(decision.text),
        "size" => DevAction::Resize(
            Size::parse(&decision.text).ok_or_else(|| anyhow::anyhow!("tamanho inválido: {}", decision.text))?,
        ),
        other => anyhow::bail!("ação desconhecida: {other}"),
    };
    app.decide(action, Source::Web)?;
    Ok(())
}

fn open_browser(url: &str) {
    let program = if cfg!(target_os = "macos") { "open" } else { "xdg-open" };
    std::process::Command::new(program)
        .arg(url)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .ok();
}
