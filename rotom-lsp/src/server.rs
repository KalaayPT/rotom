use std::path::PathBuf;
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};
use std::time::Duration;

use dashmap::DashMap;
use tower_lsp::jsonrpc::Result;
use tower_lsp::lsp_types::{
    CodeLens, CodeLensOptions, CompletionOptions, CompletionParams, CompletionResponse,
    DidChangeTextDocumentParams, DidChangeWatchedFilesParams,
    DidChangeWatchedFilesRegistrationOptions, DidCloseTextDocumentParams,
    DidOpenTextDocumentParams, DidSaveTextDocumentParams, DocumentSymbolResponse, FileChangeType,
    FileSystemWatcher, GlobPattern, GotoDefinitionResponse, Hover, HoverParams,
    HoverProviderCapability, InitializeParams, InitializeResult, InitializedParams, InlayHint,
    InlayHintOptions, MessageType, OneOf, Registration, SaveOptions, ServerCapabilities,
    SignatureHelp, SignatureHelpOptions, TextDocumentSyncCapability, TextDocumentSyncKind,
    TextDocumentSyncOptions, TextDocumentSyncSaveOptions, Url, WatchKind, WorkDoneProgressOptions,
};
use tower_lsp::{Client, LanguageServer};

use crate::code_lens::{
    build_message_code_lens, compute_script_code_lens, is_message_archive_uri, is_rotom_script_uri,
};
use crate::completions::compute_completions;
use crate::diagnostics::compute_diagnostics;
use crate::document::{DocumentCache, compute_document_symbols};
use crate::goto::{compute_goto_definition, json_message_lines};
use crate::hover::compute_hover;
use crate::inlay_hints::compute_inlay_hints;
use crate::message_refs::{MessageRef, collect_message_refs};
use crate::signature_help::compute_signature_help;

use rotom::compiler::{ast::ScriptFile, diagnostic::CompileError};
use rotom::database::{ConstantDb, DatabaseV2};
use rotom::project::config::{ProjectTypeConfig, RotomConfig, find_project_root, load_config};

/// How long to wait after the last keystroke before re-running diagnostics.
const DIAGNOSTIC_DEBOUNCE_MS: u64 = 300;

/// Per-project cached state: loaded database, project-wide constants,
/// and the uxie workspace for message lookups and script resolution.
#[derive(Clone)]
struct ProjectState {
    db: Arc<DatabaseV2>,
    constants: ConstantDb,
    workspace: Option<Arc<uxie::Workspace>>,
    message_refs: Arc<MessageRefIndex>,
}

#[derive(Default)]
struct MessageRefIndex {
    /// Per-script contributions used for didSave invalidation.
    by_file: DashMap<PathBuf, Vec<(u16, u16)>>,
    /// Per-message reverse index consumed by archive `CodeLens` requests.
    by_message: DashMap<(u16, u16), Vec<MessageRef>>,
    /// One lock per script file, used to prevent two concurrent reindex
    /// passes from inserting duplicate message references for the same file.
    reindex_locks: DashMap<PathBuf, Arc<std::sync::Mutex<()>>>,
}

/// File-local constants for a `.rotom` buffer; may carry the directive parse for diagnostics reuse.
struct RotomFileConstantsPrep {
    constants: ConstantDb,
    /// AST and recoverable lexer/parser errors from the same parse used for `#include` / `#define`.
    directive_parse_for_diagnostics: Option<(Arc<ScriptFile>, Vec<CompileError>)>,
}

pub struct RotomServer {
    client: Client,
    documents: DocumentCache,
    /// Cache of project root → loaded project state.
    projects: DashMap<PathBuf, ProjectState>,
    /// Pending diagnostic tasks per document. Old tasks are aborted when a new
    /// change arrives so we only publish diagnostics after typing pauses.
    pending_diagnostics: DashMap<Url, tokio::task::JoinHandle<()>>,
    /// Whether the client supports dynamic file-watcher registration.
    client_supports_watched_file_dynamic_registration: AtomicBool,
}

impl RotomServer {
    pub fn new(client: Client) -> Self {
        Self {
            client,
            documents: DocumentCache::new(),
            projects: DashMap::new(),
            pending_diagnostics: DashMap::new(),
            client_supports_watched_file_dynamic_registration: AtomicBool::new(false),
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
        let state = Self::load_project_state(&project_root, &config, self.client.clone()).ok()?;

        self.projects.insert(project_root, state.clone());
        Some(state)
    }

    /// Remove cached project state for the project containing `path`, if any.
    fn invalidate_project_for_path(&self, path: &std::path::Path) {
        if let Some(project_root) = find_project_root(path) {
            self.projects.remove(&project_root);
        }
    }

    /// Returns `true` when a file change at `path` warrants clearing
    /// the project-level cache (database, config, header, or generated file).
    fn should_invalidate_project_state(path: &std::path::Path) -> bool {
        let file_name = path.file_name().and_then(|s| s.to_str());
        if file_name == Some("rotom.toml") {
            return true;
        }

        let components: Vec<_> = path
            .components()
            .map(|component| component.as_os_str().to_string_lossy().into_owned())
            .collect();

        if components
            .windows(2)
            .any(|pair| pair[0] == ".rotom" && pair[1] == "command_database")
        {
            return true;
        }

        if components
            .iter()
            .any(|component| matches!(component.as_str(), "include" | "generated"))
        {
            let extension = path.extension().and_then(|s| s.to_str());
            return matches!(extension, Some("json" | "h" | "inc"));
        }

        false
    }

    /// Register project inputs whose changes should clear cached database, config, and constants.
    fn watched_file_registrations() -> Vec<Registration> {
        let watch_kind = Some(WatchKind::Create | WatchKind::Change | WatchKind::Delete);
        let watchers = [
            "**/rotom.toml",
            "**/.rotom/command_database/**/*.json",
            "**/include/**/*.{h,inc,json}",
            "**/generated/**/*.{h,inc,json}",
        ]
        .into_iter()
        .map(|pattern| FileSystemWatcher {
            glob_pattern: GlobPattern::String(pattern.to_string()),
            kind: watch_kind,
        })
        .collect();

        vec![Registration {
            id: "rotom-watched-files".to_string(),
            method: "workspace/didChangeWatchedFiles".to_string(),
            register_options: Some(serde_json::json!(
                DidChangeWatchedFilesRegistrationOptions { watchers }
            )),
        }]
    }

    /// Return whether the client can accept dynamic watched-file registration.
    fn supports_watched_file_dynamic_registration(params: &InitializeParams) -> bool {
        params
            .capabilities
            .workspace
            .as_ref()
            .and_then(|workspace| workspace.did_change_watched_files.as_ref())
            .and_then(|capabilities| capabilities.dynamic_registration)
            .unwrap_or(false)
    }

    fn load_project_state(
        root: &std::path::Path,
        config: &RotomConfig,
        client: Client,
    ) -> std::result::Result<ProjectState, String> {
        let db_path = config
            .database_file(root)
            .ok_or_else(|| "No database configured in rotom.toml".to_string())?;
        let db = DatabaseV2::load(&db_path)
            .map_err(|e| format!("Failed to load database {}: {}", db_path.display(), e))?;

        let mut constants = ConstantDb::new();
        let _ = constants.load_from_db(&db);

        let database_dir = config.database_dir(root);
        if database_dir.exists() {
            let _ = constants.load_directory(&database_dir);
        }

        let workspace: Option<Arc<uxie::Workspace>> = match config.workspace.project_type {
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
                uxie::Workspace::open(root).ok().map(Arc::new)
            }
            ProjectTypeConfig::Dspre => {
                let language = uxie::RomHeader::open(root)
                    .map_or(uxie::game::GameLanguage::English, |h| h.detect_language());
                let _ = constants.load_dspre_text_archives(root, language);
                uxie::Workspace::open(root).ok().map(Arc::new)
            }
            ProjectTypeConfig::HgEngine => {
                if let Ok(mut ws) = uxie::Workspace::open(root) {
                    let _ = ws.load_hg_engine_constants();
                    let _ = constants.load_decomp_symbols(root, (*ws.symbols).clone());
                    Some(Arc::new(ws))
                } else {
                    None
                }
            }
            ProjectTypeConfig::Generic => None,
        };

        if let Some(ws) = &workspace {
            constants.set_message_ids(ws.shared_message_ids());
        }

        let message_refs = Arc::new(MessageRefIndex::default());
        let state = ProjectState {
            db: Arc::new(db),
            constants,
            workspace,
            message_refs,
        };
        if let Some(ws) = state.workspace.clone() {
            let source_roots = config.source_roots(root);
            let state_bg = Arc::new(state.clone());
            tokio::spawn(async move {
                Self::rebuild_message_ref_index(state_bg, ws, source_roots).await;
                let _ = client.code_lens_refresh().await;
            });
        }

        Ok(state)
    }

    /// Recursively collect `.rotom` script files under a source root.
    fn collect_rotom_files(root: &std::path::Path, out: &mut Vec<PathBuf>) {
        let Ok(read_dir) = std::fs::read_dir(root) else {
            return;
        };
        for entry in read_dir.flatten() {
            let path = entry.path();
            if path.is_dir() {
                Self::collect_rotom_files(&path, out);
            } else if path.extension().and_then(|s| s.to_str()) == Some("rotom") {
                out.push(path);
            }
        }
    }

    /// Build the message-reference index in the background.
    async fn rebuild_message_ref_index(
        state: Arc<ProjectState>,
        workspace: Arc<uxie::Workspace>,
        source_roots: Vec<PathBuf>,
    ) {
        for root in source_roots {
            let mut files = Vec::new();
            Self::collect_rotom_files(&root, &mut files);
            for path in files {
                Self::reindex_message_ref_file(
                    &state.message_refs,
                    &state,
                    &workspace,
                    &state.db,
                    &path,
                );
                tokio::task::yield_now().await;
            }
        }
    }

    /// Clone project constants and apply this script's `#include` / `#define` directives.
    fn file_constants_for_index(
        state: &ProjectState,
        path: &std::path::Path,
        source: &str,
    ) -> ConstantDb {
        if path.extension().and_then(|s| s.to_str()) != Some("rotom") {
            return state.constants.clone();
        }
        if !source.contains("#include") && !source.contains("#define") {
            return state.constants.clone();
        }
        let mut file_constants = state.constants.clone();
        let lexer = rotom::compiler::Lexer::new(source);
        let mut parser = rotom::compiler::Parser::new_fallible(lexer);
        if let Ok(file) = parser.parse_script_file() {
            let script_dir = path.parent().unwrap_or_else(|| std::path::Path::new("."));
            let _ = file_constants.apply_directives(script_dir, source, &file.items);
        }
        file_constants
    }

    /// Rebuild references for a single file after save.
    ///
    /// Acquires a per-file mutex to prevent concurrent calls for the same
    /// canonical path from producing duplicate `by_message` entries.
    fn reindex_message_ref_file(
        refs: &MessageRefIndex,
        state: &ProjectState,
        workspace: &uxie::Workspace,
        db: &DatabaseV2,
        path: &std::path::Path,
    ) {
        let canonical_path = std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
        let guard = refs
            .reindex_locks
            .entry(canonical_path.clone())
            .or_insert_with(|| Arc::new(std::sync::Mutex::new(())));
        let lock = Arc::clone(&guard);
        drop(guard);
        let _reindex_guard = lock.lock().expect("reindex lock poisoned");
        if let Some((_, old_pairs)) = refs.by_file.remove(&canonical_path) {
            for pair in old_pairs {
                if let Some(mut existing) = refs.by_message.get_mut(&pair) {
                    existing.retain(|r| r.script_path != canonical_path);
                }
            }
        }
        let Ok(source) = std::fs::read_to_string(&canonical_path) else {
            return;
        };
        let constants = Self::file_constants_for_index(state, &canonical_path, &source);
        let found = collect_message_refs(&source, workspace, db, &canonical_path, Some(&constants));
        if found.is_empty() {
            refs.by_file.insert(canonical_path, Vec::new());
            return;
        }
        let mut pairs = Vec::with_capacity(found.len());
        for (pair, entry) in found {
            pairs.push(pair);
            refs.by_message.entry(pair).or_default().push(entry);
        }
        refs.by_file.insert(canonical_path, pairs);
    }

    /// Build file-local constants for a URI by cloning project-wide constants
    /// and applying any `#include` / `#define` directives found in the source.
    ///
    /// Returns [`None`] when a directive-bearing `.rotom` file fails to parse (constants fall back
    /// to project defaults elsewhere).
    fn rotom_file_constants_prep(
        state: &ProjectState,
        uri: &Url,
        source: &str,
    ) -> Option<RotomFileConstantsPrep> {
        let file_path = uri.to_file_path().ok()?;

        // Only clone and process directives for .rotom files that actually
        // have include/define statements.
        if file_path.extension().and_then(|s| s.to_str()) != Some("rotom") {
            return Some(RotomFileConstantsPrep {
                constants: state.constants.clone(),
                directive_parse_for_diagnostics: None,
            });
        }
        if !source.contains("#include") && !source.contains("#define") {
            return Some(RotomFileConstantsPrep {
                constants: state.constants.clone(),
                directive_parse_for_diagnostics: None,
            });
        }

        let mut file_constants = state.constants.clone();

        let lexer = rotom::compiler::Lexer::new(source);
        let mut parser = rotom::compiler::Parser::new_fallible(lexer);
        let file = parser.parse_script_file().ok()?;
        let parse_errors = std::mem::take(&mut parser.errors);
        let script_dir = file_path
            .parent()
            .unwrap_or_else(|| std::path::Path::new("."));
        let _ = file_constants.apply_directives(script_dir, source, &file.items);

        Some(RotomFileConstantsPrep {
            constants: file_constants,
            directive_parse_for_diagnostics: Some((Arc::new(file), parse_errors)),
        })
    }

    /// Schedule diagnostics to be published after a debounce delay.
    ///
    /// If the user is typing rapidly, old pending tasks are aborted so only
    /// the most recent change triggers a diagnostic pass.
    fn publish_diagnostics(&self, uri: &Url) {
        if !is_rotom_script_uri(uri) {
            return;
        }
        // Abort any existing pending diagnostic task for this document.
        if let Some((_, old)) = self.pending_diagnostics.remove(uri) {
            old.abort();
        }

        let uri_for_task = uri.clone();
        let client = self.client.clone();
        let text = self
            .documents
            .get(&uri_for_task)
            .map(|doc| doc.text.clone());
        let project_state = self.project_state_for_uri(&uri_for_task);

        // Compute file-local constants synchronously before spawning.
        let file_prep = project_state.as_ref().and_then(|state| {
            text.as_ref()
                .and_then(|src| Self::rotom_file_constants_prep(state, &uri_for_task, src))
        });

        let handle = tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(DIAGNOSTIC_DEBOUNCE_MS)).await;

            let Some(text) = text else {
                return;
            };
            let (db, constants, reuse_directive_parse) = if let Some(state) = project_state {
                match file_prep {
                    Some(prep) => (
                        Some(state.db),
                        prep.constants,
                        prep.directive_parse_for_diagnostics,
                    ),
                    None => (Some(state.db), state.constants.clone(), None),
                }
            } else {
                let empty = rotom::database::ConstantDb::new();
                (None, empty, None)
            };

            let diagnostics = compute_diagnostics(
                &text,
                db.as_deref(),
                Some(&constants),
                reuse_directive_parse,
            );
            client
                .publish_diagnostics(uri_for_task, diagnostics, None)
                .await;
        });

        self.pending_diagnostics.insert(uri.clone(), handle);
    }
}

#[tower_lsp::async_trait]
impl LanguageServer for RotomServer {
    async fn initialize(&self, params: InitializeParams) -> Result<InitializeResult> {
        self.client_supports_watched_file_dynamic_registration
            .store(
                Self::supports_watched_file_dynamic_registration(&params),
                Ordering::Relaxed,
            );

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
                document_symbol_provider: Some(OneOf::Right(
                    tower_lsp::lsp_types::DocumentSymbolOptions {
                        label: Some("Rotom".to_string()),
                        work_done_progress_options: WorkDoneProgressOptions {
                            work_done_progress: None,
                        },
                    },
                )),
                definition_provider: Some(OneOf::Left(true)),
                code_lens_provider: Some(CodeLensOptions {
                    resolve_provider: Some(false),
                }),
                signature_help_provider: Some(SignatureHelpOptions {
                    trigger_characters: Some(vec![
                        "(".to_string(),
                        ",".to_string(),
                        " ".to_string(),
                    ]),
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
        if !self
            .client_supports_watched_file_dynamic_registration
            .load(Ordering::Relaxed)
        {
            return;
        }

        if let Err(error) = self
            .client
            .register_capability(Self::watched_file_registrations())
            .await
        {
            self.client
                .log_message(
                    MessageType::WARNING,
                    format!("Failed to register watched files: {error}"),
                )
                .await;
        }
    }

    async fn shutdown(&self) -> Result<()> {
        Ok(())
    }

    async fn completion(&self, params: CompletionParams) -> Result<Option<CompletionResponse>> {
        let uri = &params.text_document_position.text_document.uri;
        if !is_rotom_script_uri(uri) {
            return Ok(None);
        }
        let position = params.text_document_position.position;

        let doc = self.documents.get(uri);
        let Some(doc) = doc else {
            return Ok(None);
        };

        let project_state = self.project_state_for_uri(uri);
        let db = project_state.as_ref().map(|s| s.db.clone());
        let constant_names = project_state
            .as_ref()
            .map(|state| Arc::new(state.constants.constant_names()) as Arc<Vec<String>>);

        let local_symbols = self
            .documents
            .get(uri)
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
        if !is_rotom_script_uri(uri) {
            return Ok(None);
        }
        let position = params.text_document_position_params.position;

        let doc = self.documents.get(uri);
        let Some(doc) = doc else {
            return Ok(None);
        };

        let project_state = self.project_state_for_uri(uri);
        let db = project_state.as_ref().map(|s| s.db.clone());
        let file_constants = project_state
            .as_ref()
            .and_then(|state| Self::rotom_file_constants_prep(state, uri, &doc.text))
            .map(|prep| prep.constants);
        let workspace = project_state.as_ref().and_then(|s| s.workspace.clone());
        let script_file = uri
            .to_file_path()
            .ok()
            .and_then(|p| p.file_stem()?.to_str().map(str::to_string));

        let hover = compute_hover(
            &doc.text,
            position,
            db.as_deref(),
            file_constants.as_ref(),
            workspace.as_deref(),
            script_file.as_deref(),
        );

        Ok(hover)
    }

    async fn document_symbol(
        &self,
        params: tower_lsp::lsp_types::DocumentSymbolParams,
    ) -> Result<Option<DocumentSymbolResponse>> {
        let uri = &params.text_document.uri;
        if !is_rotom_script_uri(uri) {
            return Ok(None);
        }

        let symbols = self
            .documents
            .get(uri)
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
        if !is_rotom_script_uri(uri) {
            return Ok(None);
        }
        let position = params.text_document_position_params.position;

        let doc = self.documents.get(uri);
        let Some(doc) = doc else {
            return Ok(None);
        };

        let project_state = self.project_state_for_uri(uri);
        let ws = project_state.as_ref().and_then(|s| s.workspace.clone());
        let db = project_state.as_ref().map(|s| s.db.clone());
        let script_file = uri
            .to_file_path()
            .ok()
            .and_then(|p| p.file_stem()?.to_str().map(str::to_string));

        Ok(compute_goto_definition(
            &doc.text,
            position,
            uri,
            ws.as_ref(),
            db.as_deref(),
            script_file.as_deref(),
        ))
    }

    async fn signature_help(
        &self,
        params: tower_lsp::lsp_types::SignatureHelpParams,
    ) -> Result<Option<SignatureHelp>> {
        let uri = &params.text_document_position_params.text_document.uri;
        if !is_rotom_script_uri(uri) {
            return Ok(None);
        }
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
        if is_message_archive_uri(uri) {
            let Some(state) = self.project_state_for_uri(uri) else {
                return Ok(None);
            };
            let Some(workspace) = state.workspace.as_ref() else {
                return Ok(None);
            };
            let path = uri.to_file_path().ok();
            let archive_id = path
                .as_ref()
                .and_then(|p| p.file_stem())
                .and_then(|s| s.to_str())
                .and_then(|stem| workspace.text_archive_id(stem));
            let Some(archive_id) = archive_id else {
                return Ok(None);
            };
            let _ = workspace.ensure_archive_loaded(archive_id);
            let mut indices = std::collections::BTreeSet::new();
            for (_, idx) in workspace.message_ids_for_archive(archive_id) {
                indices.insert(idx);
            }
            for entry in &state.message_refs.by_message {
                if entry.key().0 == archive_id {
                    indices.insert(entry.key().1);
                }
            }
            // Scan the archive JSON once for all entry line numbers, then index
            // into it — avoids re-reading the file per message (O(entries) vs
            // O(entries^2), and every message gets a lens here).
            let lines = path.as_deref().map(json_message_lines).unwrap_or_default();
            let mut entries = Vec::new();
            for idx in indices {
                let Some(&line) = lines.get(idx as usize) else {
                    continue;
                };
                let refs = state
                    .message_refs
                    .by_message
                    .get(&(archive_id, idx))
                    .map(|v| v.clone())
                    .unwrap_or_default();
                entries.push((line as usize, refs));
            }
            let lenses = build_message_code_lens(uri, entries);
            return Ok(if lenses.is_empty() {
                None
            } else {
                Some(lenses)
            });
        }

        if !is_rotom_script_uri(uri) {
            return Ok(None);
        }

        let doc = self.documents.get(uri);
        let Some(doc) = doc else {
            return Ok(None);
        };

        let db = self.project_state_for_uri(uri).map(|state| state.db);
        let lenses = compute_script_code_lens(&doc.text, uri, db.as_deref());
        Ok(if lenses.is_empty() {
            None
        } else {
            Some(lenses)
        })
    }

    async fn did_open(&self, params: DidOpenTextDocumentParams) {
        let uri = params.text_document.uri;
        self.documents
            .insert(uri.clone(), params.text_document.text);
        if is_message_archive_uri(&uri) {
            let _ = self.project_state_for_uri(&uri);
        }
        self.publish_diagnostics(&uri);
    }

    async fn did_change(&self, params: DidChangeTextDocumentParams) {
        let uri = params.text_document.uri;
        self.documents.apply_changes(&uri, params.content_changes);
        self.publish_diagnostics(&uri);
    }

    async fn did_save(&self, params: DidSaveTextDocumentParams) {
        let uri = params.text_document.uri;
        let Some(path) = uri.to_file_path().ok() else {
            return;
        };
        if Self::should_invalidate_project_state(&path) {
            self.invalidate_project_for_path(&path);
        }
        if path.extension().and_then(|s| s.to_str()) != Some("rotom") {
            return;
        }
        let Some(state) = self.project_state_for_uri(&uri) else {
            return;
        };
        let Some(workspace) = state.workspace.as_ref() else {
            return;
        };
        Self::reindex_message_ref_file(&state.message_refs, &state, workspace, &state.db, &path);
        let _ = self.client.code_lens_refresh().await;
    }

    async fn did_change_watched_files(&self, params: DidChangeWatchedFilesParams) {
        let mut invalidated = false;
        for change in params.changes {
            if change.typ == FileChangeType::CREATED
                || change.typ == FileChangeType::CHANGED
                || change.typ == FileChangeType::DELETED
            {
                let Ok(path) = change.uri.to_file_path() else {
                    continue;
                };
                if Self::should_invalidate_project_state(&path) {
                    self.invalidate_project_for_path(&path);
                    invalidated = true;
                }
            }
        }
        if invalidated {
            let _ = self.client.code_lens_refresh().await;
        }
    }

    async fn inlay_hint(
        &self,
        params: tower_lsp::lsp_types::InlayHintParams,
    ) -> Result<Option<Vec<InlayHint>>> {
        let uri = &params.text_document.uri;
        if !is_rotom_script_uri(uri) {
            return Ok(None);
        }

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

#[cfg(test)]
mod tests {
    use super::{MessageRefIndex, ProjectState, RotomServer};
    use crate::code_lens::build_message_code_lens;
    use rotom::database::ConstantDb;
    use rotom::database::DatabaseV2;
    use std::path::Path;
    use std::sync::Arc;
    use tower_lsp::lsp_types::{
        ClientCapabilities, DidChangeWatchedFilesClientCapabilities, InitializeParams,
        WorkspaceClientCapabilities,
    };
    use uxie::game::Game;

    fn test_db(path: &std::path::Path) -> Arc<DatabaseV2> {
        let json = r#"{
  "meta": { "version": "test" },
  "commands": {
    "MessageFromBank": {
      "type": "script_cmd",
      "id": 2,
      "description": "",
      "params": [
        { "name": "bank", "type": "u16" },
        { "name": "text_slot", "type": "msg_id" }
      ]
    }
  },
  "movements": {}
}"#;
        let db_path = path.join("commands.json");
        std::fs::write(&db_path, json).expect("db write");
        Arc::new(DatabaseV2::load(&db_path).expect("db load"))
    }

    fn test_state(root: &std::path::Path) -> ProjectState {
        ProjectState {
            db: test_db(root),
            constants: ConstantDb::new(),
            workspace: Some(Arc::new(uxie::Workspace::new(
                root.to_path_buf(),
                Game::Platinum,
            ))),
            message_refs: Arc::new(MessageRefIndex::default()),
        }
    }

    /// Mirrors the archive `CodeLens` handler: collect indices, map to JSON lines, attach refs.
    fn message_lens_entries(
        state: &ProjectState,
        archive_path: &std::path::Path,
        archive_id: u16,
    ) -> Vec<(usize, Vec<crate::message_refs::MessageRef>)> {
        let workspace = state.workspace.as_ref().expect("workspace");
        let _ = workspace.ensure_archive_loaded(archive_id);
        let mut indices = std::collections::BTreeSet::new();
        for (_, idx) in workspace.message_ids_for_archive(archive_id) {
            indices.insert(idx);
        }
        for entry in &state.message_refs.by_message {
            if entry.key().0 == archive_id {
                indices.insert(entry.key().1);
            }
        }
        let lines = super::json_message_lines(archive_path);
        let mut entries = Vec::new();
        for idx in indices {
            let Some(&line) = lines.get(idx as usize) else {
                continue;
            };
            let refs = state
                .message_refs
                .by_message
                .get(&(archive_id, idx))
                .map(|v| v.clone())
                .unwrap_or_default();
            entries.push((line as usize, refs));
        }
        entries
    }

    #[test]
    fn code_lens_didsave_invalidation() {
        let dir = tempfile::tempdir().expect("tmp");
        let root = dir.path();
        std::fs::create_dir_all(root.join("expanded/textArchives")).expect("archives");
        std::fs::create_dir_all(root.join("scripts")).expect("scripts");
        std::fs::write(
            root.join("expanded/textArchives/0199.json"),
            "{\n  \"messages\": [\n    {\"id\": \"msg_0199_00000\", \"en_US\": \"A\"},\n    {\"id\": \"msg_0199_00001\", \"en_US\": \"B\"}\n  ]\n}\n",
        )
        .expect("archive");
        let a_path = root.join("scripts/a.rotom");
        let b_path = root.join("scripts/b.rotom");
        std::fs::write(&a_path, "script a # 0:\n    MessageFromBank 199, 1\n").expect("a");
        std::fs::write(&b_path, "script b # 0:\n    MessageFromBank 199, 1\n").expect("b");

        let state = test_state(root);
        let workspace = state.workspace.as_ref().expect("ws").clone();
        RotomServer::reindex_message_ref_file(
            &state.message_refs,
            &state,
            &workspace,
            &state.db,
            &a_path,
        );
        RotomServer::reindex_message_ref_file(
            &state.message_refs,
            &state,
            &workspace,
            &state.db,
            &b_path,
        );
        assert_eq!(
            state
                .message_refs
                .by_message
                .get(&(199, 1))
                .map(|v| v.len()),
            Some(2)
        );

        std::fs::write(&b_path, "script b # 0:\n    MessageFromBank 199, 0\n").expect("b update");
        RotomServer::reindex_message_ref_file(
            &state.message_refs,
            &state,
            &workspace,
            &state.db,
            &b_path,
        );

        assert_eq!(
            state
                .message_refs
                .by_message
                .get(&(199, 1))
                .map(|v| v.len()),
            Some(1)
        );
        assert_eq!(
            state
                .message_refs
                .by_message
                .get(&(199, 0))
                .map(|v| v.len()),
            Some(1)
        );

        let archive_path = root.join("expanded/textArchives/0199.json");
        let entries = message_lens_entries(&state, &archive_path, 199);
        let line_1 = super::json_message_lines(&archive_path)[1] as usize;
        let count_at_1 = entries
            .iter()
            .find(|(line, _)| *line == line_1)
            .map(|(_, refs)| refs.len());
        assert_eq!(
            count_at_1,
            Some(1),
            "lens for msg 1 should show one ref after save"
        );
    }

    #[test]
    fn code_lens_counts_references_across_files() {
        let dir = tempfile::tempdir().expect("tmp");
        let root = dir.path();
        std::fs::create_dir_all(root.join("expanded/textArchives")).expect("archives");
        std::fs::create_dir_all(root.join("scripts")).expect("scripts");
        std::fs::write(
            root.join("expanded/textArchives/0199.json"),
            "{\n  \"messages\": [\n    {\"id\": \"msg_0199_00000\", \"en_US\": \"A\"},\n    {\"id\": \"msg_0199_00001\", \"en_US\": \"B\"}\n  ]\n}\n",
        )
        .expect("archive");
        std::fs::write(
            root.join("scripts/a.rotom"),
            "script a # 0:\n    MessageFromBank 199, 1\n",
        )
        .expect("a");
        std::fs::write(
            root.join("scripts/b.rotom"),
            "script b # 0:\n    MessageFromBank 199, 1\n",
        )
        .expect("b");

        let state = test_state(root);
        let workspace = state.workspace.as_ref().expect("ws").clone();
        RotomServer::reindex_message_ref_file(
            &state.message_refs,
            &state,
            &workspace,
            &state.db,
            &root.join("scripts/a.rotom"),
        );
        RotomServer::reindex_message_ref_file(
            &state.message_refs,
            &state,
            &workspace,
            &state.db,
            &root.join("scripts/b.rotom"),
        );

        let archive_path = root.join("expanded/textArchives/0199.json");
        let entries = message_lens_entries(&state, &archive_path, 199);
        let refs_at_1 = entries
            .into_iter()
            .find(|(_, refs)| refs.len() == 2)
            .map(|(_, refs)| refs)
            .expect("message index 1 should have two script refs");
        assert_eq!(refs_at_1.len(), 2);

        let archive_uri =
            tower_lsp::lsp_types::Url::from_file_path(&archive_path).expect("archive uri");
        let line = super::json_message_lines(&archive_path)[1] as usize;
        let lenses = build_message_code_lens(&archive_uri, vec![(line, refs_at_1)]);
        assert_eq!(lenses.len(), 1);
        assert!(
            lenses[0]
                .command
                .as_ref()
                .is_some_and(|c| c.title == "2 references")
        );
    }

    #[tokio::test]
    async fn code_lens_initial_population_background() {
        let dir = tempfile::tempdir().expect("tmp");
        let root = dir.path();
        let scripts = root.join("scripts");
        std::fs::create_dir_all(root.join("expanded/textArchives")).expect("archives");
        std::fs::create_dir_all(&scripts).expect("scripts");
        std::fs::write(
            root.join("expanded/textArchives/0199.json"),
            "{\n  \"messages\": [\n    {\"id\": \"msg_0199_00000\", \"en_US\": \"A\"},\n    {\"id\": \"msg_0199_00001\", \"en_US\": \"B\"}\n  ]\n}\n",
        )
        .expect("archive");
        std::fs::write(
            scripts.join("a.rotom"),
            "script a # 0:\n    MessageFromBank 199, 1\n",
        )
        .expect("script");

        let state = Arc::new(test_state(root));
        let workspace = state.workspace.as_ref().expect("ws").clone();
        assert!(
            state.message_refs.by_message.is_empty(),
            "index starts empty before background walk"
        );

        RotomServer::rebuild_message_ref_index(state.clone(), workspace, vec![scripts]).await;

        assert_eq!(
            state
                .message_refs
                .by_message
                .get(&(199, 1))
                .map(|v| v.len()),
            Some(1)
        );
    }

    #[test]
    fn project_state_invalidation_matches_config_and_database_paths() {
        assert!(RotomServer::should_invalidate_project_state(
            std::path::Path::new("/tmp/project/rotom.toml")
        ));
        assert!(RotomServer::should_invalidate_project_state(
            std::path::Path::new("/tmp/project/.rotom/command_database/platinum_v2.json")
        ));
        assert!(RotomServer::should_invalidate_project_state(
            std::path::Path::new("/tmp/project/include/constants/vars.h")
        ));
        assert!(!RotomServer::should_invalidate_project_state(
            std::path::Path::new("/tmp/project/res/field/scripts/script_001.rotom")
        ));
    }

    #[test]
    fn should_invalidate_project_state_for_project_config_and_database() {
        assert!(RotomServer::should_invalidate_project_state(Path::new("rotom.toml")));
        assert!(RotomServer::should_invalidate_project_state(Path::new(".rotom/command_database/platinum_v2.json")));
    }

    #[test]
    fn should_invalidate_project_state_for_include_inputs() {
        assert!(RotomServer::should_invalidate_project_state(Path::new("include/constants/items.h")));
        assert!(RotomServer::should_invalidate_project_state(Path::new("generated/events.inc")));
    }

    #[test]
    fn should_not_invalidate_project_state_for_status_or_cache_outputs() {
        assert!(!RotomServer::should_invalidate_project_state(Path::new(".rotom/status/compile_state.json")));
        assert!(!RotomServer::should_invalidate_project_state(Path::new(".rotom/cache/include-cache.json")));
    }

    #[test]
    fn watched_file_registration_covers_project_inputs() {
        let registrations = RotomServer::watched_file_registrations();
        assert_eq!(registrations.len(), 1);
        assert_eq!(registrations[0].method, "workspace/didChangeWatchedFiles");

        let options = registrations[0]
            .register_options
            .as_ref()
            .expect("watch registration should carry options");
        let patterns: Vec<String> = options["watchers"]
            .as_array()
            .expect("watchers should be an array")
            .iter()
            .map(|watcher| {
                watcher["globPattern"]
                    .as_str()
                    .expect("glob pattern should be a string")
                    .to_string()
            })
            .collect();

        assert_eq!(
            patterns,
            vec![
                "**/rotom.toml",
                "**/.rotom/command_database/**/*.json",
                "**/include/**/*.{h,inc,json}",
                "**/generated/**/*.{h,inc,json}",
            ]
        );
    }

    #[test]
    fn watched_file_registration_requires_dynamic_registration_capability() {
        assert!(!RotomServer::supports_watched_file_dynamic_registration(
            &InitializeParams::default()
        ));

        let params = InitializeParams {
            capabilities: ClientCapabilities {
                workspace: Some(WorkspaceClientCapabilities {
                    did_change_watched_files: Some(DidChangeWatchedFilesClientCapabilities {
                        dynamic_registration: Some(true),
                        ..Default::default()
                    }),
                    ..Default::default()
                }),
                ..Default::default()
            },
            ..Default::default()
        };

        assert!(RotomServer::supports_watched_file_dynamic_registration(
            &params
        ));
    }

    #[test]
    fn reindex_serialization_prevents_duplicates_on_repeat_call() {
        let dir = tempfile::tempdir().expect("tmp");
        let root = dir.path();
        std::fs::create_dir_all(root.join("expanded/textArchives")).expect("archives");
        std::fs::create_dir_all(root.join("scripts")).expect("scripts");
        std::fs::write(
            root.join("expanded/textArchives/0199.json"),
            "{\n  \"messages\": [\n    {\"id\": \"msg_0199_00000\", \"en_US\": \"A\"},\n    {\"id\": \"msg_0199_00001\", \"en_US\": \"B\"}\n  ]\n}\n",
        )
        .expect("archive");
        let file = root.join("scripts/s.rotom");
        std::fs::write(&file, "script s # 0:\n    MessageFromBank 199, 1\n").expect("write");

        let state = test_state(root);
        let workspace = state.workspace.as_ref().expect("ws").clone();

        // Call reindex twice — the lock serializes so no duplicates appear.
        RotomServer::reindex_message_ref_file(
            &state.message_refs,
            &state,
            &workspace,
            &state.db,
            &file,
        );
        RotomServer::reindex_message_ref_file(
            &state.message_refs,
            &state,
            &workspace,
            &state.db,
            &file,
        );

        let refs = state
            .message_refs
            .by_message
            .get(&(199, 1))
            .map(|v| v.len());
        assert_eq!(refs, Some(1), "no duplicate entries after repeat reindex");
    }

    #[test]
    fn reindex_lock_entry_created_after_reindex() {
        let dir = tempfile::tempdir().expect("tmp");
        let root = dir.path();
        std::fs::create_dir_all(root.join("expanded/textArchives")).expect("archives");
        std::fs::create_dir_all(root.join("scripts")).expect("scripts");
        std::fs::write(
            root.join("expanded/textArchives/0199.json"),
            "{\n  \"messages\": [\n    {\"id\": \"msg_0199_00000\", \"en_US\": \"A\"},\n    {\"id\": \"msg_0199_00001\", \"en_US\": \"B\"}\n  ]\n}\n",
        )
        .expect("archive");
        let file = root.join("scripts/s.rotom");
        std::fs::write(&file, "script s # 0:\n    MessageFromBank 199, 1\n").expect("write");

        let state = test_state(root);
        let workspace = state.workspace.as_ref().expect("ws").clone();
        RotomServer::reindex_message_ref_file(
            &state.message_refs,
            &state,
            &workspace,
            &state.db,
            &file,
        );

        // The lock entry should exist for the canonical path after reindex.
        let canonical = std::fs::canonicalize(&file).unwrap_or_else(|_| file.clone());
        assert!(state.message_refs.reindex_locks.contains_key(&canonical));
    }
}
