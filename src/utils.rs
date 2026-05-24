use std::fs;
use std::path::{Path, PathBuf};

pub fn normalize_pathbuf(path: &Path) -> PathBuf {
    let mut normalized = PathBuf::new();
    for component in path.components() {
        match component {
            std::path::Component::CurDir => {}
            std::path::Component::ParentDir => {
                normalized.pop();
            }
            _ => normalized.push(component.as_os_str()),
        }
    }
    normalized
}

pub fn normalize_path(path: &Path) -> String {
    normalize_pathbuf(path)
        .components()
        .map(|component| component.as_os_str().to_string_lossy())
        .collect::<Vec<_>>()
        .join("/")
}

pub fn is_file_exact_case(path: &Path) -> bool {
    if !path.is_file() {
        return false;
    }

    let Some(parent) = path.parent() else {
        return path.is_file();
    };
    let Some(file_name) = path.file_name() else {
        return false;
    };
    let Ok(entries) = fs::read_dir(parent) else {
        return false;
    };

    entries.filter_map(Result::ok).any(|entry| {
        entry.file_name() == file_name && entry.file_type().is_ok_and(|kind| kind.is_file())
    })
}
