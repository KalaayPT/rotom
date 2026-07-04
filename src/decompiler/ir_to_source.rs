use std::fmt::Write;

use crate::compiler::ir::{Arg, IrOpcode, TopLevelItem};
use crate::database::{ComparisonOperator, ConstantDb, DatabaseV2};

use super::disassembler::ScriptOutput;
use super::levelscript::LevelScript;

pub fn ir_to_source(
    output: &ScriptOutput,
    db: &DatabaseV2,
    constants: Option<&ConstantDb>,
) -> String {
    match output {
        ScriptOutput::Normal { items, .. } => normal_script_to_source(items, db, constants),
        ScriptOutput::Levelscript(ls) => levelscript_to_source(ls),
    }
}

/// Map a parameter name from the command database to a Uxie constant family.
/// Not perfect given how variable the naming used by the decomps is.
/// Map a command parameter name to the constant family used for reverse lookup.
fn param_semantic_family(param_name: &str) -> Option<uxie::ConstantFamily> {
    match param_name.to_ascii_lowercase().as_str() {
        "item" | "itemid" | "item_id" => Some(uxie::ConstantFamily::Item),
        "species" | "pokemon" | "pokémon" | "pokémon_id" => Some(uxie::ConstantFamily::Species),
        "move" | "moveid" | "move_id" => Some(uxie::ConstantFamily::Move),
        "sound" | "seq" | "seqid" | "seq_id" | "bgm" | "sfx" => Some(uxie::ConstantFamily::Sound),
        "trainer" | "trainerid" | "trainer_id" => Some(uxie::ConstantFamily::Trainer),
        "trainerclass" | "trainer_class" => Some(uxie::ConstantFamily::TrainerClass),
        "location" | "mapsec" => Some(uxie::ConstantFamily::Location),
        "event_id" | "eventid" | "localid" | "local_id" | "object_id" | "objectid" => {
            Some(uxie::ConstantFamily::EventId)
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

fn format_arg(arg: &Arg, param_name: Option<&str>, constants: Option<&ConstantDb>) -> String {
    match arg {
        Arg::Value(v) => {
            if param_name == Some("condition")
                && let Some(cond) = ComparisonOperator::from_id(*v as u8)
            {
                return cond.as_str().to_string();
            }
            if let Some(name) = param_name
                && let Some(family) = param_semantic_family(name)
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
    db: &DatabaseV2,
    constants: Option<&ConstantDb>,
) -> Vec<String> {
    let params = db.get_command(name).ok().map(|cmd| &cmd.params);

    args.iter()
        .enumerate()
        .map(|(i, arg)| {
            let param_name = params.and_then(|p| p.get(i)).map(|p| p.name.as_str());
            format_arg(arg, param_name, constants)
        })
        .collect()
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

fn normal_script_to_source(
    items: &[TopLevelItem],
    db: &DatabaseV2,
    constants: Option<&ConstantDb>,
) -> String {
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
                                let args_str = format_command_args(name, args, db, constants);
                                output.push_str(&args_str.join(", "));
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
    use crate::compiler::ast::FunctionHeader;
    use crate::compiler::ir::IrFunction;
    use crate::database::ConstantDb;
    use crate::decompiler::LevelScriptHeaderEntry;

    fn create_test_db() -> &'static DatabaseV2 {
        DatabaseV2::test_platinum()
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
        let source = ir_to_source(&output, db, None);

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
        let source = ir_to_source(&output, db, None);

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
        let source = ir_to_source(&output, db, None);

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
        let source = ir_to_source(&output, db, Some(&constants));

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
            format_arg(&Arg::Value(0xFF), Some("event_id"), Some(&constants)),
            "LOCALID_PLAYER"
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

        let source = ir_to_source(&output, db, None);

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

        let source = ir_to_source(&output, create_test_db(), None);

        assert!(source.contains(r#""type": "on_resume""#));
        assert!(source.contains(r#""script_id": 42"#));
    }
}
