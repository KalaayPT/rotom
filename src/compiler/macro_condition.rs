//! Shared evaluation of database macro / command variant conditions.
//!
//! Analysis (shape selection) and lowering (macro expansion) must agree on which variant
//! matches; this module holds the single implementation.

use dashmap::DashMap;
use regex::Regex;
use std::collections::HashMap;
use std::sync::LazyLock;

use uxie::c_parser::defines::eval_expr_with_parent;

use crate::compiler::ast::Expression;
use crate::database::ParamDef;

/// Macro/variant arg-count condition matcher: `1 arg`, `2 args`, `3 arg(s)`, etc.
static RE_ARG_COUNT: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^(\d+)\s+args?\(?s?\)?$").expect("static regex pattern is valid")
});

#[derive(Debug, Clone, Copy)]
pub struct MacroConditionEvalError;

/// Returns whether `condition` selects this variant for the given call arguments.
///
/// `resolve_arg_int` maps positional macro arguments to integers when possible (skipped on
/// failure). `resolve_parent` resolves free identifiers in the C-like condition expression.
pub fn evaluate_macro_variant_condition(
    condition: &str,
    args: &[Expression],
    params: &[ParamDef],
    resolve_arg_int: impl Fn(&Expression) -> Option<i32>,
    resolve_parent: impl Fn(&str) -> Option<i64>,
) -> Result<bool, MacroConditionEvalError> {
    if let Some(caps) = RE_ARG_COUNT.captures(condition) {
        let expected_count: usize = caps[1].parse().unwrap_or(0);
        return Ok(args.len() == expected_count);
    }

    let exprs: HashMap<String, String> = HashMap::new();
    let mut resolved: HashMap<String, i64> = HashMap::new();
    let cache: DashMap<String, i64> = DashMap::new();

    for (pos, param) in params.iter().enumerate() {
        if let Some(arg) = args.get(pos)
            && let Some(val) = resolve_arg_int(arg)
        {
            resolved.insert(param.name.clone(), i64::from(val));
        }
    }

    eval_expr_with_parent(condition, &exprs, &resolved, &cache, &resolve_parent)
        .map(|value| value != 0)
        .ok_or(MacroConditionEvalError)
}
