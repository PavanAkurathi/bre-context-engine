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
        let mut parser = tree_sitter::Parser::new();
        if parser
            .set_language(tree_sitter_javascript::language())
            .is_ok()
        {
            if let Some(tree) = parser.parse(&content, None) {
                let mut imports = Vec::new();
                traverse_js_ast(tree.root_node(), &content, &mut imports);
                for import in imports {
                    if let Some(path) = resolve_js_dependency_with_aliases(
                        file_path, &import, repo_root, js_aliases,
                    ) {
                        resolved.push(path);
                    }
                }
            }
        }
    }

    resolved
}
