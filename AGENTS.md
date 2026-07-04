# AGENTS.md

Behavioral guidelines for agents working on this project.

**Tradeoff:** These guidelines bias toward caution over speed. For trivial tasks, use judgment.

## 1. Think Before Coding

**Don't assume. Don't hide confusion. Surface tradeoffs.**

Before implementing:
- State your assumptions explicitly. If uncertain, ask.
- If multiple interpretations exist, present them; don't pick silently.
- If a simpler approach exists, say so. Push back when warranted.
- If something is unclear, stop. Name what's confusing. Ask.

## 2. Simplicity First

**Minimum code that solves the problem. Nothing speculative.**

- No features beyond what was asked.
- No abstractions for single-use code.
- No "flexibility" or "configurability" that wasn't requested.
- No error handling for impossible scenarios.
- If you write 200 lines and it could be 50, rewrite it.

Ask yourself: "Would a senior engineer say this is overcomplicated?" If yes, simplify.

## 3. Surgical Changes

**Touch only what you must. Clean up only your own mess.**

When editing existing code:
- Don't "improve" adjacent code, comments, or formatting.
- Don't refactor things that aren't broken.
- Match existing style, even if you'd do it differently.
- If you notice unrelated dead code, mention it; don't delete it.

When your changes create orphans:
- Remove imports/variables/functions that YOUR changes made unused.
- Don't remove pre-existing dead code unless asked.

The test: Every changed line should trace directly to the user's request.

## 4. Goal-Driven Execution

**Define success criteria. Loop until verified.**

Transform tasks into verifiable goals:
- "Add validation" → "Write tests for invalid inputs, then make them pass"
- "Fix the bug" → "Write a test that reproduces it, then make it pass"
- "Refactor X" → "Ensure tests pass before and after"

For multi-step tasks, state a brief plan:
```
1. [Step] → verify: [check]
2. [Step] → verify: [check]
3. [Step] → verify: [check]
```

Strong success criteria let you loop independently. Weak criteria ("make it work") require constant clarification.

**Clippy on touched Rust:** After substantive edits, run Clippy on the relevant crate(s), e.g. `cargo clippy -p rotom --all-targets` or `cargo clippy --workspace --all-targets` when multiple members change. Address **new** warnings in **files or modules you changed** (including lints reported for that code). 

**Coverage on new Rust tests:** When adding tests, verify the impact with `cargo llvm-cov`, e.g. `cargo llvm-cov --all-features --workspace --summary-only` for the workspace or `cargo llvm-cov -p rotom-lsp --all-features --summary-only` for one package.

## 5. Don't Reimplement Upstream Logic

**When a dependency already solves the problem, don't rewrite it.**

- Before adding evaluators, parsers, or resolvers in your crate, search the upstream crate's public API. It probably already has what you need.
- If an upstream sister crate's method is private, make it public or add a thin public seam there; never copy the logic into your crate.
- Don't round-trip through text (serialize → deserialize) when you have structured data. Pass the structures directly.

Ask yourself: "Is this logic already in the dependency's domain?" If yes, add the seam there, not here.

## 6. Parse Once, Reuse the AST

**The AST is the canonical representation. Don't re-derive it.**

- If you need to extract metadata (includes, defines, symbols) and also compile, do both from the same parsed AST.
- Don't parse the source for constants, then parse it again for codegen.
- Don't create intermediate text representations of the AST to feed to another parser.

## 7. Don't Break One Platform to Fix Another

**Platform-specific quirks belong in platform-specific code.**

- If a protocol change (LSP format, command name, argument shape) fixes editor A but breaks editor B, the fix is wrong.
- Fix the odd-one-out editor in its extension adapter, not in the shared protocol.
- The shared protocol should use the most standard/portable format. Adapters translate.

## 8. Consolidate, Don't Proliferate

**Remove cruft before adding new things.**

- If you see 5 functions that could be 3, simplify before extending.
- Don't add `_with_options` and similar variants. Use default parameters, builder patterns (the `bon` crate can be utilized here), or just let callers pass what they need.
- Don't add helper functions called exactly once. Inline them.
- **Never add a module or shared helper for a few lines** that are only used once or twice, especially when call sites still pass closures anyway and gain nothing. Duplicate the snippet inline until several real call sites justify extraction (see §9).
- When your changes make a module or function dead, delete it; don't leave it lying around.
- Before defining a new type, search the codebase. If an equivalent type already exists (same shape, same domain), use it directly. Wrapper types that only exist to rename another type are never acceptable.

## 9. Ask Before Building Infrastructure

**If you're about to add a new module, method, or abstraction, stop and ask.**

- More than 30 lines of new infrastructure code is a yellow flag.
- If the approach requires re-reading files, building synthetic content, or duplicating existing logic, it's probably wrong.
- State the approach before coding. If the user says "why aren't you just using X?", you've missed something.

## 10. Document What You Touch

**If a function lacks a doc comment and you modify it, add one.**

- Keep docs concise and in simple, easy to understand language.
- Surface-level APIs (public entry points, CLI plumbing, LSP handlers) deserve fuller docs:
  - One-line summary
  - Inputs / outputs
  - Errors or options where non-obvious
- Deep internals can be shorter; a single clear sentence is enough.
- Don't document the obvious (`/// Returns true` on `is_success`).
- Match Rust doc comment conventions (`///`).

---

**These guidelines are working if:** fewer unnecessary changes in diffs, fewer rewrites due to overcomplication, clarifying questions come before implementation rather than after mistakes, and touched Rust code is checked with Clippy without dumping unrelated cleanups into the same change.
