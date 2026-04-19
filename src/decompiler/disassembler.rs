use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};

use crate::autovar::is_autovar_param;
use crate::compiler::ast::FunctionHeader;
use crate::compiler::ir::{Arg, IrAction, IrFunction, IrOpcode, TopLevelItem};
use crate::database::{Command, DatabaseV2, normalize_command_name};

use super::decomp_error::{DecompileResult, invalid_format};
use super::levelscript::LevelScript;

#[derive(Debug, Clone)]
pub enum ScriptOutput {
    Normal(Vec<TopLevelItem>),
    Levelscript(LevelScript),
}

const JUMP_TABLE_END_MARKER: [u8; 2] = [0x13, 0xFD];
const END_MOVEMENT_OPCODE: u16 = 0xFE;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScriptType {
    Normal,
    Levelscript,
}

#[derive(Debug, Clone, PartialEq)]
enum LabelKind {
    Script {
        slot_ids: Vec<u32>,
    },
    #[allow(dead_code)]
    Function {
        id: u32,
    },
    Action {
        id: u32,
    },
    Internal,
}

#[derive(Debug, Clone)]
struct LabelInfo {
    kind: LabelKind,
    name: String,
}

pub struct Disassembler<'a> {
    db: &'a DatabaseV2,
    bytes: Vec<u8>,

    script_type: ScriptType,
    jump_table_end: usize,
    has_jump_table_marker: bool,
    script_slots: BTreeMap<usize, Vec<u32>>,
    symbols: HashMap<usize, LabelInfo>,
    action_offsets: HashSet<usize>,

    #[allow(dead_code)]
    func_counter: u32,
    action_counter: u32,
}

impl<'a> Disassembler<'a> {
    pub fn new(db: &'a DatabaseV2, bytes: Vec<u8>) -> Self {
        Self {
            db,
            bytes,
            script_type: ScriptType::Normal,
            jump_table_end: 0,
            has_jump_table_marker: false,
            script_slots: BTreeMap::new(),
            symbols: HashMap::new(),
            action_offsets: HashSet::new(),
            func_counter: 0,
            action_counter: 0,
        }
    }

    /// Detect whether this binary is a normal script or a levelscript.
    ///
    /// Detection logic:
    /// 1. If exactly 4 bytes and all zeros → Empty levelscript
    /// 2. If jump table terminator (0xFD13) is found at 4-byte aligned position → Normal script
    /// 3. If NO terminator AND len >= 7 AND byte[6] != 0 → Levelscript
    /// 4. If NO terminator AND bytes 4 and 5 are zero → Levelscript (cant be a valid address or command)
    /// 5. Otherwise → Normal script (fallback for ~3 broken scripts without terminators)
    fn detect_script_type(&self) -> ScriptType {
        if self.bytes.len() == 4 && self.bytes.iter().all(|&b| b == 0) {
            return ScriptType::Levelscript;
        }

        let has_jump_table_terminator = self.bytes.chunks_exact(4).any(|chunk| {
            chunk[0] == JUMP_TABLE_END_MARKER[0] && chunk[1] == JUMP_TABLE_END_MARKER[1]
        });

        if has_jump_table_terminator {
            return ScriptType::Normal;
        }

        if self.bytes.len() >= 7
            && (self.bytes[6] != 0 || (self.bytes[5] == 0 && self.bytes[4] == 0))
        {
            return ScriptType::Levelscript;
        }

        ScriptType::Normal
    }

    pub fn disassemble(&mut self) -> DecompileResult<ScriptOutput> {
        if self.bytes.len() < 4 {
            return Err(invalid_format("File too small to contain a valid script"));
        }

        self.script_type = self.detect_script_type();

        match self.script_type {
            ScriptType::Normal => self.disassemble_normal_script().map(ScriptOutput::Normal),
            ScriptType::Levelscript => self
                .disassemble_levelscript()
                .map(ScriptOutput::Levelscript),
        }
    }

    fn disassemble_levelscript(&self) -> DecompileResult<LevelScript> {
        LevelScript::from_bytes(&self.bytes).map_err(invalid_format)
    }

    fn disassemble_normal_script(&mut self) -> DecompileResult<Vec<TopLevelItem>> {
        self.parse_jump_table()?;
        self.discover_boundaries()?;
        self.discover_gap_targets()?;
        let items = self.disassemble_chunks()?;
        Self::validate_pointer_labels(&items)?;
        Ok(items)
    }

    fn discover_gap_targets(&mut self) -> DecompileResult<()> {
        let code_start = self.code_start();

        loop {
            let symbols_before = self.symbols.len();
            let actions_before = self.action_offsets.len();

            let mut all_offsets: BTreeSet<usize> = self.symbols.keys().copied().collect();
            all_offsets.insert(code_start);
            all_offsets.insert(self.bytes.len());
            let offsets: Vec<usize> = all_offsets.into_iter().collect();

            for i in 0..offsets.len().saturating_sub(1) {
                let gap_start = offsets[i];
                let gap_end = offsets[i + 1];

                if gap_start < code_start || gap_start >= gap_end {
                    continue;
                }

                let gap_size = gap_end - gap_start;
                let is_small_zero_padding =
                    gap_size < 4 && self.bytes[gap_start..gap_end].iter().all(|&b| b == 0);
                if is_small_zero_padding {
                    continue;
                }

                let aligned_gap_start = (gap_start + 3) & !3;
                if self.has_movement_sequence_at(aligned_gap_start, gap_end) {
                    continue;
                }

                let mut cursor = gap_start;
                while cursor < gap_end {
                    let stop = self.discover_targets_from_offset(cursor);
                    if stop <= cursor || stop >= gap_end {
                        break;
                    }

                    let aligned_stop = (stop + 3) & !3;
                    if self.has_movement_sequence_at(aligned_stop, gap_end) {
                        break;
                    }

                    cursor = stop;
                }
            }

            if self.symbols.len() == symbols_before && self.action_offsets.len() == actions_before {
                break;
            }
        }

        Ok(())
    }

    fn validate_pointer_labels(items: &[TopLevelItem]) -> DecompileResult<()> {
        let mut defined_labels: HashSet<String> = HashSet::new();
        let mut unresolved_labels: HashSet<String> = HashSet::new();

        for item in items {
            match item {
                TopLevelItem::Function(function) => {
                    for header in &function.headers {
                        defined_labels.insert(header.name.clone());
                    }
                    for instruction in &function.instructions {
                        if let IrOpcode::Label(name) = instruction {
                            defined_labels.insert(name.clone());
                        }
                    }
                }
                TopLevelItem::Action(action) => {
                    defined_labels.insert(action.name.clone());
                }
            }
        }

        for item in items {
            match item {
                TopLevelItem::Function(function) => {
                    for instruction in &function.instructions {
                        if let IrOpcode::Command { args, .. } = instruction {
                            for arg in args {
                                if let Arg::Pointer(name) = arg
                                    && !defined_labels.contains(name)
                                {
                                    unresolved_labels.insert(name.clone());
                                }
                            }
                        }
                    }
                }
                TopLevelItem::Action(action) => {
                    for instruction in &action.instructions {
                        if let IrOpcode::Command { args, .. } = instruction {
                            for arg in args {
                                if let Arg::Pointer(name) = arg
                                    && !defined_labels.contains(name)
                                {
                                    unresolved_labels.insert(name.clone());
                                }
                            }
                        }
                    }
                }
            }
        }

        if unresolved_labels.is_empty() {
            return Ok(());
        }

        let mut unresolved: Vec<String> = unresolved_labels.into_iter().collect();
        unresolved.sort();
        Err(invalid_format(format!(
            "Disassembly emitted unresolved label reference(s): {}",
            unresolved.join(", ")
        )))
    }

    fn find_jump_table_boundary(&self) -> usize {
        let mut entry_count = 0;
        let len = self.bytes.len();

        for i in (0..len).step_by(4) {
            if i + 4 > len {
                break;
            }

            let rel_offset = i32::from_le_bytes([
                self.bytes[i],
                self.bytes[i + 1],
                self.bytes[i + 2],
                self.bytes[i + 3],
            ]);

            let abs_offset = (rel_offset + ((i + 4) as i32)) as usize;

            if 0 <= (abs_offset as isize) && abs_offset < len {
                entry_count += 1;
            } else {
                break;
            }
        }

        entry_count * 4
    }

    fn parse_jump_table(&mut self) -> DecompileResult<()> {
        let marker_pos = self
            .bytes
            .windows(2)
            .position(|w| w == JUMP_TABLE_END_MARKER);

        self.has_jump_table_marker = marker_pos.is_some();
        let table_end = marker_pos.unwrap_or_else(|| self.find_jump_table_boundary());
        self.jump_table_end = table_end;

        if table_end % 4 != 0 {
            return Err(invalid_format(format!(
                "Jump table size {} is not a multiple of 4",
                table_end
            )));
        }

        let entry_count = table_end / 4;

        for i in 0..entry_count {
            let entry_offset = i * 4;
            let rel_offset = i32::from_le_bytes([
                self.bytes[entry_offset],
                self.bytes[entry_offset + 1],
                self.bytes[entry_offset + 2],
                self.bytes[entry_offset + 3],
            ]);

            let abs_offset = (rel_offset + ((i as i32 + 1) * 4)) as usize;
            let slot_id = i as u32;

            self.script_slots
                .entry(abs_offset)
                .or_default()
                .push(slot_id);
        }

        for (&offset, slot_ids) in &self.script_slots {
            let name = format!("script_{}", slot_ids[0]);
            self.symbols.insert(
                offset,
                LabelInfo {
                    kind: LabelKind::Script {
                        slot_ids: slot_ids.clone(),
                    },
                    name,
                },
            );
        }

        Ok(())
    }

    fn code_start(&self) -> usize {
        if self.has_jump_table_marker {
            self.jump_table_end + 2
        } else {
            self.jump_table_end
        }
    }

    fn discover_boundaries(&mut self) -> DecompileResult<()> {
        let code_start = self.code_start();
        let mut pending_jump_targets: Vec<usize> = Vec::new();

        let mut script_starts: Vec<usize> = self.script_slots.keys().copied().collect();
        script_starts.sort_unstable();

        for &start in &script_starts {
            self.scan_function_for_targets(start, &mut pending_jump_targets);
        }

        let mut new_targets_to_scan: Vec<usize> = pending_jump_targets
            .iter()
            .filter(|&&t| t >= code_start && t < self.bytes.len() && !self.symbols.contains_key(&t))
            .copied()
            .collect();

        while !new_targets_to_scan.is_empty() {
            let mut next_round: Vec<usize> = Vec::new();

            for target in new_targets_to_scan {
                if let std::collections::hash_map::Entry::Vacant(e) = self.symbols.entry(target) {
                    e.insert(LabelInfo {
                        kind: LabelKind::Internal,
                        name: format!("_L{:04X}", target),
                    });

                    let mut local_targets: Vec<usize> = Vec::new();
                    self.scan_function_for_targets(target, &mut local_targets);

                    for t in local_targets {
                        if t >= code_start && t < self.bytes.len() && !self.symbols.contains_key(&t)
                        {
                            next_round.push(t);
                        }
                    }
                }
            }

            new_targets_to_scan = next_round;
        }

        for target in pending_jump_targets {
            if target >= code_start && target < self.bytes.len() {
                self.symbols.entry(target).or_insert_with(|| LabelInfo {
                    kind: LabelKind::Internal,
                    name: format!("_L{:04X}", target),
                });
            }
        }

        self.insert_missing_action_symbols();

        self.discover_remaining_targets()?;
        self.insert_missing_action_symbols();

        Ok(())
    }

    fn discover_remaining_targets(&mut self) -> DecompileResult<()> {
        let code_start = self.code_start();
        let mut missed_targets: Vec<usize> = Vec::new();
        let mut missed_actions: Vec<usize> = Vec::new();

        let all_starts: Vec<usize> = self.symbols.keys().copied().collect();
        for start in all_starts {
            if let Some(info) = self.symbols.get(&start)
                && matches!(info.kind, LabelKind::Action { .. }) {
                    continue;
                }

            let mut pc = start;
            while pc + 2 <= self.bytes.len() {
                let opcode = u16::from_le_bytes([self.bytes[pc], self.bytes[pc + 1]]);

                if let Some((name, cmd)) = self.db.get_script_cmd_by_id(opcode) {
                    let cmd_size = self.command_size_at(pc, cmd);

                    if let Some(target) = self.extract_jump_target(pc, cmd)
                        && target < self.bytes.len() && !self.symbols.contains_key(&target) {
                            if Self::is_action_reference(name) && target % 4 == 0 {
                                missed_actions.push(target);
                            } else if target >= code_start {
                                missed_targets.push(target);
                            }
                        }

                    let is_terminator =
                        Self::is_hard_terminator_name(name) || Self::is_soft_terminator_name(name);
                    pc += 2 + cmd_size;

                    if is_terminator {
                        break;
                    }
                } else {
                    break;
                }
            }
        }

        for target in missed_actions {
            if !self.symbols.contains_key(&target) {
                let id = self.action_counter;
                self.action_counter += 1;
                self.action_offsets.insert(target);
                self.symbols.insert(
                    target,
                    LabelInfo {
                        kind: LabelKind::Action { id },
                        name: format!("action_{}", id),
                    },
                );
            }
        }

        for target in missed_targets {
            self.symbols.entry(target).or_insert_with(|| LabelInfo {
                        kind: LabelKind::Internal,
                        name: format!("_L{:04X}", target),
                    });
        }

        Ok(())
    }

    fn scan_function_for_targets(&mut self, start: usize, targets: &mut Vec<usize>) -> usize {
        let mut pc = start;

        while pc + 2 <= self.bytes.len() {
            let opcode = u16::from_le_bytes([self.bytes[pc], self.bytes[pc + 1]]);

            if let Some((name, cmd)) = self.db.get_script_cmd_by_id(opcode) {
                let cmd_size = self.command_size_at(pc, cmd);

                if Self::is_jump_command(name)
                    && let Some(target) = self.extract_jump_target(pc, cmd)
                {
                    targets.push(target);
                }

                if Self::is_action_reference(name)
                    && let Some(action_offset) = self.extract_action_offset(pc, cmd)
                    && action_offset < self.bytes.len()
                    && action_offset % 4 == 0
                {
                    self.action_offsets.insert(action_offset);
                }

                let is_hard_terminator = Self::is_hard_terminator_name(name);

                pc += 2 + cmd_size;

                if is_hard_terminator && !self.symbols.contains_key(&pc) {
                    break;
                }
            } else {
                break;
            }
        }

        pc
    }

    fn disassemble_chunks(&mut self) -> DecompileResult<Vec<TopLevelItem>> {
        let code_start = self.code_start();

        let mut all_offsets: BTreeSet<usize> = self.symbols.keys().copied().collect();
        all_offsets.insert(code_start);
        all_offsets.insert(self.bytes.len());

        let offsets_vec: Vec<usize> = all_offsets.into_iter().collect();
        let mut items: Vec<TopLevelItem> = Vec::new();

        for i in 0..offsets_vec.len() - 1 {
            let chunk_start = offsets_vec[i];
            let chunk_end = offsets_vec[i + 1];

            if chunk_start < code_start {
                continue;
            }

            if self.action_offsets.contains(&chunk_start) {
                let (action_opt, actual_end) =
                    self.disassemble_action_with_span(chunk_start, chunk_end)?;
                if let Some(action) = action_opt {
                    items.push(TopLevelItem::Action(action));
                }

                if actual_end < chunk_end {
                    let gap_items = self.recover_gap_items(actual_end, chunk_end)?;
                    items.extend(gap_items);
                }
            } else {
                let (func_opt, actual_end) =
                    self.disassemble_function_with_span(chunk_start, chunk_end)?;
                if let Some(func) = func_opt {
                    items.push(TopLevelItem::Function(func));
                }

                if actual_end < chunk_end {
                    let gap_items = self.recover_gap_items(actual_end, chunk_end)?;
                    items.extend(gap_items);
                }
            }
        }

        Ok(items)
    }

    fn discover_targets_from_offset(&mut self, start: usize) -> usize {
        let code_start = self.code_start();
        let mut pending: Vec<usize> = Vec::new();
        let stop_pc = self.scan_function_for_targets(start, &mut pending);

        let mut new_targets: Vec<usize> = pending
            .iter()
            .filter(|&&t| t >= code_start && t < self.bytes.len() && !self.symbols.contains_key(&t))
            .copied()
            .collect();

        while !new_targets.is_empty() {
            let mut next_round: Vec<usize> = Vec::new();

            for target in new_targets {
                if let std::collections::hash_map::Entry::Vacant(e) = self.symbols.entry(target) {
                    if self.action_offsets.contains(&target) {
                        continue;
                    }
                    e.insert(LabelInfo {
                        kind: LabelKind::Internal,
                        name: format!("_L{:04X}", target),
                    });

                    let mut local_targets: Vec<usize> = Vec::new();
                    self.scan_function_for_targets(target, &mut local_targets);

                    for t in local_targets {
                        if t >= code_start && t < self.bytes.len() && !self.symbols.contains_key(&t)
                        {
                            next_round.push(t);
                        }
                    }
                }
            }

            new_targets = next_round;
        }

        self.insert_missing_action_symbols();
        stop_pc
    }

    fn insert_missing_action_symbols(&mut self) {
        for &offset in &self.action_offsets.clone() {
            if self.symbols.contains_key(&offset) {
                continue;
            }

            let id = self.action_counter;
            self.action_counter += 1;
            self.symbols.insert(
                offset,
                LabelInfo {
                    kind: LabelKind::Action { id },
                    name: format!("action_{}", id),
                },
            );
        }
    }

    fn recover_gap_items(
        &mut self,
        gap_start: usize,
        gap_end: usize,
    ) -> DecompileResult<Vec<TopLevelItem>> {
        if gap_start >= gap_end {
            return Ok(Vec::new());
        }

        let mut items = self.scan_gap_for_movements(gap_start, gap_end)?;
        let movement_covered_end = items
            .iter()
            .filter_map(|item| match item {
                TopLevelItem::Action(action) => action
                    .instructions
                    .last()
                    .and_then(|last| {
                        if let IrOpcode::Command { name, .. } = last {
                            if Self::is_end_movement_name(name) {
                                Some(())
                            } else {
                                None
                            }
                        } else {
                            None
                        }
                    })
                    .map(|()| ()),
                TopLevelItem::Function(_) => None,
            })
            .count();

        if movement_covered_end == 0 {
            let gap_size = gap_end - gap_start;
            let is_small_zero_padding =
                gap_size < 4 && self.bytes[gap_start..gap_end].iter().all(|&b| b == 0);
            if !is_small_zero_padding {
                self.discover_targets_from_offset(gap_start);
                let (func_opt, actual_end) =
                    self.disassemble_function_with_span(gap_start, gap_end)?;
                if let Some(func) = func_opt {
                    items.push(TopLevelItem::Function(func));
                }
                if actual_end > gap_start && actual_end < gap_end {
                    let remaining = self.recover_gap_items(actual_end, gap_end)?;
                    items.extend(remaining);
                }
            }
        }

        Ok(items)
    }

    fn scan_gap_for_movements(
        &mut self,
        gap_start: usize,
        gap_end: usize,
    ) -> DecompileResult<Vec<TopLevelItem>> {
        let mut items = Vec::new();
        let mut current_start = gap_start;

        while current_start < gap_end {
            if let Some(term_pos) = self.find_next_terminator(current_start, gap_end) {
                let movement_end = term_pos + 4;

                let action_start = if current_start.is_multiple_of(4) {
                    current_start
                } else {
                    (current_start + 3) & !3
                };

                if action_start <= term_pos {
                    self.action_offsets.insert(action_start);

                    if !self.symbols.contains_key(&action_start) {
                        let id = self.action_counter;
                        self.action_counter += 1;
                        self.symbols.insert(
                            action_start,
                            LabelInfo {
                                kind: LabelKind::Action { id },
                                name: format!("action_{}", id),
                            },
                        );
                    }

                    if let Some(action) = self.disassemble_action(action_start, movement_end)? {
                        items.push(TopLevelItem::Action(action));
                    }
                }

                current_start = movement_end;
            } else {
                break;
            }
        }

        Ok(items)
    }

    fn find_next_terminator(&self, start: usize, end: usize) -> Option<usize> {
        const END_MOVEMENT_PATTERN: [u8; 4] = [0xFE, 0x00, 0x00, 0x00];

        if start + 4 > self.bytes.len() || start >= end {
            return None;
        }

        let aligned_start = (start + 3) & !3;
        let search_end = end.min(self.bytes.len() - 3);
        for pos in (aligned_start..search_end).step_by(4) {
            if pos + 4 <= self.bytes.len() {
                let window = &self.bytes[pos..pos + 4];
                if window == END_MOVEMENT_PATTERN {
                    return Some(pos);
                }
            }
        }
        None
    }

    fn disassemble_function_with_span(
        &self,
        start: usize,
        end: usize,
    ) -> DecompileResult<(Option<IrFunction>, usize)> {
        let mut instructions: Vec<IrOpcode> = Vec::new();
        let mut pc = start;

        let headers = if let Some(info) = self.symbols.get(&start) {
            match &info.kind {
                LabelKind::Script { slot_ids } => {
                    let base_name = format!("script_{}", slot_ids[0]);
                    slot_ids
                        .iter()
                        .map(|&slot_id| FunctionHeader {
                            name: base_name.clone(),
                            id: Some(slot_id),
                            is_public: true,
                        })
                        .collect()
                }
                LabelKind::Function { id } => vec![FunctionHeader {
                    name: info.name.clone(),
                    id: Some(*id),
                    is_public: false,
                }],
                LabelKind::Internal => vec![FunctionHeader {
                    name: info.name.clone(),
                    id: None,
                    is_public: false,
                }],
                LabelKind::Action { .. } => return Ok((None, start)),
            }
        } else {
            vec![FunctionHeader {
                name: format!("func_{:04X}", start),
                id: None,
                is_public: false,
            }]
        };

        while pc < end {
            if let Some(info) = self.symbols.get(&pc)
                && pc != start
            {
                instructions.push(IrOpcode::Label(info.name.clone()));
            }

            if pc + 2 > end || pc + 2 > self.bytes.len() {
                break;
            }

            let opcode = u16::from_le_bytes([self.bytes[pc], self.bytes[pc + 1]]);

            if let Some((name, cmd)) = self.db.get_script_cmd_by_id(opcode) {
                let (args, bytes_consumed) = self.decode_command_args(pc + 2, cmd)?;

                let is_hard_terminator = Self::is_hard_terminator_name(name);
                let is_soft_terminator = Self::is_soft_terminator_name(name);

                instructions.push(IrOpcode::Command {
                    name: name.clone(),
                    args,
                });

                pc += 2 + bytes_consumed;

                if is_hard_terminator && !self.symbols.contains_key(&pc) {
                    break;
                }

                if is_soft_terminator && self.should_stop_at_return(pc, end) {
                    break;
                }
            } else {
                break;
            }
        }

        if instructions.is_empty() {
            return Ok((None, pc));
        }

        Ok((
            Some(IrFunction {
                headers,
                instructions,
            }),
            pc,
        ))
    }

    #[allow(dead_code)]
    fn disassemble_function(
        &self,
        start: usize,
        end: usize,
    ) -> DecompileResult<Option<IrFunction>> {
        self.disassemble_function_with_span(start, end)
            .map(|(f, _)| f)
    }

    fn disassemble_action_with_span(
        &self,
        start: usize,
        end: usize,
    ) -> DecompileResult<(Option<IrAction>, usize)> {
        let mut instructions: Vec<IrOpcode> = Vec::new();
        let mut pc = start;

        let name = self
            .symbols
            .get(&start)
            .map_or_else(|| format!("action_{:04X}", start), |info| info.name.clone());

        while pc + 4 <= end && pc + 4 <= self.bytes.len() {
            let opcode = u16::from_le_bytes([self.bytes[pc], self.bytes[pc + 1]]);
            let param = u16::from_le_bytes([self.bytes[pc + 2], self.bytes[pc + 3]]);

            if let Some((mov_name, _)) = self.db.get_movement_by_id(opcode) {
                let args = if opcode == END_MOVEMENT_OPCODE {
                    vec![]
                } else {
                    vec![Arg::Value(i32::from(param))]
                };

                instructions.push(IrOpcode::Command {
                    name: mov_name.clone(),
                    args,
                });

                pc += 4;

                if opcode == END_MOVEMENT_OPCODE {
                    break;
                }
            } else {
                instructions.push(IrOpcode::Command {
                    name: format!("Movement_0x{:02X}", opcode),
                    args: vec![Arg::Value(i32::from(param))],
                });
                pc += 4;
            }
        }

        if instructions.is_empty() {
            return Ok((None, pc));
        }

        Ok((Some(IrAction { name, instructions }), pc))
    }

    #[allow(dead_code)]
    fn disassemble_action(&self, start: usize, end: usize) -> DecompileResult<Option<IrAction>> {
        self.disassemble_action_with_span(start, end)
            .map(|(a, _)| a)
    }

    fn decode_command_args(
        &self,
        start: usize,
        cmd: &Command,
    ) -> DecompileResult<(Vec<Arg>, usize)> {
        let params = self.get_variant_params_at(start, cmd);

        let mut binary_args: Vec<Arg> = Vec::new();
        let mut offset = start;

        for param in params {
            let size = param.param_type.size();

            if offset + size > self.bytes.len() {
                break;
            }

            let value = match size {
                1 => i32::from(self.bytes[offset]),
                2 => i32::from(u16::from_le_bytes([
                    self.bytes[offset],
                    self.bytes[offset + 1],
                ])),
                4 => {
                    if param.name == "relative_jump" {
                        let rel = i32::from_le_bytes([
                            self.bytes[offset],
                            self.bytes[offset + 1],
                            self.bytes[offset + 2],
                            self.bytes[offset + 3],
                        ]);
                        let target = (offset as i32 + rel + 4) as usize;
                        let target_is_action = self.action_offsets.contains(&target);
                        let target_is_script_boundary = if target + 2 <= self.bytes.len() {
                            let target_opcode =
                                u16::from_le_bytes([self.bytes[target], self.bytes[target + 1]]);
                            self.db.get_script_cmd_by_id(target_opcode).is_some()
                        } else {
                            false
                        };

                        if let Some(info) = self.symbols.get(&target)
                            && (target_is_action || target_is_script_boundary)
                        {
                            binary_args.push(Arg::Pointer(info.name.clone()));
                        } else {
                            return Err(invalid_format(format!(
                                "Unresolved relative jump target 0x{target:04X} (rel {rel}) at offset 0x{offset:04X}"
                            )));
                        }
                        offset += size;
                        continue;
                    }
                    i32::from_le_bytes([
                        self.bytes[offset],
                        self.bytes[offset + 1],
                        self.bytes[offset + 2],
                        self.bytes[offset + 3],
                    ])
                }
                _ => 0,
            };

            binary_args.push(Arg::Value(value));
            offset += size;
        }

        let bytes_consumed = offset - start;
        let final_args = self.omit_trailing_defaults(&binary_args, params);
        Ok((final_args, bytes_consumed))
    }

    fn get_variant_params_at<'b>(
        &'b self,
        start: usize,
        cmd: &'b Command,
    ) -> &'b [crate::database::ParamDef] {
        if cmd.variants.is_some() && start + 2 <= self.bytes.len() {
            let mode = u16::from_le_bytes([self.bytes[start], self.bytes[start + 1]]) as u8;
            let variant_params = cmd.get_variant_params(mode);
            if !variant_params.is_empty() {
                return variant_params;
            }
        }
        &cmd.params
    }

    fn omit_trailing_defaults(
        &self,
        binary_args: &[Arg],
        params: &[crate::database::ParamDef],
    ) -> Vec<Arg> {
        if binary_args.is_empty() {
            return Vec::new();
        }

        let mut trailing_defaults = 0;
        for i in (0..binary_args.len()).rev() {
            if i >= params.len() {
                break;
            }
            let param = &params[i];
            let arg = &binary_args[i];

            let matches_default = if is_autovar_param(param) {
                false
            } else if let Some(default_str) = &param.default {
                if let Arg::Value(v) = arg {
                    if let Some(default_val) = Self::parse_default_value(default_str) {
                        *v == default_val
                    } else {
                        false
                    }
                } else {
                    false
                }
            } else {
                false
            };

            if matches_default {
                trailing_defaults += 1;
            } else {
                break;
            }
        }

        let keep_count = binary_args.len() - trailing_defaults;
        binary_args[..keep_count].to_vec()
    }

    fn parse_default_value(default_str: &str) -> Option<i32> {
        let s = default_str.trim();
        if s == "TRUE" || s == "true" {
            Some(1)
        } else if s == "FALSE" || s == "false" {
            Some(0)
        } else if let Some(hex) = s.strip_prefix("0x").or_else(|| s.strip_prefix("0X")) {
            i32::from_str_radix(hex, 16).ok()
        } else {
            s.parse::<i32>().ok()
        }
    }

    fn command_size(cmd: &Command) -> usize {
        cmd.params.iter().map(|p| p.param_type.size()).sum()
    }

    fn command_size_at(&self, pc: usize, cmd: &Command) -> usize {
        if cmd.variants.is_some() && pc + 4 <= self.bytes.len() {
            let mode = u16::from_le_bytes([self.bytes[pc + 2], self.bytes[pc + 3]]) as u8;
            let variant_params = cmd.get_variant_params(mode);
            if !variant_params.is_empty() {
                return variant_params.iter().map(|p| p.param_type.size()).sum();
            }
        }
        Self::command_size(cmd)
    }

    fn is_jump_command(name: &str) -> bool {
        let key = normalize_command_name(name);
        key.contains("goto") || key.contains("jump") || key.starts_with("call")
    }

    fn is_action_reference(name: &str) -> bool {
        matches!(
            normalize_command_name(name).as_str(),
            "applymovement" | "applymovementex" | "lockformovement"
        )
    }

    fn is_hard_terminator_name(name: &str) -> bool {
        name.eq_ignore_ascii_case("end")
    }

    fn is_soft_terminator_name(name: &str) -> bool {
        name.eq_ignore_ascii_case("return")
    }

    fn is_end_movement_name(name: &str) -> bool {
        name.eq_ignore_ascii_case("EndMovement")
            || name.eq_ignore_ascii_case("end_movement")
            || name.eq_ignore_ascii_case("step_end")
    }

    fn extract_jump_target(&self, pc: usize, cmd: &Command) -> Option<usize> {
        let mut offset = pc + 2;
        let params = self.get_variant_params_at(offset, cmd);

        for param in params {
            let size = param.param_type.size();

            if param.name == "relative_jump" && offset + 4 <= self.bytes.len() {
                let rel = i32::from_le_bytes([
                    self.bytes[offset],
                    self.bytes[offset + 1],
                    self.bytes[offset + 2],
                    self.bytes[offset + 3],
                ]);
                return Some((offset as i32 + rel + 4) as usize);
            }

            offset += size;
        }

        None
    }

    fn extract_action_offset(&self, pc: usize, cmd: &Command) -> Option<usize> {
        let mut offset = pc + 2;
        let params = self.get_variant_params_at(offset, cmd);

        for param in params {
            let size = param.param_type.size();

            if param.name == "relative_jump" && offset + 4 <= self.bytes.len() {
                let rel = i32::from_le_bytes([
                    self.bytes[offset],
                    self.bytes[offset + 1],
                    self.bytes[offset + 2],
                    self.bytes[offset + 3],
                ]);
                return Some((offset as i32 + rel + 4) as usize);
            }

            offset += size;
        }

        None
    }

    fn should_stop_at_return(&self, pc: usize, end: usize) -> bool {
        let next_action = self
            .action_offsets
            .iter()
            .filter(|&&off| off > pc && off <= end)
            .min();

        if let Some(&action_off) = next_action {
            let has_code_between = self.symbols.iter().any(|(&off, info)| {
                off > pc && off < action_off && !matches!(info.kind, LabelKind::Action { .. })
            });
            !has_code_between
        } else {
            let has_code_label_ahead = self.symbols.iter().any(|(&off, info)| {
                off >= pc && off < end && !matches!(info.kind, LabelKind::Action { .. })
            });
            if has_code_label_ahead {
                return false;
            }
            let aligned_pc = (pc + 3) & !3;
            self.has_movement_sequence_at(aligned_pc, end)
        }
    }

    fn has_movement_sequence_at(&self, start: usize, end: usize) -> bool {
        if start + 4 > self.bytes.len() || start >= end {
            return false;
        }

        let scan_limit = end.min(self.bytes.len());
        let mut pos = start;
        let mut count = 0;

        while pos + 4 <= scan_limit {
            let opcode = u16::from_le_bytes([self.bytes[pos], self.bytes[pos + 1]]);

            if opcode == END_MOVEMENT_OPCODE {
                return count > 0;
            }

            if self.db.get_movement_by_id(opcode).is_none() {
                return false;
            }

            count += 1;
            pos += 4;
        }

        false
    }
}

pub fn disassemble_bytes(db: &DatabaseV2, bytes: Vec<u8>) -> DecompileResult<ScriptOutput> {
    let mut disasm = Disassembler::new(db, bytes);
    disasm.disassemble()
}
