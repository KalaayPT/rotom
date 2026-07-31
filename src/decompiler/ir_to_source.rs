use std::fmt::Write;

use crate::compiler::ir::{Arg, IrOpcode, TopLevelItem};
use crate::database::{ComparisonOperator, ConstantDb};

use super::DecompileContext;
use super::disassembler::ScriptOutput;
use super::levelscript::LevelScript;

/// Render disassembled script IR as Rotoscript source.
///
/// When the context includes a project, global script IDs are emitted as
/// canonical `module::label` references. Otherwise they remain numeric.
pub fn ir_to_source(output: &ScriptOutput, context: DecompileContext<'_>) -> String {
    match output {
        ScriptOutput::Normal { items, .. } => normal_script_to_source(items, context),
        ScriptOutput::Levelscript(ls) => levelscript_to_source(ls),
    }
}

/// Map a parameter name from the command database to a Uxie constant family.
/// Not perfect given how variable the naming used by the decomps is.
/// Map a command parameter name to the constant family used for reverse lookup.
fn param_semantic_family(
    param_name: &str,
    game_family: Option<crate::database::GameFamily>,
) -> Option<uxie::ConstantFamily> {
    match param_name.to_ascii_lowercase().as_str() {
        "item" | "itemid" | "item_id" => Some(uxie::ConstantFamily::Item),
        "species" | "pokemon" | "pokémon" | "pokémon_id" => Some(uxie::ConstantFamily::Species),
        "move" | "moveid" | "move_id" => Some(uxie::ConstantFamily::Move),
        "sound" | "seq" | "seqid" | "seq_id" | "bgm" | "sfx" => Some(uxie::ConstantFamily::Sound),
        "trainer" | "trainerid" | "trainer_id" => Some(uxie::ConstantFamily::Trainer),
        "trainerclass" | "trainer_class" => Some(uxie::ConstantFamily::TrainerClass),
        "location" | "mapsec" => Some(uxie::ConstantFamily::Location),
        "event_id" | "eventid" | "localid" | "local_id" | "object_id" | "objectid" => {
            Some(if game_family == Some(crate::database::GameFamily::HGSS) {
                uxie::ConstantFamily::LocalObject
            } else {
                uxie::ConstantFamily::EventId
            })
        }
        "flag" | "flagid" | "flag_id" | "shiny_flag" => Some(uxie::ConstantFamily::Flag),
        "var" | "variable" | "var_0" | "var_1" | "var_2" | "var_3" | "var_4" | "var_5"
        | "var_6" | "varid" | "var_id" | "destvarid" | "dest_var_id" | "variable_1"
        | "variable_2" | "retvar" | "var_dest" | "var_result" | "var_or_addend" | "var_or_trno"
        | "countdown_variable" | "var_flag" => Some(uxie::ConstantFamily::Variable),
        "ability" => Some(uxie::ConstantFamily::Ability),
        "type" | "type_1" | "type_2" => Some(uxie::ConstantFamily::Type),
        _ => None,
    }
}

fn format_arg(
    arg: &Arg,
    param_name: Option<&str>,
    constants: Option<&ConstantDb>,
    game_family: Option<crate::database::GameFamily>,
) -> String {
    match arg {
        Arg::Value(v) => {
            if param_name == Some("condition")
                && let Some(cond) = ComparisonOperator::from_id(*v as u8)
            {
                return cond.as_str().to_string();
            }
            if let Some(name) = param_name
                && let Some(family) = param_semantic_family(name, game_family)
                && let Some(constants) = constants
                && let Some(resolved) = constants.resolve_value_to_name(i64::from(*v), family)
            {
                return resolved;
            } else if let Some(resolved) = constants.and_then(|c| {
                c.resolve_value_to_name(i64::from(*v), uxie::ConstantFamily::Variable)
            }) {
                return resolved;
            }
            if *v >= 0x4000 {
                return format!("0x{:X}", v);
            }
            v.to_string()
        }
        Arg::Pointer(s) => s.clone(),
    }
}

fn format_command_args(
    name: &str,
    args: &[Arg],
    context: DecompileContext<'_>,
) -> (Vec<String>, Option<String>) {
    let db = context.db();
    let constants = context.constants();
    let project = context.project();
    let workspace = project.and_then(|project| project.workspace());
    let params = db.get_command(name).ok().map(|cmd| &cmd.params);
    let annotate_script_ids = db.is_global_script_call(name);
    let mut formatted = Vec::with_capacity(args.len());
    let mut annotations = Vec::new();

    for (i, arg) in args.iter().enumerate() {
        let param = params.and_then(|p| p.get(i));
        let param_name = param.map(|p| p.name.as_str());
        if annotate_script_ids
            && i == 0
            && let (Some(project), Arg::Value(value)) = (project, arg)
            && let Ok(script_id) = u16::try_from(*value)
            && let Some(resolved) = project.resolve_global_script_id(script_id)
        {
            formatted.push(format!("{}::{}", resolved.module, resolved.reference_label));
            continue;
        }
        formatted.push(format_arg(arg, param_name, constants, db.game_family()));

        if annotate_script_ids
            && let Some(ws) = workspace
            && let Arg::Value(v) = arg
            && let Some(note) = annotate_global_script_id(param_name, *v, ws)
        {
            annotations.push(note);
        }
    }

    let comment = (!annotations.is_empty()).then(|| annotations.join("; "));
    (formatted, comment)
}

/// Build a human-readable annotation for a global script ID.
///
/// Returns `None` when `id` is not a global script in the workspace's
/// [`GlobalScriptTable`] (i.e. it is a local/map script ID, or the table is
/// empty). Resolution mirrors the engine's `ScriptContext_LoadAndOffsetID`:
/// the ID falls into the first range whose minimum it meets, giving a
/// zero-based offset `id - range.min_script_id`. Script files number their
/// entries 1-based, so the displayed script number is `offset + 1` (e.g.
/// scriptID 2000 -> file 211, script #1).
fn annotate_global_script_id(
    param_name: Option<&str>,
    id: i32,
    workspace: &uxie::Workspace,
) -> Option<String> {
    let id_u16 = u16::try_from(id).ok()?;
    let entry = workspace.global_script_table.lookup(id_u16)?;
    let script_number = id_u16 - entry.min_script_id + 1;
    let label = param_name.unwrap_or("script_id");
    Some(format!(
        "{}: {} -> file {} (script #{script_number})",
        label,
        entry.range.display_name(),
        entry.script_file_id,
    ))
}

/// Format a sorted list of 1-based slot IDs as a compact header specifier.
///
/// A single ID emits `#N`. Multiple IDs emit `#[...]`, grouping consecutive
/// runs as ranges: `[1-3, 5]` instead of `[1, 2, 3, 5]`.
fn format_slot_list(ids: &[u32]) -> String {
    if ids.len() == 1 {
        return format!("#{}", ids[0]);
    }
    let mut parts: Vec<String> = Vec::new();
    let mut i = 0;
    while i < ids.len() {
        let start = ids[i];
        let mut end = start;
        while i + 1 < ids.len() && ids[i + 1] == ids[i] + 1 {
            i += 1;
            end = ids[i];
        }
        if end == start {
            parts.push(format!("{start}"));
        } else {
            parts.push(format!("{start}-{end}"));
        }
        i += 1;
    }
    format!("#[{}]", parts.join(", "))
}

fn normal_script_to_source(items: &[TopLevelItem], context: DecompileContext<'_>) -> String {
    let mut output = String::new();

    for (i, item) in items.iter().enumerate() {
        if i > 0 {
            output.push('\n');
        }

        match item {
            TopLevelItem::Function(func) => {
                // Collect public headers with IDs vs. those without (private labels).
                // IDs are stored 0-based; convert to 1-based for display.
                let public_ids: Vec<u32> = func
                    .headers
                    .iter()
                    .filter(|h| h.is_public)
                    .filter_map(|h| h.id.map(|id| id + 1))
                    .collect();
                let has_private = func.headers.iter().any(|h| !h.is_public);

                if !public_ids.is_empty() {
                    let name = &func.headers.iter().find(|h| h.is_public).unwrap().name;
                    let slot_list = format_slot_list(&public_ids);
                    let _ = writeln!(output, "script {} {}:", name, slot_list);
                }
                if has_private {
                    for header in func.headers.iter().filter(|h| !h.is_public) {
                        let _ = writeln!(output, "{}:", header.name);
                    }
                }

                for instr in &func.instructions {
                    match instr {
                        IrOpcode::Label(name) => {
                            let _ = writeln!(output, "{}:", name);
                        }
                        IrOpcode::Command { name, args } => {
                            let _ = write!(output, "    {}", name);
                            if !args.is_empty() {
                                output.push(' ');
                                let (args_vec, comment) = format_command_args(name, args, context);
                                output.push_str(&args_vec.join(", "));
                                if let Some(note) = comment {
                                    let _ = write!(output, "  // {note}");
                                }
                            }
                            output.push('\n');
                        }
                    }
                }
            }
            TopLevelItem::Action(action) => {
                let _ = writeln!(output, "action {}:", action.name);

                for instr in &action.instructions {
                    match instr {
                        IrOpcode::Label(name) => {
                            let _ = writeln!(output, "{}:", name);
                        }
                        IrOpcode::Command { name, args } => {
                            let _ = write!(output, "    {}", name);
                            if !args.is_empty() {
                                output.push(' ');
                                let args_str: Vec<String> = args
                                    .iter()
                                    .map(|a| match a {
                                        Arg::Value(v) => v.to_string(),
                                        Arg::Pointer(s) => s.clone(),
                                    })
                                    .collect();
                                output.push_str(&args_str.join(", "));
                            }
                            output.push('\n');
                        }
                    }
                }
            }
        }
    }

    output
}

fn levelscript_to_source(ls: &LevelScript) -> String {
    ls.to_json().unwrap_or_else(|_| "{}".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ProjectContext;
    use crate::compiler::ast::FunctionHeader;
    use crate::compiler::ir::IrFunction;
    use crate::database::{ConstantDb, DatabaseV2};
    use crate::decompiler::LevelScriptHeaderEntry;
    use crate::project::config::{
        DatabaseConfig, PathsConfig, ProjectMetadata, ProjectTypeConfig, RotomConfig,
        WorkspaceConfig,
    };
    use std::sync::Arc;

    fn create_test_db() -> &'static DatabaseV2 {
        DatabaseV2::test_platinum()
    }

    fn ws_with_platinum_global_table() -> uxie::Workspace {
        let mut ws = uxie::Workspace::new(std::path::PathBuf::new(), uxie::game::Game::Platinum);
        ws.global_script_table = uxie::script_file::GlobalScriptTable::platinum_western_hardcoded();
        ws
    }

    fn project_for_workspace(workspace: uxie::Workspace) -> ProjectContext {
        let config = RotomConfig {
            format_version: 1,
            project: ProjectMetadata {
                name: "test".to_string(),
            },
            workspace: WorkspaceConfig {
                project_type: ProjectTypeConfig::Dspre,
                game_family: Some(uxie::GameFamily::Platinum),
            },
            paths: PathsConfig {
                database_dir: ".rotom/command_database".to_string(),
                cache_dir: ".rotom/cache".to_string(),
                status_dir: ".rotom/status".to_string(),
                source_roots: Vec::new(),
                include_roots: Vec::new(),
                binary_roots: Vec::new(),
            },
            database: Some(DatabaseConfig {
                default_file: DatabaseV2::test_platinum_path().display().to_string(),
            }),
        };
        ProjectContext::from_parts(
            workspace.project_path.clone(),
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
    fn test_global_script_id_annotated_in_decompiled_output() {
        // Platinum western table: range 2000 = Common Scripts -> file 211.
        // scriptID 2050 = offset 50, i.e. the 51st script in file 211.
        let project = project_for_workspace(ws_with_platinum_global_table());

        let func = IrFunction {
            headers: vec![FunctionHeader {
                name: "TestFunc".to_string(),
                id: Some(1),
                is_public: true,
            }],
            instructions: vec![IrOpcode::Command {
                name: "CallCommonScript".to_string(),
                args: vec![Arg::Value(2050)],
            }],
        };

        let items = vec![TopLevelItem::Function(func)];
        let output = ScriptOutput::Normal {
            items,
            jump_table_end_marker_count: 1,
        };
        let source = ir_to_source(&output, DecompileContext::for_project(&project));

        assert!(
            source.contains("// scriptID: Common Scripts -> file 211 (script #51)"),
            "expected global-script annotation, got:\n{source}"
        );
    }

    #[test]
    fn test_global_script_id_emits_canonical_module_ref_when_source_is_indexed() {
        let temp = tempfile::tempdir().unwrap();
        let scripts = temp.path().join("scripts");
        std::fs::create_dir(&scripts).unwrap();
        std::fs::write(scripts.join("0211.rotom"), "script NewGame #51:\n    End\n").unwrap();
        let mut workspace =
            uxie::Workspace::new(temp.path().to_path_buf(), uxie::game::Game::Platinum);
        workspace.scripts.load_dspre_script_dir(&scripts).unwrap();
        workspace.global_script_table =
            uxie::script_file::GlobalScriptTable::platinum_western_hardcoded();
        let project = project_for_workspace(workspace);
        let output = ScriptOutput::Normal {
            items: vec![TopLevelItem::Function(IrFunction {
                headers: vec![FunctionHeader {
                    name: "TestFunc".to_string(),
                    id: Some(1),
                    is_public: true,
                }],
                instructions: vec![IrOpcode::Command {
                    name: "CallCommonScript".to_string(),
                    args: vec![Arg::Value(2050)],
                }],
            })],
            jump_table_end_marker_count: 1,
        };

        let source = ir_to_source(&output, DecompileContext::for_project(&project));

        assert!(
            source.contains("CallCommonScript CommonScripts::NewGame"),
            "{source}"
        );
        assert!(!source.contains("scriptID:"), "{source}");
    }

    #[test]
    fn test_local_script_id_not_annotated() {
        // Local script IDs (< 2000) do not resolve in the global table and must
        // not receive an annotation.
        let project = project_for_workspace(ws_with_platinum_global_table());

        let func = IrFunction {
            headers: vec![FunctionHeader {
                name: "TestFunc".to_string(),
                id: Some(1),
                is_public: true,
            }],
            instructions: vec![IrOpcode::Command {
                name: "CallCommonScript".to_string(),
                args: vec![Arg::Value(5)],
            }],
        };

        let items = vec![TopLevelItem::Function(func)];
        let output = ScriptOutput::Normal {
            items,
            jump_table_end_marker_count: 1,
        };
        let source = ir_to_source(&output, DecompileContext::for_project(&project));

        assert!(
            !source.contains("//"),
            "local script ID should not be annotated, got:\n{source}"
        );
    }

    #[test]
    fn test_no_annotation_without_workspace() {
        // Without a workspace (standalone decompile), output is unchanged.
        let db = create_test_db();

        let func = IrFunction {
            headers: vec![FunctionHeader {
                name: "TestFunc".to_string(),
                id: Some(1),
                is_public: true,
            }],
            instructions: vec![IrOpcode::Command {
                name: "CallCommonScript".to_string(),
                args: vec![Arg::Value(2050)],
            }],
        };

        let items = vec![TopLevelItem::Function(func)];
        let output = ScriptOutput::Normal {
            items,
            jump_table_end_marker_count: 1,
        };
        let source = ir_to_source(&output, DecompileContext::standalone(db, None));

        assert!(
            !source.contains("//"),
            "no annotation expected without workspace, got:\n{source}"
        );
    }

    #[test]
    fn test_condition_formatted_as_symbolic() {
        let db = create_test_db();

        let func = IrFunction {
            headers: vec![FunctionHeader {
                name: "TestFunc".to_string(),
                id: Some(1),
                is_public: true,
            }],
            instructions: vec![
                IrOpcode::Command {
                    name: "CompareVarValue".to_string(),
                    args: vec![Arg::Value(0x8000), Arg::Value(5)],
                },
                IrOpcode::Command {
                    name: "GoToIf".to_string(),
                    args: vec![Arg::Value(1), Arg::Pointer("some_label".to_string())],
                },
            ],
        };

        let items = vec![TopLevelItem::Function(func)];
        let output = ScriptOutput::Normal {
            items,
            jump_table_end_marker_count: 1,
        };
        let source = ir_to_source(&output, DecompileContext::standalone(db, None));

        assert!(
            source.contains("GoToIf EQUAL, some_label"),
            "Expected 'GoToIf EQUAL, some_label' but got: {}",
            source
        );
    }

    #[test]
    fn test_all_conditions_formatted_symbolically() {
        let db = create_test_db();

        let func = IrFunction {
            headers: vec![FunctionHeader {
                name: "TestFunc".to_string(),
                id: Some(1),
                is_public: true,
            }],
            instructions: vec![
                IrOpcode::Command {
                    name: "GoToIf".to_string(),
                    args: vec![Arg::Value(0), Arg::Pointer("label0".to_string())],
                },
                IrOpcode::Command {
                    name: "GoToIf".to_string(),
                    args: vec![Arg::Value(1), Arg::Pointer("label1".to_string())],
                },
                IrOpcode::Command {
                    name: "GoToIf".to_string(),
                    args: vec![Arg::Value(2), Arg::Pointer("label2".to_string())],
                },
                IrOpcode::Command {
                    name: "GoToIf".to_string(),
                    args: vec![Arg::Value(3), Arg::Pointer("label3".to_string())],
                },
                IrOpcode::Command {
                    name: "GoToIf".to_string(),
                    args: vec![Arg::Value(4), Arg::Pointer("label4".to_string())],
                },
                IrOpcode::Command {
                    name: "GoToIf".to_string(),
                    args: vec![Arg::Value(5), Arg::Pointer("label5".to_string())],
                },
            ],
        };

        let items = vec![TopLevelItem::Function(func)];
        let output = ScriptOutput::Normal {
            items,
            jump_table_end_marker_count: 1,
        };
        let source = ir_to_source(&output, DecompileContext::standalone(db, None));

        assert!(source.contains("GoToIf LESS, label0"), "Missing LESS");
        assert!(source.contains("GoToIf EQUAL, label1"), "Missing EQUAL");
        assert!(source.contains("GoToIf GREATER, label2"), "Missing GREATER");
        assert!(
            source.contains("GoToIf LESS_EQUAL, label3"),
            "Missing LESS_EQUAL"
        );
        assert!(
            source.contains("GoToIf GREATER_EQUAL, label4"),
            "Missing GREATER_EQUAL"
        );
        assert!(
            source.contains("GoToIf DIFFERENT, label5"),
            "Missing DIFFERENT"
        );
    }

    #[test]
    fn test_callif_condition_formatted_symbolically() {
        let db = create_test_db();

        let func = IrFunction {
            headers: vec![FunctionHeader {
                name: "TestFunc".to_string(),
                id: Some(1),
                is_public: true,
            }],
            instructions: vec![IrOpcode::Command {
                name: "CallIf".to_string(),
                args: vec![Arg::Value(1), Arg::Pointer("some_func".to_string())],
            }],
        };

        let items = vec![TopLevelItem::Function(func)];
        let output = ScriptOutput::Normal {
            items,
            jump_table_end_marker_count: 1,
        };
        let source = ir_to_source(&output, DecompileContext::standalone(db, None));

        assert!(
            source.contains("CallIf EQUAL, some_func"),
            "Expected 'CallIf EQUAL, some_func' but got: {}",
            source
        );
    }

    #[test]
    fn test_var_and_flag_args_format_symbolically_before_hex_fallback() {
        let db = create_test_db();
        let mut symbols = uxie::SymbolTable::new();
        symbols.insert_define("VAR_TEMP_x4000".to_string(), 0x4000);
        symbols.insert_define("FLAG_TEST".to_string(), 112);

        let mut constants = ConstantDb::new();
        constants.load_decomp_symbols(".", symbols);

        let func = IrFunction {
            headers: vec![FunctionHeader {
                name: "TestFunc".to_string(),
                id: Some(1),
                is_public: true,
            }],
            instructions: vec![
                IrOpcode::Command {
                    name: "SetVarFromValue".to_string(),
                    args: vec![Arg::Value(0x4000), Arg::Value(1)],
                },
                IrOpcode::Command {
                    name: "SetFlag".to_string(),
                    args: vec![Arg::Value(112)],
                },
            ],
        };

        let output = ScriptOutput::Normal {
            items: vec![TopLevelItem::Function(func)],
            jump_table_end_marker_count: 1,
        };
        let source = ir_to_source(&output, DecompileContext::standalone(db, Some(&constants)));

        assert!(
            source.contains("SetVarFromValue VAR_TEMP_x4000, 1"),
            "expected symbolic var, got: {source}"
        );
        assert!(
            source.contains("SetFlag FLAG_TEST"),
            "expected symbolic flag, got: {source}"
        );
    }

    #[test]
    fn test_event_id_args_format_symbolically() {
        let mut constants = ConstantDb::new();
        constants.load_decomp_symbols(".", uxie::SymbolTable::new());

        assert_eq!(
            format_arg(
                &Arg::Value(0xFF),
                Some("event_id"),
                Some(&constants),
                Some(crate::database::GameFamily::Platinum),
            ),
            "LOCALID_PLAYER"
        );
        assert_eq!(
            format_arg(
                &Arg::Value(0xFF),
                Some("object_id"),
                Some(&constants),
                Some(crate::database::GameFamily::HGSS),
            ),
            "obj_player"
        );
    }

    #[test]
    fn formats_private_labels_actions_and_slot_ranges() {
        let db = create_test_db();
        let func = IrFunction {
            headers: vec![
                FunctionHeader {
                    name: "script_1".to_string(),
                    id: Some(0),
                    is_public: true,
                },
                FunctionHeader {
                    name: "script_1".to_string(),
                    id: Some(1),
                    is_public: true,
                },
                FunctionHeader {
                    name: "script_1".to_string(),
                    id: Some(3),
                    is_public: true,
                },
                FunctionHeader {
                    name: "local_entry".to_string(),
                    id: None,
                    is_public: false,
                },
            ],
            instructions: vec![
                IrOpcode::Label("after_jump".to_string()),
                IrOpcode::Command {
                    name: "SetVarFromValue".to_string(),
                    args: vec![Arg::Value(0x4000), Arg::Value(7)],
                },
            ],
        };
        let action = crate::compiler::ir::IrAction {
            name: "action_0".to_string(),
            instructions: vec![IrOpcode::Command {
                name: "WalkNormalNorth".to_string(),
                args: vec![Arg::Value(3)],
            }],
        };
        let output = ScriptOutput::Normal {
            items: vec![TopLevelItem::Function(func), TopLevelItem::Action(action)],
            jump_table_end_marker_count: 1,
        };

        let source = ir_to_source(&output, DecompileContext::standalone(db, None));

        assert!(source.contains("script script_1 #[1-2, 4]:"));
        assert!(source.contains("local_entry:"));
        assert!(source.contains("after_jump:"));
        assert!(source.contains("SetVarFromValue 0x4000, 7"));
        assert!(source.contains("action action_0:"));
        assert!(source.contains("WalkNormalNorth 3"));
    }

    #[test]
    fn levelscript_output_is_json() {
        let mut levelscript = LevelScript::new();
        levelscript
            .header_entries
            .push(LevelScriptHeaderEntry::OnResume { script_id: 42 });
        let output = ScriptOutput::Levelscript(levelscript);

        let source = ir_to_source(
            &output,
            DecompileContext::standalone(create_test_db(), None),
        );

        assert!(source.contains(r#""type": "on_resume""#));
        assert!(source.contains(r#""script_id": 42"#));
    }
}
