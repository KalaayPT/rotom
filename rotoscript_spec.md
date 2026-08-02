# Rotom Language Specification

**Version:** 0.1 (Draft)
**Target:** Nintendo DS Scripting Engine (Pokemon Gen 4)
**File Extension:** `.rotom`

## 1. Lexical Structure

### 1.1 Comments
* **Single Line:** Starts with `//` and continues to the end of the line.
* **Block:** Enclosed between `/*` and `*/`.
    ```c
    // This is a single line comment
    SetVar x, 1 // Inline comment

    /* This is a block comment
       spanning multiple lines
    */
    ```

### 1.2 Identifiers
* Alphanumeric strings starting with a letter or underscore (`_`).
* Case-sensitive (e.g., `MyVar` != `myvar`).

### 1.3 Literals
* **Decimal Integers:** `0`, `42`, `-10`.
* **Hexadecimal Integers:** `0x1A`, `0x4000`.
* **String Literals:** `"Hello, trainer!"` double-quoted text; resolved to a message-archive index at compile time. See [Section 6. String Literals](#6-string-literals).

### 1.4 Labels
Labels define locations in code that can be jumped to. Syntax: `Name:` at the file level.

> [!NOTE]
> The dot-prefix form `.Name:` (e.g. `.loop_start:`) is accepted by the compiler for backward compatibility but carries no semantic distinction. Prefer plain `Name:` labels.

### 1.5 Keywords
Reserved words that cannot be used as identifiers:
* **Block Delimiters:** `script`, `action`, `End`, `Return`, `EndMovement`
* **Modifiers:** `alias`, `as`
* **Control Flow:** `if`, `then`, `else`, `endif`, `while`, `do`, `endwhile`, `match`, `with`, `case`, `endmatch`, `Jump`
* **Logical Operators:** `and`, `or`, `not`
* **Literals:** `true`, `false`

> [!NOTE]
> Keywords are matched case-insensitively: `IF`, `ENDIF`, `ENDWHILE`, etc. are all accepted by the compiler. Lowercase is the canonical style; mixed-case keywords should not be intentionally used.

---

## 2. Program Structure

A Rotom script is a flat sequence of:
1. **Aliases** (compile-time constants)
2. **Scripts** (public entry points with jump table slots)
3. **Labels** (private code blocks, not in jump table)
4. **Actions** (movement data blocks)

Code blocks are delimited by the start of the next block, not by explicit terminators.

```rotom
// 1. Aliases (all global)
alias 0x800C as VAR_RESULT
alias 2550 as FLAG_Badge

// 2. Public script (in jump table)
script GymLeader #1:
    if FLAG_Badge == 1 then
        Jump AlreadyFought
    endif

AlreadyFought:
    End

// 3. Label (not in jump table)
HelperCode:
    Message 1
    Return

// 4. Actions
action MovePlayer:
    WalkLeft 1
    FaceUp
EndMovement
```

### 2.1 Script Declarations

Scripts are public entry points that appear in the jump table. They require a slot number.

```rotom
// Public script with jump-table slot #1
script Main #1:
    Message 1
    End

// Stacked headers (multiple jump table entries pointing to same code)
script TalkToNPC #5:
script InteractWithSign #6:
    Message 10
    End
```

* `script Name #N:` - Script with jump-table slot N (required). Slot IDs are 1-based.
* The colon after the header is required.

#### Bracketed Slot Lists

A single header can assign multiple jump-table slots at once with a `#[…]` list. The list supports individual IDs and `lo-hi` inclusive ranges, separated by commas. This is particularly useful for battle scripts and similar tables where one handler covers a sequential block of slot IDs.

```rotom
// Slots 5, 10, 11, 12, 13, 14, and 15 all point to Handler
script Handler #[5, 10-15]:
    Message 1
    End
```

`#[N]` with a single value is equivalent to `#N`.

### 2.2 Labels

Labels are code blocks that are not in the jump table. They are used for:
- Shared code that multiple scripts jump to
- Helper routines
- Fall-through code organization

```rotom
// Bare label syntax
SharedHandler:
    Message 10
    End

NoCoinCase:
    Message 5
    CloseMessage
    End
```

### 2.3 Fall-Through

Code blocks that don't end with `End` or `Return` fall through to the next block in source order. This is useful for:
- Multiple entry points sharing common code
- Sequential code organization matching decomp style

```rotom
script SlotMachine_9 #9:
    SetVar LOCALID, 9
    GoTo SlotMachine_Common
    // No End - next block follows in binary

script SlotMachine_10 #10:
    SetVar LOCALID, 10
    GoTo SlotMachine_Common

SlotMachine_Common:
    PlayFanfare SEQ_SE_CONFIRM
    LockAll
    // ... shared implementation
    End
```

### 2.4 Terminators

* `End` - Terminates script execution entirely. The script stops running.
* `Return` - Returns control to the caller. Used for sub-routines called via `Call`.

Note: These are commands that emit bytecode, not structural delimiters.

### 2.5 Preprocessor Directives

`#include` and `#define` can appear at the top level of a script file. Both are primarily intended for decomp projects, where the script can reference the same constant headers the C code uses, but they work in any project that has a configured workspace.

#### `#include`

Loads constants from an external header file (`.h`). In a decomp project the path is resolved relative to the project's `include/` directory, matching the decomp build's include search path:

```rotom
#include "constants/items.h"
#include "constants/flags.h"

script GivePotion #1:
    AddItem ITEM_POTION, 1
    End
```

An unresolved include path is a compile error in project mode.

#### `#define`

Defines a file-local numeric constant that the compiler resolves like any other constant name:

```rotom
#define REWARD_ITEM 44  // ITEM_GREAT_BALL

script RewardPlayer #1:
    AddItem REWARD_ITEM, 1
    End
```

> [!NOTE]
> `#define` exists primarily as a compatibility surface for decomp header files pulled in via `#include`. For constants you define directly in a script, prefer [`alias`](#31-aliases); it is the idiomatic Rotom form and integrates with LSP diagnostics and unused-constant warnings.

Both directives require the compiler to have a project workspace configured. In single-file compilation mode they are parsed and silently accepted.

## 3. Aliases & Variables

The Gen 4 Pokemon games have many persistent variables, but only 14 of them are script-local and script like CPU registers:
* 0x8000-0x800B: 12 normal variables
    * 0x8008: used for "switch" type checks in the original binaries, can be used freely
* 0x800C: used as "result" variable, but can also be used freely
* 0x800D: special: "last interacted" overworld, which triggered the script execution

### 3.1 Aliases

Aliases are compile-time constants that map a name to a number. All aliases are global.

* Syntax: `alias Value as Name`
* Can be defined at the top level of the file or inside a script or label body
* Visible from the point of definition onward: any alias can only be used *after* it has been defined. This also allows for shadowing/redefinitions

```rotom
alias 0x8000 as VAR_TEMP
alias 0x800C as VAR_RESULT
alias 1500 as SEQ_SE_CONFIRM
```

### 3.2 Built-in Constants

The compiler loads constants from the database, including:
- Sound IDs: `SEQ_SE_CONFIRM`, etc.
- Special overworld IDs
- Direction constants

User aliases can shadow (override) built-in constants.

### 3.3 Condition Identifiers

Commands with a `condition` parameter (like `GoToIf`, `CallIf`) accept symbolic condition names:

| Identifier | Value | Meaning |
|------------|-------|---------|
| `LESS` | 0 | Less than |
| `EQUAL` | 1 | Equal to |
| `GREATER` | 2 | Greater than |
| `LESS_EQUAL` | 3 | Less than or equal |
| `GREATER_EQUAL` | 4 | Greater than or equal |
| `DIFFERENT` | 5 | Not equal |

Example usage:
```rotom
CompareVarValue VAR_TEMP, 5
GoToIf EQUAL, HandleFive
GoToIf GREATER, HandleLarge
```

The decompiler also outputs these symbolic names instead of numeric values.

### 3.4 Variable Heuristics

Script command operands use a 16-bit value space split at `0x4000`. When compiling or decompiling numeric operands, the heuristic is:
* Immediate value: `0x0000`-`0x3FFF`
* Variable ID: `0x4000`-`0xFFFF`

These ranges are often used by commands to distinguish between whether to check a variable or an immediate value.

## 4. Control Flow

### 4.1 Conditionals (if)

Supported Operators: ==, !=, >, <, >=, <=.

```rotom
if x == 5 then
    // 'Then' Block
else
    // 'Else' Block
endif
```

**Else-If Chaining:**
```rotom
if x == 1 then
    Message 1
else if x == 2 then
    Message 2
else
    Message 0
endif
```
Note: `else if` is parsed as a nested if statement within the else block.

Compiler Behavior:
* Generates a Compare command followed by a JumpIf (inverted logic).
* Normalization: The hardware strictly requires Compare VAR, VALUE. If you write if 5 > x, the compiler swaps the operands to Compare x, 5 (so the comparison reads x < 5). The JumpIf then skips the body when the condition is false, using the inverted comparison: JumpIf GREATER_EQUAL.

### 4.2 Match Statements

Match statements provide pattern matching against a variable or expression result:

```rotom
match VAR_RESULT with
    case 0:
        Message 1
    case 1, 2:
        Message 2
    else:
        Message 3
endmatch
```

* **Syntax:** `match <subject> with ... endmatch`
* **Cases:** `case <value>:` or `case <value1>, <value2>:` for multiple values
* **Default:** Optional `else:` block for unmatched values
* **Per-case optimization:** A case with a single value whose body is a lone `Call` or `Jump` is optimized to emit `CompareVarValue` + a conditional branch to the target (`CallIf EQUAL` for `Call`, `GoToIf EQUAL` for `Jump`) instead of the typical compare/jump/body/goto pattern. This optimization is applied per-case, so mixed match statements benefit from it.

Match statements also work with autovar commands:
```rotom
match ShowYesNoMenu() with
    case 0:
        Call HandleNo
    case 1:
        Call HandleYes
endmatch
```

### 4.3 Menu Builders

Menu builders describe a text menu and the labels reached by each selection:

```rotom
script ChooseAction #1:
    Menu(
        "Talk" -> Talk,
        ("Leave", "Are you sure?") -> Leave,
    )
    End
```

`Menu` uses the script's local text archive for literal entry text. `MenuGlobal` uses the game's
global menu-entry archive instead:

```rotom
script UseMenu #1:
    MenuGlobal(
        "Use" -> UseItem,
        "Cancel" -> Cancel,
    )
    End
```

A builder accepts 1 to 28 entries. The optional `(label, hover)` form adds help text that changes
with the selected entry. It selects list mode in Diamond/Pearl and Platinum; HGSS displays the
help text on the top screen.

`label -> target`, `.position()`, `.cursor()`, `.prompt()`, `.cancel(target)`, and
`.cancel(label -> target)` work in every game and menu type.

| Syntax | D/P normal | D/P list | Platinum normal | Platinum list | HGSS touch/list |
| --- | --- | --- | --- | --- | --- |
| `label -> target` | Yes | Yes | Yes | Yes | Yes |
| `(label, hover) -> target` | No | Yes | No | Yes | Yes |
| `.position(x, y)` | Yes | Yes | Yes | Yes | Yes |
| `.cursor(index)` | Yes | Yes | Yes | Yes | Yes |
| `.prompt(text)` | Yes | Yes | Yes | Yes | Yes |
| `.cancel(target)` | Yes | Yes | Yes | Yes | Yes |
| `.cancel(label -> target)` | Yes | Yes | Yes | Yes | Yes |
| `.cancel((label, hover) -> target)` | No | Yes | No | Yes | Yes |
| `.scrollable()` / `.scrollable(bool)` | `false` | `true` | `false` | `true` | No |
| `.columns(count)` | Yes | No | Yes | No | No |
| `.width(tiles)` | No | No | No | Yes | No |
| `.anchor(left \| right)` | No | No | Yes* | Yes* | No |

`.position(x, y)` defaults to `(1, 1)`, and `.cursor(index)` defaults to `0`. A prompt uses the
script's local text archive. `.cancel(target)` enables B-cancel without adding an entry. The
dispatch form also adds its label, and optional hover text, as the final selectable entry. Without
either form, B-cancel is disabled.
Hover entries and `.width(...)` require list mode. Combining either with `.scrollable(false)`
emits a warning because the explicit normal-menu request cannot be honored.
Diamond/Pearl and Platinum normal menus warn above 8 rows with a prompt or 11 rows without one.
Multi-column menus require the total entry count to divide evenly by the column count; an explicit
cancel entry is included in that total. List menus display up to 8 rows at once and scroll when more
entries are present.
Calling `.scrollable()` without an argument is equivalent to `.scrollable(true)`.
Platinum right anchoring is limited to one-column normal menus and auto-width list menus, so
`.anchor(right)` cannot be combined with `.columns(...)` or `.width(...)`.

Entry and cancel targets must be labels, scripts, or actions. Selection targets use the
zero-based entry index. An explicit cancel entry counts toward the 28-entry limit.

### 4.4 Loops (while)

```rotom
while x < 10 do
    AddVar x, 1
endwhile
```

### 4.5 Jumps and Calls
* `Jump LabelName` - Unconditional jump to a label or script
* `Call ScriptName` - Call a script/helper, execution returns after `Return`

`GoTo` is accepted as a synonym for `Jump` and compiles identically.

Restriction: You cannot Jump to a variable alias. You can only jump to Labels or Scripts.

### 4.6 Expressions in Conditions

Conditions support call-expression syntax for commands that return values:
```rotom
if GetPlayerX() == 10 then
    if GetPlayerY() == 20 then
        Message 1
    endif
endif
```

Arithmetic expressions are supported in command arguments:
```rotom
SetVar x, 1 + 2 * 3    // Evaluates to 7 (standard precedence)
SetVar y, (1 + 2) * 3  // Evaluates to 9 (parentheses override)
```
Note: Complex expressions in conditions (e.g., `if x + 1 == 5`) are not yet supported.

## 5. Commands & Actions

### 5.1 Script Commands

Native hardware commands defined in the game database.
* Syntax: `CommandName Arg1, Arg2, ...` (assembly-style) or `CommandName(Arg1, Arg2)` (call-style)
* Both forms are equivalent and can be used interchangeably.
* Argument Resolution:
    * If Arg is an Integer, it passes raw.
    * If Arg is a Variable Alias, it resolves to the ID (e.g., 0x4000).
    * If Arg is a Label/Script name, it passes a reference to that location's offset.

### 5.1.1 Database-Defined Call Shapes

The JSON command database can accept more than one source-level call shape for the same opcode.

* `params`: The command's normal binary arg list.
* `default`: A value filled in when the caller leaves that arg out.
* `variants`: Extra call shapes the compiler should accept.
* `condition`: How to pick a variant. Conditions are checked in order. `else` is the fallback.
* `emit_args`: Optional rewrite expressions that turn the chosen source args back into the normal
  binary arg list.

For script commands, the compiler does this:
1. Pick a call shape. First-arg `const` variants are checked first, then conditional variants in
   DB order, then the base `params`.
2. Apply defaults on the chosen shape.
3. If that shape has `emit_args`, rewrite the args.
4. Lower and encode the final args normally.

This lets the DB describe decomp-style sugar without changing the real binary layout. For example,
`ViewRankings scope, page, record` can be accepted and rewritten to the engine's normal two-arg
form.

### 5.1.2 Database Macros

Database entries with `type: "macro"` are compile-time sugar, not hardware opcodes.

* `params` and `default` define the accepted macro args.
* `variants` and `condition` can pick alternate macro call shapes or expansions.
* `expansion` is a list of Rotom statements emitted after `$param` substitution.

For macros, the compiler does this:
1. Pick the macro call shape using the same variant rules as script commands.
2. Apply defaults on that selected source shape.
3. Select the macro expansion variant, if any.
4. Substitute `$param` placeholders and parse the expanded statements as normal Rotom code.

This makes DB macros useful for overloads, constant-based rewrites, and reusable helpers without
changing the underlying command set.

### 5.2 Actions

Special blocks containing only movement commands.
* Strict Mode: Actions cannot contain control flow logic (if, while, Jump) or aliases.
* Terminator: Actions must end with `EndMovement`.
* Self-contained: Actions are always fully encapsulated (no fall-through).
* Usage: Actions are referenced by specific commands (e.g., `ApplyMovement OW_ID, ActionName`).

```rotom
action WalkPattern:
    WalkRight 3
    WalkDown 2
    FaceLeft
EndMovement
```

Actions may also be defined inline in a command argument. Inline actions use the same
movement-only validation and command syntax as named actions:

```rotom
ApplyMovement LOCALID_PLAYER, action(WalkFastEast 2)

ApplyMovement LOCALID_PLAYER, action(
    WalkFastEast 2
    FaceNorth
)
```

The closing `)` acts as an implicit `EndMovement`; writing `EndMovement` before it is also valid.
Movement commands remain newline-delimited, except that a one-command action may share the line
with its parentheses. The compiler emits anonymous inline action data after all explicit
top-level items.

## 6. String Literals

String literals let you write message text directly as a command argument. The compiler looks up the string in the file's associated text archive, adding it if it does not exist, and substitutes the resulting message index at compile time.

```rotom
script NPC #1:
    Message "Hello, trainer!"
    WaitButton
    End
```

### 6.1 Syntax

Strings are enclosed in double quotes (`"`). A `\"` sequence inside a string emits the chatot alias `["]`, which maps to the curly-quote character (charmap 0x01B4). There is no raw double-quote in the Gen 4 charmap, so this is the correct way to include one in dialog text.

### 6.2 Multiline Strings

A string can span multiple source lines by containing a real newline inside the quotes. Each line becomes a separate _segment_. Leading whitespace on continuation lines is stripped automatically, so the source can be indented to match the surrounding code without polluting the text content:

```rotom
Message "Hello, I'm on the first line,
         and I'm on the second line."
```

The segments are joined with a space when written to the archive, so the example above stores `"Hello, I'm on the first line, and I'm on the second line."`. The game's dialog engine handles its own line-breaking when displaying the text.

The compiler also tracks per-segment widths and emits a **`MessageLineTooLong`** warning when a segment exceeds the maximum dialog line width.

### 6.3 Automatic Word Wrapping with `format()`

Wrapping a string in `format()` instructs the compiler to insert automatic word-wrap breaks so the text fits within the in-game dialog box, without needing manual `\n`/`\r`/`\f` markers:

```rotom
Message format("A long line of dialogue that needs to fit neatly inside the dialog box.")
```

`format()` suppresses the `MessageLineTooLong` warning because line-breaking is handled automatically.

> [!NOTE]
> String literals and `format()` require the compiler to have a project workspace configured so it knows which text archive to write to. In single-file compilation mode they produce a compile error.

### 6.4 Encoding Validation

All string content is validated against the chatot charmap at compile time. Characters not present in the charmap produce a compile error with a precise source location.

---

## 7. Error Handling

The compiler reports errors with source locations using the following categories:

* **Lexer Errors:** Invalid tokens, unclosed block comments
* **Parse Errors:** Unexpected tokens, missing delimiters (endif, endwhile, EndMovement)
* **Semantic Errors:**
    * Undefined symbol references
    * Duplicate definitions in the same scope
    * Invalid jump targets (jumping to a variable instead of a label)
    * Control flow inside Actions
    * Missing slot number on script declarations

Example error output:
```
error: Undefined symbol: 'undefined_var'
  --> script.rotom:15:5
   |
15 |     SetVar undefined_var, 1
   |            ^^^^^^^^^^^^^
```

## 8. Compiler Pipeline (Technical)

1. **Lexer:** Source → Tokens.
2. **Parser:** Tokens → AST (Statement nodes).
    * Scripts end at next script/label/action/EOF
    * Actions are self-contained (end at EndMovement)
3. **Semantic Analysis:**
    * Registers Symbols (scripts, labels, actions, aliases).
    * Validates references and label existence.
    * Enforces "Movement-Only" rules for Actions.
    * Checks for undefined references and duplicate definitions.
4. **Lowering (IR Generation):**
    * Flattens If/While blocks into Labels and Jumps.
    * Swaps comparison operands to match hardware (Val == Var → Var == Val).
    * Generates Symbolic IR (Command { name: "SetVar" }).
    * Inverts conditions for jump-if semantics.
5. **Codegen (Assembler):**
    * Maps Symbolic Names to Hex IDs using JSON DB.
    * Calculates byte offsets for Labels.
    * Writes jump table and binary output.
    * Emits code in source order (preserves fall-through semantics).
6. **Decompiler (Reverse):**
    * Parses binary jump table to find entry points.
    * Discovers all jump targets to identify label boundaries.
    * Generates flat Rotom source matching binary layout.

## 9. Binary Format (Reference)

The compiled script binary consists of:
1. **Jump Table:** Array of 4-byte offsets pointing to public script entry points
    * Sorted by slot number (not source order)
    * Terminated by `0xFD13` marker
2. **Script Data:** Concatenated script and label bytecode
    * Commands are 2-byte IDs followed by parameters
    * Parameters are 1, 2 or 4 bytes depending on command definition
    * Code emitted in source order (fall-through preserved)
3. **Movement Data:** Separate section for action bytecode
    * Movement commands are 2-byte ID + 2-byte parameter
    * Named actions are emitted in source order; anonymous inline actions follow all explicit top-level items
    * Every action begins at a 4-byte-aligned offset
    * Default parameter for most movements is 1 (e.g., `WalkNorth` = `WalkNorth 1`)
    * Movements also accept explicit arguments even when DB says 0 params (e.g., `Delay8 4`)

### 9.1 Movement Command Behavior

Movement commands have special handling for parameters:

| Command | DB Params | User Args | Behavior |
|---------|-----------|-----------|----------|
| `WalkNorth` | 0 | 0 | Defaults to 1 step |
| `WalkNorth 3` | 0 | 1 | Walks 3 steps |
| `Delay8` | 0 | 0 | Defaults to 1 frame (0x01) |
| `Delay8 4` | 0 | 1 | Waits 4 frames (0x04) |
| `EndMovement` | 0 | 0 | No parameter emitted |

This allows natural syntax while maintaining binary compatibility with the game engine.

## 10. DSPRE Compatibility

Rotom includes a transpiler for DSPRE script format:

| DSPRE Syntax | Rotom Syntax |
|--------------|--------------|
| `Script N:` | `script script_N #N:` |
| `Function N:` | `func_N:` (bare label) |
| `Action N:` | `action action_N:` |
| `Script#N` | `script_N` |
| `Function#N` | `func_N` |
| `UseScript_#N` | `Jump script_N` |
| `Overworld.0` | `0` (descriptor stripped) |
| `arg1 arg2 arg3` | `arg1, arg2, arg3` (comma-separated) |
