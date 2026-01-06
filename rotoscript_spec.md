# Rotom Language Specification

**Version:** 0.1 (Draft)
**Target:** Nintendo DS Scripting Engine (DSPRE)
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

### 1.4 Labels
Labels define locations in code for Jumps or Pointers.
* **Global Labels:** End with a colon. Visible everywhere. Can be jumped to from any function.
    * Syntax: `MyGlobalLabel:`
* **Local Labels:** Start with a dot, optionally end with a colon. Visible only within the current Function.
    * Syntax: `.loop_start:` or `.loop_start`
    * Note: The colon is optional for local labels. Both forms are equivalent.

### 1.5 Keywords
Reserved words that cannot be used as identifiers:
* **Block Delimiters:** `function`, `action`, `End`, `Return`
* **Modifiers:** `public`, `global`, `alias`, `as`
* **Control Flow:** `if`, `then`, `else`, `endif`, `while`, `do`, `endwhile`, `Jump`
* **Logical Operators:** `and`, `or`, `not`
* **Literals:** `true`, `false`

---

## 2. Program Structure

A Rotom script consists of three sections, usually in this order:
1.  **Global Definitions** (Aliases)
2.  **Code Blocks** (Functions)
3.  **Data Blocks** (Actions / Movement)

```c
// 1. Globals
global alias 2550 as FLAG_Badge

// 2. Functions
public function GymLeader #1
    if FLAG_Badge == 1 then // this is an implicit "CheckFlag" instruction and compiles down to such
        Jump .already_fought // for commands (jump is a script command opcode), newlines are delimiters
    endif
    .already_fought:
    End
End

// 3. Actions
action MovePlayer
    WalkLeft 1
    FaceUp
End
```

### 2.1 Function Declarations

Functions are the primary code containers. They can have multiple entry points for jump-table aliasing.

```rotom
// Private function (not in jump table)
function Helper
    Message 1
End

// Public function with explicit jump-table ID
public function Main #1
    Call Helper
End

// Multiple headers pointing to same code (jump-table aliasing)
public function TalkToNPC #5
public function InteractWithSign #6
function SharedHandler
    Message 10
End
```

* `public` - Function appears in the jump table (callable from game engine)
* `#N` - Explicit jump-table slot assignment
* `function Name:` - Colon suffix is optional (for legacy compatibility)

### 2.2 Terminators

* `End` - Terminates script execution entirely. The script stops running.
* `Return` - Returns control to the caller. Used for sub-functions called via `Call`.

## 3. Scoping & Variables

The Gen 4 Pokemon games have many persistent variables, but only 14 of them are script-local and function exactly exactly like CPU registers:
* 0x8000-0x800B: 12 normal variables
* 0x800C: used as "result" variable, but can be used freely
* 0x800D: special: "last interacted" overworld, which triggered the script execution

### 3.1 Aliases

Aliases are compile-time constants (macros) that map a name to a number.

This is in leu of a planned (TODO) "register allocation" system for variables to be auto-assigned.
* Syntax: `alias Value as Name`
* **Global Alias:** Defined at the top level with `global` prefix. Visible in all functions.
    * Syntax: `global alias 0x8000 as MyVar`
* **Local Alias:** Defined inside a function (no `global` prefix). Visible only in that function.
    * Syntax: `alias 0x8001 as TempVar`

### 3.2 Scoping Rules

1. Shadowing: A Local Alias can shadow (hide) a Global Alias of the same name.
2. Redeclaration: A name cannot be redefined within the exact same scope block.
    * Rationale: Prevents "Time Travel" bugs where the meaning of a variable changes halfway through a block after code has already been generated.

### 3.3 Variable Heuristics (Compiler Logic)

The compiler infers the "type" of a number based on the Nintendo DS Memory Map:
* Value (Immediate): 0x0000 to 0x3FFF
* Variable (Pointer): 0x4000 and above (e.g., Flags, Script Vars). // this logic isnt flawless and what really matters is command expectation types but this works for now

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
* Normalization: The hardware strictly requires Compare VAR, VALUE. If you write if 5 > x, the compiler swaps operands to Compare x, 5 and flips the operator to <=.

### 4.2 Loops (while)

```rotom
while x < 10 do
    AddVar x, 1
endwhile
```

### 4.3 Jumps and Calls
* `Jump LabelName` - Unconditional jump to a label
* `Jump .local_label` - Jump to a local label within the same function
* `Call FunctionName` - Call a function, execution returns after `Return`

Restriction: You cannot Jump to a variable alias. You can only jump to (or call) Labels or Functions.

### 4.4 Expressions in Conditions

Conditions support function-call syntax for commands that return values:
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

## 5. Commands & Functions

### 5.1 Script Commands

Native hardware commands defined in the game database.
* Syntax: CommandName Arg1, Arg2, ...
* Argument Resolution:
    * If Arg is an Integer, it passes raw.
    * If Arg is a Variable Alias, it resolves to the ID (e.g., 0x4000).
    * If Arg is a Label, it passes a reference (LabelRef) to that label's offset.

### 5.2 Actions

Special blocks containing only movement commands.
* Strict Mode: Actions cannot contain control flow logic (if, while, Jump) or aliases.
* Terminator: Actions must end with `End`.
* Usage: Actions are referenced by specific commands (e.g., `ApplyMovement OW_ID, @ActionName`).

```rotom
action WalkPattern
    WalkRight 3
    WalkDown 2
    FaceLeft
End
```

## 6. Error Handling

The compiler reports errors with source locations using the following categories:

* **Lexer Errors:** Invalid tokens, unclosed block comments
* **Parse Errors:** Unexpected tokens, missing delimiters (endif, endwhile, End)
* **Semantic Errors:**
    * Undefined symbol references
    * Duplicate definitions in the same scope
    * Invalid jump targets (jumping to a variable instead of a label)
    * Control flow inside Actions
    * Global aliases defined inside functions

Example error output:
```
error: Undefined symbol: 'undefined_var'
  --> script.rotom:15:5
   |
15 |     SetVar undefined_var, 1
   |            ^^^^^^^^^^^^^
```

## 7. Compiler Pipeline (Technical)
1. Lexer: Source → Tokens.
2. Parser: Tokens → AST (Statement nodes).
3. Semantic Analysis:
    * Registers Symbols (functions, actions, labels, aliases).
    * Validates scopes and label existence.
    * Enforces "Movement-Only" rules for Actions.
    * Checks for undefined references and duplicate definitions.
4. Lowering (IR Generation):
    * Flattens If/While blocks into Labels and Jumps.
    * Swaps comparison operands to match hardware (Val == Var → Var == Val).
    * Generates Symbolic IR (Command { name: "SetVar" }).
    * Inverts conditions for jump-if semantics.
5. Codegen (Assembler):
    * Maps Symbolic Names to Hex IDs using JSON DB.
    * Calculates byte offsets for Labels.
    * Writes jump table and binary output.
6. Disassembler (Reverse):
    * Parses binary jump table to find entry points.
    * Iteratively discovers functions/actions via call analysis.
    * Generates human-readable Rotom source.

## 8. Binary Format (Reference)

The compiled script binary consists of:
1. **Jump Table:** Array of 4-byte offsets pointing to public function entry points
    * Terminated by `0xFD13` marker
2. **Script Data:** Concatenated function and action bytecode
    * Commands are 2-byte IDs followed by parameters
    * Parameters are 2 or 4 bytes depending on command definition
3. **Movement Data:** Separate section for action bytecode
    * Movement commands are 2-byte ID + 2-byte parameter

## 9. Future Work (TODO)

[] codegen
[] simple decompilation
[] tests
  [x] lexer tests
  [x] parser tests
  [x] semantic analysis tests
  [] codegen tests
[] binary matching against known scripts
[] macro support
[] decompilation into high-level logic
[] Register allocation for automatic variable assignment
[] Constant folding for compile-time arithmetic
[] Complex expressions in conditions (`if x + 1 == 5`)
[] Type checking against command parameter expectations
[] Optimization passes (dead code elimination, jump threading)
