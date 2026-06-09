<img src="docs/rotom.gif" align="right" width="120" alt="Animated sprite of Rotom from Pokemon Black and White"/>

# `rotom`

**rotom** is a high-level scripting language and toolchain for Pokémon Generation 4 (Diamond/Pearl/Platinum/HGSS) romhacking/modding projects.
Inspired by [poryscript](https://github.com/huderlem/poryscript) for Gen 3.


---

## What This Project Does

Rotom provides a complete compiler toolchain for the Gen 4 Pokémon scripting engine:

### Core Features
- **High-level syntax** with control flow (`if/else`, `while`, `Jump`)
- **Full legacy tool support:** DSPRE `.script` and Decomp `.s` translation/conversion layer
- **JSON Levelscripts:** levelscripts now exist in declarative JSON format
- **Byte-Matching Compilation:** de- and compilation preserve all semantics and oddities from original script files.
- **Decomp integration** - Automatically loads constants from your pokeplatinum/pokediamond headers/jsons
- **Fall-through semantics** - Preserves the game engine's organization where scripts flow into each other
- **Decompiler** - Disassemble binary scripts back to source (normal scripts to `.rotom`, levelscripts to JSON)
- **Editor support** - LSP support and editor integrations for diagnostics, completion, hover, go-to-definition, inlay hints, syntax highlighting, etc. (more on this later)

### Language Features
- **Scripts** with explicit jump table slots, callable from events/levelscripts: `script Main #1:`, or `#[1-3, 5, 6]` for multiple slots at once
- **Private labels** for internal code organization (these were called functions in DSPRE): `HelperCode:`
- **Aliases** for constants: `alias 0x800C as VAR_RESULT`
- **Actions** for movement data: `action WalkPattern: ... EndMovement`
- **Rich control flow**: Nested `if/else/endif`, `while/endwhile`, `match/endmatch`, `break`
- **Autovar**: Commands that return results can be used directly in conditions (e.g., `if CheckPlayerOnBike() then`), inspired by the feature of the same name from PoryScript
- **String literals**: Write message text directly in your script without needing to touch text archives. Use `format()` to let the compiler handle word wrapping
- **Preprocessor**: `#include` / `#define` for decomp header integration

---

## Project Status

### Completed
- Full byte-matching compiler/decompiler pipeline from source to binary and back
- Transpiler for all DSPRE and decomps formats
- LSP, syntax highlighting, editor extensions, error reporting
- Constant loading from database, text banks (DSPRE) and decomp projects' JSON and header files
- Full test infrastructure with DSPRE and decomp fixtures

### Roadmap
- Resolve GlobalScript IDs to script files and integrate in workspace symbol table
- Internal variable aliases e.g. `VAR_0x8008`, `VAR_RESULT`
- Graph colouring for variable liveness analysis, which will allow for:
  - Variable allocation for automatic assignment
  - Complex expressions in conditions (`if x + 1 == 5`)
  - Fully-featured for loops, will need graph colouring for counting
- Decompiler pattern matching for `match`/`while`/`if` reconstruction
- Derive includes for utilized text banks from map headers and GlobalScript table (DSPRE)

---

## Quick Start

### Installation

See [INSTALL.md](INSTALL.md) for setting up rotom for DSPRE/decomp/hge projects and extension support.

### Initialize a project
```bash
rotom init /path/to/project
```

`rotom init` creates `rotom.toml` and seeds `.rotom/command_database/`. Project compile/decompile commands use that config by default.

### Convert legacy scripts
```bash
rotom convert
```

Migrates a project's existing legacy scripts to Rotom source: DSPRE `.script` files and decomp `.s` files become `.rotom` (levelscripts become `.json`), with the originals backed up first. `rotom init` reports how many convertible files it finds and asks you whether to convert them rightaway, without needing to run this; pass `--dry-run` to preview the conversions without writing anything.

Note: for DSPRE, it actually freshly disassembles from the binaries. This is done to avoid database and symbol conflicts.

### Compile a project
```bash
rotom compile
```

#### Compile a single script with an explicit database
```bash
rotom compile -d .rotom/command_database/platinum_v2.json -i script.rotom -o script.bin
```

#### Decompile a single binary with an explicit database
```bash
rotom decompile -d .rotom/command_database/platinum_v2.json -i script.bin -o script.rotom
```

> [!IMPORTANT]
> The single-file commands also accept folders for batch compilation/decompilation.

### Editor Support

Rotom includes `rotom-lsp`, a Language Server Protocol server for editor features such as diagnostics, autocomplete, hover, go-to-definition, inlay hints, signature help, and code lenses.

Editor integrations for VS Code, Zed, and Neovim are being developed alongside Rotom in [rotom-extensions](TBD). Tree-sitter grammar and highlighting support live in [`tree-sitter-rotom`](https://github.com/KalaayPT/tree-sitter-rotom).

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
action NPC_WalkAway:
    WalkDown 3
    WalkLeft 2
    FaceDown
EndMovement
```

### Match Statements

Use `match` to dispatch based on a variable's value:

> [!IMPORTANT]
> The branches are exclusive and have no fall-through semantics. 
> A case whose body is a single `Call` or `Jump` is optimized into a `CallIf` or `GoToIf` to the target respectively; any other case body uses exclusive `Jump` branching.
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

### String Literals

Instead of managing a separate text file and referencing messages by number, you can write text directly in your script:

```rotom
script NPC #1:
    Message "Hello, trainer!"
    WaitButton
    End
```

This works with any command that takes a text argument: menu entries, bank-specific messages, and more:

```rotom
AddMenuEntryImm "Option A", 4
MessageFromBank 1, "text here"
```

The compiler takes care of storing the text in the right place automatically.

Strings can span multiple lines. Leading whitespace at the start of each new line is stripped, so you can indent freely without it showing up in-game:

```rotom
script LongSpeech #1:
    Message "Hello there! I've been waiting
             for a trainer as strong as you
             to come along."
    WaitButton
    End
```

Wrap a string in `format()` to have the compiler automatically insert word-wrap breaks so the text fits the in-game dialog box:

```rotom
script Explanation #1:
    Message format("This text is too long for one line but format will handle the wrapping for you automatically.")
    WaitButton
    End
```

> [!IMPORTANT]
> String literals and `format()` only work when compiling as part of a project; single-file compilation doesn't have the context to know where to store the text.

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

Rotom is built with two core principles:

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
