//! `wavelet-lsp` — a basic Language Server for the Wavelet language.
//!
//! It speaks LSP over stdio and provides, all driven by `wavelet::read_file`:
//!   • live syntax diagnostics (publish on open/change),
//!   • completion (special forms, builtins, names defined in the file),
//!   • hover (special-form/builtin blurbs and `///` doc comments),
//!   • document symbols (top-level `Def`/`DefType`/`DefMacro`).
//!
//! See `README.md` in this directory for editor wiring.

mod analysis;
mod line_index;

use std::collections::HashMap;
use std::error::Error;

use line_index::LineIndex;
use lsp_server::{Connection, Message, Response};
use lsp_types::{
    CompletionOptions, CompletionParams, CompletionResponse, DidChangeTextDocumentParams,
    DidCloseTextDocumentParams, DidOpenTextDocumentParams, DocumentSymbolParams,
    DocumentSymbolResponse, HoverParams, HoverProviderCapability, OneOf,
    PublishDiagnosticsParams, ServerCapabilities, TextDocumentSyncCapability, TextDocumentSyncKind,
    Url,
};
use serde_json::Value;

type LspResult<T> = Result<T, Box<dyn Error + Sync + Send>>;

/// Open documents, keyed by URI. We keep the source text and a line index for
/// position conversion; both are rebuilt on every change (documents are small).
#[derive(Default)]
struct Documents(HashMap<Url, (String, LineIndex)>);

impl Documents {
    fn set(&mut self, uri: Url, text: String) {
        let index = LineIndex::new(&text);
        self.0.insert(uri, (text, index));
    }

    fn get(&self, uri: &Url) -> Option<&(String, LineIndex)> {
        self.0.get(uri)
    }
}

fn main() -> LspResult<()> {
    // Answer `--version` without opening the LSP stdio connection, so it works
    // from a plain shell (and lets package managers smoke-test the binary).
    if std::env::args()
        .nth(1)
        .is_some_and(|a| a == "--version" || a == "-V" || a == "version")
    {
        println!("wavelet-lsp {}", env!("CARGO_PKG_VERSION"));
        return Ok(());
    }

    let (connection, io_threads) = Connection::stdio();

    let capabilities = serde_json::to_value(ServerCapabilities {
        text_document_sync: Some(TextDocumentSyncCapability::Kind(TextDocumentSyncKind::FULL)),
        completion_provider: Some(CompletionOptions::default()),
        hover_provider: Some(HoverProviderCapability::Simple(true)),
        document_symbol_provider: Some(OneOf::Left(true)),
        ..Default::default()
    })?;

    connection.initialize(capabilities)?;
    // Pass `connection` by value so it is dropped when `main_loop` returns,
    // closing the writer channel; otherwise `io_threads.join()` would block
    // forever waiting on a sender that is still alive.
    main_loop(connection)?;
    io_threads.join()?;
    Ok(())
}

fn main_loop(connection: Connection) -> LspResult<()> {
    let mut docs = Documents::default();

    for msg in &connection.receiver {
        match msg {
            Message::Request(req) => {
                if connection.handle_shutdown(&req)? {
                    return Ok(());
                }
                let response = match req.method.as_str() {
                    "textDocument/completion" => {
                        let params: CompletionParams = serde_json::from_value(req.params)?;
                        let uri = params.text_document_position.text_document.uri;
                        // A `file://` URI gives a filesystem path, which the
                        // analysis uses to locate `wit/deps`; other schemes
                        // (untitled buffers) yield `None` and skip that step.
                        let path = uri.to_file_path().ok();
                        let result = docs.get(&uri).map(|(text, _)| {
                            CompletionResponse::Array(analysis::completions(
                                text,
                                path.as_deref(),
                            ))
                        });
                        ok_response(req.id, result)?
                    }
                    "textDocument/hover" => {
                        let params: HoverParams = serde_json::from_value(req.params)?;
                        let pos = params.text_document_position_params;
                        let uri = pos.text_document.uri;
                        let result = docs.get(&uri).and_then(|(text, index)| {
                            let offset = index.offset(text, pos.position);
                            analysis::hover(text, index, offset)
                        });
                        ok_response(req.id, result)?
                    }
                    "textDocument/documentSymbol" => {
                        let params: DocumentSymbolParams = serde_json::from_value(req.params)?;
                        let uri = params.text_document.uri;
                        let result = docs.get(&uri).map(|(text, index)| {
                            DocumentSymbolResponse::Nested(analysis::document_symbols(text, index))
                        });
                        ok_response(req.id, result)?
                    }
                    _ => Response { id: req.id, result: Some(Value::Null), error: None },
                };
                connection.sender.send(Message::Response(response))?;
            }
            Message::Notification(not) => match not.method.as_str() {
                "textDocument/didOpen" => {
                    let p: DidOpenTextDocumentParams = serde_json::from_value(not.params)?;
                    docs.set(p.text_document.uri.clone(), p.text_document.text);
                    publish_diagnostics(&connection, &docs, &p.text_document.uri)?;
                }
                "textDocument/didChange" => {
                    let p: DidChangeTextDocumentParams = serde_json::from_value(not.params)?;
                    // Full sync: the last change holds the entire new document.
                    if let Some(change) = p.content_changes.into_iter().last() {
                        docs.set(p.text_document.uri.clone(), change.text);
                        publish_diagnostics(&connection, &docs, &p.text_document.uri)?;
                    }
                }
                "textDocument/didClose" => {
                    let p: DidCloseTextDocumentParams = serde_json::from_value(not.params)?;
                    docs.0.remove(&p.text_document.uri);
                    // Clear any diagnostics the client is still showing.
                    send_diagnostics(&connection, p.text_document.uri, Vec::new())?;
                }
                _ => {}
            },
            Message::Response(_) => {}
        }
    }
    Ok(())
}

/// Build a JSON-RPC success response, serializing `None` results to `null`.
fn ok_response<T: serde::Serialize>(
    id: lsp_server::RequestId,
    result: Option<T>,
) -> LspResult<Response> {
    let value = match result {
        Some(r) => serde_json::to_value(r)?,
        None => Value::Null,
    };
    Ok(Response { id, result: Some(value), error: None })
}

fn publish_diagnostics(connection: &Connection, docs: &Documents, uri: &Url) -> LspResult<()> {
    if let Some((text, index)) = docs.get(uri) {
        let diags = analysis::diagnostics(text, index);
        send_diagnostics(connection, uri.clone(), diags)?;
    }
    Ok(())
}

fn send_diagnostics(
    connection: &Connection,
    uri: Url,
    diagnostics: Vec<lsp_types::Diagnostic>,
) -> LspResult<()> {
    let params = PublishDiagnosticsParams { uri, diagnostics, version: None };
    connection.sender.send(Message::Notification(lsp_server::Notification {
        method: "textDocument/publishDiagnostics".to_string(),
        params: serde_json::to_value(params)?,
    }))?;
    Ok(())
}
