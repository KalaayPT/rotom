use tower_lsp::lsp_types::{
    ParameterInformation, ParameterLabel, Position as LspPosition, SignatureHelp,
    SignatureInformation,
};

use rotom::compiler::{
    lexer::is_identifier_char,
    sourcemap::{Position as SourcePosition, SourceMap},
};
use rotom::database::{Command, DatabaseV2};

/// Produce LSP signature help for the command being typed at the cursor.
pub fn compute_signature_help(
    source: &str,
    position: LspPosition,
    db: Option<&DatabaseV2>,
) -> Option<SignatureHelp> {
    let db = db?;
    let map = SourceMap::new(source);
    let byte_offset = map.position_to_byte(SourcePosition {
        line: position.line,
        character: position.character,
    });

    let (command_name, param_index) = extract_command_context(source, byte_offset)?;

    let builtin_name = command_name.strip_prefix('.').unwrap_or(&command_name);
    if let Some(sig) = builtin_signature_help(builtin_name, param_index) {
        return Some(sig);
    }

    let cmd = db.get_command(&command_name).ok()?;

    Some(build_signature_help(&command_name, cmd, param_index))
}

/// Walk backward from the cursor to find the command name and current parameter index.
///
/// Handles both call-style (`GiveItem(5, |)`) and space-separated (`GiveItem 5, |`).
#[allow(clippy::cast_possible_truncation)]
pub fn extract_command_context(source: &str, byte_offset: usize) -> Option<(String, u32)> {
    let line_start = source[..byte_offset].rfind('\n').map_or(0, |i| i + 1);
    let before_cursor = &source[line_start..byte_offset];

    // Call-style: `CommandName(arg1, arg2, |`
    if let Some(open_paren) = before_cursor.rfind('(') {
        let inside = &before_cursor[open_paren..];
        if !inside.contains(')') {
            let name = word_ending_at(&before_cursor[..open_paren])?;
            return Some((name, inside.matches(',').count() as u32));
        }
    }

    // Space-separated: `CommandName arg1, arg2, |`
    let trimmed = before_cursor.trim_start();
    let space = trimmed.find(' ')?;
    let name = &trimmed[..space];
    if name.is_empty() || !name.chars().all(is_identifier_char) {
        return None;
    }
    Some((
        name.to_string(),
        trimmed[space..].matches(',').count() as u32,
    ))
}

/// Extract the last identifier word in `text` (the word ending at `text.len()`).
fn word_ending_at(text: &str) -> Option<String> {
    let trimmed = text.trim_end();
    let start = trimmed
        .rfind(|c: char| !is_identifier_char(c))
        .map_or(0, |i| {
            i + trimmed[i..].chars().next().map_or(1, char::len_utf8)
        });
    let word = &trimmed[start..];
    if word.is_empty() {
        None
    } else {
        Some(word.to_string())
    }
}

fn builtin_signature_help(name: &str, active_param: u32) -> Option<SignatureHelp> {
    match name {
        "Menu" | "MenuGlobal" => Some(menu_builder_signature(name, active_param)),
        "anchor" => Some(menu_method_signature(
            "anchor(left | right)",
            "Select the Platinum menu anchor; right requires a one-column or auto-width menu.",
            &["left | right"],
            active_param,
        )),
        "position" => Some(menu_method_signature(
            "position(x, y)",
            "Set the menu position.",
            &["x", "y"],
            active_param,
        )),
        "cursor" => Some(menu_method_signature(
            "cursor(index)",
            "Set the initially selected entry.",
            &["index"],
            active_param,
        )),
        "prompt" => Some(menu_method_signature(
            "prompt(text)",
            "Show a message before opening the menu.",
            &["text"],
            active_param,
        )),
        "width" => Some(menu_method_signature(
            "width(tiles)",
            "Set the Platinum list-menu width.",
            &["tiles"],
            active_param,
        )),
        "columns" => Some(menu_method_signature(
            "columns(count)",
            "Set the number of columns in a Diamond/Pearl or Platinum normal menu.",
            &["count"],
            active_param,
        )),
        "scrollable" => Some(menu_method_signature(
            "scrollable(bool = true)",
            "Select list-menu mode in Diamond/Pearl or Platinum. Omitting the argument enables it.",
            &["bool = true"],
            active_param,
        )),
        "cancel" => Some(menu_method_signature(
            "cancel(target | entry -> target)",
            "Set the B-cancel target, optionally adding a selectable cancel entry.",
            &["target | entry -> target"],
            active_param,
        )),
        "format" => Some(SignatureHelp {
            signatures: vec![SignatureInformation {
                label: "format(string)".to_string(),
                documentation: Some(tower_lsp::lsp_types::Documentation::String(
                    "Word-wraps a message string to fit the dialog box. \
                     Each source newline defines a segment boundary; \
                     segments wider than the dialog are word-wrapped within."
                        .to_string(),
                )),
                parameters: Some(vec![ParameterInformation {
                    label: ParameterLabel::Simple("string".to_string()),
                    documentation: Some(tower_lsp::lsp_types::Documentation::String(
                        "The message text to format.".to_string(),
                    )),
                }]),
                active_parameter: Some(active_param),
            }],
            active_signature: Some(0),
            active_parameter: Some(active_param),
        }),
        _ => None,
    }
}

fn menu_builder_signature(name: &str, active_param: u32) -> SignatureHelp {
    menu_method_signature(
        &format!("{name}(entry -> target, ...)"),
        "An entry is a label or a (label, hover) pair.",
        &["entries"],
        active_param,
    )
}

fn menu_method_signature(
    label: &str,
    documentation: &str,
    parameter_labels: &[&str],
    active_param: u32,
) -> SignatureHelp {
    SignatureHelp {
        signatures: vec![SignatureInformation {
            label: label.to_string(),
            documentation: Some(tower_lsp::lsp_types::Documentation::String(
                documentation.to_string(),
            )),
            parameters: Some(
                parameter_labels
                    .iter()
                    .map(|parameter| ParameterInformation {
                        label: ParameterLabel::Simple((*parameter).to_string()),
                        documentation: None,
                    })
                    .collect(),
            ),
            active_parameter: Some(active_param),
        }],
        active_signature: Some(0),
        active_parameter: Some(active_param),
    }
}

fn build_signature_help(name: &str, cmd: &Command, active_param: u32) -> SignatureHelp {
    let param_labels: Vec<String> = cmd
        .params
        .iter()
        .map(|p| {
            if p.optional {
                format!("[{}]", p.name)
            } else {
                p.name.clone()
            }
        })
        .collect();

    let label = format!("{name}({})", param_labels.join(", "));

    let parameters: Vec<ParameterInformation> = param_labels
        .into_iter()
        .map(|label| ParameterInformation {
            label: ParameterLabel::Simple(label),
            documentation: None,
        })
        .collect();

    SignatureHelp {
        signatures: vec![SignatureInformation {
            label,
            documentation: cmd
                .description
                .as_ref()
                .map(|d| tower_lsp::lsp_types::Documentation::String(d.clone())),
            parameters: Some(parameters),
            active_parameter: Some(active_param),
        }],
        active_signature: Some(0),
        active_parameter: Some(active_param),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_db() -> &'static DatabaseV2 {
        DatabaseV2::test_platinum()
    }

    #[test]
    fn extract_command_context_supports_space_and_call_style() {
        assert_eq!(
            extract_command_context("    Message 0", "    Message 0".len()),
            Some(("Message".to_string(), 0))
        );
        assert_eq!(
            extract_command_context("    MsgBoxExtern 1, 2", "    MsgBoxExtern 1, 2".len()),
            Some(("MsgBoxExtern".to_string(), 1))
        );
        assert_eq!(
            extract_command_context("    format(\"hello\", ", "    format(\"hello\", ".len()),
            Some(("format".to_string(), 1))
        );
    }

    #[test]
    fn extract_command_context_rejects_non_command_contexts() {
        assert_eq!(extract_command_context("    ", 4), None);
        assert_eq!(
            extract_command_context("Message(0)", "Message(0)".len()),
            None
        );
    }

    #[test]
    fn builtin_format_signature_is_returned() {
        let source = "script Test #1:\n    Message format(\"hello\"\n";
        let help = compute_signature_help(
            source,
            LspPosition {
                line: 1,
                character: 26,
            },
            Some(test_db()),
        )
        .expect("expected signature help");

        assert_eq!(help.active_parameter, Some(0));
        assert_eq!(help.signatures[0].label, "format(string)");
    }

    #[test]
    fn menu_builder_signature_is_returned() {
        let source = "script Test #1:\n    Menu(10 -> Target\n";
        let help = compute_signature_help(
            source,
            LspPosition {
                line: 1,
                character: u32::try_from(source.lines().nth(1).unwrap().len()).unwrap(),
            },
            Some(test_db()),
        )
        .expect("expected menu signature help");

        assert_eq!(help.signatures[0].label, "Menu(entry -> target, ...)");
    }

    #[test]
    fn explicit_cancel_entry_signature_is_returned() {
        let source = "script Test #1:\n    Menu(10 -> Target).cancel(\"Abort\" -> Cancel\n";
        let help = compute_signature_help(
            source,
            LspPosition {
                line: 1,
                character: u32::try_from(source.lines().nth(1).unwrap().len()).unwrap(),
            },
            Some(test_db()),
        )
        .expect("expected cancel signature help");

        assert_eq!(help.signatures[0].label, "cancel(target | entry -> target)");
    }

    #[test]
    fn database_command_signature_uses_active_parameter() {
        let source = "script Test #1:\n    Message \n";
        let help = compute_signature_help(
            source,
            LspPosition {
                line: 1,
                character: 12,
            },
            Some(test_db()),
        )
        .expect("expected signature help");

        assert_eq!(help.active_parameter, Some(0));
        assert!(help.signatures[0].label.starts_with("Message("));
        assert!(
            help.signatures[0]
                .parameters
                .as_ref()
                .is_some_and(|p| !p.is_empty())
        );
    }

    #[test]
    fn missing_database_or_unknown_command_returns_none() {
        assert!(
            compute_signature_help(
                "script Test #1:\n    Message ",
                LspPosition {
                    line: 1,
                    character: 12,
                },
                None,
            )
            .is_none()
        );
        assert!(
            compute_signature_help(
                "script Test #1:\n    NotACommand ",
                LspPosition {
                    line: 1,
                    character: 16,
                },
                Some(test_db()),
            )
            .is_none()
        );
    }
}
