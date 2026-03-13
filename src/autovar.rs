use crate::database::{Command, ParamDef};

pub const VAR_RESULT: i32 = 0x800C;

const AUTOVAR_DEFAULT_VALUES: &[&str] = &["VAR_RESULT", "0x800C"];

pub fn is_autovar_param(param: &ParamDef) -> bool {
    // This intentionally treats only VAR_RESULT-style defaults as autovar.
    // It relies on the database invariant that no non-autovar result default
    // appears outside the final/defaultable result slot.
    param.default.as_ref().is_some_and(|default| {
        AUTOVAR_DEFAULT_VALUES
            .iter()
            .any(|v| default.eq_ignore_ascii_case(v))
    })
}

pub fn autovar_param_index(cmd: &Command) -> Option<usize> {
    cmd.params.iter().position(is_autovar_param)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::database::{CommandType, ParamType, Variant};

    fn make_param(name: &str, default: Option<&str>) -> ParamDef {
        ParamDef {
            name: name.to_string(),
            param_type: ParamType::Var,
            const_value: None,
            default: default.map(std::string::ToString::to_string),
            optional: false,
        }
    }

    fn make_command(params: Vec<ParamDef>) -> Command {
        Command {
            cmd_type: CommandType::ScriptCmd,
            id: Some(1),
            legacy_name: None,
            description: None,
            params,
            variants: Option::<Vec<Variant>>::None,
            expansion: None,
        }
    }

    #[test]
    fn test_is_autovar_param_true_for_result_defaults() {
        let p1 = make_param("destVarID", Some("VAR_RESULT"));
        let p2 = make_param("successVar", Some("0x800C"));
        assert!(is_autovar_param(&p1));
        assert!(is_autovar_param(&p2));
    }

    #[test]
    fn test_is_autovar_param_true_without_name_heuristics() {
        let p = make_param("someOtherParam", Some("VAR_RESULT"));
        assert!(is_autovar_param(&p));
    }

    #[test]
    fn test_autovar_param_index_returns_expected_slot() {
        let cmd = make_command(vec![
            make_param("item", None),
            make_param("amount", None),
            make_param("destVarID", Some("VAR_RESULT")),
        ]);
        assert_eq!(autovar_param_index(&cmd), Some(2));
    }
}
