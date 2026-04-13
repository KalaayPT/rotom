use crate::compiler::token::TokenType;

/// Normalize structural keywords and accepted aliases that are part of the
/// rotom language spec rather than DB-backed command naming.
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

#[cfg(test)]
mod tests {
    use super::*;

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
}
