<img src="docs/rotom.gif" align="right" width="120" alt="Animated sprite of Rotom from Pokemon Black and White"/>

# `rotom`

**rotom** is a high-level scripting language and toolchain for Pokémon Generation 4 (Diamond/Pearl/Platinum/HGSS) romhacking/modding projects.
Inspired by [poryscript](https://github.com/huderlem/poryscript) for Gen 3.


---

## What This Project Does

Rotom provides a complete compiler toolchain for Nintendo DS scripting engine:

### Core Features
- **High-level syntax** with control flow (`if/else`, `while`, `Jump`)
- **Three-way compilation**: `.rotom` → `.bin`, `.script` (DSPRE) → `.bin`, `.s` (decomp asm) → `.bin`
- **Levelscript support** - Full compilation and decompilation of levelscripts (map init scripts)
- **Semantic preserving** - Compiles back to byte-matching binaries matching the original game scripts
- **Decomp integration** - Automatically loads constants from your pokeplatinum/pokediamond build
- **Fall-through semantics** - Preserves the original game's code organization where functions flow into each other
- **Parallel batch compilation** - Compile entire directories at once with `rayon`
- **Decompiler** - Disassemble binary scripts back to source (normal scripts to `.rotom`, levelscripts to JSON)

### Language Features
- **Public Functions** with explicit jump table slots: `function Main #1:`
- **Private labels** for internal code organization: `HelperCode:`
- **Aliases** for constants: `alias 0x800C as VAR_RESULT`
- **Actions** for movement data: `action WalkPattern ... EndMovement`
- **Rich control flow**: Nested `if/else/endif`, `while/endwhile`, `match/endmatch`, `break`, local jumps, `Call`/`Return`
- **Inline labels**: `.local_label:` for local jumps within functions
- **Autovar**: Commands that return results can be used directly in conditions (e.g., `if CheckPlayerOnBike() then`), inspired by the feature of the same name from PoryScript

---

## Project Status

### Completed
- Full compiler pipeline from source to binary
- DSPRE script format transpiler (legacy tool compatibility)
- Decomp assembly (`*.s`) transpiler for seamless integration
- Levelscript transpiler (decomp `InitScriptEntry_*` macros → binary)
- Levelscript decompiler (binary → JSON)
- Normal script decompiler (binary → `.rotom` source)
- Bytecode emission with jump table ordering (sorted by slot ID)
- Movement command semantics (default parameters, interleaving with functions)
- Fall-through code generation matching decomp style
- Multi-format batch compilation with parallel processing
- Rich error reporting with source locations
- Constant loading from database and decomp projects
- Test infrastructure with fixtures from pokeplatinum
- 100% byte-matching verification against pokeplatinum scripts (1124/1124)

### Roadmap
- Register allocation for automatic variable assignment
- Constant folding for compile-time arithmetic
- Complex expressions in conditions (`if x + 1 == 5`)
- Optimization passes
- Decompiler pattern matching for `match`/`while`/`if` reconstruction
- More comprehensive test coverage

---

## Quick Start

### Installation
```bash
cargo build --release
```

### Compile a script
```bash
# Single file
rotom compile -d database.json -i script.rotom -o script.bin

# Directory (parallel compilation)
rotom compile -d database.json -i scripts/ -o output/

# Use decomp constants
rotom compile -d database.json -i script.rotom --decomp-root C:/dev/pokeplatinum
```

### Decompile a binary
> [!WARNING] **Not yet implemented** - Coming soon!

```bash
# Normal script → .rotom source
rotom decompile -d database.json -i script.bin -o script.rotom

# Levelscript → JSON (automatically detected)
rotom decompile -d database.json -i init_script.bin -o init_script.json

# Directory (parallel decompilation)
rotom decompile -d database.json -i binaries/ -o output/
```

### Run tests
```bash
cargo test
```

### Code Quality
```bash
cargo clippy
```

The project uses `clippy.toml` for code quality configuration. Pedantic and nursery lints are enabled by default in `Cargo.toml`.

---

## Example: Rotom Syntax

```rotom
// === Constants ===
alias 0x800C as VAR_RESULT
alias 0x800D as VAR_LASTTALKED

// === Public function (in jump table) ===
function Main #1:
    // If it's an NPC
    if VAR_LASTTALKED != 0 then
        Call TalkToNPC
    else
        Message 1  // It's a sign
    endif
    End

// === Private label (helper function) ===
TalkToNPC:
    FacePlayer
    Message 2
    WaitAButton
    Return

// === Movement action ===
action NPC_WalkAway
    WalkDown 3
    WalkLeft 2
    FaceDown
EndMovement
```

### Match Statements

Use `match` to dispatch based on a variable's value:

```rotom
function HandleChoice #1:
    match VAR_RESULT where
        case 0:
            Message 1
        case 1, 2:
            Message 2
        else:
            Message 3
    endmatch
    End
```

### Autovar: Commands in Conditions

Commands that return a result (those with a `destVar`/`destVarID` parameter defaulting to `VAR_RESULT`) can be used directly in conditions. The compiler automatically:
1. Emits the command with `VAR_RESULT` as the destination
2. Compares the result appropriately

```rotom
function BikeCheck #1:
    // Bare call - equivalent to: CheckPlayerOnBike VAR_RESULT; if VAR_RESULT == 1
    if CheckPlayerOnBike() then
        Message 1
    endif

    // With explicit comparison
    if ShowYesNoMenu() == 0 then
        Message 2
    endif

    // In match statements
    match ShowYesNoMenu() where
        case 0:
            Call HandleNo
        case 1:
            Call HandleYes
    endmatch
    End
```

Commands with additional parameters work too:

```rotom
function ItemCheck #1:
    // AddItem(item, amount) - destVarID is automatically VAR_RESULT
    if AddItem(ITEM_POTION, 5) then
        Message 1  // Success
    else
        Message 2  // Bag full
    endif
    End
```

### Break Statement

Use `break` to exit a `while` loop early:

```rotom
function SearchLoop #1:
    while VAR_COUNTER < 10 do
        if VAR_RESULT == TARGET_VALUE then
            break
        endif
        AddVar VAR_COUNTER, 1
    endwhile
    End
```

---

## Project Structure

```
src/
├── compiler/          # Core compilation pipeline
│   ├── lexer.rs       # Tokenization
│   ├── parser.rs      # AST generation
│   ├── analysis.rs    # Semantic analysis
│   ├── ir/            # Intermediate representation
│   └── codegen.rs     # Binary emission
├── decompiler/        # Disassembly and decompilation
│   ├── disassembler.rs # Binary → IR conversion
│   ├── ir_to_source.rs # IR → source text
│   └── levelscript.rs  # Levelscript types and binary parsing
├── database.rs        # Command/constant database
├── transpiler/        # Format converters
│   ├── DSPRE.rs       # DSPRE .script format
│   ├── decomp.rs      # Decomp .s assembly format
│   └── levelscript_decomp.rs # Decomp levelscript macros
├── lib.rs             # Library API
└── main.rs            # CLI entry point

tests/fixtures/        # Test scripts and expected binaries
```

---

## Design Philosophy

Rotom is built with three core principles:

1. **Fidelity to source**: Compile back to byte-matching binaries that match the decompiled game scripts
2. **Developer experience**: Clean syntax, rich error messages, seamless decomp integration
3. **Parallel-first**: Built with `rayon` for fast batch compilation of entire projects

---

## Contributing

This is an active development project. The codebase is structured, well-tested, and follows modern Rust practices (2024 edition).

1. Tests are in `tests/` with fixtures
2. The compiler uses a database-driven approach (JSON command definitions)
3. Error handling uses rich diagnostics via `codespan-reporting`

---

## License

See LICENSE file for details.

---

## Related Projects

- [uxie](https://github.com/KalaayPT/uxie) - Data fetching library for Gen 4 romhacking
- [chatot](https://github.com/YakoSWG/chatot) - Text processing library for Gen 4 romhacking
- [poryscript](https://github.com/huderlem/poryscript) - High-level scripting for Gen 3 (inspiration)
- [pokeplatinum](https://github.com/pret/pokeplatinum) - Pokemon Platinum decompilation
- [pokeheartgold](https://github.com/pret/pokeheartgold) - Pokemon HeartGold decompilation

---

**"This bizarre Pokémon appears to be a will-o'-the-wisp powered by electricity. Be wary, as Rotom is both smart and mischievous."** -- Pokédex entry in Pokémon: Legends Arceus
