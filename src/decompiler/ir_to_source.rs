use crate::compiler::ir::{Arg, IrOpcode, TopLevelItem};

pub fn ir_to_source(items: &[TopLevelItem]) -> String {
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
                            output.push_str(&format!("function {} #{}:\n", header.name, id));
                        } else {
                            output.push_str(&format!("function {}:\n", header.name));
                        }
                    } else {
                        output.push_str(&format!("{}:\n", header.name));
                    }
                }

                for instr in &func.instructions {
                    match instr {
                        IrOpcode::Label(name) => {
                            if name.starts_with('.') || name.starts_with('_') {
                                output.push_str(&format!("{}:\n", name));
                            } else {
                                output.push_str(&format!("{}:\n", name));
                            }
                        }
                        IrOpcode::Command { name, args } => {
                            output.push_str(&format!("    {}", name));
                            if !args.is_empty() {
                                output.push(' ');
                                let args_str: Vec<String> = args
                                    .iter()
                                    .map(|a| match a {
                                        Arg::Value(v) => {
                                            if *v >= 0x4000 {
                                                format!("0x{:X}", v)
                                            } else {
                                                v.to_string()
                                            }
                                        }
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
            TopLevelItem::Action(action) => {
                output.push_str(&format!("action {}\n", action.name));

                for instr in &action.instructions {
                    match instr {
                        IrOpcode::Label(name) => {
                            output.push_str(&format!("{}:\n", name));
                        }
                        IrOpcode::Command { name, args } => {
                            output.push_str(&format!("    {}", name));
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
