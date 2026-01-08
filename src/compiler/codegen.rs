use std::collections::HashMap;

use crate::{
    compiler::{
        ParseResult,
        ir::{Arg, IrOpcode, TopLevelItem},
        parse_error::codegen_error,
        parser::JUMP_TABLE_END_MARKER,
    },
    database::{Command, DatabaseV2},
};

pub struct Emitter<'a> {
    db: &'a DatabaseV2,
    output: Vec<u8>,
    pc: usize,

    // Symbol tables
    function_offsets: HashMap<String, usize>,
    label_offsets: HashMap<String, usize>,
    action_offsets: HashMap<String, usize>,

    // Jump table: (slot_id (meaning function ID), function_name)
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
    pub fn emit_script_file(&mut self, items: &Vec<TopLevelItem>) -> ParseResult<Vec<u8>> {
        // Collect jump table slots from all functions
        // Sort by slot ID to match game expectations (jump table entries are indexed by slot ID)
        self.jump_table_slots = items
            .iter()
            .filter_map(|item| {
                if let TopLevelItem::Function(f) = item {
                    Some(f.jump_table_slots().collect::<Vec<_>>())
                } else {
                    None
                }
            })
            .flatten()
            .collect();
        // Sort by slot ID to ensure correct ordering
        self.jump_table_slots.sort_by_key(|(slot_id, _)| *slot_id);
        for (_, func_name) in self.jump_table_slots.clone() {
            // Placeholder for function offset
            self.relocations.push(Relocation {
                offset: self.pc,
                target: func_name.clone(),
            });
            self.emit_u32(0);
        }
        self.output.extend_from_slice(&JUMP_TABLE_END_MARKER);
        self.pc += JUMP_TABLE_END_MARKER.len();

        // Emit all items in order (functions and actions interleaved)
        for item in items {
            match item {
                TopLevelItem::Function(ir_func) => {
                    self.function_offsets
                        .insert(ir_func.name().to_string(), self.pc);
                    for ir_op in &ir_func.instructions {
                        self.emit_ir_opcode(ir_op)?;
                    }
                }
                TopLevelItem::Action(ir_action) => {
                    // actions need to be 4-byte aligned
                    while self.pc % 4 != 0 {
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
            let relative = (*target_offset as i32) - (reloc.offset as i32) - 4;
            let offset_bytes = (relative as u32).to_le_bytes();
            self.output[reloc.offset..reloc.offset + 4].copy_from_slice(&offset_bytes);
        }
        while self.output.len() % 2 != 0 {
            self.emit_u8(0);
        }
        Ok(self.output.clone())
    }
    pub fn emit_ir_opcode(&mut self, ir_op: &IrOpcode) -> ParseResult<()> {
        match ir_op {
            IrOpcode::Label(name) => {
                self.label_offsets.insert(name.clone(), self.pc);
            }
            IrOpcode::Command { name, args } => {
                let cmd = self.db.get_command(name)?;
                self.emit_command(&name, cmd, args)?;
            }
        }
        Ok(())
    }
    /// Emit a movement command (for actions)
    /// Movement format: u16 opcode + u16 param (always 4 bytes per movement)
    pub fn emit_movement(&mut self, ir_op: &IrOpcode) -> ParseResult<()> {
        match ir_op {
            IrOpcode::Label(name) => {
                self.label_offsets.insert(name.clone(), self.pc);
            }
            IrOpcode::Command { name, args } => {
                let cmd = self.db.get_movement(name)?;
                let opcode = cmd.id.unwrap();
                self.emit_u16(opcode);
                // If script provides an arg, use it. Otherwise use default.
                // EndMovement defaults to 0, all other movements default to 1.
                let param = if let Some(arg) = args.first() {
                    arg.unwrap_value() as u16
                } else if name == "EndMovement" {
                    0
                } else {
                    1
                };
                self.emit_u16(param);
            }
        }
        Ok(())
    }
    pub fn emit_command(&mut self, name: &str, cmd: &Command, args: &Vec<Arg>) -> ParseResult<()> {
        self.emit_u16(cmd.id.unwrap());
        for (i, param) in cmd.params.clone().into_iter().enumerate() {
            let arg = args.get(i).ok_or_else(|| {
                codegen_error(format!(
                    "Not enough arguments for command '{}', expected {}, got {}",
                    name,
                    cmd.params.len(),
                    args.len()
                ))
            })?;
            if param.name == "relative_jump" && matches!(arg, Arg::Pointer(_)) {
                // Special handling for relative jump offsets
                self.relocations.push(Relocation {
                    offset: self.pc,
                    target: arg.unwrap_pointer(),
                });
                self.emit_u32(0); // Placeholder
                continue;
            }
            let param_size = param.param_type.size();
            match param_size {
                1 => self.emit_u8(arg.unwrap_value() as u8),
                2 => self.emit_u16(arg.unwrap_value() as u16),
                4 => self.emit_u32(arg.unwrap_value() as u32),
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
    use crate::compiler::ir::{IrOpcode, IrAction, IrFunction, TopLevelItem};
    use crate::compiler::ast::{FunctionHeader, ScriptFile};
    use crate::database::{DatabaseV2, ConstantDb};

    /// Helper function to create a test database
    fn create_test_db() -> DatabaseV2 {
        DatabaseV2::load(std::path::Path::new("src/db/platinum_v2.json")).unwrap_or_else(|_| {
            // Create a minimal test database if the real one isn't available
            DatabaseV2 {
                meta: crate::database::DatabaseMeta {
                    version: "Test".to_string(),
                    generated_at: None,
                    generated_from: None,
                },
                commands: std::collections::HashMap::new(),
                sounds: std::collections::HashMap::new(),
                comparison_operators: std::collections::HashMap::new(),
                overworld_directions: std::collections::HashMap::new(),
                special_overworlds: std::collections::HashMap::new(),
            }
        })
    }

    #[test]
    fn test_emit_u8() {
        let db = create_test_db();
        let mut emitter = Emitter::new(&db);
        
        emitter.emit_u8(0x42);
        
        assert_eq!(emitter.output, vec![0x42]);
        assert_eq!(emitter.pc, 1);
    }

    #[test]
    fn test_emit_u16() {
        let db = create_test_db();
        let mut emitter = Emitter::new(&db);
        
        emitter.emit_u16(0x1234);
        
        assert_eq!(emitter.output, vec![0x34, 0x12]); // Little endian
        assert_eq!(emitter.pc, 2);
    }

    #[test]
    fn test_emit_u32() {
        let db = create_test_db();
        let mut emitter = Emitter::new(&db);
        
        emitter.emit_u32(0x12345678);
        
        assert_eq!(emitter.output, vec![0x78, 0x56, 0x34, 0x12]); // Little endian
        assert_eq!(emitter.pc, 4);
    }

    #[test]
    fn test_emit_simple_movement() {
        let db = create_test_db();
        let mut emitter = Emitter::new(&db);
        
        // Emit a simple movement: FaceNorth (opcode 0) with param 1
        emitter.emit_u16(0);  // opcode for FaceNorth
        emitter.emit_u16(1);  // param
        
        assert_eq!(emitter.output.len(), 4);
        assert_eq!(emitter.output, vec![0x00, 0x00, 0x01, 0x00]);
    }

    #[test]
    fn test_emit_movement_with_custom_param() {
        let db = create_test_db();
        let mut emitter = Emitter::new(&db);
        
        // Emit a movement with custom parameter: Delay8 (opcode 63) with param 5
        emitter.emit_u16(63);  // opcode for Delay8
        emitter.emit_u16(5);   // param
        
        assert_eq!(emitter.output.len(), 4);
        assert_eq!(emitter.output, vec![0x3F, 0x00, 0x05, 0x00]);
    }

    #[test]
    fn test_emit_end_movement() {
        let db = create_test_db();
        let mut emitter = Emitter::new(&db);
        
        // Emit EndMovement (opcode 0xFE) with param 0
        emitter.emit_u16(0xFE);
        emitter.emit_u16(0);
        
        assert_eq!(emitter.output.len(), 4);
        assert_eq!(emitter.output, vec![0xFE, 0x00, 0x00, 0x00]);
    }

    #[test]
    fn test_emit_sequence() {
        let db = create_test_db();
        let mut emitter = Emitter::new(&db);
        
        // Emit a sequence of movements
        emitter.emit_u16(0);  // FaceNorth
        emitter.emit_u16(1);
        emitter.emit_u16(1);  // FaceSouth
        emitter.emit_u16(1);
        emitter.emit_u16(2);  // FaceEast
        emitter.emit_u16(1);
        
        assert_eq!(emitter.output.len(), 12);
        assert_eq!(emitter.pc, 12);
    }

    #[test]
    fn test_emit_ir_movement() {
        let db = create_test_db();
        let mut emitter = Emitter::new(&db);
        
        // Create a simple IR movement action
        let ir_action = IrAction {
            name: "TestAction".to_string(),
            instructions: vec![
                IrOpcode::Command { name: "FaceNorth".to_string(), args: vec![] },
                IrOpcode::Command { name: "WalkNormalSouth".to_string(), args: vec![Arg::Value(3)] },
                IrOpcode::Command { name: "EndMovement".to_string(), args: vec![] },
            ],
        };
        
        // Emit the action using the available method
        for op in &ir_action.instructions {
            let result = emitter.emit_movement(op);
            assert!(result.is_ok());
        }
        
        assert!(emitter.output.len() > 0);
        // FaceNorth(1) + WalkNormalSouth(3) + EndMovement(0) = 3 movements * 4 bytes each = 12 bytes
        assert_eq!(emitter.output.len(), 12);
    }

    #[test]
    fn test_emit_ir_function_with_movements() {
        let db = create_test_db();
        let mut emitter = Emitter::new(&db);
        
        // Create a simple IR function with movements using real movement names
        let ir_func = IrFunction {
            headers: vec![FunctionHeader {
                name: "TestFunc".to_string(),
                id: Some(1),
                is_public: true,
            }],
            instructions: vec![
                IrOpcode::Command { name: "FaceNorth".to_string(), args: vec![] },
                IrOpcode::Command { name: "EndMovement".to_string(), args: vec![] },
            ],
        };
        
        // Emit the function's instructions using emit_movement (which handles no-arg movements)
        for op in &ir_func.instructions {
            let result = emitter.emit_movement(op);
            assert!(result.is_ok(), "Failed to emit {:?}: {:?}", op, result);
        }
        
        assert!(emitter.output.len() > 0);
        // FaceNorth(1) + EndMovement(0) = 2 movements * 4 bytes each = 8 bytes
        assert_eq!(emitter.output.len(), 8);
    }

    #[test]
    fn test_emit_multiple_functions() {
        let db = create_test_db();
        let mut emitter = Emitter::new(&db);
        
        // Create multiple IR functions using real movement names
        let func1_instructions = vec![
            IrOpcode::Command { name: "FaceNorth".to_string(), args: vec![] },
            IrOpcode::Command { name: "EndMovement".to_string(), args: vec![] },
        ];
        
        let func2_instructions = vec![
            IrOpcode::Command { name: "FaceSouth".to_string(), args: vec![] },
            IrOpcode::Command { name: "EndMovement".to_string(), args: vec![] },
        ];
        
        for op in &func1_instructions {
            emitter.emit_movement(op).unwrap();
        }
        for op in &func2_instructions {
            emitter.emit_movement(op).unwrap();
        }
        
        // Each function should have 2 movements * 4 bytes = 8 bytes
        assert_eq!(emitter.output.len(), 16);
    }

    #[test]
    fn test_pc_increments_correctly() {
        let db = create_test_db();
        let mut emitter = Emitter::new(&db);
        
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
        let mut emitter = Emitter::new(&db);
        
        emitter.emit_u8(0xAA);
        emitter.emit_u8(0xBB);
        emitter.emit_u16(0xCCDD);
        
        assert_eq!(emitter.output, vec![0xAA, 0xBB, 0xDD, 0xCC]);
    }

    #[test]
    fn test_empty_action() {
        let db = create_test_db();
        let mut emitter = Emitter::new(&db);
        
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
        let mut emitter = Emitter::new(&db);
        
        // Create an action with many movements using real movement names
        let instructions = vec![
            IrOpcode::Command { name: "FaceNorth".to_string(), args: vec![] },
            IrOpcode::Command { name: "FaceSouth".to_string(), args: vec![] },
            IrOpcode::Command { name: "FaceEast".to_string(), args: vec![] },
            IrOpcode::Command { name: "FaceWest".to_string(), args: vec![] },
            IrOpcode::Command { name: "FaceNorth".to_string(), args: vec![] },
            IrOpcode::Command { name: "FaceSouth".to_string(), args: vec![] },
            IrOpcode::Command { name: "FaceEast".to_string(), args: vec![] },
            IrOpcode::Command { name: "FaceWest".to_string(), args: vec![] },
            IrOpcode::Command { name: "FaceNorth".to_string(), args: vec![] },
            IrOpcode::Command { name: "FaceSouth".to_string(), args: vec![] },
            IrOpcode::Command { name: "EndMovement".to_string(), args: vec![] },
        ];
        
        // Emit all movements
        for op in &instructions {
            emitter.emit_movement(op).unwrap();
        }
        
        // 11 movements * 4 bytes = 44 bytes
        assert_eq!(emitter.output.len(), 44);
    }
}
