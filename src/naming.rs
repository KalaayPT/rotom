use crate::compiler::token::TokenType;
use crate::compiler::{
    ParseResult,
    parse_error::{database_error, parse_error},
};
use crate::database::{CommandType, DatabaseV2, normalize_command_name};

pub fn normalize_control_keyword(raw: &str) -> Option<TokenType> {
    match raw.to_ascii_lowercase().as_str() {
        "function" => Some(TokenType::Function),
        "public" => Some(TokenType::Public),
        "action" => Some(TokenType::Action),
        "alias" => Some(TokenType::Alias),
        "global" => Some(TokenType::Global),
        "true" => Some(TokenType::True),
        "false" => Some(TokenType::False),
        "if" => Some(TokenType::If),
        "then" => Some(TokenType::Then),
        "else" => Some(TokenType::Else),
        "endif" => Some(TokenType::EndIf),
        "while" => Some(TokenType::While),
        "do" => Some(TokenType::Do),
        "endwhile" => Some(TokenType::EndWhile),
        "match" => Some(TokenType::Match),
        "with" => Some(TokenType::With),
        "case" => Some(TokenType::Case),
        "endmatch" => Some(TokenType::EndMatch),
        "break" => Some(TokenType::Break),
        "end" => Some(TokenType::End),
        "endmovement" => Some(TokenType::EndMovement),
        "return" => Some(TokenType::Return),
        "jump" | "goto" => Some(TokenType::Jump),
        "and" => Some(TokenType::And),
        "or" => Some(TokenType::Or),
        "not" => Some(TokenType::Not),
        "as" => Some(TokenType::As),
        _ => None,
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NameResolution {
    pub canonical_name: String,
    pub lookup_key: String,
}

fn snake_to_pascal_case(input: &str) -> String {
    let mut out = String::new();

    for part in input.split('_').filter(|part| !part.is_empty()) {
        let mut chars = part.chars();
        if let Some(first) = chars.next() {
            out.push(first.to_ascii_uppercase());
            out.extend(chars.map(|c| c.to_ascii_lowercase()));
        }
    }

    out
}

fn canonical_command_name(db_key: &str) -> String {
    if db_key.chars().any(|c| c.is_ascii_uppercase()) {
        db_key.to_string()
    } else {
        snake_to_pascal_case(db_key)
    }
}

fn command_matches_required_type(
    cmd_type: &CommandType,
    required_type: Option<&CommandType>,
) -> bool {
    match required_type {
        Some(required) => cmd_type == required,
        None => true,
    }
}

fn parse_prefixed_command_id(raw: &str, prefix: &str) -> Option<u16> {
    let id_str = raw.strip_prefix(prefix)?;

    if id_str.is_empty() {
        return None;
    }

    u16::from_str_radix(id_str, 16).ok()
}

fn parse_scrcmd_id(raw: &str) -> Option<u16> {
    if let Some(id) = parse_prefixed_command_id(raw, "ScrCmd_") {
        return Some(id);
    }

    if let Some(suffix) = raw.strip_prefix("scrcmd_") {
        if suffix.is_empty() {
            return None;
        }
        if !suffix.chars().all(|c| c.is_ascii_digit()) {
            return None;
        }
        return suffix.parse::<u16>().ok();
    }

    None
}

fn resolve_by_numeric_id(
    db: &DatabaseV2,
    id: u16,
    required_type: Option<CommandType>,
) -> Option<NameResolution> {
    let expected_type = required_type.unwrap_or(CommandType::ScriptCmd);

    let (key, _) = db
        .commands
        .iter()
        .find(|(_, cmd)| cmd.id == Some(id) && cmd.cmd_type == expected_type)?;

    Some(NameResolution {
        canonical_name: canonical_command_name(key),
        lookup_key: key.clone(),
    })
}

fn resolve_by_prefixed_id(
    db: &DatabaseV2,
    raw: &str,
    prefix: &str,
    required_type: Option<CommandType>,
) -> Option<NameResolution> {
    let id = parse_prefixed_command_id(raw, prefix)?;
    resolve_by_numeric_id(db, id, required_type)
}

fn collect_matches<'a>(
    db: &'a DatabaseV2,
    raw: &str,
    required_type: Option<CommandType>,
) -> Vec<(&'a String, &'a crate::database::Command)> {
    let normalized_raw = normalize_command_name(raw);

    db.commands
        .iter()
        .filter(|(_, cmd)| command_matches_required_type(&cmd.cmd_type, required_type.as_ref()))
        .filter(|(key, cmd)| {
            *key == raw
                || cmd.legacy_name.as_deref() == Some(raw)
                || normalize_command_name(key) == normalized_raw
                || cmd
                    .legacy_name
                    .as_deref()
                    .map(normalize_command_name)
                    .as_deref()
                    == Some(normalized_raw.as_str())
        })
        .collect()
}

fn resolve_normalized_name(
    db: &DatabaseV2,
    raw: &str,
    required_type: Option<CommandType>,
    kind_label: &str,
) -> ParseResult<NameResolution> {
    if let Some(id) = parse_scrcmd_id(raw)
        && let Some(resolved) = resolve_by_numeric_id(db, id, required_type.clone())
    {
        return Ok(resolved);
    }

    if let Some(resolved) = resolve_by_prefixed_id(db, raw, "Dummy", required_type.clone()) {
        return Ok(resolved);
    }

    let matches = collect_matches(db, raw, required_type);

    if matches.is_empty() {
        return Err(database_error(format!(
            "{} '{}' not found in database",
            kind_label, raw
        )));
    }

    let mut resolutions: Vec<NameResolution> = matches
        .into_iter()
        .map(|(key, _)| NameResolution {
            canonical_name: canonical_command_name(key),
            lookup_key: key.to_string(),
        })
        .collect();

    resolutions.sort_by(|a, b| a.lookup_key.cmp(&b.lookup_key));
    resolutions.dedup_by(|a, b| a.lookup_key == b.lookup_key);

    if resolutions.len() > 1 {
        let options = resolutions
            .iter()
            .map(|r| format!("{} ({})", r.canonical_name, r.lookup_key))
            .collect::<Vec<_>>()
            .join(", ");

        return Err(parse_error(
            0..0,
            format!(
                "{} name '{}' is ambiguous; candidates: {}",
                kind_label, raw, options
            ),
        ));
    }

    Ok(resolutions.remove(0))
}

pub fn resolve_command_name(db: &DatabaseV2, raw: &str) -> ParseResult<NameResolution> {
    resolve_normalized_name(db, raw, None, "Command")
}

pub fn resolve_script_command_name(db: &DatabaseV2, raw: &str) -> ParseResult<NameResolution> {
    resolve_normalized_name(db, raw, Some(CommandType::ScriptCmd), "Command")
}

pub fn resolve_movement_name(db: &DatabaseV2, raw: &str) -> ParseResult<NameResolution> {
    resolve_normalized_name(db, raw, Some(CommandType::Movement), "Movement")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::database::{Command, CommandType, DatabaseMeta, DatabaseV2};
    use std::collections::HashMap;

    fn test_command(cmd_type: CommandType, id: u16, legacy_name: Option<&str>) -> Command {
        Command {
            cmd_type,
            id: Some(id),
            legacy_name: legacy_name.map(ToString::to_string),
            description: None,
            params: Vec::new(),
            variants: None,
            expansion: None,
        }
    }

    fn test_db() -> DatabaseV2 {
        let mut commands = HashMap::new();
        commands.insert(
            "copyvar".to_string(),
            test_command(CommandType::ScriptCmd, 0x0010, None),
        );
        commands.insert(
            "goto_if".to_string(),
            test_command(CommandType::ScriptCmd, 0x001C, Some("GotoIf")),
        );
        commands.insert(
            "compare_var_to_value".to_string(),
            test_command(CommandType::ScriptCmd, 0x0020, Some("CompareVarValue")),
        );
        commands.insert(
            "walk_north".to_string(),
            test_command(CommandType::Movement, 0x0001, Some("WalkNorth")),
        );
        commands.insert(
            "Message".to_string(),
            test_command(CommandType::ScriptCmd, 0x0030, Some("MessageLegacy")),
        );

        DatabaseV2 {
            meta: DatabaseMeta {
                version: "test".to_string(),
                generated_at: None,
                generated_from: None,
            },
            commands,
            sounds: HashMap::new(),
            comparison_operators: HashMap::new(),
            overworld_directions: HashMap::new(),
            special_overworlds: HashMap::new(),
        }
    }

    #[test]
    fn test_normalize_control_keyword_accepts_aliases_and_case() {
        assert_eq!(normalize_control_keyword("jump"), Some(TokenType::Jump));
        assert_eq!(normalize_control_keyword("Jump"), Some(TokenType::Jump));
        assert_eq!(normalize_control_keyword("goto"), Some(TokenType::Jump));
        assert_eq!(normalize_control_keyword("GoTo"), Some(TokenType::Jump));
        assert_eq!(
            normalize_control_keyword("EndMovement"),
            Some(TokenType::EndMovement)
        );
        assert_eq!(
            normalize_control_keyword("endmovement"),
            Some(TokenType::EndMovement)
        );
        assert_eq!(normalize_control_keyword("return"), Some(TokenType::Return));
        assert_eq!(normalize_control_keyword("IF"), Some(TokenType::If));
    }

    #[test]
    fn test_resolve_command_name_prefers_pascalized_db_key() {
        let db = test_db();
        let resolved = resolve_command_name(&db, "copyvar").expect("copyvar should resolve");
        assert_eq!(resolved.lookup_key, "copyvar");
        assert_eq!(resolved.canonical_name, "Copyvar");

        let resolved = resolve_command_name(&db, "goto_if").expect("goto_if should resolve");
        assert_eq!(resolved.lookup_key, "goto_if");
        assert_eq!(resolved.canonical_name, "GotoIf");
    }

    #[test]
    fn test_resolve_command_name_accepts_exact_and_legacy_forms() {
        let db = test_db();

        let resolved =
            resolve_command_name(&db, "CompareVarValue").expect("legacy form should resolve");
        assert_eq!(resolved.lookup_key, "compare_var_to_value");
        assert_eq!(resolved.canonical_name, "CompareVarToValue");

        let resolved = resolve_command_name(&db, "compare_var_to_value")
            .expect("exact db key form should resolve");
        assert_eq!(resolved.lookup_key, "compare_var_to_value");
        assert_eq!(resolved.canonical_name, "CompareVarToValue");
    }

    #[test]
    fn test_resolve_movement_name() {
        let db = test_db();

        let resolved = resolve_movement_name(&db, "walk_north").expect("movement should resolve");
        assert_eq!(resolved.lookup_key, "walk_north");
        assert_eq!(resolved.canonical_name, "WalkNorth");

        let resolved =
            resolve_movement_name(&db, "WalkNorth").expect("legacy movement form should resolve");
        assert_eq!(resolved.lookup_key, "walk_north");
        assert_eq!(resolved.canonical_name, "WalkNorth");
    }

    #[test]
    fn test_resolve_script_command_name_by_scrcmd_hex_id() {
        let db = test_db();
        let resolved =
            resolve_script_command_name(&db, "ScrCmd_001C").expect("ScrCmd alias should resolve");
        assert_eq!(resolved.lookup_key, "goto_if");
        assert_eq!(resolved.canonical_name, "GotoIf");
    }

    #[test]
    fn test_resolve_script_command_name_by_scrcmd_decimal_id() {
        let db = test_db();
        let resolved =
            resolve_script_command_name(&db, "scrcmd_28").expect("decimal scrcmd should resolve");
        assert_eq!(resolved.lookup_key, "goto_if");
        assert_eq!(resolved.canonical_name, "GotoIf");
    }

    #[test]
    fn test_resolve_script_command_name_uppercase_numeric_alias_is_still_hex() {
        let db = test_db();
        let resolved =
            resolve_script_command_name(&db, "ScrCmd_0010").expect("numeric alias should resolve");
        assert_eq!(resolved.lookup_key, "copyvar");
        assert_eq!(resolved.canonical_name, "Copyvar");
    }

    #[test]
    fn test_resolve_script_command_name_rejects_non_decimal_lowercase_alias() {
        let db = test_db();
        assert!(
            resolve_script_command_name(&db, "scrcmd_001C").is_err(),
            "lowercase scrcmd aliases must remain decimal-only"
        );
        assert!(
            resolve_script_command_name(&db, "scrcmd_AB").is_err(),
            "lowercase scrcmd aliases must reject non-decimal payloads"
        );
    }

    #[test]
    fn test_resolve_command_name_keeps_pascal_db_keys() {
        let db = test_db();
        let resolved =
            resolve_command_name(&db, "MessageLegacy").expect("legacy alias should resolve");
        assert_eq!(resolved.lookup_key, "Message");
        assert_eq!(resolved.canonical_name, "Message");
    }
}
