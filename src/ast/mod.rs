pub mod java;
pub mod javascript;
pub mod rust;

use std::collections::HashMap;
use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};

use crate::resolution::{
    resolve_java_dependency, resolve_js_dependency_with_aliases, resolve_rust_dependency,
};
pub use java::traverse_java_ast;
pub use javascript::traverse_js_ast;
pub use rust::traverse_rust_ast;

pub fn find_ast_dependencies(file_path: &Path, repo_root: &Path) -> Vec<PathBuf> {
    find_ast_dependencies_with_aliases(file_path, repo_root, &HashMap::new())
}

pub fn find_ast_dependencies_with_aliases(
    file_path: &Path,
    repo_root: &Path,
    js_aliases: &HashMap<String, Vec<String>>,
) -> Vec<PathBuf> {
    let mut file = match fs::File::open(file_path) {
        Ok(f) => f,
        Err(_) => return Vec::new(),
    };
    let mut content = Vec::new();
    if file.read_to_end(&mut content).is_err() {
        return Vec::new();
    }

    let extension = file_path
        .extension()
        .and_then(|ext| ext.to_str())
        .unwrap_or("");
    let mut resolved = Vec::new();

    if extension == "rs" {
        let mut parser = tree_sitter::Parser::new();
        if parser.set_language(tree_sitter_rust::language()).is_ok() {
            if let Some(tree) = parser.parse(&content, None) {
                let mut imports = Vec::new();
                traverse_rust_ast(tree.root_node(), &content, &mut imports);
                for import in imports {
                    if let Some(path) = resolve_rust_dependency(file_path, &import, repo_root) {
                        resolved.push(path);
                    }
                }
            }
        }
    } else if extension == "java" {
        let mut parser = tree_sitter::Parser::new();
        if parser.set_language(tree_sitter_java::language()).is_ok() {
            if let Some(tree) = parser.parse(&content, None) {
                let mut imports = Vec::new();
                traverse_java_ast(tree.root_node(), &content, &mut imports);
                for import in imports {
                    resolved.extend(resolve_java_dependency(file_path, &import, repo_root));
                }
            }
        }
    } else if matches!(extension, "js" | "jsx" | "ts" | "tsx") {
        let mut visited = std::collections::HashSet::new();
        resolve_js_file_dependencies_rec(
            file_path,
            repo_root,
            js_aliases,
            &mut visited,
            &mut resolved,
            true,
            0,
        );
    }

    resolved
}

fn resolve_js_file_dependencies_rec(
    file_path: &Path,
    repo_root: &Path,
    js_aliases: &HashMap<String, Vec<String>>,
    visited: &mut std::collections::HashSet<PathBuf>,
    resolved_paths: &mut Vec<PathBuf>,
    follow_imports: bool,
    depth: usize,
) {
    if depth > 3 {
        return;
    }
    let canonical = match fs::canonicalize(file_path) {
        Ok(p) => p,
        Err(_) => file_path.to_path_buf(),
    };
    if visited.contains(&canonical) {
        return;
    }
    visited.insert(canonical);

    let mut file = match fs::File::open(file_path) {
        Ok(f) => f,
        Err(_) => return,
    };
    let mut content = Vec::new();
    if file.read_to_end(&mut content).is_err() {
        return;
    }

    let extension = file_path
        .extension()
        .and_then(|ext| ext.to_str())
        .unwrap_or("");
    if !matches!(extension, "js" | "jsx" | "ts" | "tsx") {
        return;
    }

    let mut parser = tree_sitter::Parser::new();
    let lang = match extension {
        "ts" => tree_sitter_typescript::language_typescript(),
        "tsx" => tree_sitter_typescript::language_tsx(),
        _ => tree_sitter_javascript::language(),
    };
    if parser.set_language(lang).is_err() {
        return;
    }
    let Some(tree) = parser.parse(&content, None) else {
        return;
    };

    let mut imports = Vec::new();
    let mut re_exports = Vec::new();
    javascript::traverse_js_ast_detailed(tree.root_node(), &content, &mut imports, &mut re_exports);

    if follow_imports {
        for import in imports {
            if let Some(path) = resolve_js_dependency_with_aliases(
                file_path, &import, repo_root, js_aliases,
            ) {
                let normalized = crate::utils::normalize_pathbuf(&path);
                if !resolved_paths.contains(&normalized) {
                    resolved_paths.push(normalized.clone());
                }
                resolve_js_file_dependencies_rec(
                    &normalized,
                    repo_root,
                    js_aliases,
                    visited,
                    resolved_paths,
                    false,
                    depth + 1,
                );
            }
        }
    }

    for re_export in re_exports {
        if let Some(path) = resolve_js_dependency_with_aliases(
            file_path, &re_export, repo_root, js_aliases,
        ) {
            let normalized = crate::utils::normalize_pathbuf(&path);
            if !resolved_paths.contains(&normalized) {
                resolved_paths.push(normalized.clone());
            }
            resolve_js_file_dependencies_rec(
                &normalized,
                repo_root,
                js_aliases,
                visited,
                resolved_paths,
                false,
                depth + 1,
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn test_transitive_barrel_resolution_typescript() {
        let temp_dir = tempdir().unwrap();
        let root = temp_dir.path();
        std::fs::create_dir_all(root.join("src/_namespaces")).unwrap();

        // 1. Write core implementation files
        std::fs::write(root.join("src/parser.ts"), "export class Parser {}").unwrap();
        std::fs::write(root.join("src/types.ts"), "export interface Type {}").unwrap();

        // 2. Write the barrel namespace file (src/_namespaces/ts.ts)
        std::fs::write(
            root.join("src/_namespaces/ts.ts"),
            r#"
                export * from "../parser";
                export * from "../types";
            "#,
        )
        .unwrap();

        // 3. Write active file (src/scanner.ts)
        std::fs::write(
            root.join("src/scanner.ts"),
            r#"
                import { Parser } from "./_namespaces/ts";
                export class Scanner {}
            "#,
        )
        .unwrap();

        let deps = find_ast_dependencies(&root.join("src/scanner.ts"), root);

        // Verify it resolves BOTH the direct barrel import AND the transitive re-exports
        let rel_deps: Vec<String> = deps
            .iter()
            .map(|p| p.strip_prefix(root).unwrap().to_string_lossy().to_string())
            .collect();

        assert!(rel_deps.contains(&"src/_namespaces/ts.ts".to_string()));
        assert!(rel_deps.contains(&"src/parser.ts".to_string()));
        assert!(rel_deps.contains(&"src/types.ts".to_string()));
    }
}
