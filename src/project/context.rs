//! Loaded project resources shared by compilation, decompilation, and editor tooling.

use std::collections::{BTreeSet, HashMap};
use std::ops::Range;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use snafu::ResultExt;
use uxie::Workspace;
use xxhash_rust::xxh3::xxh3_64;

use super::config::{ProjectTypeConfig, RotomConfig};
use super::error::{IoSnafu, ProjectError, Result};
use crate::compiler::ast::StatementKind;
use crate::database::{ConstantDb, DatabaseV2, GameFamily};
use crate::{PreparedScript, prepare_script_source};

/// One public script entry declared in a workspace source file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GlobalScriptSymbol {
    /// Public source label.
    pub name: String,
    /// One-based jump-table slot from `script name #N:`.
    pub slot: u32,
    /// Definition span in the original on-disk source.
    pub span: Range<usize>,
}

#[derive(Debug)]
struct IndexedGlobalScriptFile {
    pub path: PathBuf,
    prepared: PreparedScript,
    symbols: Vec<GlobalScriptSymbol>,
}

/// A resolved global script source reference.
#[derive(Debug, Clone)]
pub struct ResolvedGlobalScript {
    /// Absolute script ID passed to the game command.
    pub script_id: u16,
    /// Script archive file containing the target.
    pub script_file_id: u16,
    /// Canonical global range module.
    pub module: String,
    /// Resolved public source symbol.
    pub symbol: GlobalScriptSymbol,
    /// How this reference must be spelled after `module::` to name
    /// [`Self::script_id`] again. Usually [`GlobalScriptSymbol::name`], but a
    /// label shared by several jump-table slots cannot single one out, so the
    /// slot number is used instead.
    pub reference_label: String,
    /// Source file containing the symbol.
    pub path: PathBuf,
    /// Original source text used by editor navigation.
    pub source: Arc<str>,
}

/// A loaded project and the resources derived from its configuration.
pub struct ProjectContext {
    root: PathBuf,
    config: RotomConfig,
    db: Arc<DatabaseV2>,
    project_constants: ConstantDb,
    workspace: Option<Arc<Workspace>>,
    database_hash: u64,
    constant_cache_rebuilt: bool,
    global_script_files: HashMap<u16, IndexedGlobalScriptFile>,
    global_script_errors: HashMap<u16, String>,
    global_script_file_ids_by_path: HashMap<PathBuf, u16>,
}

impl ProjectContext {
    /// Load all required project resources, returning errors for incomplete project state.
    ///
    /// # Errors
    /// Returns an error when configured database, constants, cache, or required
    /// workspace resources cannot be loaded.
    pub fn load(root: &Path, config: &RotomConfig) -> Result<Self> {
        Self::load_inner(root, config, false, false)
    }

    /// Load project resources while tolerating unavailable optional workspace data.
    ///
    /// Configuration and the command database remain required. Failures in
    /// project constants, cached symbols, and workspace enrichment are ignored.
    ///
    /// # Errors
    /// Returns an error when the command database is missing or unreadable.
    pub fn load_tolerant(root: &Path, config: &RotomConfig) -> Result<Self> {
        Self::load_inner(root, config, true, true)
    }

    /// Load required decompilation resources while treating workspace enrichment as optional.
    pub(crate) fn load_for_decompile(root: &Path, config: &RotomConfig) -> Result<Self> {
        Self::load_inner(root, config, false, true)
    }

    /// Construct a context from already-loaded resources while deriving all project caches.
    ///
    /// This is intended for embedders that already own the loading policy. The
    /// global-script cache and shared message IDs are always derived here.
    pub fn from_parts(
        root: PathBuf,
        config: RotomConfig,
        db: Arc<DatabaseV2>,
        mut project_constants: ConstantDb,
        workspace: Option<Arc<Workspace>>,
    ) -> Self {
        if let Some(workspace) = &workspace {
            project_constants.set_message_ids(workspace.shared_message_ids());
        }
        let (global_script_files, global_script_errors, global_script_file_ids_by_path) =
            index_global_scripts(workspace.as_deref(), &db);
        Self {
            root,
            config,
            db,
            project_constants,
            workspace,
            database_hash: 0,
            constant_cache_rebuilt: false,
            global_script_files,
            global_script_errors,
            global_script_file_ids_by_path,
        }
    }

    /// Load project resources under the selected strictness policy.
    #[allow(clippy::too_many_lines)]
    fn load_inner(
        root: &Path,
        config: &RotomConfig,
        tolerant: bool,
        optional_workspace: bool,
    ) -> Result<Self> {
        let db_path = config
            .database_file(root)
            .ok_or(ProjectError::MissingDefaultDatabase)?;
        let database_hash = std::fs::read(&db_path)
            .map(|bytes| xxh3_64(&bytes))
            .context(IoSnafu {
                action: "Failed to hash database file",
                path: db_path.clone(),
            })?;
        let db = Arc::new(DatabaseV2::load(&db_path).map_err(ProjectError::from)?);
        let mut project_constants = ConstantDb::new();
        project_constants.load_from_db(&db);

        let database_dir = config.database_dir(root);
        if database_dir.exists() {
            let loaded = project_constants.load_directory(&database_dir);
            if !tolerant {
                loaded.map_err(ProjectError::from)?;
            }
        }

        let mut constant_cache_rebuilt = false;
        let workspace = match config.workspace.project_type {
            ProjectTypeConfig::Decomp => {
                if let Some(game_family) = config.game_family() {
                    match Workspace::load_cached_symbols(
                        &config.cache_dir(root),
                        root,
                        &config.include_roots(root),
                        game_family,
                    ) {
                        Ok((symbols, rebuilt)) => {
                            project_constants.load_decomp_symbols(root, (*symbols).clone());
                            constant_cache_rebuilt = rebuilt;
                        }
                        Err(_) if tolerant => {}
                        Err(source) => {
                            return Err(ProjectError::Io {
                                action: "Failed to load constant cache",
                                path: config.cache_dir(root),
                                source,
                            });
                        }
                    }
                } else if !tolerant {
                    return Err(ProjectError::MissingGameFamily);
                }
                open_workspace(root, config, optional_workspace)?
            }
            ProjectTypeConfig::Dspre => {
                let workspace = open_workspace(root, config, optional_workspace)?;
                let language = if let Some(workspace) = &workspace {
                    project_constants.load_dspre_symbols((*workspace.symbols).clone());
                    workspace.language
                } else {
                    uxie::GameLanguage::English
                };
                let _ = project_constants.load_dspre_text_archives(root, language);
                workspace
            }
            ProjectTypeConfig::HgEngine => {
                let mut workspace = match Workspace::open(root) {
                    Ok(workspace) => Some(workspace),
                    Err(_) if tolerant => None,
                    Err(source) => {
                        return Err(ProjectError::Io {
                            action: "Failed to open HgEngine workspace",
                            path: root.to_path_buf(),
                            source,
                        });
                    }
                };
                if let Some(workspace) = workspace.as_mut() {
                    let loaded = workspace.load_hg_engine_constants();
                    if !tolerant {
                        loaded.context(IoSnafu {
                            action: "Failed to load HgEngine constants",
                            path: root.to_path_buf(),
                        })?;
                    }
                    project_constants.load_decomp_symbols(root, (*workspace.symbols).clone());
                }
                workspace.map(Arc::new)
            }
            ProjectTypeConfig::Generic if optional_workspace => open_workspace(root, config, true)?,
            ProjectTypeConfig::Generic => open_workspace(root, config, false)?,
        };

        if let Some(workspace) = &workspace {
            project_constants.set_message_ids(workspace.shared_message_ids());
        }
        let (global_script_files, global_script_errors, global_script_file_ids_by_path) =
            index_global_scripts(workspace.as_deref(), &db);

        Ok(Self {
            root: root.to_path_buf(),
            config: config.clone(),
            db,
            project_constants,
            workspace,
            database_hash,
            constant_cache_rebuilt,
            global_script_files,
            global_script_errors,
            global_script_file_ids_by_path,
        })
    }

    /// Return the project root.
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Return the loaded project configuration.
    pub fn config(&self) -> &RotomConfig {
        &self.config
    }

    /// Return the command database selected by the project configuration.
    pub fn db(&self) -> &DatabaseV2 {
        &self.db
    }

    /// Return constants loaded for the whole project.
    pub fn project_constants(&self) -> &ConstantDb {
        &self.project_constants
    }

    /// Return the Uxie workspace when this project has one available.
    pub fn workspace(&self) -> Option<&Workspace> {
        self.workspace.as_deref()
    }

    /// Return the hash of the loaded command database file.
    pub(crate) fn database_hash(&self) -> u64 {
        self.database_hash
    }

    /// Return whether loading rebuilt the project constant cache.
    pub(crate) fn constant_cache_rebuilt(&self) -> bool {
        self.constant_cache_rebuilt
    }

    /// Resolve a canonical range module or filename-stem alias and public label.
    pub fn resolve_global_script_ref(
        &self,
        module: &str,
        label: &str,
    ) -> std::result::Result<ResolvedGlobalScript, String> {
        let workspace = self.workspace.as_deref().ok_or_else(|| {
            "No project workspace is available for global script resolution".to_string()
        })?;
        let entry = if let Some(entry) = workspace.global_script_table.find_by_module_name(module) {
            entry.clone()
        } else {
            let file_id = workspace
                .scripts
                .get_id(module)
                .and_then(|id| u16::try_from(id).ok())
                .or_else(|| module.parse::<u16>().ok())
                .ok_or_else(|| format!("Unknown global script module '{module}'"))?;
            let entries: Vec<_> = workspace
                .global_script_table
                .entries_for_script_file_id(file_id)
                .collect();
            match entries.as_slice() {
                [] => {
                    return Err(format!(
                        "Script file '{module}' does not back a global script range"
                    ));
                }
                [entry] => (*entry).clone(),
                _ => {
                    let modules = entries
                        .iter()
                        .map(|entry| entry.range.module_name())
                        .collect::<Vec<_>>()
                        .join(", ");
                    return Err(format!(
                        "Global script module alias '{module}' is ambiguous; use one of: {modules}"
                    ));
                }
            }
        };

        let file =
            self.indexed_global_script_file(&entry.range.module_name(), entry.script_file_id)?;
        let symbol = select_symbol(&file.symbols, label)
            .map_err(|reason| format!("{reason} in module '{}'", entry.range.module_name()))?
            .clone();

        let slot_offset = symbol.slot.checked_sub(1).ok_or_else(|| {
            format!(
                "Script label '{label}' in module '{}' uses invalid slot #0",
                entry.range.module_name()
            )
        })?;
        let script_id = u32::from(entry.min_script_id)
            .checked_add(slot_offset)
            .and_then(|id| u16::try_from(id).ok())
            .ok_or_else(|| {
                format!(
                    "Script slot #{} exceeds module '{}' global ID range",
                    symbol.slot,
                    entry.range.module_name()
                )
            })?;
        if workspace
            .global_script_table
            .lookup(script_id)
            .is_none_or(|resolved| resolved.min_script_id != entry.min_script_id)
        {
            return Err(format!(
                "Script slot #{} exceeds module '{}' global ID range",
                symbol.slot,
                entry.range.module_name()
            ));
        }

        Ok(ResolvedGlobalScript {
            script_id,
            script_file_id: entry.script_file_id,
            module: entry.range.module_name(),
            symbol,
            reference_label: label.to_string(),
            path: file.path.clone(),
            source: Arc::clone(&file.prepared.source),
        })
    }

    /// Resolve a numeric global script ID back to its canonical source symbol.
    ///
    /// Never fails: an ID outside every known range, or one whose archive has
    /// no indexed source, simply has no source symbol to name.
    pub fn resolve_global_script_id(&self, script_id: u16) -> Option<ResolvedGlobalScript> {
        let workspace = self.workspace.as_deref()?;
        let entry = workspace.global_script_table.lookup(script_id)?;
        let slot = u32::from(script_id - entry.min_script_id + 1);
        let file = self.global_script_files.get(&entry.script_file_id)?;
        let mut matches = file.symbols.iter().filter(|symbol| symbol.slot == slot);
        let symbol = matches.next()?.clone();
        if matches.next().is_some() {
            return None;
        }

        // The label alone only names this slot when no other slot answers to
        // it; a function spanning `#[42, 44, 50, 57]` carries one label for
        // four slots. Fall back to the slot number, which `module::44` accepts
        // and no source label can collide with.
        let names_this_slot = file
            .symbols
            .iter()
            .filter(|candidate| candidate.name == symbol.name)
            .count()
            == 1
            || slot_label(&symbol.name) == Some(slot);
        let reference_label = if names_this_slot {
            symbol.name.clone()
        } else {
            slot.to_string()
        };

        Some(ResolvedGlobalScript {
            script_id,
            script_file_id: entry.script_file_id,
            module: entry.range.module_name(),
            symbol,
            reference_label,
            path: file.path.clone(),
            source: Arc::clone(&file.prepared.source),
        })
    }

    /// Return prepared source when `path` is one of the indexed global scripts.
    pub(crate) fn prepared_global_script(&self, path: &Path) -> Option<&PreparedScript> {
        let file_id = self.global_script_file_ids_by_path.get(path)?;
        self.global_script_files
            .get(file_id)
            .map(|file| &file.prepared)
    }

    /// Return one indexed global source or its preparation error.
    fn indexed_global_script_file(
        &self,
        module: &str,
        file_id: u16,
    ) -> std::result::Result<&IndexedGlobalScriptFile, String> {
        if let Some(file) = self.global_script_files.get(&file_id) {
            return Ok(file);
        }
        if let Some(error) = self.global_script_errors.get(&file_id) {
            return Err(format!("Could not index module '{module}': {error}"));
        }
        Err(format!(
            "No source file is available for global script module '{module}' (file {file_id})"
        ))
    }
}

/// Open the configured workspace, optionally tolerating unavailable project data.
fn open_workspace(
    root: &Path,
    config: &RotomConfig,
    tolerant: bool,
) -> Result<Option<Arc<Workspace>>> {
    match Workspace::open(root) {
        Ok(workspace) => Ok(Some(Arc::new(workspace))),
        Err(_) if tolerant => Ok(None),
        Err(source)
            if source.kind() == std::io::ErrorKind::NotFound
                || source.kind() == std::io::ErrorKind::IsADirectory =>
        {
            let game = config
                .game_family()
                .unwrap_or(GameFamily::Platinum)
                .default_game();
            Ok(Some(Arc::new(Workspace::new(root.to_path_buf(), game))))
        }
        Err(source) => Err(ProjectError::Io {
            action: "Failed to open workspace",
            path: root.to_path_buf(),
            source,
        }),
    }
}

/// Prepare and index every source file referenced by the workspace's global ranges.
fn index_global_scripts(
    workspace: Option<&Workspace>,
    db: &DatabaseV2,
) -> (
    HashMap<u16, IndexedGlobalScriptFile>,
    HashMap<u16, String>,
    HashMap<PathBuf, u16>,
) {
    let Some(workspace) = workspace else {
        return (HashMap::new(), HashMap::new(), HashMap::new());
    };
    let mut files = HashMap::new();
    let mut errors = HashMap::new();
    let mut paths = HashMap::new();
    let file_ids: BTreeSet<_> = workspace
        .global_script_table
        .entries()
        .iter()
        .map(|entry| entry.script_file_id)
        .collect();

    for file_id in file_ids {
        let Some(path) = workspace.script_source_path(file_id) else {
            continue;
        };
        match index_script_file(&path, workspace, db) {
            Ok(file) => {
                paths.insert(path.clone(), file_id);
                if let Ok(canonical) = std::fs::canonicalize(&path) {
                    paths.insert(canonical, file_id);
                }
                files.insert(file_id, file);
            }
            Err(error) => {
                errors.insert(file_id, error);
            }
        }
    }

    (files, errors, paths)
}

/// Pick the public symbol that `label` names.
///
/// One function can occupy several jump-table slots
/// (`script script_42 #[42, 44, 50, 57]:`), so a label may name four symbols
/// while a sibling slot names none. A label that matches exactly one symbol
/// wins; otherwise the slot it encodes decides.
fn select_symbol<'a>(
    symbols: &'a [GlobalScriptSymbol],
    label: &str,
) -> std::result::Result<&'a GlobalScriptSymbol, String> {
    let by_name: Vec<_> = symbols
        .iter()
        .filter(|symbol| symbol.name == label)
        .collect();
    let by_slot: Vec<_> = slot_label(label)
        .map(|slot| symbols.iter().filter(|s| s.slot == slot).collect())
        .unwrap_or_default();

    match (by_name.as_slice(), by_slot.as_slice()) {
        ([symbol], _) | (_, [symbol]) => Ok(symbol),
        ([], []) => Err(format!("Script label '{label}' was not found")),
        _ => Err(format!("Script label '{label}' has multiple public slots")),
    }
}

/// The jump-table slot a `module::label` reference names, when the label is a
/// slot rather than a source symbol.
///
/// The decompiler renders a global script reference from the target's script
/// ID. It writes the target's own label when that label names exactly this
/// slot, and the bare slot number (`CommonScripts::44`) when the label is
/// shared by several slots. `script_<slot>` is the older spelling of the same
/// thing and stays accepted so already-converted sources keep compiling.
/// Returns `None` for hand-written labels.
fn slot_label(label: &str) -> Option<u32> {
    let digits = label.strip_prefix("script_").unwrap_or(label);
    if digits.is_empty() || !digits.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    digits.parse().ok()
}

/// Prepare one global source and collect its public script labels and slots.
fn index_script_file(
    path: &Path,
    workspace: &uxie::Workspace,
    db: &DatabaseV2,
) -> std::result::Result<IndexedGlobalScriptFile, String> {
    let source = std::fs::read_to_string(path)
        .map_err(|error| format!("failed to read '{}': {error}", path.display()))?;
    let source = Arc::<str>::from(source);
    let extension = path.extension().and_then(|ext| ext.to_str()).unwrap_or("");
    let prepared = prepare_script_source(path, source, Some(workspace), db)
        .map_err(|failure| format!("failed to prepare '{}': {}", path.display(), failure.error))?;
    let mut symbols = Vec::new();
    for item in &prepared.ast.items {
        let StatementKind::Function { headers, .. } = &item.node else {
            continue;
        };
        for header in headers {
            let Some(slot) = header.id.filter(|_| header.is_public) else {
                continue;
            };
            let span = definition_span(&prepared.source, extension, &header.name, slot)
                .unwrap_or_else(|| {
                    item.span.clone().start.min(prepared.source.len())
                        ..item.span.end.min(prepared.source.len())
                });
            symbols.push(GlobalScriptSymbol {
                name: header.name.clone(),
                slot,
                span,
            });
        }
    }

    Ok(IndexedGlobalScriptFile {
        path: path.to_path_buf(),
        prepared,
        symbols,
    })
}

/// Locate a public script definition in its original pre-transpilation source.
fn definition_span(source: &str, extension: &str, name: &str, slot: u32) -> Option<Range<usize>> {
    let mut offset = 0;
    for line in source.split_inclusive('\n') {
        let trimmed = line.trim();
        let matches = match extension {
            "rotom" => trimmed
                .strip_prefix("script ")
                .and_then(|rest| rest.split_whitespace().next())
                .is_some_and(|candidate| candidate == name),
            "s" => trimmed
                .split_once(':')
                .is_some_and(|(candidate, _)| candidate.trim() == name),
            "script" => {
                trimmed
                    .strip_prefix("Script ")
                    .and_then(|rest| rest.split_once(':'))
                    .and_then(|(id, _)| id.trim().parse::<u32>().ok())
                    == Some(slot)
            }
            _ => false,
        };
        if matches {
            return Some(offset..offset + line.trim_end_matches(['\r', '\n']).len());
        }
        offset += line.len();
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::project::config::{DatabaseConfig, PathsConfig, ProjectMetadata, WorkspaceConfig};
    use uxie::game::Game;
    use uxie::script_file::{GlobalScriptEntry, GlobalScriptTable};

    fn workspace_with_source(source: &str) -> (tempfile::TempDir, uxie::Workspace) {
        let temp = tempfile::tempdir().unwrap();
        let script_dir = temp.path().join("scripts");
        std::fs::create_dir(&script_dir).unwrap();
        std::fs::write(script_dir.join("0211.rotom"), source).unwrap();

        let mut workspace = uxie::Workspace::new(temp.path().to_path_buf(), Game::Platinum);
        workspace
            .scripts
            .load_dspre_script_dir(&script_dir)
            .unwrap();
        workspace.global_script_table =
            GlobalScriptTable::from_entries(vec![GlobalScriptEntry::new(
                2000,
                211,
                213,
                "Common Scripts",
            )]);
        (temp, workspace)
    }

    fn test_config(
        project_type: ProjectTypeConfig,
        game_family: Option<GameFamily>,
        database_dir: &str,
    ) -> RotomConfig {
        RotomConfig {
            format_version: 1,
            project: ProjectMetadata {
                name: "test".to_string(),
            },
            workspace: WorkspaceConfig {
                project_type,
                game_family,
            },
            paths: PathsConfig {
                database_dir: database_dir.to_string(),
                cache_dir: ".rotom/cache".to_string(),
                status_dir: ".rotom/status".to_string(),
                source_roots: vec!["scripts".to_string()],
                include_roots: Vec::new(),
                binary_roots: Vec::new(),
            },
            database: Some(DatabaseConfig {
                default_file: DatabaseV2::test_platinum_path().display().to_string(),
            }),
        }
    }

    fn context_for_workspace(
        temp: &tempfile::TempDir,
        workspace: uxie::Workspace,
    ) -> ProjectContext {
        let config = test_config(
            ProjectTypeConfig::Dspre,
            Some(GameFamily::Platinum),
            ".rotom/command_database",
        );
        ProjectContext::from_parts(
            temp.path().to_path_buf(),
            config,
            Arc::new(
                DatabaseV2::load(DatabaseV2::test_platinum_path())
                    .expect("failed to load test database"),
            ),
            ConstantDb::new(),
            Some(Arc::new(workspace)),
        )
    }

    #[test]
    fn strict_load_requires_decomp_game_family_but_tolerant_load_recovers() {
        let temp = tempfile::tempdir().unwrap();
        let config = test_config(ProjectTypeConfig::Decomp, None, ".rotom/command_database");

        assert!(matches!(
            ProjectContext::load(temp.path(), &config),
            Err(ProjectError::MissingGameFamily)
        ));

        let project = ProjectContext::load_tolerant(temp.path(), &config).unwrap();
        assert!(project.workspace().is_none());
        assert_eq!(project.config(), &config);
    }

    #[test]
    fn tolerant_load_ignores_invalid_project_constant_files() {
        let temp = tempfile::tempdir().unwrap();
        let constants_dir = temp.path().join("constants");
        std::fs::create_dir(&constants_dir).unwrap();
        std::fs::write(constants_dir.join("broken.json"), "not json").unwrap();
        let config = test_config(ProjectTypeConfig::Generic, None, "constants");

        assert!(ProjectContext::load(temp.path(), &config).is_err());
        assert!(ProjectContext::load_tolerant(temp.path(), &config).is_ok());
    }

    #[test]
    fn decompile_load_tolerates_invalid_optional_dspre_workspace() {
        let temp = tempfile::tempdir().unwrap();
        std::fs::write(temp.path().join("header.bin"), [0]).unwrap();
        let config = test_config(
            ProjectTypeConfig::Dspre,
            Some(GameFamily::Platinum),
            ".rotom/command_database",
        );

        assert!(ProjectContext::load(temp.path(), &config).is_err());
        let project = ProjectContext::load_for_decompile(temp.path(), &config).unwrap();
        assert!(project.workspace().is_none());
    }

    #[test]
    fn resolves_canonical_and_filename_modules() {
        let (temp, workspace) = workspace_with_source("script NewGame #2:\n    End\n");
        let project = context_for_workspace(&temp, workspace);

        let canonical = project
            .resolve_global_script_ref("CommonScripts", "NewGame")
            .unwrap();
        let filename = project
            .resolve_global_script_ref("0211", "NewGame")
            .unwrap();

        assert_eq!(canonical.script_id, 2001);
        assert_eq!(filename.script_id, 2001);
        assert_eq!(canonical.symbol.span, 0..18);
    }

    #[test]
    fn resolves_every_slot_of_a_multi_slot_script() {
        // One function, three entry points. Only slot 42 carries the label, so
        // the other two can only be named by slot.
        let (temp, workspace) = workspace_with_source("script script_42 #[42, 44, 50]:\n    End\n");
        let project = context_for_workspace(&temp, workspace);

        for (label, expected_id) in [("script_42", 2041), ("44", 2043), ("50", 2049)] {
            let resolved = project
                .resolve_global_script_ref("CommonScripts", label)
                .unwrap_or_else(|error| panic!("{label} did not resolve: {error}"));
            assert_eq!(resolved.script_id, expected_id, "{label}");
        }

        assert!(
            project
                .resolve_global_script_ref("CommonScripts", "43")
                .is_err(),
            "slot 43 is not an entry point of this script"
        );
    }

    #[test]
    fn still_resolves_the_legacy_script_slot_spelling() {
        // Sources converted before `module::<slot>` existed spell a slot as
        // `script_<slot>`, a label the decompiler invented and that matches no
        // definition. Those projects must keep compiling.
        let (temp, workspace) = workspace_with_source("script script_42 #[42, 44, 50]:\n    End\n");
        let project = context_for_workspace(&temp, workspace);

        for (label, expected_id) in [("script_44", 2043), ("script_50", 2049)] {
            let resolved = project
                .resolve_global_script_ref("CommonScripts", label)
                .unwrap_or_else(|error| panic!("{label} did not resolve: {error}"));
            assert_eq!(resolved.script_id, expected_id, "{label}");
        }

        // A real label still wins over the slot it happens to encode.
        let (temp, workspace) =
            workspace_with_source("script script_44 #7:\n    End\n\nscript Other #44:\n    End\n");
        let project = context_for_workspace(&temp, workspace);
        assert_eq!(
            project
                .resolve_global_script_ref("CommonScripts", "script_44")
                .unwrap()
                .script_id,
            2006,
            "a defined label must not be reinterpreted as a slot"
        );
    }

    #[test]
    fn names_a_multi_slot_script_by_slot_when_its_label_cannot() {
        let (temp, workspace) = workspace_with_source("script script_42 #[42, 44, 50]:\n    End\n");
        let project = context_for_workspace(&temp, workspace);

        // Slot 42 owns the label, so the label names it unambiguously.
        let owning = project.resolve_global_script_id(2041).unwrap();
        assert_eq!(owning.reference_label, "script_42");

        // Slots 44 and 50 answer to the same label, so it cannot single them
        // out; the slot number can, and no source label can collide with it.
        for (script_id, expected) in [(2043u16, "44"), (2049, "50")] {
            let resolved = project.resolve_global_script_id(script_id).unwrap();
            assert_eq!(resolved.reference_label, expected);
            assert_eq!(resolved.symbol.name, "script_42");
        }

        // Whatever spelling is emitted must name the same script again.
        for script_id in [2041u16, 2043, 2049] {
            let emitted = project.resolve_global_script_id(script_id).unwrap();
            let reparsed = project
                .resolve_global_script_ref(&emitted.module, &emitted.reference_label)
                .unwrap_or_else(|error| {
                    panic!("{} did not round-trip: {error}", emitted.reference_label)
                });
            assert_eq!(reparsed.script_id, script_id);
        }
    }

    #[test]
    fn unknown_global_script_ids_resolve_to_nothing_rather_than_failing() {
        let (temp, workspace) = workspace_with_source("script script_42 #[42, 44, 50]:\n    End\n");
        let project = context_for_workspace(&temp, workspace);

        // Outside every configured range, and inside the range but backed by no
        // slot: both leave the decompiler to emit the raw literal.
        assert!(project.resolve_global_script_id(1).is_none());
        assert!(project.resolve_global_script_id(2042).is_none());
    }

    #[test]
    fn resolves_global_id_to_canonical_module_and_label() {
        let (temp, workspace) = workspace_with_source("script NewGame #2:\n    End\n");
        let project = context_for_workspace(&temp, workspace);

        let resolved = project.resolve_global_script_id(2001).unwrap();

        assert_eq!(resolved.module, "CommonScripts");
        assert_eq!(resolved.symbol.name, "NewGame");
    }

    #[test]
    fn rejects_ambiguous_filename_module() {
        let (temp, mut workspace) = workspace_with_source("script Battle #1:\n    End\n");
        workspace.global_script_table = GlobalScriptTable::from_entries(vec![
            GlobalScriptEntry::new(5000, 211, 213, "Double Battles"),
            GlobalScriptEntry::new(3000, 211, 213, "Single Battles"),
        ]);
        let project = context_for_workspace(&temp, workspace);

        let error = project
            .resolve_global_script_ref("0211", "Battle")
            .unwrap_err();

        assert!(error.contains("ambiguous"), "{error}");
        assert!(error.contains("SingleBattles"), "{error}");
        assert!(error.contains("DoubleBattles"), "{error}");
    }

    #[test]
    fn reverse_resolution_names_shared_labels_by_slot() {
        let (temp, workspace) = workspace_with_source("script Shared #[1, 2]:\n    End\n");
        let project = context_for_workspace(&temp, workspace);

        // `Shared` covers both slots, so neither can be spelled with it.
        for (script_id, expected) in [(2000u16, "1"), (2001, "2")] {
            let resolved = project.resolve_global_script_id(script_id).unwrap();
            assert_eq!(resolved.reference_label, expected);
            assert_eq!(resolved.symbol.name, "Shared");
            assert_eq!(
                project
                    .resolve_global_script_ref(&resolved.module, &resolved.reference_label)
                    .unwrap()
                    .script_id,
                script_id
            );
        }
    }

    #[test]
    fn definition_span_handles_decomp_and_dspre_sources() {
        let decomp = "    ScriptEntry NewGame\n    ScriptEntryEnd\n\nNewGame:\n    End\n";
        let decomp_start = decomp.find("NewGame:").unwrap();
        assert_eq!(
            definition_span(decomp, "s", "NewGame", 1),
            Some(decomp_start..decomp_start + "NewGame:".len())
        );

        let dspre = "Script 1:\nEnd\n\nScript 2:\nEnd\n";
        let dspre_start = dspre.find("Script 2:").unwrap();
        assert_eq!(
            definition_span(dspre, "script", "script_2", 2),
            Some(dspre_start..dspre_start + "Script 2:".len())
        );
    }
}
