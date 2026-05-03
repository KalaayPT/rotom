<img src="docs/rotom.gif" align="right" width="120" alt="Animated sprite of Rotom from Pokemon Black and White"/>

# `rotom`

**rotom** is a high-level scripting language and toolchain for Pokémon Generation 4 (Diamond/Pearl/Platinum/HGSS) romhacking/modding projects.
Inspired by [poryscript](https://github.com/huderlem/poryscript) for Gen 3.


---

## What This Project Does

Rotom provides a complete compiler toolchain for Nintendo DS scripting engine:

### Core Features
- **High-level syntax** with control flow (`if/else`, `while`, `Jump`)
- **Full legacy tool support:** DSPRE `.script` and Decomp `.s` translation layer
- **JSON Levelscripts:** levelscripts now exist in declarative JSON format
- **Byte-Matching Compilation:** de- and compilation preserve all semantics and oddities from original script files.
- **Decomp integration** - Automatically loads constants from your pokeplatinum/pokediamond headers/jsons
- **Fall-through semantics** - Preserves the game engine's organization where functions flow into each other
- **Decompiler** - Disassemble binary scripts back to source (normal scripts to `.rotom`, levelscripts to JSON)

### Language Features
- **Scripts** with explicit jump table slots, callable from events/levelscripts: `script Main #1:`
- **Private labels** for internal code organization (these were called functions in DSPRE): `HelperCode:`
- **Aliases** for constants: `alias 0x800C as VAR_RESULT`
- **Actions** for movement data: `action WalkPattern ... EndMovement`
- **Rich control flow**: Nested `if/else/endif`, `while/endwhile`, `match/endmatch`, `break`
- **Autovar**: Commands that return results can be used directly in conditions (e.g., `if CheckPlayerOnBike() then`), inspired by the feature of the same name from PoryScript

---

## Project Status

### Completed
- Full compiler pipeline from source to binary
- DSPRE script format transpiler
- Decomp assembly (`*.s`) transpiler for seamless integration
- Levelscript transpiler (decomp `InitScriptEntry_*` macros → binary)
- Levelscript decompiler (binary → JSON)
- Normal script decompiler (binary → `.rotom` source)
- Bytecode emission with jump table ordering (sorted by slot ID)
- Movement command semantics (default parameters, interleaving with functions)
- Fall-through code generation matching decomp style and engine functionality
- Multi-format batch compilation with parallel processing
- Rich error reporting with source locations
- Constant loading from database, text banks (DSPRE) and decomp projects' JSON and header files
- Test infrastructure with fixtures from pokeplatinum
- 100% byte-matching verification against decomp scripts

### Roadmap
- Graph colouring for variable liveness analysis
- "Register allocation" for automatic variable assignment
- Constant folding for compile-time arithmetic
- Complex expressions in conditions (`if x + 1 == 5`)
- Optimization passes
- Decompiler pattern matching for `match`/`while`/`if` reconstruction
- Internal variable aliases e.g. `VAR_0x8008`, `VAR_RESULT`
- Fully-featured for loops, will need graph colouring for counting


---

## Quick Start

### Installation
```bash
cargo build --release
```

### Compile a single script
```bash
rotom compile -d database.json -i script.rotom -o script.bin
```

### Decompile a single binary
```bash
rotom decompile -d database.json -i script.bin -o script.rotom
```

> [!IMPORTANT]
> These commands also accept entire folders

---

## Example: Rotom Syntax

```rotom
// === Constants ===
alias 0x800C as VAR_RESULT
alias 0x800D as VAR_LASTTALKED

// === Public script (in jump table) ===
script Main #1:
    // If it's an NPC
    if VAR_LASTTALKED != 0 then
        Call TalkToNPC
    else
        Message 1  // It's a sign
    endif
    End

// === Private label (helper code) ===
TalkToNPC:
    FacePlayer
    Message 2
    WaitButton
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

> [!IMPORTANT]
> The branches are exclusive and have no fall-through semantics. 
> Bare `Call` statements get optimized into `CallIf`s and any other statement creates exclusive `Jump` branching.
> If you really need fall-through semantics, that effect can be achieved with labels. 

```rotom
script HandleChoice #1:
    match VAR_RESULT with
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
script BikeCheck #1:
    // Bare call - equivalent to: CheckPlayerOnBike VAR_RESULT; if VAR_RESULT == 1
    if CheckPlayerOnBike() then
        Message 1
    endif

    // With explicit comparison
    if ShowYesNoMenu() == 0 then
        Message 2
    endif

    // In match statements
    match ShowYesNoMenu() with
        case 0:
            Call HandleNo // note: these get optimized into CallIf instructions
        case 1:
            Call HandleYes
    endmatch
    End
```

Commands with additional parameters work too:

```rotom
script ItemCheck #1:
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
script SearchLoop #1:
    while VAR_COUNTER < 10 do
        if VAR_RESULT == TARGET_VALUE then
            break
        endif
        AddVar VAR_COUNTER, 1
    endwhile
    End
```
---

## Design Philosophy

Rotom is built with three core principles:

1. **Fidelity to source**: Compile back to byte-matching binaries that match the original game scripts for smaller patch sizes and decomp compatibility.
2. **Developer experience**: Clean syntax, rich error messages, seamless decomp integration

---

## Contributing

see [CONTRIBUTING.MD](CONTRIBUTING.md) for details.

---

## License

MIT License. See [LICENSE](LICENSE) for details.

---

## Related Projects

- [uxie](https://github.com/KalaayPT/uxie) - Data fetching library for Gen 4 romhacking. Used heavily by Rotom.
- [chatot](https://github.com/YakoSWG/chatot) - Text processing library for Gen 4 romhacking
- [poryscript](https://github.com/huderlem/poryscript) - High-level scripting for Gen 3 (inspiration)
- [pokeplatinum](https://github.com/pret/pokeplatinum) - Pokemon Platinum decompilation
- [pokeheartgold](https://github.com/pret/pokeheartgold) - Pokemon HeartGold decompilation

---

**"This bizarre Pokémon appears to be a will-o'-the-wisp powered by electricity. Be wary, as Rotom is both smart and mischievous."** -- Pokédex entry in Pokémon: Legends Arceus
