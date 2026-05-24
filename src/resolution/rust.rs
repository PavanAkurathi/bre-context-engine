use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};

use crate::ast::rust::traverse_rust_use_aliases;
use crate::utils::{is_file_exact_case, normalize_pathbuf};

pub fn get_crate_roots(repo_root: &Path) -> Vec<(String, PathBuf)> {
    let mut crates = Vec::new();
    let root_cargo = repo_root.join("Cargo.toml");
    if let Some(name) = package_name_from_cargo(&root_cargo) {
        crates.push((name.replace('-', "_"), crate_source_root(&root_cargo)));
    }
    for entry in walkdir::WalkDir::new(repo_root)
        .min_depth(1)
        .max_depth(3)
        .into_iter()
        .filter_map(|e| e.ok())
    {
        if entry.file_type().is_dir() {
            let cargo_toml = entry.path().join("Cargo.toml");
            if cargo_toml.exists() && cargo_toml != root_cargo {
                let name = package_name_from_cargo(&cargo_toml)
                    .unwrap_or_else(|| entry.file_name().to_string_lossy().to_string())
                    .replace('-', "_");
                crates.push((name, crate_source_root(&cargo_toml)));
            }
        }
    }
    crates
}

pub fn package_name_from_cargo(cargo_toml: &Path) -> Option<String> {
    cargo_value_from_section(cargo_toml, "package", "name")
}

pub fn lib_path_from_cargo(cargo_toml: &Path) -> Option<String> {
    cargo_value_from_section(cargo_toml, "lib", "path")
}

pub fn cargo_value_from_section(cargo_toml: &Path, section: &str, key: &str) -> Option<String> {
    let content = fs::read_to_string(cargo_toml).ok()?;
    let mut in_section = false;

    for raw_line in content.lines() {
        let line = raw_line
            .split_once('#')
            .map_or(raw_line, |(before_comment, _)| before_comment)
            .trim();

        if line.starts_with('[') && line.ends_with(']') {
            let header = line.trim_start_matches('[').trim_end_matches(']').trim();
            in_section = header == section;
            continue;
        }

        if !in_section {
            continue;
        }

        let Some((lhs, rhs)) = line.split_once('=') else {
            continue;
        };
        if lhs.trim() == key {
            return Some(rhs.trim().trim_matches('"').trim_matches('\'').to_string());
        }
    }
    None
}

pub fn crate_source_root(cargo_toml: &Path) -> PathBuf {
    let manifest_dir = cargo_toml.parent().unwrap_or_else(|| Path::new("."));
    if let Some(lib_path) = lib_path_from_cargo(cargo_toml) {
        let lib_file = normalize_pathbuf(&manifest_dir.join(lib_path));
        if let Some(parent) = lib_file.parent() {
            return parent.to_path_buf();
        }
    }

    let default_src = manifest_dir.join("src");
    if default_src.exists() {
        default_src
    } else {
        manifest_dir.to_path_buf()
    }
}

pub fn resolve_rust_module_path(source_root: &Path, parts: &[&str]) -> Option<PathBuf> {
    for len in (1..=parts.len()).rev() {
        let subpath = parts[..len].join("/");
        let file_candidate = source_root.join(format!("{subpath}.rs"));
        if is_file_exact_case(&file_candidate) {
            return Some(file_candidate);
        }
        let mod_candidate = source_root.join(format!("{subpath}/mod.rs"));
        if is_file_exact_case(&mod_candidate) {
            return Some(mod_candidate);
        }
    }

    if parts.len() <= 1 {
        let lib_candidate = source_root.join("lib.rs");
        if is_file_exact_case(&lib_candidate) {
            return Some(lib_candidate);
        }

        let main_candidate = source_root.join("main.rs");
        if is_file_exact_case(&main_candidate) {
            return Some(main_candidate);
        }
    }

    None
}

pub fn rust_crate_aliases(repo_root: &Path) -> HashMap<String, String> {
    let mut aliases = HashMap::new();
    let mut parser = tree_sitter::Parser::new();
    if parser.set_language(tree_sitter_rust::language()).is_err() {
        return aliases;
    }

    for (_crate_name, source_root) in get_crate_roots(repo_root) {
        let lib_rs = source_root.join("lib.rs");
        let Ok(content) = fs::read_to_string(lib_rs) else {
            continue;
        };

        let Some(tree) = parser.parse(content.as_bytes(), None) else {
            continue;
        };

        let mut discovered_aliases = Vec::new();
        traverse_rust_use_aliases(
            tree.root_node(),
            content.as_bytes(),
            &mut discovered_aliases,
        );
        for (target, alias) in discovered_aliases {
            if let Some(target_crate) = target.split("::").next() {
                aliases.insert(alias, target_crate.to_string());
            }
        }
    }

    aliases
}

pub fn resolve_rust_dependency(
    active_file: &Path,
    import_str: &str,
    repo_root: &Path,
) -> Option<PathBuf> {
    let parent = active_file.parent()?;

    // 1. Resolve workspace sibling crates first
    let workspace_crates = get_crate_roots(repo_root);
    let crate_aliases = rust_crate_aliases(repo_root);
    for (crate_name, crate_path) in workspace_crates {
        let mut possible_names = vec![crate_name.clone()];
        possible_names.extend(
            crate_aliases
                .iter()
                .filter_map(|(alias, target)| (target == &crate_name).then_some(alias.clone())),
        );

        for possible_name in possible_names {
            let prefix = format!("{}::", possible_name);
            if import_str.starts_with(&prefix) {
                let clean = import_str.strip_prefix(&prefix)?;
                let parts: Vec<&str> = clean.split("::").collect();
                if let Some(path) = resolve_rust_module_path(&crate_path, &parts) {
                    return Some(path);
                }
            }
        }
    }

    // 2. Resolve local modules within the same crate
    let mut crate_root = None;
    let mut current = Some(parent);
    while let Some(path) = current {
        let manifest = path.join("Cargo.toml");
        if manifest.exists() {
            crate_root = Some(crate_source_root(&manifest));
            break;
        }
        current = path.parent();
    }
    let crate_root = crate_root.unwrap_or_else(|| parent.to_path_buf());

    if import_str.starts_with("crate::") {
        let clean = import_str.strip_prefix("crate::")?;
        let parts: Vec<&str> = clean.split("::").collect();
        if let Some(path) = resolve_rust_module_path(&crate_root, &parts) {
            return Some(path);
        }
    } else if import_str.starts_with("super::") {
        let mut current_dir = parent.to_path_buf();
        let mut clean = import_str;
        while clean.starts_with("super::") {
            current_dir = current_dir.parent()?.to_path_buf();
            clean = clean.strip_prefix("super::")?;
        }
        let parts: Vec<&str> = clean.split("::").collect();
        if let Some(path) = resolve_rust_module_path(&current_dir, &parts) {
            return Some(path);
        }
    } else {
        let clean = import_str.strip_prefix("self::").unwrap_or(import_str);
        let parts: Vec<&str> = clean.split("::").collect();
        if let Some(path) = resolve_rust_module_path(parent, &parts) {
            return Some(path);
        }
    }

    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ast::find_ast_dependencies;

    #[test]
    fn test_rust_workspace_resolution() {
        let temp_dir = tempfile::tempdir().unwrap();
        let root = temp_dir.path();

        // Create workspace Cargo.toml
        fs::write(
            root.join("Cargo.toml"),
            "[workspace]\nmembers = [\"crate_a\", \"crate_b\"]",
        )
        .unwrap();

        // Create crate_a
        let crate_a = root.join("crate_a");
        fs::create_dir_all(crate_a.join("src")).unwrap();
        fs::write(crate_a.join("Cargo.toml"), "[package]\nname = \"crate_a\"").unwrap();
        let file_a = crate_a.join("src/lib.rs");
        fs::write(&file_a, "use crate_b::helper;").unwrap();

        // Create crate_b
        let crate_b = root.join("crate_b");
        fs::create_dir_all(crate_b.join("src")).unwrap();
        fs::write(crate_b.join("Cargo.toml"), "[package]\nname = \"crate_b\"").unwrap();
        let file_b = crate_b.join("src/helper.rs");
        fs::write(&file_b, "pub fn helper() {}").unwrap();

        // Test resolving import in crate_a pointing to crate_b
        let resolved = resolve_rust_dependency(&file_a, "crate_b::helper", root);
        assert!(resolved.is_some());
        let path = resolved.unwrap();
        assert!(path.ends_with("crate_b/src/helper.rs"));
    }

    #[test]
    fn test_rust_example_resolves_root_crate_dependencies() {
        let temp_dir = tempfile::tempdir().unwrap();
        let root = temp_dir.path();
        fs::write(
            root.join("Cargo.toml"),
            "[package]\nname = \"game-engine\"\nversion = \"0.1.0\"\n",
        )
        .unwrap();
        fs::create_dir_all(root.join("examples")).unwrap();
        fs::create_dir_all(root.join("src/input")).unwrap();
        fs::write(
            root.join("examples/input_test.rs"),
            "use game_engine::input::keyboard::KeyInput;\nfn main() {}\n",
        )
        .unwrap();
        fs::write(root.join("src/lib.rs"), "pub mod input;\n").unwrap();
        fs::write(root.join("src/input/mod.rs"), "pub mod keyboard;\n").unwrap();
        fs::write(root.join("src/input/keyboard.rs"), "pub struct KeyInput;\n").unwrap();

        let deps = find_ast_dependencies(&root.join("examples/input_test.rs"), root);

        assert!(deps
            .iter()
            .any(|path| path.ends_with("src/input/keyboard.rs")));
    }

    #[test]
    fn test_rust_workspace_crate_uses_lib_path_as_source_root() {
        let temp_dir = tempfile::tempdir().unwrap();
        let root = temp_dir.path();
        fs::write(
            root.join("Cargo.toml"),
            "[workspace]\nmembers = [\"crates/avian2d\"]\n",
        )
        .unwrap();
        fs::create_dir_all(root.join("crates/avian2d/examples")).unwrap();
        fs::create_dir_all(root.join("src")).unwrap();
        fs::write(
            root.join("crates/avian2d/Cargo.toml"),
            "[package]\nname = \"avian2d\"\n\n[lib]\npath = \"../../src/lib.rs\"\n",
        )
        .unwrap();
        fs::write(root.join("src/lib.rs"), "pub mod math;\n").unwrap();
        fs::write(root.join("src/math.rs"), "pub type Scalar = f32;\n").unwrap();
        fs::write(
            root.join("crates/avian2d/examples/collision_layers.rs"),
            "use avian2d::math::*;\nfn main() { let _: Scalar = 1.0; }\n",
        )
        .unwrap();

        let deps = find_ast_dependencies(
            &root.join("crates/avian2d/examples/collision_layers.rs"),
            root,
        );

        assert!(deps.iter().any(|path| path.ends_with("src/math.rs")));
    }

    #[test]
    fn test_rust_reexported_crate_alias_resolves_workspace_dependency() {
        let temp_dir = tempfile::tempdir().unwrap();
        let root = temp_dir.path();
        fs::write(
            root.join("Cargo.toml"),
            "[package]\nname = \"root-app\"\nversion = \"0.1.0\"\n\n[workspace]\nmembers = [\"crates/helper-crate\"]\n",
        )
        .unwrap();
        fs::create_dir_all(root.join("src")).unwrap();
        fs::create_dir_all(root.join("examples")).unwrap();
        fs::create_dir_all(root.join("crates/helper-crate/src")).unwrap();
        fs::write(root.join("src/lib.rs"), "pub use helper_crate as helper;\n").unwrap();
        fs::write(
            root.join("examples/alias.rs"),
            "use helper::Thing;\nfn main() { let _ = Thing; }\n",
        )
        .unwrap();
        fs::write(
            root.join("crates/helper-crate/Cargo.toml"),
            "[package]\nname = \"helper-crate\"\nversion = \"0.1.0\"\n",
        )
        .unwrap();
        fs::write(
            root.join("crates/helper-crate/src/lib.rs"),
            "pub struct Thing;\n",
        )
        .unwrap();

        let deps = find_ast_dependencies(&root.join("examples/alias.rs"), root);

        assert!(deps
            .iter()
            .any(|path| path.ends_with("crates/helper-crate/src/lib.rs")));
    }

    #[test]
    fn test_grouped_rust_reexported_crate_alias_resolves_workspace_dependency() {
        let temp_dir = tempfile::tempdir().unwrap();
        let root = temp_dir.path();
        fs::write(
            root.join("Cargo.toml"),
            "[package]\nname = \"root-app\"\nversion = \"0.1.0\"\n\n[workspace]\nmembers = [\"crates/helper-crate\", \"crates/secondary-crate\"]\n",
        )
        .unwrap();
        fs::create_dir_all(root.join("src")).unwrap();
        fs::create_dir_all(root.join("examples")).unwrap();
        fs::create_dir_all(root.join("crates/helper-crate/src")).unwrap();
        fs::create_dir_all(root.join("crates/secondary-crate/src")).unwrap();
        fs::write(
            root.join("src/lib.rs"),
            "pub use {\n    helper_crate as helper,\n    secondary_crate as secondary,\n};\n",
        )
        .unwrap();
        fs::write(
            root.join("examples/grouped_alias.rs"),
            "use helper::Thing;\nuse secondary::Other;\nfn main() { let _ = (Thing, Other); }\n",
        )
        .unwrap();
        fs::write(
            root.join("crates/helper-crate/Cargo.toml"),
            "[package]\nname = \"helper-crate\"\nversion = \"0.1.0\"\n",
        )
        .unwrap();
        fs::write(
            root.join("crates/helper-crate/src/lib.rs"),
            "pub struct Thing;\n",
        )
        .unwrap();
        fs::write(
            root.join("crates/secondary-crate/Cargo.toml"),
            "[package]\nname = \"secondary-crate\"\nversion = \"0.1.0\"\n",
        )
        .unwrap();
        fs::write(
            root.join("crates/secondary-crate/src/lib.rs"),
            "pub struct Other;\n",
        )
        .unwrap();

        let deps = find_ast_dependencies(&root.join("examples/grouped_alias.rs"), root);

        assert!(deps
            .iter()
            .any(|path| path.ends_with("crates/helper-crate/src/lib.rs")));
        assert!(deps
            .iter()
            .any(|path| path.ends_with("crates/secondary-crate/src/lib.rs")));
    }
}
