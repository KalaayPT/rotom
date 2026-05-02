//! Rotom Language Server
//!
//! Implements the Language Server Protocol (LSP) for the Rotom scripting
//! language.

use tower_lsp::{LspService, Server};

mod completions;
mod diagnostics;
mod document;
mod goto;
mod hover;
mod server;

#[tokio::main]
async fn main() {
    let stdin = tokio::io::stdin();
    let stdout = tokio::io::stdout();

    let (service, socket) = LspService::new(server::RotomServer::new);
    Server::new(stdin, stdout, socket).serve(service).await;
}
