use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};

use crate::utils::is_file_exact_case;

const JS_RESOLUTION_EXTENSIONS: &[&str] = &["js", "ts", "jsx", "tsx", "json"];

fn resolve_js_path_candidate(resolved_path: &Path) -> Option<PathBuf> {
    if is_file_exact_case(resolved_path) {
        return Some(resolved_path.to_path_buf());
    }

    for ext in JS_RESOLUTION_EXTENSIONS {
        let candidate = resolved_path.with_extension(ext);
        if is_file_exact_case(&candidate) {
            return Some(candidate);
        }
    }

    for ext in JS_RESOLUTION_EXTENSIONS {
        let candidate = resolved_path.join(format!("index.{ext}"));
        if is_file_exact_case(&candidate) {
            return Some(candidate);
        }
    }

    None
}

fn alias_match(pattern: &str, import_str: &str) -> Option<String> {
    let pattern = pattern.trim();
    if pattern.is_empty() {
        return None;
    }

    if let Some((prefix, suffix_pattern)) = pattern.split_once('*') {
        if import_str.starts_with(prefix) && import_str.ends_with(suffix_pattern) {
            let suffix_end = import_str.len().saturating_sub(suffix_pattern.len());
            if suffix_end >= prefix.len() {
                let suffix = &import_str[prefix.len()..suffix_end];
                return Some(suffix.trim_start_matches('/').to_string());
            }
        }
        return None;
    }

    if import_str == pattern {
        return Some(String::new());
    }

    let slash_prefix = format!("{}/", pattern.trim_end_matches('/'));
    import_str
        .strip_prefix(&slash_prefix)
        .map(ToString::to_string)
}

fn apply_alias_target(target: &str, suffix: &str) -> PathBuf {
    let target = target.trim();
    let target = target.strip_prefix("./").unwrap_or(target);

    if target.contains('*') {
        PathBuf::from(target.replace('*', suffix))
    } else if suffix.is_empty() {
        PathBuf::from(target)
    } else {
        Path::new(target).join(suffix)
    }
}

pub fn resolve_configured_js_aliases(
    import_str: &str,
    repo_root: &Path,
    aliases: &HashMap<String, Vec<String>>,
) -> Option<PathBuf> {
    let mut alias_entries: Vec<_> = aliases.iter().collect();
    alias_entries.sort_by(|(left, _), (right, _)| {
        right
            .len()
            .cmp(&left.len())
            .then_with(|| left.as_str().cmp(right.as_str()))
    });

    for (alias, targets) in alias_entries {
        let Some(suffix) = alias_match(alias, import_str) else {
            continue;
        };

        for target in targets {
            let target_path = apply_alias_target(target, &suffix);
            let resolved_path = repo_root.join(target_path);
            if let Some(candidate) = resolve_js_path_candidate(&resolved_path) {
                return Some(candidate);
            }
        }
    }

    None
}

pub fn resolve_tsconfig_paths(import_str: &str, repo_root: &Path) -> Option<PathBuf> {
    let tsconfig_path = repo_root.join("tsconfig.json");
    let jsconfig_path = repo_root.join("jsconfig.json");
    let config_file = if tsconfig_path.exists() {
        Some(tsconfig_path)
    } else if jsconfig_path.exists() {
        Some(jsconfig_path)
    } else {
        None
    };

    let config_file = config_file?;
    let content = fs::read_to_string(&config_file).ok()?;
    let json: serde_json::Value = serde_json::from_str(&content).ok()?;

    let base_url = json
        .pointer("/compilerOptions/baseUrl")
        .and_then(|v| v.as_str())
        .unwrap_or(".");

    let paths = json
        .pointer("/compilerOptions/paths")
        .and_then(|v| v.as_object())?;

    for (alias, mappings) in paths {
        let Some(suffix) = alias_match(alias, import_str) else {
            continue;
        };
        let Some(mappings_arr) = mappings.as_array() else {
            continue;
        };

        for mapping in mappings_arr {
            let Some(raw_mapping) = mapping.as_str() else {
                continue;
            };
            let mapped_path = apply_alias_target(raw_mapping, &suffix);
            let resolved_path = repo_root.join(base_url).join(mapped_path);
            if let Some(candidate) = resolve_js_path_candidate(&resolved_path) {
                return Some(candidate);
            }
        }
    }
    None
}

pub fn resolve_package_json_imports(import_str: &str, repo_root: &Path) -> Option<PathBuf> {
    let pkg_path = repo_root.join("package.json");
    if !pkg_path.exists() {
        return None;
    }
    let content = fs::read_to_string(&pkg_path).ok()?;
    let json: serde_json::Value = serde_json::from_str(&content).ok()?;
    let imports = json.get("imports")?.as_object()?;

    for (key, val) in imports {
        let val_str = val.as_str()?;
        // Case 1: Exact match (e.g. "#db" -> "./src/db/index.js")
        if key == import_str {
            let target = val_str.strip_prefix("./").unwrap_or(val_str);
            let candidate = repo_root.join(target);
            if let Some(candidate) = resolve_js_path_candidate(&candidate) {
                return Some(candidate);
            }
        }
        // Case 2: Wildcard match (e.g. "#utils/*" -> "./src/utils/*")
        if key.contains('*') {
            let Some(suffix) = alias_match(key, import_str) else {
                continue;
            };
            let target_path = apply_alias_target(val_str, &suffix);
            let resolved_path = repo_root.join(target_path);
            if let Some(candidate) = resolve_js_path_candidate(&resolved_path) {
                return Some(candidate);
            }
        }
    }
    None
}

pub fn resolve_vite_aliases(import_str: &str, repo_root: &Path) -> Option<PathBuf> {
    let vite_ts = repo_root.join("vite.config.ts");
    let vite_js = repo_root.join("vite.config.js");
    let config_path = if vite_ts.exists() {
        Some(vite_ts)
    } else if vite_js.exists() {
        Some(vite_js)
    } else {
        None
    };
    let config_path = config_path?;
    let content = fs::read_to_string(&config_path).ok()?;

    // Regex for: find: '@', replacement: path.resolve(__dirname, 'src')
    if let Ok(re_find_replacement) = regex::Regex::new(
        r#"find:\s*['"]([^'"]+)['"]\s*,\s*replacement:\s*(?:path\.resolve\(__dirname,\s*)?['"]([^'"]+)['"]"#,
    ) {
        for cap in re_find_replacement.captures_iter(&content) {
            let find = &cap[1];
            let replacement = &cap[2];
            let find_clean = find.replace("*", "");
            if import_str.starts_with(&find_clean) {
                if let Some(suffix) = import_str.strip_prefix(&find_clean) {
                    let suffix_clean = suffix.strip_prefix('/').unwrap_or(suffix);
                    let replacement_clean = replacement.strip_prefix("./").unwrap_or(replacement);
                    let resolved_path = repo_root.join(replacement_clean).join(suffix_clean);
                    if let Some(candidate) = resolve_js_path_candidate(&resolved_path) {
                        return Some(candidate);
                    }
                }
            }
        }
    }

    // Regex for: '@': path.resolve(__dirname, './src') or '@': './src'
    if let Ok(re_key_value) = regex::Regex::new(
        r#"['"]([^'"]+)['"]\s*:\s*(?:path\.resolve\(__dirname,\s*)?['"]([^'"]+)['"]"#,
    ) {
        for cap in re_key_value.captures_iter(&content) {
            let key = &cap[1];
            let val = &cap[2];
            if [
                "target",
                "host",
                "changeOrigin",
                "rewrite",
                "input",
                "output",
                "name",
            ]
            .contains(&key)
            {
                continue;
            }
            let key_clean = key.replace("*", "");
            if import_str.starts_with(&key_clean) {
                if let Some(suffix) = import_str.strip_prefix(&key_clean) {
                    let suffix_clean = suffix.strip_prefix('/').unwrap_or(suffix);
                    let val_clean = val.strip_prefix("./").unwrap_or(val);
                    let resolved_path = repo_root.join(val_clean).join(suffix_clean);
                    if let Some(candidate) = resolve_js_path_candidate(&resolved_path) {
                        return Some(candidate);
                    }
                }
            }
        }
    }

    None
}

pub fn resolve_js_dependency(
    active_file: &Path,
    import_str: &str,
    repo_root: &Path,
) -> Option<PathBuf> {
    resolve_js_dependency_with_aliases(active_file, import_str, repo_root, &HashMap::new())
}

pub fn resolve_js_dependency_with_aliases(
    active_file: &Path,
    import_str: &str,
    repo_root: &Path,
    aliases: &HashMap<String, Vec<String>>,
) -> Option<PathBuf> {
    if let Some(resolved) = resolve_configured_js_aliases(import_str, repo_root, aliases) {
        return Some(resolved);
    }

    if let Some(resolved) = resolve_tsconfig_paths(import_str, repo_root) {
        return Some(resolved);
    }

    if let Some(resolved) = resolve_package_json_imports(import_str, repo_root) {
        return Some(resolved);
    }

    if let Some(resolved) = resolve_vite_aliases(import_str, repo_root) {
        return Some(resolved);
    }

    if !import_str.starts_with('.') {
        return None;
    }

    let parent = active_file.parent()?;
    let resolved_path = parent.join(import_str);

    resolve_js_path_candidate(&resolved_path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_tsconfig_alias_resolution() {
        let temp_dir = tempfile::tempdir().unwrap();
        let root = temp_dir.path();

        let tsconfig = r#"{
            "compilerOptions": {
                "baseUrl": ".",
                "paths": {
                    "@/components/*": ["src/components/*"],
                    "~/*": ["*"]
                }
            }
        }"#;

        fs::write(root.join("tsconfig.json"), tsconfig).unwrap();
        fs::create_dir_all(root.join("src/components")).unwrap();
        fs::write(
            root.join("src/components/Button.ts"),
            "export const Button = () => {};",
        )
        .unwrap();

        let resolved = resolve_tsconfig_paths("@/components/Button", root);
        assert!(resolved.is_some());
        let path = resolved.unwrap();
        assert!(path.ends_with("src/components/Button.ts"));
    }

    #[test]
    fn test_configured_alias_resolution_handles_dynamic_vite_style_values() {
        let temp_dir = tempfile::tempdir().unwrap();
        let root = temp_dir.path();

        fs::create_dir_all(root.join("src/components")).unwrap();
        fs::write(
            root.join("src/components/Card.tsx"),
            "export const Card = () => {};",
        )
        .unwrap();

        let mut aliases = HashMap::new();
        aliases.insert("@/*".to_string(), vec!["src/*".to_string()]);
        aliases.insert(
            "@components/*".to_string(),
            vec!["src/components/*".to_string()],
        );

        let active_file = root.join("src/App.tsx");
        let resolved =
            resolve_js_dependency_with_aliases(&active_file, "@/components/Card", root, &aliases);
        assert!(resolved.is_some());
        assert!(resolved.unwrap().ends_with("src/components/Card.tsx"));

        let resolved_specific =
            resolve_js_dependency_with_aliases(&active_file, "@components/Card", root, &aliases);
        assert!(resolved_specific.is_some());
        assert!(resolved_specific
            .unwrap()
            .ends_with("src/components/Card.tsx"));
    }

    #[test]
    fn test_pkg_json_and_vite_fallback() {
        let temp_dir = tempfile::tempdir().unwrap();
        let root = temp_dir.path();

        // 1. package.json imports field
        let pkg_content = r##"{
            "name": "test-pkg",
            "imports": {
                "#utils/*": "./src/utils/*",
                "#db": "./src/db/connection.ts"
            }
        }"##;
        fs::write(root.join("package.json"), pkg_content).unwrap();

        fs::create_dir_all(root.join("src/utils")).unwrap();
        fs::create_dir_all(root.join("src/db")).unwrap();
        fs::write(
            root.join("src/utils/math.ts"),
            "export const add = () => {}",
        )
        .unwrap();
        fs::write(root.join("src/db/connection.ts"), "export const db = {}").unwrap();

        let active_file = root.join("src/index.ts");

        let resolved_wildcard = resolve_js_dependency(&active_file, "#utils/math", root);
        assert!(resolved_wildcard.is_some());
        assert!(resolved_wildcard.unwrap().ends_with("src/utils/math.ts"));

        let resolved_exact = resolve_js_dependency(&active_file, "#db", root);
        assert!(resolved_exact.is_some());
        assert!(resolved_exact.unwrap().ends_with("src/db/connection.ts"));

        // 2. vite.config.js alias mapping
        let vite_content = r#"
            import { defineConfig } from 'vite';
            import path from 'path';
            export default defineConfig({
                resolve: {
                    alias: {
                        '@components': path.resolve(__dirname, './src/components'),
                        '~': './src'
                    }
                }
            });
        "#;
        fs::write(root.join("vite.config.js"), vite_content).unwrap();

        fs::create_dir_all(root.join("src/components")).unwrap();
        fs::write(
            root.join("src/components/Card.tsx"),
            "export const Card = () => {}",
        )
        .unwrap();

        let resolved_vite_key_val = resolve_js_dependency(&active_file, "@components/Card", root);
        assert!(resolved_vite_key_val.is_some());
        assert!(resolved_vite_key_val
            .unwrap()
            .ends_with("src/components/Card.tsx"));
    }
}
