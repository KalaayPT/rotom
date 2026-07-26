use std::collections::{HashMap, HashSet};
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
    build_message_code_lens, collect_global_script_refs, compute_script_code_lens,
    is_message_archive_uri, is_rotom_script_uri,
};
use crate::completions::compute_completions;
use crate::diagnostics::compute_diagnostics;
use crate::document::{DocumentCache, compute_document_symbols};
use crate::goto::{compute_goto_definition, json_message_lines};
use crate::hover::compute_hover;
use crate::inlay_hints::compute_inlay_hints;
use crate::message_refs::{MessageRef, collect_message_refs};
use crate::signature_help::compute_signature_help;

use rotom::database::ConstantDb;
use rotom::project::config::{RotomConfig, find_project_root, load_config};

/// How long to wait after the last keystroke before re-running diagnostics.
const DIAGNOSTIC_DEBOUNCE_MS: u64 = 300;

/// Per-project cached state: loaded database, project-wide constants,
/// and the uxie workspace for message lookups and script resolution.
#[derive(Clone)]
struct ProjectState {
    project: Arc<rotom::ProjectContext>,
    message_refs: Arc<MessageRefIndex>,
    global_script_refs: Arc<GlobalScriptRefIndex>,
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

type GlobalScriptTarget = (PathBuf, String);

#[derive(Default)]
struct GlobalScriptRefIndex {
    /// Per-script contributions used for save invalidation.
    by_file: DashMap<PathBuf, Vec<GlobalScriptTarget>>,
    /// References grouped by resolved target source path and public label.
    by_target: DashMap<GlobalScriptTarget, Vec<tower_lsp::lsp_types::Location>>,
    reindex_locks: DashMap<PathBuf, Arc<std::sync::Mutex<()>>>,
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

    /// Remove cached project state and return the affected project root.
    fn invalidate_project_for_path(&self, path: &std::path::Path) -> Option<PathBuf> {
        let project_root = find_project_root(path)?;
        self.projects.remove(&project_root);
        Some(project_root)
    }

    /// Reschedule diagnostics for open Rotoscript documents in one project.
    fn publish_project_diagnostics(&self, project_root: &std::path::Path) {
        for uri in self.documents.uris() {
            let belongs_to_project = uri
                .to_file_path()
                .ok()
                .and_then(|path| find_project_root(&path))
                .is_some_and(|root| root == project_root);
            if belongs_to_project && is_rotom_script_uri(&uri) {
                self.publish_diagnostics(&uri);
            }
        }
    }

    /// Returns `true` when a file change at `path` warrants clearing
    /// the project-level cache (database, config, constants, or script source).
    fn should_invalidate_project_state(path: &std::path::Path) -> bool {
        let file_name = path.file_name().and_then(|s| s.to_str());
        if matches!(
            file_name,
            Some("rotom.toml" | "scripts.order" | "script_manager.c" | "fieldmap.c" | "arm9.bin")
        ) {
            return true;
        }
        if matches!(
            path.extension().and_then(|s| s.to_str()),
            Some("rotom" | "s" | "script")
        ) {
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
            "**/scripts.order",
            "**/src/{script_manager,fieldmap}.c",
            "**/arm9.bin",
            "**/.rotom/command_database/**/*.json",
            "**/include/**/*.{h,inc,json}",
            "**/generated/**/*.{h,inc,json}",
            "**/*.{rotom,s,script}",
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

    /// Load project resources and editor-specific indexes for one project root.
    fn load_project_state(
        root: &std::path::Path,
        config: &RotomConfig,
        client: Client,
    ) -> std::result::Result<ProjectState, String> {
        let project = Arc::new(
            rotom::ProjectContext::load_tolerant(root, config)
                .map_err(|error| error.to_string())?,
        );
        let message_refs = Arc::new(MessageRefIndex::default());
        let state = ProjectState {
            project,
            message_refs,
            global_script_refs: Arc::new(GlobalScriptRefIndex::default()),
        };
        if state.project.workspace().is_some() {
            let source_roots = state.project.config().source_roots(state.project.root());
            let state_bg = Arc::new(state.clone());
            tokio::spawn(async move {
                Self::rebuild_project_ref_indexes(state_bg, source_roots).await;
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

    /// Build project-wide reverse-reference indexes in the background.
    async fn rebuild_project_ref_indexes(state: Arc<ProjectState>, source_roots: Vec<PathBuf>) {
        if state.project.workspace().is_none() {
            return;
        }
        for root in source_roots {
            let mut files = Vec::new();
            Self::collect_rotom_files(&root, &mut files);
            for path in files {
                Self::reindex_message_ref_file(&state.message_refs, &state, &path);
                Self::reindex_global_script_ref_file(
                    &state.global_script_refs,
                    &state.project,
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
            return state.project.project_constants().clone();
        }
        if !source.contains("#include") && !source.contains("#define") {
            return state.project.project_constants().clone();
        }
        let mut file_constants = state.project.project_constants().clone();
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
        path: &std::path::Path,
    ) {
        let Some(workspace) = state.project.workspace() else {
            return;
        };
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
        let found = collect_message_refs(
            &source,
            workspace,
            state.project.db(),
            &canonical_path,
            Some(&constants),
        );
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

    /// Rebuild resolved global-script references contributed by one source file.
    fn reindex_global_script_ref_file(
        refs: &GlobalScriptRefIndex,
        project: &rotom::ProjectContext,
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
        let Ok(uri) = Url::from_file_path(&canonical_path) else {
            return;
        };
        if let Some((_, old_targets)) = refs.by_file.remove(&canonical_path) {
            for target in old_targets {
                if let Some(mut existing) = refs.by_target.get_mut(&target) {
                    existing.retain(|reference| reference.uri != uri);
                }
            }
        }
        let Ok(source) = std::fs::read_to_string(&canonical_path) else {
            return;
        };
        let found = collect_global_script_refs(&source, &uri, project);
        let mut targets = Vec::with_capacity(found.len());
        for reference in found {
            let target_path =
                std::fs::canonicalize(&reference.target_path).unwrap_or(reference.target_path);
            let target = (target_path, reference.target_label);
            targets.push(target.clone());
            refs.by_target
                .entry(target)
                .or_default()
                .push(reference.location);
        }
        refs.by_file.insert(canonical_path, targets);
    }

    /// Return cross-file references grouped by public label for one source URI.
    fn global_script_refs_for_uri(
        refs: &GlobalScriptRefIndex,
        uri: &Url,
    ) -> HashMap<String, Vec<tower_lsp::lsp_types::Location>> {
        let Some(path) = uri.to_file_path().ok() else {
            return HashMap::new();
        };
        let target_path = std::fs::canonicalize(&path).unwrap_or(path);
        refs.by_target
            .iter()
            .filter(|entry| entry.key().0 == target_path)
            .map(|entry| (entry.key().1.clone(), entry.value().clone()))
            .collect()
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
        let source_path = uri_for_task.to_file_path().ok();

        let handle = tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(DIAGNOSTIC_DEBOUNCE_MS)).await;

            let Some(text) = text else {
                return;
            };
            let diagnostics = compute_diagnostics(
                &text,
                project_state.as_ref().map(|state| state.project.as_ref()),
                source_path.as_deref(),
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
                    trigger_characters: Some(vec![
                        " ".to_string(),
                        ".".to_string(),
                        ">".to_string(),
                    ]),
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
        let db = project_state.as_ref().map(|state| state.project.db());
        let constant_names = project_state.as_ref().map(|state| {
            Arc::new(state.project.project_constants().constant_names()) as Arc<Vec<String>>
        });

        let local_symbols = self
            .documents
            .get(uri)
            .map(|doc| compute_document_symbols(&doc.text));

        let items = compute_completions(
            &doc.text,
            position,
            db,
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
        let project = project_state.as_ref().map(|state| state.project.as_ref());
        let source_path = uri.to_file_path().ok();

        let hover = compute_hover(&doc.text, position, project, source_path.as_deref());

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
        let project = project_state.as_ref().map(|state| state.project.as_ref());
        let script_file = uri
            .to_file_path()
            .ok()
            .and_then(|p| p.file_stem()?.to_str().map(str::to_string));

        Ok(compute_goto_definition(
            &doc.text,
            position,
            uri,
            project,
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

        let project_state = self.project_state_for_uri(uri);
        let db = project_state.as_ref().map(|state| state.project.db());

        Ok(compute_signature_help(&doc.text, position, db))
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
            let Some(workspace) = state.project.workspace() else {
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

        let project_state = self.project_state_for_uri(uri);
        let project = project_state.as_ref().map(|state| state.project.as_ref());
        let global_refs = project_state
            .as_ref()
            .map(|state| Self::global_script_refs_for_uri(&state.global_script_refs, uri));
        let lenses = compute_script_code_lens(&doc.text, uri, project, global_refs.as_ref());
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
        if Self::should_invalidate_project_state(&path)
            && let Some(project_root) = self.invalidate_project_for_path(&path)
        {
            self.publish_project_diagnostics(&project_root);
        }
        if path.extension().and_then(|s| s.to_str()) != Some("rotom") {
            return;
        }
        let Some(state) = self.project_state_for_uri(&uri) else {
            return;
        };
        Self::reindex_message_ref_file(&state.message_refs, &state, &path);
        Self::reindex_global_script_ref_file(&state.global_script_refs, &state.project, &path);
        let _ = self.client.code_lens_refresh().await;
    }

    async fn did_change_watched_files(&self, params: DidChangeWatchedFilesParams) {
        let mut invalidated_projects = HashSet::new();
        for change in params.changes {
            if change.typ == FileChangeType::CREATED
                || change.typ == FileChangeType::CHANGED
                || change.typ == FileChangeType::DELETED
            {
                let Ok(path) = change.uri.to_file_path() else {
                    continue;
                };
                if Self::should_invalidate_project_state(&path)
                    && let Some(project_root) = self.invalidate_project_for_path(&path)
                {
                    invalidated_projects.insert(project_root);
                }
            }
        }
        for project_root in &invalidated_projects {
            self.publish_project_diagnostics(project_root);
        }
        if !invalidated_projects.is_empty() {
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

        let project_state = self.project_state_for_uri(uri);
        let db = project_state.as_ref().map(|state| state.project.db());

        let hints = compute_inlay_hints(&doc.text, db);
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
    use super::{GlobalScriptRefIndex, MessageRefIndex, ProjectState, RotomServer};
    use crate::code_lens::{build_message_code_lens, compute_script_code_lens};
    use rotom::database::ConstantDb;
    use rotom::database::DatabaseV2;
    use rotom::project::config::{
        DatabaseConfig, PathsConfig, ProjectMetadata, ProjectTypeConfig, RotomConfig,
        WorkspaceConfig,
    };
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
        let config = RotomConfig {
            format_version: 1,
            project: ProjectMetadata {
                name: "test".to_string(),
            },
            workspace: WorkspaceConfig {
                project_type: ProjectTypeConfig::Dspre,
                game_family: Some(rotom::GameFamily::Platinum),
            },
            paths: PathsConfig {
                database_dir: ".rotom/command_database".to_string(),
                cache_dir: ".rotom/cache".to_string(),
                status_dir: ".rotom/status".to_string(),
                source_roots: vec!["scripts".to_string()],
                include_roots: Vec::new(),
                binary_roots: Vec::new(),
            },
            database: Some(DatabaseConfig {
                default_file: "commands.json".to_string(),
            }),
        };
        let project = rotom::ProjectContext::from_parts(
            root.to_path_buf(),
            config,
            test_db(root),
            ConstantDb::new(),
            Some(Arc::new(uxie::Workspace::new(
                root.to_path_buf(),
                Game::Platinum,
            ))),
        );
        ProjectState {
            project: Arc::new(project),
            message_refs: Arc::new(MessageRefIndex::default()),
            global_script_refs: Arc::new(GlobalScriptRefIndex::default()),
        }
    }

    /// Mirrors the archive `CodeLens` handler: collect indices, map to JSON lines, attach refs.
    fn message_lens_entries(
        state: &ProjectState,
        archive_path: &std::path::Path,
        archive_id: u16,
    ) -> Vec<(usize, Vec<crate::message_refs::MessageRef>)> {
        let workspace = state.project.workspace().expect("workspace");
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
        RotomServer::reindex_message_ref_file(&state.message_refs, &state, &a_path);
        RotomServer::reindex_message_ref_file(&state.message_refs, &state, &b_path);
        assert_eq!(
            state
                .message_refs
                .by_message
                .get(&(199, 1))
                .map(|v| v.len()),
            Some(2)
        );

        std::fs::write(&b_path, "script b # 0:\n    MessageFromBank 199, 0\n").expect("b update");
        RotomServer::reindex_message_ref_file(&state.message_refs, &state, &b_path);

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
        RotomServer::reindex_message_ref_file(
            &state.message_refs,
            &state,
            &root.join("scripts/a.rotom"),
        );
        RotomServer::reindex_message_ref_file(
            &state.message_refs,
            &state,
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
    async fn script_code_lens_counts_global_references_across_files() {
        let dir = tempfile::tempdir().expect("tmp");
        let root = dir.path();
        let scripts = root.join("scripts");
        std::fs::create_dir_all(&scripts).expect("scripts");
        let target_path = scripts.join("0003.rotom");
        let caller_path = scripts.join("0004.rotom");
        let target_source = "script script_7 #7:\n    End\n";
        std::fs::write(&target_path, target_source).expect("target");
        std::fs::write(
            &caller_path,
            "script Main #1:\n    CallStd CommonScripts::script_7\n    End\n",
        )
        .expect("caller");

        let mut workspace = uxie::Workspace::new(root.to_path_buf(), Game::HeartGold);
        workspace
            .scripts
            .load_dspre_script_dir(&scripts)
            .expect("script table");
        workspace.global_script_table = uxie::script_file::GlobalScriptTable::from_entries(vec![
            uxie::script_file::GlobalScriptEntry::with_range(
                2000,
                3,
                40,
                uxie::script_file::GlobalScriptRange::CommonScripts,
            ),
        ]);
        let config = RotomConfig {
            format_version: 1,
            project: ProjectMetadata {
                name: "test".to_string(),
            },
            workspace: WorkspaceConfig {
                project_type: ProjectTypeConfig::Dspre,
                game_family: Some(rotom::GameFamily::HGSS),
            },
            paths: PathsConfig {
                database_dir: ".rotom/command_database".to_string(),
                cache_dir: ".rotom/cache".to_string(),
                status_dir: ".rotom/status".to_string(),
                source_roots: vec!["scripts".to_string()],
                include_roots: Vec::new(),
                binary_roots: Vec::new(),
            },
            database: Some(DatabaseConfig {
                default_file: DatabaseV2::test_hgss_path().display().to_string(),
            }),
        };
        let db = Arc::new(DatabaseV2::load(DatabaseV2::test_hgss_path()).expect("db"));
        let state = Arc::new(ProjectState {
            project: Arc::new(rotom::ProjectContext::from_parts(
                root.to_path_buf(),
                config,
                db,
                ConstantDb::new(),
                Some(Arc::new(workspace)),
            )),
            message_refs: Arc::new(MessageRefIndex::default()),
            global_script_refs: Arc::new(GlobalScriptRefIndex::default()),
        });
        RotomServer::rebuild_project_ref_indexes(Arc::clone(&state), vec![scripts]).await;

        let target_uri =
            tower_lsp::lsp_types::Url::from_file_path(&target_path).expect("target uri");
        let global_refs =
            RotomServer::global_script_refs_for_uri(&state.global_script_refs, &target_uri);
        let lenses = compute_script_code_lens(
            target_source,
            &target_uri,
            Some(state.project.as_ref()),
            Some(&global_refs),
        );
        assert_eq!(
            lenses[0]
                .command
                .as_ref()
                .map(|command| command.title.as_str()),
            Some("1 reference")
        );

        std::fs::write(&caller_path, "script Main #1:\n    End\n").expect("caller update");
        RotomServer::reindex_global_script_ref_file(
            &state.global_script_refs,
            &state.project,
            &caller_path,
        );
        let global_refs =
            RotomServer::global_script_refs_for_uri(&state.global_script_refs, &target_uri);
        let lenses = compute_script_code_lens(
            target_source,
            &target_uri,
            Some(state.project.as_ref()),
            Some(&global_refs),
        );
        assert_eq!(
            lenses[0]
                .command
                .as_ref()
                .map(|command| command.title.as_str()),
            Some("0 references")
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
        assert!(
            state.message_refs.by_message.is_empty(),
            "index starts empty before background walk"
        );

        RotomServer::rebuild_project_ref_indexes(state.clone(), vec![scripts]).await;

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
        assert!(RotomServer::should_invalidate_project_state(
            std::path::Path::new("/tmp/project/res/field/scripts/script_001.rotom")
        ));
        assert!(RotomServer::should_invalidate_project_state(
            std::path::Path::new("/tmp/project/res/field/scripts/scripts.order")
        ));
        assert!(RotomServer::should_invalidate_project_state(
            std::path::Path::new("/tmp/project/src/script_manager.c")
        ));
        assert!(RotomServer::should_invalidate_project_state(
            std::path::Path::new("/tmp/project/arm9/arm9.bin")
        ));
        assert!(RotomServer::should_invalidate_project_state(
            std::path::Path::new("/tmp/project/res/field/scripts/script_002.s")
        ));
        assert!(RotomServer::should_invalidate_project_state(
            std::path::Path::new("/tmp/project/scripts/0003.script")
        ));
    }

    #[test]
    fn should_invalidate_project_state_for_project_config_and_database() {
        assert!(RotomServer::should_invalidate_project_state(Path::new(
            "rotom.toml"
        )));
        assert!(RotomServer::should_invalidate_project_state(Path::new(
            ".rotom/command_database/platinum_v2.json"
        )));
    }

    #[test]
    fn should_invalidate_project_state_for_include_inputs() {
        assert!(RotomServer::should_invalidate_project_state(Path::new(
            "include/constants/items.h"
        )));
        assert!(RotomServer::should_invalidate_project_state(Path::new(
            "generated/events.inc"
        )));
    }

    #[test]
    fn should_not_invalidate_project_state_for_status_or_cache_outputs() {
        assert!(!RotomServer::should_invalidate_project_state(Path::new(
            ".rotom/status/compile_state.json"
        )));
        assert!(!RotomServer::should_invalidate_project_state(Path::new(
            ".rotom/cache/include-cache.json"
        )));
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
                "**/scripts.order",
                "**/src/{script_manager,fieldmap}.c",
                "**/arm9.bin",
                "**/.rotom/command_database/**/*.json",
                "**/include/**/*.{h,inc,json}",
                "**/generated/**/*.{h,inc,json}",
                "**/*.{rotom,s,script}",
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

        // Call reindex twice — the lock serializes so no duplicates appear.
        RotomServer::reindex_message_ref_file(&state.message_refs, &state, &file);
        RotomServer::reindex_message_ref_file(&state.message_refs, &state, &file);

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
        RotomServer::reindex_message_ref_file(&state.message_refs, &state, &file);

        // The lock entry should exist for the canonical path after reindex.
        let canonical = std::fs::canonicalize(&file).unwrap_or_else(|_| file.clone());
        assert!(state.message_refs.reindex_locks.contains_key(&canonical));
    }
}
