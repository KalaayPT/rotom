use std::path::PathBuf;
use std::sync::Arc;

use dashmap::DashMap;
use tower_lsp::jsonrpc::Result;
use tower_lsp::lsp_types::*;
use tower_lsp::{Client, LanguageServer};

use crate::completions::compute_completions;
use crate::document::DocumentCache;
use crate::diagnostics::compute_diagnostics;

use rotom::database::{ConstantDb, DatabaseV2};
use rotom::project::config::{find_project_root, load_config, ProjectTypeConfig, RotomConfig};

/// Per-project cached state: loaded database and constants.
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
}

impl RotomServer {
    pub fn new(client: Client) -> Self {
        Self {
            client,
            documents: DocumentCache::new(),
            projects: DashMap::new(),
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
                    .map(|h| h.detect_language())
                    .unwrap_or(uxie::game::GameLanguage::English);
                let _ = constants.load_dspre_text_archives(root, language);
            }
            ProjectTypeConfig::Generic => {}
        }

        Ok(ProjectState {
            db: Arc::new(db),
            constants,
        })
    }

    /// Re-compute and publish diagnostics for the given document.
    async fn publish_diagnostics(&self, uri: &Url) {
        // Clone text out of the map so we don't hold the DashMap guard across .await.
        let text = self.documents.get(uri).map(|doc| doc.clone());
        let Some(text) = text else {
            return;
        };

        let (db, constants) = match self.project_state_for_uri(uri) {
            Some(state) => (Some(state.db), Some(state.constants)),
            None => (None, None),
        };

        let diagnostics = compute_diagnostics(
            &text,
            db.as_ref().map(|a| a.as_ref()),
            constants.as_ref(),
        );
        self.client
            .publish_diagnostics(uri.clone(), diagnostics, None)
            .await;
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

        let text = self.documents.get(uri).map(|doc| doc.clone());
        let Some(text) = text else {
            return Ok(None);
        };

        let (db, constants) = match self.project_state_for_uri(uri) {
            Some(state) => (Some(state.db), Some(state.constants)),
            None => (None, None),
        };

        let items = compute_completions(
            &text,
            position,
            db.as_ref().map(|a| a.as_ref()),
            constants.as_ref(),
        );

        Ok(Some(CompletionResponse::Array(items)))
    }

    async fn did_open(&self, params: DidOpenTextDocumentParams) {
        let uri = params.text_document.uri;
        self.documents.insert(uri.clone(), params.text_document.text);
        self.publish_diagnostics(&uri).await;
    }

    async fn did_change(&self, params: DidChangeTextDocumentParams) {
        let uri = params.text_document.uri;
        self.documents
            .apply_changes(uri.clone(), params.content_changes);
        self.publish_diagnostics(&uri).await;
    }

    async fn did_close(&self, params: DidCloseTextDocumentParams) {
        let uri = params.text_document.uri;
        // Clear stale diagnostics so the editor doesn't keep showing old errors.
        self.client
            .publish_diagnostics(uri.clone(), vec![], None)
            .await;
        self.documents.remove(&uri);
    }
}
