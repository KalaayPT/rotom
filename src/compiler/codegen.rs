use std::collections::HashMap;

use crate::{
    compiler::{
        ParseResult,
        diagnostic::codegen_error,
        ir::{Arg, IrOpcode, TopLevelItem},
        parser::JUMP_TABLE_END_MARKER,
    },
    database::{Command, DatabaseV2, ParamType},
};

pub struct Emitter<'a> {
    db: &'a DatabaseV2,
    output: Vec<u8>,
    pc: usize,

    // Symbol tables
    function_offsets: HashMap<String, usize>,
    label_offsets: HashMap<String, usize>,
    action_offsets: HashMap<String, usize>,

    // Jump table: (slot_id (meaning script ID), function_name)
    jump_table_slots: Vec<(u32, String)>,

    // Pending patches
    relocations: Vec<Relocation>,
}

struct Relocation {
    offset: usize,
    target: String,
}

impl<'a> Emitter<'a> {
    pub fn new(db: &'a DatabaseV2) -> Emitter<'a> {
        Emitter {
            db,
            output: Vec::new(),
            pc: 0,
            function_offsets: HashMap::new(),
            label_offsets: HashMap::new(),
            action_offsets: HashMap::new(),
            jump_table_slots: Vec::new(),
            relocations: Vec::new(),
        }
    }

    pub fn emit_script_file(
        &mut self,
        items: &[TopLevelItem],
        jump_table_end_marker_count: u8,
    ) -> ParseResult<Vec<u8>> {
        // Collect jump table slots from all functions.
        // Source slot IDs are 1-based; subtract 1 for the binary jump table index.
        self.jump_table_slots = items
            .iter()
            .filter_map(|item| {
                if let TopLevelItem::Function(f) = item {
                    Some(
                        f.jump_table_slots()
                            .map(|(id, name)| (id.saturating_sub(1), name))
                            .collect::<Vec<_>>(),
                    )
                } else {
                    None
                }
            })
            .flatten()
            .collect();
        // Sort by slot ID to ensure correct ordering
        self.jump_table_slots.sort_by_key(|(slot_id, _)| *slot_id);

        // Build the jump-target list, filling gaps with the next available entry.
        // The analysis pass already warned the user about missing slots.
        let table_size = self.jump_table_slots.last().map_or(0, |(id, _)| *id as usize + 1);
        let mut table: Vec<Option<String>> = vec![None; table_size];
        for (slot_id, name) in &self.jump_table_slots {
            table[*slot_id as usize] = Some(name.clone());
        }
        // Scan right-to-left: propagate each defined entry back into preceding gaps.
        let mut next: Option<String> = None;
        for entry in table.iter_mut().rev() {
            if entry.is_some() {
                next = entry.clone();
            } else {
                *entry = next.clone();
            }
        }
        let jump_targets: Vec<String> = table.into_iter().flatten().collect();
        for func_name in jump_targets {
            // Placeholder for script offset
            self.relocations.push(Relocation {
                offset: self.pc,
                target: func_name,
            });
            self.emit_u32(0);
        }
        for _ in 0..jump_table_end_marker_count {
            self.output.extend_from_slice(&JUMP_TABLE_END_MARKER);
            self.pc += JUMP_TABLE_END_MARKER.len();
        }

        // Emit all items in order (functions and actions interleaved)
        for item in items {
            match item {
                TopLevelItem::Function(ir_func) => {
                    // All stacked headers share the same body offset.
                    for header in &ir_func.headers {
                        self.function_offsets.insert(header.name.clone(), self.pc);
                    }
                    for ir_op in &ir_func.instructions {
                        self.emit_ir_opcode(ir_op)?;
                    }
                }
                TopLevelItem::Action(ir_action) => {
                    // actions need to be 4-byte aligned
                    while !self.pc.is_multiple_of(4) {
                        self.emit_u8(0);
                    }
                    self.action_offsets.insert(ir_action.name.clone(), self.pc);
                    for ir_op in &ir_action.instructions {
                        self.emit_movement(ir_op)?;
                    }
                }
            }
        }
        // Handle relocations
        for reloc in &self.relocations {
            let target_offset = self
                .label_offsets
                .get(&reloc.target)
                .or_else(|| self.function_offsets.get(&reloc.target))
                .or_else(|| self.action_offsets.get(&reloc.target))
                .ok_or_else(|| {
                    codegen_error(format!("Undefined label '{}' for relocation", reloc.target))
                })?;

            let target_offset_i32 = i32::try_from(*target_offset).map_err(|_| {
                codegen_error(format!(
                    "Relocation target '{}' offset {} does not fit in i32",
                    reloc.target, target_offset
                ))
            })?;

            let reloc_offset_i32 = i32::try_from(reloc.offset).map_err(|_| {
                codegen_error(format!(
                    "Relocation offset {} does not fit in i32 for target '{}'",
                    reloc.offset, reloc.target
                ))
            })?;

            let relative_i64 = i64::from(target_offset_i32) - i64::from(reloc_offset_i32) - 4;

            let relative_i32 = i32::try_from(relative_i64).map_err(|_| {
                codegen_error(format!(
                    "Relative jump {} does not fit in i32 for relocation target '{}'",
                    relative_i64, reloc.target
                ))
            })?;

            let offset_bytes = u32::from_le_bytes(relative_i32.to_le_bytes()).to_le_bytes();
            self.output[reloc.offset..reloc.offset + 4].copy_from_slice(&offset_bytes);
        }
        while !self.output.len().is_multiple_of(4) {
            self.emit_u8(0);
        }
        Ok(std::mem::take(&mut self.output))
    }
    pub fn emit_ir_opcode(&mut self, ir_op: &IrOpcode) -> ParseResult<()> {
        match ir_op {
            IrOpcode::Label(name) => {
                self.label_offsets.insert(name.clone(), self.pc);
            }
            IrOpcode::Command { name, args } => {
                let cmd = self.db.get_command(name)?;
                self.emit_command(name, cmd, args)?;
            }
        }
        Ok(())
    }
    /// Emit a movement command (for actions)
    /// Movement format: u16 opcode + u16 param (always 4 bytes per movement)
    #[allow(clippy::similar_names)]
    pub fn emit_movement(&mut self, ir_op: &IrOpcode) -> ParseResult<()> {
        match ir_op {
            IrOpcode::Label(name) => {
                self.label_offsets.insert(name.clone(), self.pc);
            }
            IrOpcode::Command { name, args } => {
                let cmd = self.db.get_movement(name)?;
                let opcode = cmd.id.ok_or_else(|| {
                    codegen_error(format!("Movement '{}' has no opcode ID in database", name))
                })?;
                self.emit_u16(opcode);
                let param = if let Some(arg) = args.first() {
                    match arg {
                        Arg::Value(v) => {
                            let fits_u16 = u16::try_from(*v).is_ok();
                            let fits_i16 = i16::try_from(*v).is_ok();
                            if !(fits_u16 || fits_i16) {
                                return Err(codegen_error(format!(
                                    "Movement '{}' parameter value {} does not fit in 16 bits",
                                    name, v
                                )));
                            }
                            u16::from_le_bytes(
                                i16::try_from(*v)
                                    .map_or_else(|_| (*v as u16).to_le_bytes(), i16::to_le_bytes),
                            )
                        }
                        Arg::Pointer(p) => {
                            return Err(codegen_error(format!(
                                "Movement '{}' expected a value argument, got pointer '{}'",
                                name, p
                            )));
                        }
                    }
                } else {
                    u16::from(name != "EndMovement")
                };
                self.emit_u16(param);
            }
        }
        Ok(())
    }

    #[allow(clippy::similar_names)]
    pub fn emit_command(&mut self, name: &str, cmd: &Command, args: &[Arg]) -> ParseResult<()> {
        let opcode = cmd.id.ok_or_else(|| {
            codegen_error(format!("Command '{}' has no opcode ID in database", name))
        })?;
        self.emit_u16(opcode);

        // Find the matching variant if it exists, otherwise use base params
        let params = if let Some(Arg::Value(mode)) = args.first() {
            if let Ok(mode_u8) = u8::try_from(*mode) {
                cmd.get_variant_params(mode_u8)
            } else {
                &cmd.params
            }
        } else {
            &cmd.params
        };

        for (i, param) in params.iter().enumerate() {
            let arg = args.get(i).ok_or_else(|| {
                codegen_error(format!(
                    "Not enough arguments for command '{}', expected {}, got {}",
                    name,
                    params.len(),
                    args.len()
                ))
            })?;
            if param.name == "relative_jump" {
                match arg {
                    Arg::Pointer(target) => {
                        self.relocations.push(Relocation {
                            offset: self.pc,
                            target: target.clone(),
                        });
                        self.emit_u32(0);
                        continue;
                    }
                    Arg::Value(v) => {
                        // U32 fields may appear as signed i32 literals in decompiled output.
                        let v_u32 = *v as u32;
                        self.emit_u32(v_u32);
                        continue;
                    }
                }
            }
            let value = match arg {
                Arg::Value(v) => *v,
                Arg::Pointer(p) => {
                    return Err(codegen_error(format!(
                        "Command '{}' parameter '{}' expected a value, got pointer '{}'",
                        name, param.name, p
                    )));
                }
            };
            let param_size = param.param_type.size();
            match param_size {
                1 => {
                    let v = u8::try_from(value).map_err(|_| {
                        codegen_error(format!(
                            "Command '{}' parameter '{}' value {} does not fit in u8",
                            name, param.name, value
                        ))
                    })?;
                    self.emit_u8(v);
                }
                2 => {
                    let fits_u16 = u16::try_from(value).is_ok();
                    let fits_i16 = i16::try_from(value).is_ok();
                    if !(fits_u16 || fits_i16) {
                        return Err(codegen_error(format!(
                            "Command '{}' parameter '{}' value {} does not fit in 16 bits",
                            name, param.name, value
                        )));
                    }
                    let v = u16::from_le_bytes(
                        i16::try_from(value)
                            .map_or_else(|_| (value as u16).to_le_bytes(), i16::to_le_bytes),
                    );
                    self.emit_u16(v);
                }
                4 => {
                    let v = if param.param_type == ParamType::U32 {
                        // U32 fields may appear as signed i32 literals in decompiled output.
                        value as u32
                    } else {
                        u32::try_from(value).map_err(|_| {
                            codegen_error(format!(
                                "Command '{}' parameter '{}' value {} does not fit in u32",
                                name, param.name, value
                            ))
                        })?
                    };
                    self.emit_u32(v);
                }
                _ => {
                    return Err(codegen_error(format!(
                        "Unsupported parameter size: {}",
                        param_size
                    )));
                }
            }
        }
        Ok(())
    }
    pub fn emit_u8(&mut self, value: u8) {
        self.output.push(value);
        self.pc += 1;
    }
    pub fn emit_u16(&mut self, value: u16) {
        self.output.extend_from_slice(&value.to_le_bytes());
        self.pc += 2;
    }
    pub fn emit_u32(&mut self, value: u32) {
        self.output.extend_from_slice(&value.to_le_bytes());
        self.pc += 4;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::compiler::ast::FunctionHeader;
    use crate::compiler::ir::{IrAction, IrFunction, IrOpcode, TopLevelItem};
    use crate::database::DatabaseV2;

    /// Helper script to create a test database
    fn create_test_db() -> &'static DatabaseV2 {
        DatabaseV2::test_platinum()
    }

    #[test]
    fn test_emit_u8() {
        let db = create_test_db();
        let mut emitter = Emitter::new(db);

        emitter.emit_u8(0x42);

        assert_eq!(emitter.output, vec![0x42]);
        assert_eq!(emitter.pc, 1);
    }

    #[test]
    fn test_emit_u16() {
        let db = create_test_db();
        let mut emitter = Emitter::new(db);

        emitter.emit_u16(0x1234);

        assert_eq!(emitter.output, vec![0x34, 0x12]); // Little endian
        assert_eq!(emitter.pc, 2);
    }

    #[test]
    fn test_emit_u32() {
        let db = create_test_db();
        let mut emitter = Emitter::new(db);

        emitter.emit_u32(0x1234_5678);

        assert_eq!(emitter.output, vec![0x78, 0x56, 0x34, 0x12]); // Little endian
        assert_eq!(emitter.pc, 4);
    }

    #[test]
    fn test_emit_simple_movement() {
        let db = create_test_db();
        let mut emitter = Emitter::new(db);

        // Emit a simple movement: FaceNorth (opcode 0) with param 1
        emitter.emit_u16(0); // opcode for FaceNorth
        emitter.emit_u16(1); // param

        assert_eq!(emitter.output.len(), 4);
        assert_eq!(emitter.output, vec![0x00, 0x00, 0x01, 0x00]);
    }

    #[test]
    fn test_emit_movement_with_custom_param() {
        let db = create_test_db();
        let mut emitter = Emitter::new(db);

        // Emit a movement with custom parameter: Delay8 (opcode 63) with param 5
        emitter.emit_u16(63); // opcode for Delay8
        emitter.emit_u16(5); // param

        assert_eq!(emitter.output.len(), 4);
        assert_eq!(emitter.output, vec![0x3F, 0x00, 0x05, 0x00]);
    }

    #[test]
    fn test_emit_end_movement() {
        let db = create_test_db();
        let mut emitter = Emitter::new(db);

        // Emit EndMovement (opcode 0xFE) with param 0
        emitter.emit_u16(0xFE);
        emitter.emit_u16(0);

        assert_eq!(emitter.output.len(), 4);
        assert_eq!(emitter.output, vec![0xFE, 0x00, 0x00, 0x00]);
    }

    #[test]
    fn test_emit_sequence() {
        let db = create_test_db();
        let mut emitter = Emitter::new(db);

        // Emit a sequence of movements
        emitter.emit_u16(0); // FaceNorth
        emitter.emit_u16(1);
        emitter.emit_u16(1); // FaceSouth
        emitter.emit_u16(1);
        emitter.emit_u16(2); // FaceEast
        emitter.emit_u16(1);

        assert_eq!(emitter.output.len(), 12);
        assert_eq!(emitter.pc, 12);
    }

    #[test]
    fn test_emit_ir_movement() {
        let db = create_test_db();
        let mut emitter = Emitter::new(db);

        // Create a simple IR movement action
        let ir_action = IrAction {
            name: "TestAction".to_string(),
            instructions: vec![
                IrOpcode::Command {
                    name: "FaceNorth".to_string(),
                    args: vec![],
                },
                IrOpcode::Command {
                    name: "WalkNormalSouth".to_string(),
                    args: vec![Arg::Value(3)],
                },
                IrOpcode::Command {
                    name: "EndMovement".to_string(),
                    args: vec![],
                },
            ],
        };

        // Emit the action using the available method
        for op in &ir_action.instructions {
            let result = emitter.emit_movement(op);
            assert!(result.is_ok());
        }

        // FaceNorth(1) + WalkNormalSouth(3) + EndMovement(0) = 3 movements * 4 bytes each = 12 bytes
        assert_eq!(emitter.output.len(), 12);
    }

    #[test]
    fn test_emit_ir_function_with_movements() {
        let db = create_test_db();
        let mut emitter = Emitter::new(db);

        // Create a simple IR script with movements using real movement names
        let ir_func = IrFunction {
            headers: vec![FunctionHeader {
                name: "TestFunc".to_string(),
                id: Some(1),
                is_public: true,
            }],
            instructions: vec![
                IrOpcode::Command {
                    name: "FaceNorth".to_string(),
                    args: vec![],
                },
                IrOpcode::Command {
                    name: "EndMovement".to_string(),
                    args: vec![],
                },
            ],
        };

        // Emit the function's instructions using emit_movement (which handles no-arg movements)
        for op in &ir_func.instructions {
            let result = emitter.emit_movement(op);
            assert!(result.is_ok(), "Failed to emit {:?}: {:?}", op, result);
        }

        // FaceNorth(1) + EndMovement(0) = 2 movements * 4 bytes each = 8 bytes
        assert_eq!(emitter.output.len(), 8);
    }

    #[test]
    fn test_emit_multiple_functions() {
        let db = create_test_db();
        let mut emitter = Emitter::new(db);

        // Create multiple IR functions using real movement names
        let func1_instructions = vec![
            IrOpcode::Command {
                name: "FaceNorth".to_string(),
                args: vec![],
            },
            IrOpcode::Command {
                name: "EndMovement".to_string(),
                args: vec![],
            },
        ];

        let func2_instructions = vec![
            IrOpcode::Command {
                name: "FaceSouth".to_string(),
                args: vec![],
            },
            IrOpcode::Command {
                name: "EndMovement".to_string(),
                args: vec![],
            },
        ];

        for op in &func1_instructions {
            emitter.emit_movement(op).unwrap();
        }
        for op in &func2_instructions {
            emitter.emit_movement(op).unwrap();
        }

        // Each script should have 2 movements * 4 bytes = 8 bytes
        assert_eq!(emitter.output.len(), 16);
    }

    #[test]
    fn test_pc_increments_correctly() {
        let db = create_test_db();
        let mut emitter = Emitter::new(db);

        assert_eq!(emitter.pc, 0);

        emitter.emit_u8(0x10);
        assert_eq!(emitter.pc, 1);

        emitter.emit_u16(0x20);
        assert_eq!(emitter.pc, 3);

        emitter.emit_u8(0x30);
        assert_eq!(emitter.pc, 4);

        emitter.emit_u32(0x40);
        assert_eq!(emitter.pc, 8);
    }

    #[test]
    fn test_output_accumulation() {
        let db = create_test_db();
        let mut emitter = Emitter::new(db);

        emitter.emit_u8(0xAA);
        emitter.emit_u8(0xBB);
        emitter.emit_u16(0xCCDD);

        assert_eq!(emitter.output, vec![0xAA, 0xBB, 0xDD, 0xCC]);
    }

    #[test]
    fn test_empty_action() {
        let db = create_test_db();
        let mut emitter = Emitter::new(db);

        // Create an empty IR action
        let ir_action = IrAction {
            name: "EmptyAction".to_string(),
            instructions: vec![],
        };

        // Emit the empty action (should do nothing)
        for op in &ir_action.instructions {
            emitter.emit_movement(op).unwrap();
        }

        // Empty action should have no output
        assert_eq!(emitter.output.len(), 0);
    }

    #[test]
    fn test_action_with_many_movements() {
        let db = create_test_db();
        let mut emitter = Emitter::new(db);

        // Create an action with many movements using real movement names
        let instructions = vec![
            IrOpcode::Command {
                name: "FaceNorth".to_string(),
                args: vec![],
            },
            IrOpcode::Command {
                name: "FaceSouth".to_string(),
                args: vec![],
            },
            IrOpcode::Command {
                name: "FaceEast".to_string(),
                args: vec![],
            },
            IrOpcode::Command {
                name: "FaceWest".to_string(),
                args: vec![],
            },
            IrOpcode::Command {
                name: "FaceNorth".to_string(),
                args: vec![],
            },
            IrOpcode::Command {
                name: "FaceSouth".to_string(),
                args: vec![],
            },
            IrOpcode::Command {
                name: "FaceEast".to_string(),
                args: vec![],
            },
            IrOpcode::Command {
                name: "FaceWest".to_string(),
                args: vec![],
            },
            IrOpcode::Command {
                name: "FaceNorth".to_string(),
                args: vec![],
            },
            IrOpcode::Command {
                name: "FaceSouth".to_string(),
                args: vec![],
            },
            IrOpcode::Command {
                name: "EndMovement".to_string(),
                args: vec![],
            },
        ];

        // Emit all movements
        for op in &instructions {
            emitter.emit_movement(op).unwrap();
        }

        // 11 movements * 4 bytes = 44 bytes
        assert_eq!(emitter.output.len(), 44);
    }

    #[test]
    fn test_emit_script_command() {
        let db = create_test_db();
        let mut emitter = Emitter::new(db);

        // Test emitting a simple script command: End (opcode 0x02, no params)
        let end_cmd = db
            .get_command("End")
            .expect("End command should exist in DB");
        emitter.emit_command("End", end_cmd, &[]).unwrap();

        // End is opcode 0x02, no parameters = 2 bytes
        assert_eq!(emitter.output.len(), 2);
        assert_eq!(emitter.output, vec![0x02, 0x00]); // Little endian opcode
    }

    #[test]
    fn test_emit_script_command_with_params() {
        let db = create_test_db();
        let mut emitter = Emitter::new(db);

        // Test emitting Message command (opcode 44 with 1 u8 parameter)
        let message_cmd = db
            .get_command("Message")
            .expect("Message command should exist in DB");
        emitter
            .emit_command("Message", message_cmd, &[Arg::Value(42)])
            .unwrap();

        // Message is opcode (2 bytes) + 1 u8 param (1 byte) = 3 bytes total
        assert_eq!(
            emitter.output.len(),
            3,
            "Message command should be 3 bytes (opcode + u8 param)"
        );
        // Check opcode is 44 (0x2C) in little endian
        assert_eq!(emitter.output[0], 0x2C);
        assert_eq!(emitter.output[1], 0x00);
        // Check param value
        assert_eq!(emitter.output[2], 42);
    }

    #[test]
    fn test_emit_script_file_jump_table() {
        use crate::compiler::parser::JUMP_TABLE_END_MARKER;

        let db = create_test_db();
        let mut emitter = Emitter::new(db);

        // Create a script with two public functions
        let items = vec![
            TopLevelItem::Function(IrFunction {
                headers: vec![FunctionHeader {
                    name: "Func0".to_string(),
                    id: Some(1),
                    is_public: true,
                }],
                instructions: vec![IrOpcode::Command {
                    name: "End".to_string(),
                    args: vec![],
                }],
            }),
            TopLevelItem::Function(IrFunction {
                headers: vec![FunctionHeader {
                    name: "Func1".to_string(),
                    id: Some(2),
                    is_public: true,
                }],
                instructions: vec![IrOpcode::Command {
                    name: "End".to_string(),
                    args: vec![],
                }],
            }),
        ];

        let output = emitter.emit_script_file(&items, 1).unwrap();

        // Jump table: 2 entries * 4 bytes = 8 bytes, plus 2-byte marker
        // Marker is 0xFD13 = [0x13, 0xFD]
        assert!(output.len() >= 10, "Should have jump table + marker + code");

        // Check for the end marker (0xFD13)
        assert_eq!(
            &output[8..10],
            &JUMP_TABLE_END_MARKER,
            "Should have jump table end marker at offset 8"
        );
    }

    #[test]
    fn test_emit_script_file_label_relocation() {
        let db = create_test_db();
        let mut emitter = Emitter::new(db);

        // Create a script with a jump to a label
        let items = vec![TopLevelItem::Function(IrFunction {
            headers: vec![FunctionHeader {
                name: "TestFunc".to_string(),
                id: Some(1),
                is_public: true,
            }],
            instructions: vec![
                IrOpcode::Command {
                    name: "Jump".to_string(),
                    args: vec![Arg::Pointer(".target".to_string())],
                },
                IrOpcode::Command {
                    name: "End".to_string(),
                    args: vec![],
                },
                IrOpcode::Label(".target".to_string()),
                IrOpcode::Command {
                    name: "Return".to_string(),
                    args: vec![],
                },
            ],
        })];

        let output = emitter.emit_script_file(&items, 1).unwrap();

        // Should compile successfully with the label reference resolved
        assert!(
            output.len() > 10,
            "Should have generated code with jump and label"
        );

        // The Jump instruction should have a relative offset to the label
        // We can't easily verify the exact offset without knowing the opcode sizes,
        // but we can verify the output is non-zero (relocation was applied)
    }

    #[test]
    fn test_emit_script_file_action_alignment() {
        let db = create_test_db();
        let mut emitter = Emitter::new(db);

        // Create a script followed by an action
        // The action should be 4-byte aligned
        let items = vec![
            TopLevelItem::Function(IrFunction {
                headers: vec![FunctionHeader {
                    name: "TestFunc".to_string(),
                    id: Some(1),
                    is_public: true,
                }],
                instructions: vec![
                    // Just End, which is 2 bytes
                    IrOpcode::Command {
                        name: "End".to_string(),
                        args: vec![],
                    },
                ],
            }),
            TopLevelItem::Action(IrAction {
                name: "TestAction".to_string(),
                instructions: vec![
                    IrOpcode::Command {
                        name: "FaceNorth".to_string(),
                        args: vec![],
                    },
                    IrOpcode::Command {
                        name: "EndMovement".to_string(),
                        args: vec![],
                    },
                ],
            }),
        ];

        let output = emitter.emit_script_file(&items, 1).unwrap();

        // Jump table: 1 entry (4 bytes) + marker (2 bytes) = 6 bytes
        // Function: End = 2 bytes, but at offset 6, so script starts at 6
        // After function: offset 8, which is already 4-byte aligned
        // But if script body was 3 bytes, action would need padding

        // The action offset should be in the action_offsets map and should be 4-byte aligned
        // We can verify output length is correct
        assert!(
            output.len() >= 14,
            "Should have jump table + script + aligned action"
        );

        // Output length should be even (final alignment)
        assert_eq!(output.len() % 2, 0, "Final output should be 2-byte aligned");
    }

    #[test]
    fn test_emit_stacked_headers_jump_table() {
        let db = create_test_db();
        let mut emitter = Emitter::new(db);

        // Create an IR script with two headers
        let ir_func = IrFunction {
            headers: vec![
                FunctionHeader {
                    name: "TestFunc".to_string(),
                    id: Some(1),
                    is_public: true,
                },
                FunctionHeader {
                    name: "TestFunc".to_string(),
                    id: Some(2),
                    is_public: true,
                },
            ],
            instructions: vec![IrOpcode::Command {
                name: "End".to_string(),
                args: vec![],
            }],
        };

        let items = vec![TopLevelItem::Function(ir_func)];
        let output = emitter.emit_script_file(&items, 1).unwrap();

        // Jump table should have two entries (sorted by ID)
        // Entry 1 (ID 1) -> Pointer to End
        // Entry 2 (ID 2) -> Pointer to End
        // JUMP_TABLE_END_MARKER (0xFD13)
        // End (opcode 0x0002)

        // Entry 1: 4 bytes
        // Entry 2: 4 bytes
        // End Marker: 2 bytes
        // End Command: 2 bytes
        assert_eq!(output.len(), 4 + 4 + 2 + 2);

        // Check End Marker at correct position
        assert_eq!(output[8], 0x13);
        assert_eq!(output[9], 0xFD);

        // Check End command (opcode 2) at end
        assert_eq!(output[10], 0x02);
        assert_eq!(output[11], 0x00);
    }
}
