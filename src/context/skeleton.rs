use std::path::Path;

pub const PRETOKENIZE_SKELETONIZE_BYTES: usize = 24_000;
pub const LARGE_FILE_SKELETON_MAX_BYTES: usize = 16_000;
pub const SKELETON_LINE_MAX_CHARS: usize = 240;

pub fn is_skeletonizable_extension(extension: &str) -> bool {
    matches!(
        extension
            .trim_start_matches('.')
            .to_ascii_lowercase()
            .as_str(),
        "rs" | "java" | "js" | "jsx" | "ts" | "tsx"
    )
}

pub fn skeletonize_file(content: &str, extension: &str) -> String {
    let ext = extension.to_lowercase();
    let ext_clean = ext.trim_start_matches('.');
    let mut parser = tree_sitter::Parser::new();
    let lang = match ext_clean {
        "rs" => tree_sitter_rust::language(),
        "java" => tree_sitter_java::language(),
        "js" | "jsx" => tree_sitter_javascript::language(),
        "ts" => tree_sitter_typescript::language_typescript(),
        "tsx" => tree_sitter_typescript::language_tsx(),
        _ => return content.to_string(),
    };

    if parser.set_language(lang).is_err() {
        return content.to_string();
    }
    let Some(tree) = parser.parse(content, None) else {
        return content.to_string();
    };

    let mut ranges_to_prune = Vec::new();
    collect_prune_ranges(tree.root_node(), &mut ranges_to_prune);

    ranges_to_prune.sort_by(|a, b| b.0.cmp(&a.0));

    let mut result = content.to_string();
    for (start, end) in ranges_to_prune {
        if start < end && end <= result.len() {
            result.replace_range(start..end, " /* omitted body */ ");
        }
    }

    result
}

fn is_js_declaration_line(trimmed: &str) -> bool {
    let starts_with_declaration = [
        "import ",
        "export ",
        "class ",
        "interface ",
        "type ",
        "function ",
        "const ",
        "let ",
        "var ",
        "enum ",
        "namespace ",
        "abstract class ",
        "constructor(",
        "public ",
        "private ",
        "protected ",
        "readonly ",
        "static ",
        "async ",
        "override ",
    ]
    .iter()
    .any(|prefix| trimmed.starts_with(prefix));

    let method_signature = trimmed.ends_with('{')
        && trimmed.contains('(')
        && ![
            "if ",
            "for ",
            "while ",
            "switch ",
            "catch ",
            "return ",
            "describe(",
            "it(",
        ]
        .iter()
        .any(|prefix| trimmed.starts_with(prefix));

    starts_with_declaration || method_signature
}

fn is_rust_declaration_line(trimmed: &str) -> bool {
    [
        "#[",
        "use ",
        "pub use ",
        "mod ",
        "pub mod ",
        "struct ",
        "pub struct ",
        "enum ",
        "pub enum ",
        "trait ",
        "pub trait ",
        "type ",
        "pub type ",
        "fn ",
        "pub fn ",
        "impl ",
        "const ",
        "pub const ",
        "static ",
        "pub static ",
    ]
    .iter()
    .any(|prefix| trimmed.starts_with(prefix))
        || (trimmed.contains(" fn ") && trimmed.ends_with('{'))
}

fn is_java_declaration_line(trimmed: &str) -> bool {
    [
        "package ",
        "import ",
        "class ",
        "interface ",
        "enum ",
        "record ",
        "@interface ",
        "public ",
        "protected ",
        "private ",
        "abstract ",
        "final ",
        "static ",
        "@",
    ]
    .iter()
    .any(|prefix| trimmed.starts_with(prefix))
        || (trimmed.contains('(') && trimmed.ends_with('{'))
}

fn append_skeleton_line(output: &mut String, line: &str) {
    let mut clipped: String = line.chars().take(SKELETON_LINE_MAX_CHARS).collect();
    if line.chars().count() > SKELETON_LINE_MAX_CHARS {
        clipped.push_str(" ...");
    }
    output.push_str(&clipped);
    output.push('\n');
}

/// Lossy fallback for files that are large enough to skeletonize before token budgeting.
/// It preserves likely declarations and emits omission markers, but tree-sitter remains
/// the precise skeletonizer for normal-sized files.
pub fn fast_skeletonize_large_file(content: &str, extension: &str) -> String {
    let ext = extension.trim_start_matches('.').to_ascii_lowercase();
    let mut output =
        String::from("/* BRE large-file skeleton: declarations retained, bodies omitted. */\n");
    let mut omitted = false;

    for (idx, line) in content.lines().enumerate() {
        if output.len() >= LARGE_FILE_SKELETON_MAX_BYTES {
            output.push_str("/* ... */\n");
            break;
        }

        let trimmed = line.trim_start();
        let keep = match ext.as_str() {
            "rs" => idx < 20 || is_rust_declaration_line(trimmed),
            "java" => idx < 20 || is_java_declaration_line(trimmed),
            "js" | "jsx" | "ts" | "tsx" => idx < 20 || is_js_declaration_line(trimmed),
            _ => false,
        };

        if keep {
            append_skeleton_line(&mut output, line);
            omitted = false;
        } else if !omitted {
            output.push_str("/* ... */\n");
            omitted = true;
        }
    }

    if output.len() < content.len() {
        output
    } else {
        content.to_string()
    }
}

pub fn skeletonize_for_budget(content: &str, extension: &str) -> String {
    if content.len() > PRETOKENIZE_SKELETONIZE_BYTES {
        fast_skeletonize_large_file(content, extension)
    } else {
        skeletonize_file(content, extension)
    }
}

pub fn prepare_watching_content_for_budget(path: &str, content: String) -> (String, bool) {
    if content.len() <= PRETOKENIZE_SKELETONIZE_BYTES {
        return (content, false);
    }

    let ext = Path::new(path)
        .extension()
        .and_then(|ext| ext.to_str())
        .unwrap_or("");
    if !is_skeletonizable_extension(ext) {
        return (content, false);
    }

    let skeleton = skeletonize_for_budget(&content, ext);
    if skeleton.len() < content.len() {
        (skeleton, true)
    } else {
        (content, false)
    }
}

pub fn estimate_token_count(content: &str) -> usize {
    content.len().div_ceil(4)
}

pub fn max_context_file_count(max_tokens: usize) -> usize {
    (max_tokens / 1_500).clamp(16, 36)
}

pub fn collect_prune_ranges(node: tree_sitter::Node, ranges: &mut Vec<(usize, usize)>) {
    let kind = node.kind();

    let is_rust_fn_body =
        kind == "block" && node.parent().is_some_and(|p| p.kind() == "function_item");

    let is_js_fn_body = kind == "statement_block"
        && node.parent().is_some_and(|p| {
            matches!(
                p.kind(),
                "function_declaration" | "method_definition" | "arrow_function" | "function"
            )
        });

    let is_java_method_body = kind == "block"
        && node.parent().is_some_and(|p| {
            matches!(
                p.kind(),
                "method_declaration"
                    | "constructor_declaration"
                    | "compact_constructor_declaration"
            )
        });

    if is_rust_fn_body || is_js_fn_body || is_java_method_body {
        let start = node.start_byte() + 1;
        let end = node.end_byte() - 1;
        if start < end {
            ranges.push((start, end));
            return;
        }
    }

    let mut cursor = node.walk();
    if cursor.goto_first_child() {
        loop {
            collect_prune_ranges(cursor.node(), ranges);
            if !cursor.goto_next_sibling() {
                break;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_ast_skeletonization() {
        let rust_code = r#"
            fn add(a: i32, b: i32) -> i32 {
                let sum = a + b;
                sum
            }
            impl Math {
                fn subtract(&self, a: i32, b: i32) -> i32 {
                    a - b
                }
            }
        "#;
        let skeleton_rust = skeletonize_file(rust_code, "rs");
        assert!(skeleton_rust.contains("fn add(a: i32, b: i32) -> i32 { /* omitted body */ }"));
        assert!(skeleton_rust.contains("fn subtract(&self, a: i32, b: i32) -> i32 { /* omitted body */ }"));

        let js_code = r#"
            function multiply(a, b) {
                return a * b;
            }
            const divide = (a, b) => {
                return a / b;
            };
            class Calc {
                square(x) {
                    return x * x;
                }
            }
        "#;
        let skeleton_js = skeletonize_file(js_code, "js");
        assert!(skeleton_js.contains("function multiply(a, b) { /* omitted body */ }"));
        assert!(skeleton_js.contains("const divide = (a, b) => { /* omitted body */ };"));
        assert!(skeleton_js.contains("square(x) { /* omitted body */ }"));

        let java_code = r#"
            package com.acme;

            public class MathService {
                public int add(int a, int b) {
                    int sum = a + b;
                    return sum;
                }
            }
        "#;
        let skeleton_java = skeletonize_file(java_code, "java");
        assert!(skeleton_java.contains("public int add(int a, int b) { /* omitted body */ }"));
    }

    #[test]
    fn test_fast_skeletonizer_does_not_treat_comments_as_declarations() {
        let header = (0..20)
            .map(|idx| format!("const bootstrap{idx} = {idx};\n"))
            .collect::<String>();
        let large_body = "    doWork();\n".repeat(2_000);
        let content = format!(
            "{header}// Example: const user = new UserService();\nexport class RealService {{\n  run() {{\n{large_body}  }}\n}}\n"
        );

        let skeleton = fast_skeletonize_large_file(&content, "ts");

        assert!(!skeleton.contains("Example: const user"));
        assert!(skeleton.contains("export class RealService"));
        assert!(skeleton.contains("run() {"));
    }

    #[test]
    fn test_fast_skeletonizer_marks_multiline_signatures_as_lossy() {
        let header = (0..20)
            .map(|idx| format!("const bootstrap{idx} = {idx};\n"))
            .collect::<String>();
        let large_body = "    return id;\n".repeat(2_000);
        let content = format!(
            "{header}export function loadUser(\n  id: string,\n  options: LoadOptions,\n) {{\n{large_body}}}\n"
        );

        let skeleton = fast_skeletonize_large_file(&content, "ts");

        assert!(skeleton.contains("export function loadUser("));
        assert!(!skeleton.contains("options: LoadOptions"));
        assert!(skeleton.contains("/* ... */"));
    }

    #[test]
    fn test_typescript_skeletonization_retains_types() {
        let ts_code = r#"
            interface UserInfo<T> {
                id: string;
                data: T;
                roles: string[];
            }

            export class UserManager<T> implements UserInfo<T> {
                id: string;
                data: T;
                roles: string[];

                constructor(id: string, data: T) {
                    this.id = id;
                    this.data = data;
                    this.roles = ["user"];
                }

                public getRoles(): string[] {
                    return this.roles;
                }

                private updateData(newData: T): void {
                    this.data = newData;
                }
            }
        "#;
        let skeleton = skeletonize_file(ts_code, "ts");

        // Verify interface definition remains fully intact
        assert!(skeleton.contains("interface UserInfo<T> {"));
        assert!(skeleton.contains("id: string;"));
        assert!(skeleton.contains("data: T;"));
        assert!(skeleton.contains("roles: string[];"));

        // Verify class signatures are preserved
        assert!(skeleton.contains("export class UserManager<T> implements UserInfo<T> {"));

        // Verify method and constructor bodies are stripped and replaced with placeholders
        assert!(skeleton.contains("constructor(id: string, data: T) { /* omitted body */ }"));
        assert!(skeleton.contains("public getRoles(): string[] { /* omitted body */ }"));
        assert!(skeleton.contains("private updateData(newData: T): void { /* omitted body */ }"));

        // Verify implementation details inside bodies are gone
        assert!(!skeleton.contains("this.roles = [\"user\"]"));
        assert!(!skeleton.contains("return this.roles"));
        assert!(!skeleton.contains("this.data = newData"));
    }
}
