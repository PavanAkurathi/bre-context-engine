pub fn traverse_js_ast(node: tree_sitter::Node, source: &[u8], imports: &mut Vec<String>) {
    let kind = node.kind();
    if kind == "import_statement" || kind == "export_statement" {
        for child in node.children(&mut node.walk()) {
            if child.kind() == "string" {
                if let Ok(text) = child.utf8_text(source) {
                    let path = text.trim_matches(|c| c == '\'' || c == '"' || c == '`');
                    imports.push(path.to_string());
                }
            }
        }
    } else if kind == "call_expression" {
        let is_require = if let Some(func_node) = node.child_by_field_name("function") {
            func_node.kind() == "identifier" && (func_node.utf8_text(source) == Ok("require"))
        } else if let Some(first_child) = node.child(0) {
            first_child.kind() == "identifier" && (first_child.utf8_text(source) == Ok("require"))
        } else {
            false
        };

        if is_require {
            if let Some(args_node) = node.child_by_field_name("arguments") {
                for child in args_node.children(&mut args_node.walk()) {
                    if child.kind() == "string" {
                        if let Ok(text) = child.utf8_text(source) {
                            let path = text.trim_matches(|c| c == '\'' || c == '"' || c == '`');
                            imports.push(path.to_string());
                        }
                    }
                }
            }
        }
    }

    for child in node.children(&mut node.walk()) {
        traverse_js_ast(child, source, imports);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_traverse_js_ast() {
        let source = b"
            import foo from './helper';
            const bar = require('../utils');
            export { baz } from './exporter';
        ";
        let mut parser = tree_sitter::Parser::new();
        parser
            .set_language(tree_sitter_javascript::language())
            .unwrap();
        let tree = parser.parse(source, None).unwrap();
        let mut imports = Vec::new();
        traverse_js_ast(tree.root_node(), source, &mut imports);
        assert!(imports.contains(&"./helper".to_string()));
        assert!(imports.contains(&"../utils".to_string()));
        assert!(imports.contains(&"./exporter".to_string()));
    }
}
