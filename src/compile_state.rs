use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::io::{self, Write};
use std::path::{Path, PathBuf};

pub const COMPILE_STATE_VERSION: u32 = 4;
pub const COMPILER_VERSION: &str = env!("CARGO_PKG_VERSION");

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct CompileState {
    pub version: u32,
    pub db_hash: u64,
    pub compiler_version: String,
    pub entries: HashMap<PathBuf, FileState>,
}
/// Represents a binary quirk that may be present in a compiled binary file that is an exception to the normal pattern.
/// Used for byte matching.
///
/// A `None` value indicates that nothing out of the ordinary is present. This implies that `jump_table_end_marker_count` being `None` is technically the same as it being `Some(1)`.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct BinaryQuirk {
    /// some files have different amounts of jump table end marker bytes
    pub jump_table_end_marker_count: Option<u8>,
    /// some levelscript files have extra padding
    pub levelscript_padding: Option<u8>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct FileState {
    pub source_hash: u64,
    pub output_hash: u64,
    pub dependency_hashes: HashMap<PathBuf, u64>,
    pub status: FileStatus,
    pub last_compiled: DateTime<Utc>,
    #[serde(default)]
    pub quirks: BinaryQuirk,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum FileStatus {
    Compiled,
    Transpiled,
    Decompiled,
    Dirty,
}

impl FileState {
    pub fn compiled(
        source_hash: u64,
        output_hash: u64,
        dependency_hashes: HashMap<PathBuf, u64>,
    ) -> Self {
        Self {
            source_hash,
            output_hash,
            dependency_hashes,
            status: FileStatus::Compiled,
            last_compiled: Utc::now(),
            quirks: BinaryQuirk::default(),
        }
    }

    pub fn decompiled(source_hash: u64, output_hash: u64) -> Self {
        Self {
            source_hash,
            output_hash,
            dependency_hashes: HashMap::new(),
            status: FileStatus::Decompiled,
            last_compiled: Utc::now(),
            quirks: BinaryQuirk::default(),
        }
    }

    pub fn dirty(
        source_hash: u64,
        output_hash: u64,
        dependency_hashes: HashMap<PathBuf, u64>,
    ) -> Self {
        Self {
            source_hash,
            output_hash,
            dependency_hashes,
            status: FileStatus::Dirty,
            last_compiled: Utc::now(),
            quirks: BinaryQuirk::default(),
        }
    }

    pub fn transpiled(
        source_hash: u64,
        output_hash: u64,
        dependency_hashes: HashMap<PathBuf, u64>,
    ) -> Self {
        Self {
            source_hash,
            output_hash,
            dependency_hashes,
            status: FileStatus::Transpiled,
            last_compiled: Utc::now(),
            quirks: BinaryQuirk::default(),
        }
    }

    #[must_use]
    pub fn with_quirks(mut self, quirks: BinaryQuirk) -> Self {
        self.quirks = quirks;
        self
    }
}

impl Default for CompileState {
    fn default() -> Self {
        Self {
            version: COMPILE_STATE_VERSION,
            db_hash: 0,
            compiler_version: COMPILER_VERSION.to_string(),
            entries: HashMap::new(),
        }
    }
}

impl CompileState {
    pub fn load(path: &Path) -> io::Result<Self> {
        let content = std::fs::read_to_string(path)?;
        serde_json::from_str(&content).map_err(|source| {
            io::Error::new(
                io::ErrorKind::InvalidData,
                format!("Failed to parse compile state {}: {source}", path.display()),
            )
        })
    }

    pub fn load_or_default(path: &Path) -> io::Result<Self> {
        match Self::load(path) {
            Ok(state) => Ok(state),
            Err(source) if source.kind() == io::ErrorKind::NotFound => Ok(Self::default()),
            Err(source) if source.kind() == io::ErrorKind::InvalidData => Ok(Self::default()),
            Err(source) => Err(source),
        }
    }

    pub fn save(&self, path: &Path) -> io::Result<()> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }

        let content = serde_json::to_string_pretty(self).map_err(|source| {
            io::Error::new(
                io::ErrorKind::InvalidData,
                format!(
                    "Failed to serialize compile state {}: {source}",
                    path.display()
                ),
            )
        })?;

        let temp_name = path.file_name().map_or_else(
            || "compile-state.tmp".to_string(),
            |name| format!("{}.{}.tmp", name.to_string_lossy(), std::process::id()),
        );
        let temp_path = path.with_file_name(temp_name);
        let mut file = std::fs::File::create(&temp_path)?;
        file.write_all(content.as_bytes())?;
        file.sync_all()?;
        drop(file);

        if let Err(source) = std::fs::rename(&temp_path, path) {
            let _ = std::fs::remove_file(&temp_path);
            return Err(source);
        }

        Ok(())
    }

    pub fn needs_rebuild(
        &self,
        db_hash: u64,
        compiler_version: &str,
        constant_cache_rebuilt: bool,
    ) -> bool {
        constant_cache_rebuilt
            || self.version != COMPILE_STATE_VERSION
            || self.db_hash != db_hash
            || self.compiler_version != compiler_version
    }

    pub fn file_is_stale(
        &self,
        relative_path: &Path,
        source_hash: u64,
        output_hash: Option<u64>,
        dependency_hashes: &HashMap<PathBuf, u64>,
    ) -> bool {
        let Some(entry) = self.entries.get(relative_path) else {
            return true;
        };

        entry.status == FileStatus::Dirty
            || entry.source_hash != source_hash
            || output_hash.is_none()
            || Some(entry.output_hash) != output_hash
            || entry.dependency_hashes != *dependency_hashes
    }

    pub fn retain_only(&mut self, relative_paths: impl IntoIterator<Item = PathBuf>) {
        let keep: HashSet<PathBuf> = relative_paths.into_iter().collect();
        self.entries.retain(|path, _| keep.contains(path));
    }

    pub fn mark_metadata(&mut self, db_hash: u64, compiler_version: &str) {
        self.version = COMPILE_STATE_VERSION;
        self.db_hash = db_hash;
        self.compiler_version = compiler_version.to_string();
    }
}

#[cfg(test)]
mod tests {
    use super::{BinaryQuirk, COMPILER_VERSION, CompileState, FileState, FileStatus};
    use std::{collections::HashMap, path::PathBuf, sync::LazyLock};
    use tempfile::tempdir;

    static TEST_SCRIPT_PATH: LazyLock<PathBuf> =
        LazyLock::new(|| PathBuf::from("scripts/test.rotom"));

    fn sample_state() -> CompileState {
        let mut entries = HashMap::new();
        entries.insert(
            TEST_SCRIPT_PATH.clone(),
            FileState::compiled(
                10,
                20,
                HashMap::from([(PathBuf::from("include/test.h"), 30)]),
            ),
        );

        CompileState {
            version: super::COMPILE_STATE_VERSION,
            db_hash: 99,
            compiler_version: COMPILER_VERSION.to_string(),
            entries,
        }
    }

    #[test]
    fn compile_state_round_trips_through_disk() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("compile-state.json");
        let state = sample_state();

        state.save(&path).unwrap();

        assert_eq!(CompileState::load(&path).unwrap(), state);
    }

    #[test]
    fn needs_rebuild_detects_hash_version_and_cache_changes() {
        let mut state = sample_state();

        assert!(!state.needs_rebuild(99, COMPILER_VERSION, false));
        assert!(state.needs_rebuild(100, COMPILER_VERSION, false));
        assert!(state.needs_rebuild(99, "future-version", false));
        assert!(state.needs_rebuild(99, COMPILER_VERSION, true));
        state.version = super::COMPILE_STATE_VERSION - 1;
        assert!(state.needs_rebuild(99, COMPILER_VERSION, false));
    }

    #[test]
    fn file_is_stale_detects_missing_dirty_or_changed_inputs() {
        let mut dependency_hashes = HashMap::new();
        dependency_hashes.insert(PathBuf::from("include/test.h"), 30);
        let mut state = sample_state();

        assert!(!state.file_is_stale(&TEST_SCRIPT_PATH, 10, Some(20), &dependency_hashes));
        assert!(state.file_is_stale(
            &PathBuf::from("scripts/missing.rotom"),
            10,
            Some(20),
            &dependency_hashes
        ));
        assert!(state.file_is_stale(&TEST_SCRIPT_PATH, 11, Some(20), &dependency_hashes));
        assert!(state.file_is_stale(&TEST_SCRIPT_PATH, 10, None, &dependency_hashes));
        assert!(state.file_is_stale(&TEST_SCRIPT_PATH, 10, Some(21), &dependency_hashes));

        dependency_hashes.insert(PathBuf::from("include/test.h"), 31);
        assert!(state.file_is_stale(&TEST_SCRIPT_PATH, 10, Some(20), &dependency_hashes));

        state.entries.get_mut(&*TEST_SCRIPT_PATH).unwrap().status = FileStatus::Dirty;
        assert!(state.file_is_stale(
            &TEST_SCRIPT_PATH,
            10,
            Some(20),
            &HashMap::from([(PathBuf::from("include/test.h"), 30,)])
        ));
    }

    #[test]
    fn retain_only_drops_removed_sources() {
        let mut state = sample_state();
        state.entries.insert(
            PathBuf::from("scripts/other.rotom"),
            FileState::compiled(1, 2, HashMap::new()),
        );

        state.retain_only([TEST_SCRIPT_PATH.clone()]);

        assert_eq!(state.entries.len(), 1);
        assert!(state.entries.contains_key(&*TEST_SCRIPT_PATH));
    }

    #[test]
    fn load_or_default_recovers_from_invalid_json() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("compile-state.json");
        std::fs::write(&path, "{invalid").unwrap();

        let state = CompileState::load_or_default(&path).unwrap();

        assert_eq!(state, CompileState::default());
    }

    #[test]
    fn save_overwrites_existing_state_atomically() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("compile-state.json");
        let mut state = sample_state();
        state.save(&path).unwrap();

        state.db_hash = 1234;
        state.save(&path).unwrap();

        assert_eq!(CompileState::load(&path).unwrap().db_hash, 1234);
        assert_eq!(
            dir.path()
                .read_dir()
                .unwrap()
                .filter_map(Result::ok)
                .filter(|entry| entry.file_name().to_string_lossy().ends_with(".tmp"))
                .count(),
            0
        );
    }

    #[test]
    fn load_or_default_propagates_non_recovery_errors() {
        let dir = tempdir().unwrap();
        let error = CompileState::load_or_default(dir.path()).unwrap_err();

        assert_ne!(error.kind(), std::io::ErrorKind::NotFound);
        assert_ne!(error.kind(), std::io::ErrorKind::InvalidData);
    }

    #[test]
    fn file_state_constructors_preserve_status_dependencies_and_quirks() {
        let state = FileState::decompiled(10, 20);

        assert_eq!(state.status, FileStatus::Decompiled);
        assert!(state.dependency_hashes.is_empty());

        let dependencies = HashMap::from([(PathBuf::from("include/test.h"), 30)]);
        let transpiled = FileState::transpiled(10, 20, dependencies.clone());
        assert_eq!(transpiled.status, FileStatus::Transpiled);
        assert_eq!(transpiled.dependency_hashes, dependencies);

        let quirks = BinaryQuirk {
            jump_table_end_marker_count: Some(2),
            levelscript_padding: Some(4),
        };
        let dirty = FileState::dirty(10, 20, HashMap::new()).with_quirks(quirks);
        assert_eq!(dirty.status, FileStatus::Dirty);
        assert_eq!(dirty.quirks, quirks);
    }
}
