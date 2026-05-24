use std::fs;
use std::path::{Path, PathBuf};

use crate::utils::is_file_exact_case;

const JAVA_MANIFESTS: &[&str] = &[
    "pom.xml",
    "build.gradle",
    "build.gradle.kts",
    "settings.gradle",
    "settings.gradle.kts",
];

const JAVA_SOURCE_DIRS: &[&str] = &[
    "src/main/java",
    "src/test/java",
    "src/integrationTest/java",
    "src/it/java",
    "src/generated/java",
    "src/generated-sources/java",
];

pub fn resolve_java_dependency(
    active_file: &Path,
    import_str: &str,
    repo_root: &Path,
) -> Vec<PathBuf> {
    let import_str = import_str.trim();
    if import_str.is_empty() || import_str.starts_with("java.") || import_str.starts_with("javax.")
    {
        return Vec::new();
    }

    let source_roots = java_source_roots(active_file, repo_root);
    if import_str.ends_with(".*") {
        let package_path = import_str.trim_end_matches(".*").replace('.', "/");
        return source_roots
            .iter()
            .flat_map(|source_root| java_files_in_package(&source_root.join(&package_path)))
            .collect();
    }

    let parts: Vec<&str> = import_str.split('.').collect();
    for len in (1..=parts.len()).rev() {
        let candidate_rel = format!("{}.java", parts[..len].join("/"));
        for source_root in &source_roots {
            let candidate = source_root.join(&candidate_rel);
            if is_file_exact_case(&candidate) {
                return vec![candidate];
            }
        }
    }

    Vec::new()
}

fn java_source_roots(active_file: &Path, repo_root: &Path) -> Vec<PathBuf> {
    let mut roots = Vec::new();

    if let Some(project_root) = nearest_java_project_root(active_file, repo_root) {
        push_java_source_roots(&project_root, &mut roots);
    }

    push_java_source_roots(repo_root, &mut roots);

    for entry in walkdir::WalkDir::new(repo_root)
        .min_depth(1)
        .max_depth(5)
        .into_iter()
        .filter_entry(|entry| should_descend_java_search(entry.path()))
        .filter_map(Result::ok)
        .filter(|entry| entry.file_type().is_file())
    {
        if is_java_manifest(entry.path()) {
            if let Some(project_root) = entry.path().parent() {
                push_java_source_roots(project_root, &mut roots);
            }
        }
    }

    dedupe_existing_paths(roots)
}

fn nearest_java_project_root(active_file: &Path, repo_root: &Path) -> Option<PathBuf> {
    let mut current = active_file.parent();
    while let Some(dir) = current {
        if JAVA_MANIFESTS
            .iter()
            .any(|manifest| dir.join(manifest).is_file())
        {
            return Some(dir.to_path_buf());
        }
        if dir == repo_root {
            break;
        }
        current = dir.parent();
    }

    None
}

fn push_java_source_roots(project_root: &Path, roots: &mut Vec<PathBuf>) {
    for source_dir in JAVA_SOURCE_DIRS {
        roots.push(project_root.join(source_dir));
    }
}

fn dedupe_existing_paths(paths: Vec<PathBuf>) -> Vec<PathBuf> {
    let mut deduped = Vec::new();
    for path in paths {
        if path.is_dir() && !deduped.iter().any(|existing| existing == &path) {
            deduped.push(path);
        }
    }
    deduped
}

fn is_java_manifest(path: &Path) -> bool {
    path.file_name()
        .and_then(|name| name.to_str())
        .is_some_and(|name| JAVA_MANIFESTS.contains(&name))
}

fn should_descend_java_search(path: &Path) -> bool {
    let Some(name) = path.file_name().and_then(|name| name.to_str()) else {
        return true;
    };

    !matches!(
        name,
        ".git" | "target" | "build" | "out" | ".gradle" | "node_modules"
    )
}

fn java_files_in_package(package_dir: &Path) -> Vec<PathBuf> {
    let Ok(entries) = fs::read_dir(package_dir) else {
        return Vec::new();
    };

    entries
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| {
            path.is_file()
                && path
                    .extension()
                    .and_then(|ext| ext.to_str())
                    .is_some_and(|ext| ext == "java")
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_java_maven_import_resolution() {
        let temp_dir = tempfile::tempdir().unwrap();
        let root = temp_dir.path();
        fs::create_dir_all(root.join("src/main/java/com/acme/web")).unwrap();
        fs::create_dir_all(root.join("src/main/java/com/acme/service")).unwrap();
        fs::write(root.join("pom.xml"), "<project />").unwrap();
        let active_file = root.join("src/main/java/com/acme/web/Controller.java");
        fs::write(&active_file, "class Controller {}").unwrap();
        fs::write(
            root.join("src/main/java/com/acme/service/UserService.java"),
            "class UserService {}",
        )
        .unwrap();

        let deps = resolve_java_dependency(&active_file, "com.acme.service.UserService", root);

        assert!(deps
            .iter()
            .any(|path| path.ends_with("src/main/java/com/acme/service/UserService.java")));
    }

    #[test]
    fn test_java_static_import_resolution_resolves_owning_class() {
        let temp_dir = tempfile::tempdir().unwrap();
        let root = temp_dir.path();
        fs::create_dir_all(root.join("src/main/java/com/acme/web")).unwrap();
        fs::create_dir_all(root.join("src/main/java/com/acme/config")).unwrap();
        fs::write(root.join("build.gradle"), "plugins { id 'java' }").unwrap();
        let active_file = root.join("src/main/java/com/acme/web/Controller.java");
        fs::write(&active_file, "class Controller {}").unwrap();
        fs::write(
            root.join("src/main/java/com/acme/config/Flags.java"),
            "class Flags { static final boolean ENABLED = true; }",
        )
        .unwrap();

        let deps = resolve_java_dependency(&active_file, "com.acme.config.Flags.ENABLED", root);

        assert!(deps
            .iter()
            .any(|path| path.ends_with("src/main/java/com/acme/config/Flags.java")));
    }
}
