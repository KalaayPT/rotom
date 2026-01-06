use std::collections::HashMap;

use crate::{
    compiler::{
        IrFunction, ParseResult,
        ir::{Arg, IrAction, IrOpcode},
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
    pub fn emit_script_file(
        &mut self,
        ir_functions: &Vec<IrFunction>,
        ir_actions: &Vec<IrAction>,
    ) -> ParseResult<Vec<u8>> {
        self.jump_table_slots = ir_functions
            .iter()
            .flat_map(|f| f.jump_table_slots())
            .collect();
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
        for ir_func in ir_functions {
            self.function_offsets
                .insert(ir_func.name().to_string(), self.pc);
            for ir_op in &ir_func.instructions {
                self.emit_ir_opcode(ir_op)?;
            }
        }
        for ir_action in ir_actions {
            // actions need to be 4-byte aligned
            while self.pc % 4 != 0 {
                self.emit_u8(0);
            }
            self.action_offsets.insert(ir_action.name.clone(), self.pc);
            for ir_op in &ir_action.instructions {
                self.emit_movement(ir_op)?;
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
                // Movement always has exactly one u16 param (default 0)
                let param = args.first().map(|a| a.unwrap_value() as u16).unwrap_or(0);
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
