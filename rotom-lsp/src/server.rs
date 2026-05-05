use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use dashmap::DashMap;
use tower_lsp::jsonrpc::Result;
use tower_lsp::lsp_types::{
    CodeLens, CodeLensOptions, CompletionOptions, CompletionParams, CompletionResponse,
    DidChangeTextDocumentParams, DidCloseTextDocumentParams, DidOpenTextDocumentParams,
    DocumentSymbolResponse, GotoDefinitionResponse, Hover, HoverParams, HoverProviderCapability,
    InlayHint, InlayHintOptions, InitializeParams, InitializeResult, InitializedParams,
    MessageType, SaveOptions, ServerCapabilities, SignatureHelp, SignatureHelpOptions,
    TextDocumentSyncCapability, TextDocumentSyncKind, TextDocumentSyncOptions,
    TextDocumentSyncSaveOptions, Url, WorkDoneProgressOptions, OneOf,
};
use tower_lsp::{Client, LanguageServer};

use crate::code_lens::compute_code_lens;
use crate::completions::compute_completions;
use crate::document::{compute_document_symbols, DocumentCache};
use crate::diagnostics::compute_diagnostics;
use crate::goto::compute_goto_definition;
use crate::hover::compute_hover;
use crate::inlay_hints::compute_inlay_hints;
use crate::signature_help::compute_signature_help;

use rotom::database::{ConstantDb, DatabaseV2};
use rotom::project::config::{find_project_root, load_config, ProjectTypeConfig, RotomConfig};

/// How long to wait after the last keystroke before re-running diagnostics.
const DIAGNOSTIC_DEBOUNCE_MS: u64 = 300;

/// Per-project cached state: loaded database and project-wide constants.
#[derive(Clone)]
struct ProjectState {
    db: Arc<DatabaseV2>,
    constants: ConstantDb,
}

pub struct RotomServer {
    client: Client,
    documents: DocumentCache,
    /// Cache of project root → loaded project state.
    projects: DashMap<PathBuf, ProjectState>,
    /// Pending diagnostic tasks per document. Old tasks are aborted when a new
    /// change arrives so we only publish diagnostics after typing pauses.
    pending_diagnostics: DashMap<Url, tokio::task::JoinHandle<()>>,
}

impl RotomServer {
    pub fn new(client: Client) -> Self {
        Self {
            client,
            documents: DocumentCache::new(),
            projects: DashMap::new(),
            pending_diagnostics: DashMap::new(),
        }
    }

    /// Resolve the project root for a file URI and load (or reuse) its database/constants.
    fn project_state_for_uri(&self, uri: &Url) -> Option<ProjectState> {
        let file_path = uri.to_file_path().ok()?;
        let project_root = find_project_root(&file_path)?;

        // Fast path: already cached.
        if let Some(state) = self.projects.get(&project_root) {
            return Some(state.clone());
        }

        // Slow path: load config, database, and constants for this project.
        let config = load_config(&project_root).ok()?;
        let state = Self::load_project_state(&project_root, &config).ok()?;

        self.projects.insert(project_root, state.clone());
        Some(state)
    }

    fn load_project_state(
        root: &std::path::Path,
        config: &RotomConfig,
    ) -> std::result::Result<ProjectState, String> {
        let db_path = config
            .database_file(root)
            .ok_or_else(|| "No database configured in rotom.toml".to_string())?;
        let db = DatabaseV2::load(&db_path).map_err(|e| {
            format!("Failed to load database {}: {}", db_path.display(), e)
        })?;

        let mut constants = ConstantDb::new();
        let _ = constants.load_from_db(&db);

        let database_dir = config.database_dir(root);
        if database_dir.exists() {
            let _ = constants.load_directory(&database_dir);
        }

        match config.workspace.project_type {
            ProjectTypeConfig::Decomp => {
                if let Some(game_family) = config.game_family() {
                    let cache_dir = config.cache_dir(root);
                    if let Ok((symbols, _rebuilt)) = uxie::Workspace::load_cached_symbols(
                        &cache_dir,
                        root,
                        &config.include_roots(root),
                        game_family,
                    ) {
                        let _ = constants.load_decomp_symbols(root, (*symbols).clone());
                    }
                }
            }
            ProjectTypeConfig::Dspre => {
                let language = uxie::RomHeader::open(root)
                    .map_or(uxie::game::GameLanguage::English, |h| h.detect_language());
                let _ = constants.load_dspre_text_archives(root, language);
            }
            ProjectTypeConfig::Generic | ProjectTypeConfig::HgEngine => {}
        }

        Ok(ProjectState {
            db: Arc::new(db),
            constants,
        })
    }

    /// Build file-local constants for a URI by cloning project-wide constants
    /// and applying any `#include` / `#define` directives found in the source.
    fn file_constants_for_uri(&self, uri: &Url, source: &str) -> Option<ConstantDb> {
        let state = self.project_state_for_uri(uri)?;
        let mut file_constants = state.constants.clone();

        let file_path = uri.to_file_path().ok()?;
        if file_path.extension().and_then(|s| s.to_str()) != Some("rotom") {
            return Some(file_constants);
        }

        let lexer = rotom::compiler::Lexer::new(source);
        let mut parser = rotom::compiler::Parser::new_fallible(lexer);
        let file = parser.parse_script_file().ok()?;
        let script_dir = file_path.parent().unwrap_or_else(|| std::path::Path::new("."));
        let _ = file_constants.apply_directives(script_dir, source, &file.items);

        Some(file_constants)
    }

    /// Schedule diagnostics to be published after a debounce delay.
    ///
    /// If the user is typing rapidly, old pending tasks are aborted so only
    /// the most recent change triggers a diagnostic pass.
    fn publish_diagnostics(&self, uri: &Url) {
        // Abort any existing pending diagnostic task for this document.
        if let Some((_, old)) = self.pending_diagnostics.remove(uri) {
            old.abort();
        }

        let uri_for_task = uri.clone();
        let client = self.client.clone();
        let text = self.documents.get(&uri_for_task).map(|doc| doc.text.clone());
        let project_state = self.project_state_for_uri(&uri_for_task);

        // Compute file-local constants synchronously before spawning.
        let file_constants = text.as_ref().and_then(|src| {
            self.file_constants_for_uri(&uri_for_task, src)
        });

        let handle = tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(DIAGNOSTIC_DEBOUNCE_MS)).await;

            let Some(text) = text else {
                return;
            };
            let (db, constants) = if let Some(state) = project_state {
                let c = file_constants.unwrap_or_else(|| state.constants.clone());
                (Some(state.db), c)
            } else {
                let empty = rotom::database::ConstantDb::new();
                (None, empty)
            };

            let diagnostics = compute_diagnostics(&text, db.as_deref(), Some(&constants));
            client.publish_diagnostics(uri_for_task, diagnostics, None).await;
        });

        self.pending_diagnostics.insert(uri.clone(), handle);
    }
}

#[tower_lsp::async_trait]
impl LanguageServer for RotomServer {
    async fn initialize(&self, _: InitializeParams) -> Result<InitializeResult> {
        Ok(InitializeResult {
            capabilities: ServerCapabilities {
                text_document_sync: Some(TextDocumentSyncCapability::Options(
                    TextDocumentSyncOptions {
                        open_close: Some(true),
                        change: Some(TextDocumentSyncKind::INCREMENTAL),
                        will_save: None,
                        will_save_wait_until: None,
                        save: Some(TextDocumentSyncSaveOptions::SaveOptions(SaveOptions {
                            include_text: Some(false),
                        })),
                    },
                )),
                completion_provider: Some(CompletionOptions {
                    resolve_provider: Some(false),
                    trigger_characters: Some(vec![" ".to_string()]),
                    all_commit_characters: None,
                    work_done_progress_options: WorkDoneProgressOptions {
                        work_done_progress: None,
                    },
                    completion_item: None,
                }),
                hover_provider: Some(HoverProviderCapability::Simple(true)),
                document_symbol_provider: Some(
                    OneOf::Right(
                        tower_lsp::lsp_types::DocumentSymbolOptions {
                            label: Some("Rotom".to_string()),
                            work_done_progress_options: WorkDoneProgressOptions {
                                work_done_progress: None,
                            },
                        },
                    ),
                ),
                definition_provider: Some(OneOf::Left(true)),
                code_lens_provider: Some(CodeLensOptions {
                    resolve_provider: Some(false),
                }),
                signature_help_provider: Some(SignatureHelpOptions {
                    trigger_characters: Some(vec!["(".to_string(), ",".to_string(), " ".to_string()]),
                    retrigger_characters: Some(vec![",".to_string(), " ".to_string()]),
                    work_done_progress_options: WorkDoneProgressOptions {
                        work_done_progress: None,
                    },
                }),
                inlay_hint_provider: Some(OneOf::Right(
                    tower_lsp::lsp_types::InlayHintServerCapabilities::Options(InlayHintOptions {
                        resolve_provider: Some(false),
                        work_done_progress_options: WorkDoneProgressOptions {
                            work_done_progress: None,
                        },
                    }),
                )),
                // Only advertise capabilities that are actually implemented.
                // Re-enable each one as the corresponding handler is wired up.
                ..Default::default()
            },
            ..Default::default()
        })
    }

    async fn initialized(&self, _: InitializedParams) {
        self.client
            .log_message(MessageType::INFO, "Rotom LSP initialized")
            .await;
    }

    async fn shutdown(&self) -> Result<()> {
        Ok(())
    }

    async fn completion(&self, params: CompletionParams) -> Result<Option<CompletionResponse>> {
        let uri = &params.text_document_position.text_document.uri;
        let position = params.text_document_position.position;

        let doc = self.documents.get(uri);
        let Some(doc) = doc else {
            return Ok(None);
        };

        let db = self.project_state_for_uri(uri).map(|s| s.db);
        let file_constants = self.file_constants_for_uri(uri, &doc.text);
        let constant_names = file_constants.as_ref().map(|c| {
            Arc::new(c.constant_names()) as Arc<Vec<String>>
        });

        let local_symbols = self.documents.get(uri)
            .map(|doc| compute_document_symbols(&doc.text));

        let items = compute_completions(
            &doc.text,
            position,
            db.as_deref(),
            constant_names,
            local_symbols.as_deref(),
        );

        Ok(Some(CompletionResponse::Array(items)))
    }

    async fn hover(&self, params: HoverParams) -> Result<Option<Hover>> {
        let uri = &params.text_document_position_params.text_document.uri;
        let position = params.text_document_position_params.position;

        let doc = self.documents.get(uri);
        let Some(doc) = doc else {
            return Ok(None);
        };

        let db = self.project_state_for_uri(uri).map(|s| s.db);
        let file_constants = self.file_constants_for_uri(uri, &doc.text);

        let hover = compute_hover(
            &doc.text,
            position,
            db.as_deref(),
            file_constants.as_ref(),
        );

        Ok(hover)
    }

    async fn document_symbol(
        &self,
        params: tower_lsp::lsp_types::DocumentSymbolParams,
    ) -> Result<Option<DocumentSymbolResponse>> {
        let uri = &params.text_document.uri;

        let symbols = self.documents.get(uri)
            .map(|doc| compute_document_symbols(&doc.text));
        let Some(symbols) = symbols else {
            return Ok(None);
        };

        if symbols.is_empty() {
            Ok(None)
        } else {
            Ok(Some(DocumentSymbolResponse::Nested(symbols)))
        }
    }

    async fn goto_definition(
        &self,
        params: tower_lsp::lsp_types::GotoDefinitionParams,
    ) -> Result<Option<GotoDefinitionResponse>> {
        let uri = &params.text_document_position_params.text_document.uri;
        let position = params.text_document_position_params.position;

        let doc = self.documents.get(uri);
        let Some(doc) = doc else {
            return Ok(None);
        };

        Ok(compute_goto_definition(&doc.text, position, uri))
    }

    async fn signature_help(
        &self,
        params: tower_lsp::lsp_types::SignatureHelpParams,
    ) -> Result<Option<SignatureHelp>> {
        let uri = &params.text_document_position_params.text_document.uri;
        let position = params.text_document_position_params.position;

        let doc = self.documents.get(uri);
        let Some(doc) = doc else {
            return Ok(None);
        };

        let db = match self.project_state_for_uri(uri) {
            Some(state) => Some(state.db),
            None => None,
        };

        Ok(compute_signature_help(&doc.text, position, db.as_deref()))
    }

    async fn code_lens(
        &self,
        params: tower_lsp::lsp_types::CodeLensParams,
    ) -> Result<Option<Vec<CodeLens>>> {
        let uri = &params.text_document.uri;

        let doc = self.documents.get(uri);
        let Some(doc) = doc else {
            return Ok(None);
        };

        let db = match self.project_state_for_uri(uri) {
            Some(state) => Some(state.db),
            None => None,
        };

        let lenses = compute_code_lens(&doc.text, uri, db.as_deref());
        if lenses.is_empty() {
            Ok(None)
        } else {
            Ok(Some(lenses))
        }
    }

    async fn did_open(&self, params: DidOpenTextDocumentParams) {
        let uri = params.text_document.uri;
        self.documents.insert(uri.clone(), params.text_document.text);
        self.publish_diagnostics(&uri);
    }

    async fn did_change(&self, params: DidChangeTextDocumentParams) {
        let uri = params.text_document.uri;
        self.documents
            .apply_changes(&uri, params.content_changes);
        self.publish_diagnostics(&uri);
    }

    async fn inlay_hint(
        &self,
        params: tower_lsp::lsp_types::InlayHintParams,
    ) -> Result<Option<Vec<InlayHint>>> {
        let uri = &params.text_document.uri;

        let doc = self.documents.get(uri);
        let Some(doc) = doc else {
            return Ok(None);
        };

        let db = match self.project_state_for_uri(uri) {
            Some(state) => Some(state.db),
            None => None,
        };

        let hints = compute_inlay_hints(&doc.text, db.as_deref());
        if hints.is_empty() {
            Ok(None)
        } else {
            Ok(Some(hints))
        }
    }

    async fn did_close(&self, params: DidCloseTextDocumentParams) {
        let uri = params.text_document.uri;
        // Cancel any pending diagnostic task for this document.
        if let Some((_, old)) = self.pending_diagnostics.remove(&uri) {
            old.abort();
        }
        // Clear stale diagnostics so the editor doesn't keep showing old errors.
        self.client
            .publish_diagnostics(uri.clone(), vec![], None)
            .await;
        self.documents.remove(&uri);
    }
}
