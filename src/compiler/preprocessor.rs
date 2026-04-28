#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IncludeDirective {
    pub path: String,
    pub line_number: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PreprocessResult {
    pub cleaned_source: String,
    pub includes: Vec<IncludeDirective>,
}

/// removes `#include` and `#define` directives
pub fn preprocess(source: &str) -> PreprocessResult {
    let mut cleaned_source = String::with_capacity(source.len());
    let mut includes = Vec::new();

    for (line_idx, line) in source.lines().enumerate() {
        let trimmed = line.trim();
        if trimmed.starts_with("#include") {
            if let Some(path) = extract_include_path(trimmed) {
                includes.push(IncludeDirective {
                    path,
                    line_number: line_idx + 1,
                });
            }
            cleaned_source.push('\n');
            continue;
        }

        if trimmed.starts_with("#define") {
            cleaned_source.push('\n');
            continue;
        }

        cleaned_source.push_str(line);
        cleaned_source.push('\n');
    }

    PreprocessResult {
        cleaned_source,
        includes,
    }
}

fn extract_include_path(line: &str) -> Option<String> {
    let include_target = line.strip_prefix("#include")?.trim();

    if let Some(path) = include_target
        .strip_prefix('"')
        .and_then(|path| path.strip_suffix('"'))
    {
        return Some(path.to_string());
    }

    include_target
        .strip_prefix('<')
        .and_then(|path| path.strip_suffix('>'))
        .map(ToString::to_string)
}

#[cfg(test)]
mod tests {
    use super::{IncludeDirective, preprocess};

    #[test]
    fn preprocess_removes_directives_but_preserves_line_numbers() {
        let source = r#"#include "constants/test.h"
#define TEST_VALUE 7
script Main #1:
    Message TEST_VALUE
End
"#;

        let result = preprocess(source);

        assert_eq!(
            result.includes,
            vec![IncludeDirective {
                path: "constants/test.h".to_string(),
                line_number: 1,
            }]
        );
        assert_eq!(
            result.cleaned_source,
            "\n\nscript Main #1:\n    Message TEST_VALUE\nEnd\n"
        );
    }
}
