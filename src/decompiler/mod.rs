pub mod decomp_error;
pub mod disassembler;
pub mod ir_to_source;
pub mod levelscript;

use crate::{ConstantDb, DatabaseV2, ProjectContext};

/// Borrowed resources used to render disassembled scripts.
#[derive(Clone, Copy)]
pub struct DecompileContext<'a> {
    db: &'a DatabaseV2,
    constants: Option<&'a ConstantDb>,
    project: Option<&'a ProjectContext>,
}

impl<'a> DecompileContext<'a> {
    /// Use the database and project-wide constants from a loaded project.
    pub fn for_project(project: &'a ProjectContext) -> Self {
        Self {
            db: project.db(),
            constants: Some(project.project_constants()),
            project: Some(project),
        }
    }

    /// Use explicitly supplied resources without project-backed resolution.
    pub fn standalone(db: &'a DatabaseV2, constants: Option<&'a ConstantDb>) -> Self {
        Self {
            db,
            constants,
            project: None,
        }
    }

    pub(crate) fn db(self) -> &'a DatabaseV2 {
        self.db
    }

    pub(crate) fn constants(self) -> Option<&'a ConstantDb> {
        self.constants
    }

    pub(crate) fn project(self) -> Option<&'a ProjectContext> {
        self.project
    }
}

pub use decomp_error::{DecompileError, DecompileResult};
pub use disassembler::{Disassembler, ScriptOutput, ScriptType, disassemble_bytes};
pub use ir_to_source::ir_to_source;
pub use levelscript::{
    LevelScript, LevelScriptHeaderEntry, LevelScriptValidationError, LevelScriptVarConditionEntry,
};
