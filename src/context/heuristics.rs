use git2::{Repository, Status, StatusOptions};
use regex::Regex;
use std::collections::{BTreeSet, HashMap, HashSet};
use std::fs;
use std::path::Path;
use std::sync::OnceLock;
use walkdir::{DirEntry, WalkDir};

use crate::config::BreConfig;
use crate::utils::normalize_path;
use crate::weights::HeuristicWeights;

const MANIFEST_FILE_NAMES: &[&str] = &[
    "Cargo.toml",
    "package.json",
    "pyproject.toml",
    "go.mod",
    "pom.xml",
    "build.gradle",
    "build.gradle.kts",
    "settings.gradle",
    "settings.gradle.kts",
    "requirements.txt",
    "Pipfile",
    "poetry.lock",
    "setup.py",
    "setup.cfg",
    "CMakeLists.txt",
    "BUILD",
    "BUILD.bazel",
    "MODULE.bazel",
    "WORKSPACE",
    "WORKSPACE.bazel",
    "global.json",
    "Directory.Build.props",
    "Directory.Build.targets",
];

const MANIFEST_FILE_EXTENSIONS: &[&str] = &["csproj", "fsproj", "vbproj", "sln"];

pub fn validate_active_files(config: &BreConfig, root_path: &Path) -> Result<(), String> {
    for active_file in &config.active_files {
        let path = root_path.join(active_file);
        if !path.is_file() {
            return Err(format!(
                "invalid task contract: active file `{active_file}` does not exist or is not a file"
            ));
        }

        fs::File::open(&path).map_err(|err| {
            format!("invalid task contract: active file `{active_file}` is not readable: {err}")
        })?;
    }

    Ok(())
}

pub fn context_symbol_candidates(
    config: &BreConfig,
    root_path: &Path,
    min_active_occurrences: usize,
) -> HashSet<String> {
    let mut symbols = task_symbol_candidates(config);
    let active_source = active_text(config, root_path);
    for (symbol, count) in extract_symbol_candidate_counts(&active_source) {
        if count >= min_active_occurrences
            && !active_files_define_symbol(config, root_path, &symbol)
        {
            symbols.insert(symbol);
        }
    }

    symbols
}

pub fn task_symbol_candidates(config: &BreConfig) -> HashSet<String> {
    let mut task_text = String::new();
    task_text.push_str(&config.goal);
    task_text.push('\n');
    task_text.push_str(&config.acceptance_criteria.join("\n"));
    extract_symbol_candidates(&task_text)
}

pub fn active_text(config: &BreConfig, root_path: &Path) -> String {
    let mut text = String::new();
    for active_file in &config.active_files {
        if let Ok(content) = read_context_file(root_path, active_file) {
            text.push('\n');
            text.push_str(&content);
        }
    }
    text
}

pub fn active_files_define_symbol(config: &BreConfig, root_path: &Path, symbol: &str) -> bool {
    for active_file in &config.active_files {
        let path = root_path.join(active_file);
        let Ok(content) = read_context_file(root_path, active_file) else {
            continue;
        };
        let extension = path.extension().and_then(|ext| ext.to_str()).unwrap_or("");
        if file_defines_symbol(&content, symbol, extension) {
            return true;
        }
    }
    false
}

pub fn file_defines_any_symbol(
    root_path: &Path,
    rel_path: &str,
    symbols: &HashSet<String>,
) -> bool {
    if symbols.is_empty() {
        return false;
    }

    let path = root_path.join(rel_path);
    let Ok(content) = fs::read_to_string(&path) else {
        return false;
    };
    let extension = path.extension().and_then(|ext| ext.to_str()).unwrap_or("");

    symbols
        .iter()
        .any(|symbol| file_defines_symbol(&content, symbol, extension))
}

pub fn task_path_terms(config: &BreConfig) -> HashSet<String> {
    static WORD_RE: OnceLock<Regex> = OnceLock::new();
    let word_re =
        WORD_RE.get_or_init(|| Regex::new(r"[A-Za-z][A-Za-z0-9]+").expect("valid word regex"));
    const STOP_PATH_TERMS: &[&str] = &[
        "active",
        "agent",
        "behavior",
        "browser",
        "common",
        "context",
        "dispatch",
        "files",
        "handling",
        "implementation",
        "improve",
        "inspect",
        "models",
        "platform",
        "resolution",
        "service",
        "services",
        "should",
        "source",
        "workbench",
    ];

    let mut text = String::new();
    text.push_str(&config.goal);
    text.push('\n');
    text.push_str(&config.acceptance_criteria.join("\n"));
    text.push('\n');
    text.push_str(&config.active_files.join("\n"));

    word_re
        .find_iter(&text)
        .map(|m| m.as_str().to_ascii_lowercase())
        .filter(|word| word.len() >= 4)
        .filter(|word| !STOP_PATH_TERMS.contains(&word.as_str()))
        .collect()
}

pub fn path_matches_task_terms(path: &Path, terms: &HashSet<String>) -> bool {
    if terms.is_empty() {
        return true;
    }

    path.components().any(|component| {
        let component = component.as_os_str().to_string_lossy().to_ascii_lowercase();
        terms
            .iter()
            .any(|term| component.contains(term) || term.contains(&component))
    })
}

pub fn find_symbol_dependencies(
    config: &BreConfig,
    root_path: &Path,
    weights: &HeuristicWeights,
) -> HashMap<String, usize> {
    let symbols = context_symbol_candidates(config, root_path, 2);

    if symbols.is_empty() {
        return HashMap::new();
    }

    let active: BTreeSet<String> = config.active_files.iter().cloned().collect();
    let mut dependencies = HashMap::new();
    let mut symbol_files = Vec::new();

    for entry in WalkDir::new(root_path)
        .into_iter()
        .filter_entry(should_descend)
        .filter_map(Result::ok)
        .filter(|entry| entry.file_type().is_file())
    {
        if !is_symbol_search_file(entry.path()) {
            continue;
        }

        let relative_path = match entry.path().strip_prefix(root_path) {
            Ok(path) => path,
            Err(_) => continue,
        };
        let relative = normalize_path(relative_path);
        if active.contains(&relative) {
            continue;
        }
        if is_test_like_file(relative_path) && !task_mentions_tests(config) {
            continue;
        }
        if is_broad_context_dir(relative_path) && !task_mentions_tests(config) {
            continue;
        }

        symbol_files.push((
            entry.path().to_path_buf(),
            relative_path.to_path_buf(),
            relative,
        ));
    }

    let use_task_scoped_scan = symbol_files.len() > 1500;
    let task_terms = if use_task_scoped_scan {
        task_path_terms(config)
    } else {
        HashSet::new()
    };

    for (absolute_path, relative_path, relative) in symbol_files {
        if use_task_scoped_scan && !path_matches_task_terms(&relative_path, &task_terms) {
            continue;
        }

        let Ok(content) = fs::read_to_string(&absolute_path) else {
            continue;
        };
        let extension = absolute_path
            .extension()
            .and_then(|ext| ext.to_str())
            .unwrap_or("");

        let match_count = symbols
            .iter()
            .filter(|symbol| file_defines_symbol(&content, symbol, extension))
            .count();
        if match_count > 0 {
            dependencies.insert(
                relative,
                weights.symbol_hit_base + match_count.min(4) * weights.symbol_hit_step,
            );
        }
    }

    dependencies
}

pub fn find_active_manifest_files(config: &BreConfig, root_path: &Path) -> HashSet<String> {
    let mut manifests = HashSet::new();

    for active_file in &config.active_files {
        let mut current = root_path.join(active_file).parent().map(Path::to_path_buf);
        while let Some(dir) = current {
            if let Some(manifest) = find_manifest_in_dir(&dir) {
                if let Ok(relative) = manifest.strip_prefix(root_path) {
                    manifests.insert(normalize_path(relative));
                }
                break;
            }
            current = dir.parent().map(Path::to_path_buf);
        }
    }

    manifests
}

fn find_manifest_in_dir(dir: &Path) -> Option<std::path::PathBuf> {
    for manifest_name in MANIFEST_FILE_NAMES {
        let manifest = dir.join(manifest_name);
        if manifest.is_file() {
            return Some(manifest);
        }
    }

    let entries = fs::read_dir(dir).ok()?;
    for entry in entries.filter_map(Result::ok) {
        let path = entry.path();
        if !path.is_file() {
            continue;
        }
        let Some(extension) = path.extension().and_then(|ext| ext.to_str()) else {
            continue;
        };
        if MANIFEST_FILE_EXTENSIONS.contains(&extension) {
            return Some(path);
        }
    }

    None
}

pub fn is_symbol_search_file(path: &Path) -> bool {
    matches!(
        path.extension().and_then(|ext| ext.to_str()),
        Some("rs" | "java" | "js" | "jsx" | "ts" | "tsx")
    )
}

pub fn task_mentions_tests(config: &BreConfig) -> bool {
    let mut task_text = String::new();
    task_text.push_str(&config.goal.to_lowercase());
    task_text.push('\n');
    task_text.push_str(&config.acceptance_criteria.join("\n").to_lowercase());

    task_text
        .split(|ch: char| !ch.is_ascii_alphanumeric())
        .any(|word| matches!(word, "test" | "tests" | "spec" | "coverage" | "regression"))
}

pub fn is_test_like_file(path: &Path) -> bool {
    let Some(file_name) = path.file_name().and_then(|name| name.to_str()) else {
        return false;
    };

    file_name.contains(".test.")
        || file_name.contains(".spec.")
        || file_name.ends_with("_test.rs")
        || file_name.ends_with("_spec.rs")
}

pub fn test_subject_stem(path: &Path) -> Option<String> {
    let stem = path.file_stem()?.to_string_lossy();
    let without_suffix = stem
        .strip_suffix(".test")
        .or_else(|| stem.strip_suffix(".spec"))
        .or_else(|| stem.strip_suffix("_test"))
        .or_else(|| stem.strip_suffix("_spec"))
        .unwrap_or(&stem);
    Some(without_suffix.to_string())
}

pub fn is_matching_active_test(candidate: &Path, active_files: &[String]) -> bool {
    if !is_test_like_file(candidate) {
        return false;
    }

    let candidate_parent = candidate.parent();
    let Some(candidate_stem) = test_subject_stem(candidate) else {
        return false;
    };

    active_files.iter().any(|active_file| {
        let active_path = Path::new(active_file);
        active_path.parent() == candidate_parent
            && active_path
                .file_stem()
                .map(|stem| stem.to_string_lossy() == candidate_stem)
                .unwrap_or(false)
    })
}

pub fn extract_symbol_candidates(text: &str) -> HashSet<String> {
    extract_symbol_candidate_counts(text).into_keys().collect()
}

pub fn extract_symbol_candidate_counts(text: &str) -> HashMap<String, usize> {
    static IDENT_RE: OnceLock<Regex> = OnceLock::new();
    let ident_re = IDENT_RE
        .get_or_init(|| Regex::new(r"\b[A-Za-z_][A-Za-z0-9_]*\b").expect("valid ident regex"));

    let mut counts = HashMap::new();
    for ident_match in ident_re.find_iter(text) {
        let start = ident_match.start();
        let before_ident = &text[..start];
        if before_ident.ends_with("::") || before_ident.ends_with('.') {
            continue;
        }
        let ident = ident_match.as_str();
        if is_symbol_candidate(ident) {
            *counts.entry(ident.to_string()).or_insert(0) += 1;
        }
    }

    counts
}

pub fn is_symbol_candidate(ident: &str) -> bool {
    const STOP_WORDS: &[&str] = &[
        "Assets",
        "BTreeMap",
        "BTreeSet",
        "Camera2d",
        "Clone",
        "Commands",
        "Copy",
        "Debug",
        "Default",
        "DefaultPlugins",
        "HashMap",
        "HashSet",
        "Option",
        "ResMut",
        "Result",
        "Startup",
        "String",
        "Transform",
    ];

    if STOP_WORDS.contains(&ident) {
        return false;
    }

    let is_snake_case = ident.len() >= 8
        && ident.contains('_')
        && ident
            .chars()
            .all(|ch| ch.is_ascii_lowercase() || ch.is_ascii_digit() || ch == '_');

    let is_pascal_or_camel = ident.len() >= 7
        && ident.chars().any(|ch| ch.is_ascii_uppercase())
        && ident.chars().any(|ch| ch.is_ascii_lowercase());

    is_snake_case || is_pascal_or_camel
}

pub fn file_defines_symbol(content: &str, symbol: &str, extension: &str) -> bool {
    if !content.contains(symbol) {
        return false;
    }

    let escaped = regex::escape(symbol);
    if extension == "rs" {
        let rust_definition = Regex::new(&format!(
            r"(?m)^\s*(?:pub(?:\([^)]*\))?\s+)?(?:struct|enum|trait|type|fn|const|static|mod)\s+{escaped}\b"
        ))
        .expect("valid Rust symbol regex");
        return rust_definition.is_match(content);
    }

    if extension == "java" {
        let java_definition = Regex::new(&format!(
            r"(?m)^\s*(?:(?:public|protected|private|abstract|final|static)\s+)*(?:class|interface|enum|record|@interface)\s+{escaped}\b"
        ))
        .expect("valid Java symbol regex");
        return java_definition.is_match(content);
    }

    if matches!(extension, "js" | "jsx" | "ts" | "tsx") {
        let js_definition = Regex::new(&format!(
            r"(?m)^\s*(?:export\s+)?(?:class|interface|type|function|const|let|var)\s+{escaped}\b"
        ))
        .expect("valid JavaScript symbol regex");
        return js_definition.is_match(content);
    }

    false
}

pub fn is_broad_context_dir(path: &Path) -> bool {
    path.components().any(|component| {
        matches!(
            component.as_os_str().to_str(),
            Some(
                "examples"
                    | "example"
                    | "demo"
                    | "demos"
                    | "sample"
                    | "samples"
                    | "templates"
                    | "scaffolds"
                    | "create-cloudflare"
                    | "test"
                    | "tests"
                    | "benches"
                    | "docs"
                    | "resources"
                    | "fixtures"
                    | "__tests__"
            )
        )
    })
}

pub fn read_context_file(root_path: &Path, rel_path: &str) -> Result<String, String> {
    let path = root_path.join(rel_path);
    fs::read_to_string(&path)
        .or_else(|_| fs::read(&path).map(|bytes| String::from_utf8_lossy(&bytes).into_owned()))
        .map_err(|err| format!("failed to read context file `{rel_path}`: {err}"))
}

pub fn get_git_dirty_files(repo_path: &Path) -> Vec<String> {
    let Ok(repo) = Repository::discover(repo_path) else {
        return Vec::new();
    };
    let Some(workdir) = repo.workdir() else {
        return Vec::new();
    };
    let repo_root = repo_path
        .canonicalize()
        .unwrap_or_else(|_| repo_path.to_path_buf());

    let mut options = StatusOptions::new();
    options.include_untracked(true).recurse_untracked_dirs(true);

    let Ok(statuses) = repo.statuses(Some(&mut options)) else {
        return Vec::new();
    };

    statuses
        .iter()
        .filter_map(|entry| {
            let status = entry.status();
            let is_dirty = status.intersects(
                Status::WT_MODIFIED | Status::INDEX_MODIFIED | Status::WT_NEW | Status::INDEX_NEW,
            );

            if is_dirty {
                entry.path().and_then(|path| {
                    let absolute_path = workdir.join(path);
                    absolute_path
                        .strip_prefix(&repo_root)
                        .ok()
                        .map(normalize_path)
                })
            } else {
                None
            }
        })
        .collect()
}

pub fn is_critical_file(path_str: &str) -> bool {
    static CRITICAL_FILE_RE: OnceLock<Regex> = OnceLock::new();

    let critical_file_re = CRITICAL_FILE_RE.get_or_init(|| {
        Regex::new(concat!(
            r"^(\.env|\.env\.example|tsconfig(\.[^/]+)?\.json|jsconfig(\.[^/]+)?\.json|",
            r"pom\.xml|build\.gradle(\.kts)?|settings\.gradle(\.kts)?|gradle\.properties|",
            r"requirements\.txt|Pipfile|poetry\.lock|setup\.(py|cfg)|CMakeLists\.txt|",
            r"BUILD|BUILD\.bazel|MODULE\.bazel|WORKSPACE|WORKSPACE\.bazel|global\.json|",
            r"Directory\.Build\.(props|targets)|[^/]+\.(sln|csproj|fsproj|vbproj))$|",
            r"(^|/)(Dockerfile|docker-compose\.ya?ml|nginx\.conf|action\.ya?ml|actionlint\.ya?ml|",
            r"appsettings(\.[^/]+)?\.json|application(\.[^/]+)?\.(ya?ml|properties)|",
            r"bootstrap(\.[^/]+)?\.(ya?ml|properties)|Chart\.yaml|values(\.[^/]+)?\.ya?ml|",
            r"server\.properties|producer\.properties|consumer\.properties|connect-[^/]+\.properties|",
            r"schema-registry\.properties)$|",
            r"(^|/)(k8s|kubernetes|deploy|deployments|helm|charts)/.*\.ya?ml$|",
            r"(^|/)kafka/.*\.(properties|ya?ml|json)$|",
            r"\.tf$|^\.github/workflows/[^/]+\.ya?ml$",
        ))
        .expect("valid critical file regex")
    });

    critical_file_re.is_match(path_str)
}

pub fn get_critical_hidden_files(root_path: &Path) -> Vec<String> {
    let workflows_path = root_path.join(".github").join("workflows");
    let Ok(entries) = fs::read_dir(workflows_path) else {
        return Vec::new();
    };

    entries
        .filter_map(Result::ok)
        .filter_map(|entry| {
            let path = entry.path();

            if !path.is_file() {
                return None;
            }

            let relative_path = path.strip_prefix(root_path).ok()?;
            let relative = normalize_path(relative_path);

            is_critical_file(&relative).then_some(relative)
        })
        .collect()
}

pub fn should_descend(entry: &DirEntry) -> bool {
    if !entry.file_type().is_dir() {
        return true;
    }

    if entry.depth() == 0 {
        return true;
    }

    let Some(name) = entry.file_name().to_str() else {
        return true;
    };

    !name.starts_with('.')
        && name != "target"
        && name != "node_modules"
        && name != "dist"
        && name != "build"
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_git_dirty_files_are_relative_to_requested_subdirectory() {
        let temp_dir = tempfile::tempdir().unwrap();
        let root = temp_dir.path();
        let subdir = root.join("sub");
        fs::create_dir_all(subdir.join("src")).unwrap();

        let status = std::process::Command::new("git")
            .arg("init")
            .arg("-q")
            .current_dir(root)
            .status()
            .unwrap();
        assert!(status.success());

        fs::write(subdir.join("src/dirty.rs"), "pub fn dirty() {}\n").unwrap();

        let dirty_files = get_git_dirty_files(&subdir);

        assert!(dirty_files.contains(&"src/dirty.rs".to_string()));
    }
}
