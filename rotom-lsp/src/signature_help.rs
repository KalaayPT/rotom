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

    if let Some(sig) = builtin_signature_help(&command_name, param_index) {
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
    Some((name.to_string(), trimmed[space..].matches(',').count() as u32))
}

/// Extract the last identifier word in `text` (the word ending at `text.len()`).
fn word_ending_at(text: &str) -> Option<String> {
    let trimmed = text.trim_end();
    let start = trimmed
        .rfind(|c: char| !is_identifier_char(c))
        .map_or(0, |i| i + trimmed[i..].chars().next().map_or(1, char::len_utf8));
    let word = &trimmed[start..];
    if word.is_empty() {
        None
    } else {
        Some(word.to_string())
    }
}

fn builtin_signature_help(name: &str, active_param: u32) -> Option<SignatureHelp> {
    match name {
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
            documentation: cmd.description.as_ref().map(|d| {
                tower_lsp::lsp_types::Documentation::String(d.clone())
            }),
            parameters: Some(parameters),
            active_parameter: Some(active_param),
        }],
        active_signature: Some(0),
        active_parameter: Some(active_param),
    }
}
