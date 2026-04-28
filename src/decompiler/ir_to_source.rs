use std::fmt::Write;

use crate::compiler::ir::{Arg, IrOpcode, TopLevelItem};
use crate::database::{ComparisonOperator, DatabaseV2};

use super::disassembler::ScriptOutput;
use super::levelscript::LevelScript;

pub fn ir_to_source(output: &ScriptOutput, db: &DatabaseV2) -> String {
    match output {
        ScriptOutput::Normal { items, .. } => normal_script_to_source(items, db),
        ScriptOutput::Levelscript(ls) => levelscript_to_source(ls),
    }
}

fn format_arg(arg: &Arg, param_name: Option<&str>) -> String {
    match arg {
        Arg::Value(v) => {
            if param_name == Some("condition")
                && let Some(cond) = ComparisonOperator::from_id(*v as u8)
            {
                return cond.as_str().to_string();
            }
            if *v >= 0x4000 {
                format!("0x{:X}", v)
            } else {
                v.to_string()
            }
        }
        Arg::Pointer(s) => s.clone(),
    }
}

fn format_command_args(name: &str, args: &[Arg], db: &DatabaseV2) -> Vec<String> {
    let params = db.get_command(name).ok().map(|cmd| &cmd.params);

    args.iter()
        .enumerate()
        .map(|(i, arg)| {
            let param_name = params.and_then(|p| p.get(i)).map(|p| p.name.as_str());
            format_arg(arg, param_name)
        })
        .collect()
}

fn normal_script_to_source(items: &[TopLevelItem], db: &DatabaseV2) -> String {
    let mut output = String::new();

    for (i, item) in items.iter().enumerate() {
        if i > 0 {
            output.push('\n');
        }

        match item {
            TopLevelItem::Function(func) => {
                for header in &func.headers {
                    if header.is_public {
                        if let Some(id) = header.id {
                            let _ = writeln!(output, "script {} #{}:", header.name, id);
                        } else {
                            let _ = writeln!(output, "script {}:", header.name);
                        }
                    } else {
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
                                let args_str = format_command_args(name, args, db);
                                output.push_str(&args_str.join(", "));
                            }
                            output.push('\n');
                        }
                    }
                }
            }
            TopLevelItem::Action(action) => {
                let _ = writeln!(output, "action {}", action.name);

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
        let source = ir_to_source(&output, db);

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
        let source = ir_to_source(&output, db);

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
        let source = ir_to_source(&output, db);

        assert!(
            source.contains("CallIf EQUAL, some_func"),
            "Expected 'CallIf EQUAL, some_func' but got: {}",
            source
        );
    }
}
