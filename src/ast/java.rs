pub fn traverse_java_ast(node: tree_sitter::Node, source: &[u8], imports: &mut Vec<String>) {
    if node.kind() == "import_declaration" {
        if let Ok(text) = node.utf8_text(source) {
            if let Some(import) = java_import_text(text) {
                imports.push(import.to_string());
            }
        }
        return;
    }

    for child in node.children(&mut node.walk()) {
        traverse_java_ast(child, source, imports);
    }
}

fn java_import_text(import_declaration: &str) -> Option<&str> {
    let import = import_declaration
        .trim()
        .trim_end_matches(';')
        .trim()
        .strip_prefix("import")?
        .trim();
    let import = import.strip_prefix("static").map_or(import, str::trim);
    (!import.is_empty()).then_some(import)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_traverse_java_ast_collects_imports() {
        let source = b"
            package com.acme.web;

            import com.acme.service.UserService;
            import static com.acme.config.Flags.ENABLED;

            class Controller {}
        ";
        let mut parser = tree_sitter::Parser::new();
        parser.set_language(tree_sitter_java::language()).unwrap();
        let tree = parser.parse(source, None).unwrap();
        let mut imports = Vec::new();
        traverse_java_ast(tree.root_node(), source, &mut imports);

        assert!(imports.contains(&"com.acme.service.UserService".to_string()));
        assert!(imports.contains(&"com.acme.config.Flags.ENABLED".to_string()));
    }
}
