pub fn traverse_rust_ast(node: tree_sitter::Node, source: &[u8], imports: &mut Vec<String>) {
    let kind = node.kind();
    if kind == "use_declaration" {
        if let Ok(text) = node.utf8_text(source) {
            if let Some(use_tree) = rust_use_tree_text(text) {
                expand_rust_use_tree(use_tree, "", imports);
            }
        }
    } else if kind == "mod_item" {
        let has_block = node.child_by_field_name("body").is_some()
            || node
                .children(&mut node.walk())
                .any(|c| c.kind() == "declaration_list");
        if !has_block {
            if let Some(name_node) = node.child_by_field_name("name") {
                if let Ok(name) = name_node.utf8_text(source) {
                    imports.push(name.to_string());
                }
            } else {
                let mut found_mod = false;
                for child in node.children(&mut node.walk()) {
                    if child.kind() == "mod" {
                        found_mod = true;
                    } else if found_mod && child.kind() == "identifier" {
                        if let Ok(name) = child.utf8_text(source) {
                            imports.push(name.to_string());
                        }
                        break;
                    }
                }
            }
        }
    } else {
        for child in node.children(&mut node.walk()) {
            traverse_rust_ast(child, source, imports);
        }
    }
}

pub fn traverse_rust_use_aliases(
    node: tree_sitter::Node,
    source: &[u8],
    aliases: &mut Vec<(String, String)>,
) {
    if node.kind() == "use_declaration" {
        if let Ok(text) = node.utf8_text(source) {
            if text.trim_start().starts_with("pub") {
                if let Some(use_tree) = rust_use_tree_text(text) {
                    expand_rust_use_aliases(use_tree, "", aliases);
                }
            }
        }
        return;
    }

    for child in node.children(&mut node.walk()) {
        traverse_rust_use_aliases(child, source, aliases);
    }
}

fn rust_use_tree_text(declaration: &str) -> Option<&str> {
    let declaration = declaration.trim().trim_end_matches(';').trim();
    if let Some(use_tree) = declaration.strip_prefix("use") {
        return Some(use_tree.trim());
    }
    declaration
        .find(" use ")
        .map(|idx| declaration[idx + " use ".len()..].trim())
}

fn expand_rust_use_aliases(use_tree: &str, prefix: &str, aliases: &mut Vec<(String, String)>) {
    let use_tree = use_tree.trim();
    if use_tree.is_empty() {
        return;
    }

    if let Some(open_index) = find_top_level_char(use_tree, '{') {
        let before_group = use_tree[..open_index].trim().trim_end_matches("::");
        let Some(close_index) = find_matching_brace(use_tree, open_index) else {
            return;
        };

        let group_prefix = join_rust_path(prefix, before_group);
        let group_body = &use_tree[open_index + 1..close_index];
        for item in split_top_level_commas(group_body) {
            expand_rust_use_aliases(item, &group_prefix, aliases);
        }
        return;
    }

    let Some((path, alias)) = split_rust_alias(use_tree) else {
        return;
    };
    let alias = alias.trim();
    if alias.is_empty() || alias == "_" {
        return;
    }

    let target_path = join_rust_path(prefix, path);
    if target_path.is_empty() {
        return;
    }

    aliases.push((target_path, alias.to_string()));
}

fn expand_rust_use_tree(use_tree: &str, prefix: &str, imports: &mut Vec<String>) {
    let use_tree = use_tree.trim();
    if use_tree.is_empty() {
        return;
    }

    if let Some(open_index) = find_top_level_char(use_tree, '{') {
        let before_group = use_tree[..open_index].trim().trim_end_matches("::");
        let Some(close_index) = find_matching_brace(use_tree, open_index) else {
            return;
        };

        let group_prefix = join_rust_path(prefix, before_group);
        let group_body = &use_tree[open_index + 1..close_index];
        for item in split_top_level_commas(group_body) {
            expand_rust_use_tree(item, &group_prefix, imports);
        }
        return;
    }

    let item = use_tree
        .split_once(" as ")
        .map_or(use_tree, |(path, _alias)| path)
        .trim();
    let import_path = join_rust_path(prefix, item);
    if import_path.is_empty() {
        return;
    }

    if let Some(module_path) = import_path.strip_suffix("::*") {
        imports.push(module_path.to_string());
    } else if let Some(module_path) = import_path.strip_suffix("::self") {
        imports.push(module_path.to_string());
    } else {
        imports.push(import_path);
    }
}

fn split_rust_alias(input: &str) -> Option<(&str, &str)> {
    for (idx, _) in input.match_indices("as") {
        let before_is_boundary = input[..idx]
            .chars()
            .next_back()
            .is_none_or(|ch| !is_rust_ident_char(ch));
        let after_is_boundary = input[idx + "as".len()..]
            .chars()
            .next()
            .is_none_or(|ch| !is_rust_ident_char(ch));
        if before_is_boundary && after_is_boundary {
            return Some((&input[..idx], &input[idx + "as".len()..]));
        }
    }

    None
}

fn is_rust_ident_char(ch: char) -> bool {
    ch == '_' || ch.is_ascii_alphanumeric()
}

fn find_top_level_char(input: &str, target: char) -> Option<usize> {
    let mut depth = 0usize;
    for (idx, ch) in input.char_indices() {
        match ch {
            _ if depth == 0 && ch == target => return Some(idx),
            '{' => depth += 1,
            '}' => depth = depth.saturating_sub(1),
            _ => {}
        }
    }
    None
}

fn find_matching_brace(input: &str, open_index: usize) -> Option<usize> {
    let mut depth = 0usize;
    for (idx, ch) in input
        .char_indices()
        .skip_while(|(idx, _)| *idx < open_index)
    {
        match ch {
            '{' => depth += 1,
            '}' => {
                depth = depth.saturating_sub(1);
                if depth == 0 {
                    return Some(idx);
                }
            }
            _ => {}
        }
    }
    None
}

fn split_top_level_commas(input: &str) -> Vec<&str> {
    let mut parts = Vec::new();
    let mut depth = 0usize;
    let mut start = 0usize;

    for (idx, ch) in input.char_indices() {
        match ch {
            '{' => depth += 1,
            '}' => depth = depth.saturating_sub(1),
            ',' if depth == 0 => {
                let part = input[start..idx].trim();
                if !part.is_empty() {
                    parts.push(part);
                }
                start = idx + ch.len_utf8();
            }
            _ => {}
        }
    }

    let part = input[start..].trim();
    if !part.is_empty() {
        parts.push(part);
    }

    parts
}

fn join_rust_path(prefix: &str, item: &str) -> String {
    let item = item.trim().trim_start_matches("::").trim_end_matches("::");
    let prefix = prefix.trim().trim_end_matches("::");

    if item.is_empty() || item == "self" {
        prefix.to_string()
    } else if prefix.is_empty() {
        item.to_string()
    } else {
        format!("{prefix}::{item}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_traverse_rust_ast() {
        let source = b"
            use crate::db::connection;
            use crate::utils;
            use crate::db::{pool, transaction};
            use crate::{auth::session, config};
            mod test_helpers;
            mod local_implementation {
                fn local() {}
            }
        ";
        let mut parser = tree_sitter::Parser::new();
        parser.set_language(tree_sitter_rust::language()).unwrap();
        let tree = parser.parse(source, None).unwrap();
        let mut imports = Vec::new();
        traverse_rust_ast(tree.root_node(), source, &mut imports);
        assert!(imports.contains(&"crate::db::connection".to_string()));
        assert!(imports.contains(&"crate::utils".to_string()));
        assert!(imports.contains(&"crate::db::pool".to_string()));
        assert!(imports.contains(&"crate::db::transaction".to_string()));
        assert!(imports.contains(&"crate::auth::session".to_string()));
        assert!(imports.contains(&"crate::config".to_string()));
        assert!(imports.contains(&"test_helpers".to_string()));
        assert!(!imports.contains(&"local_implementation".to_string()));
    }

    #[test]
    fn test_traverse_rust_use_aliases_handles_grouped_reexports() {
        let source = b"
            pub use {
                helper_crate as helper,
                another_crate::module as secondary,
                crate::{local as local_alias},
            };
        ";
        let mut parser = tree_sitter::Parser::new();
        parser.set_language(tree_sitter_rust::language()).unwrap();
        let tree = parser.parse(source, None).unwrap();
        let mut aliases = Vec::new();
        traverse_rust_use_aliases(tree.root_node(), source, &mut aliases);

        assert!(aliases.contains(&("helper_crate".to_string(), "helper".to_string())));
        assert!(aliases.contains(&("another_crate::module".to_string(), "secondary".to_string())));
        assert!(aliases.contains(&("crate::local".to_string(), "local_alias".to_string())));
    }
}
