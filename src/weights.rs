use crate::config::{ContextTray, ContextTrayKind, DiagnosticError, FileDecisionTrace};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::Path;
use crate::utils::normalize_path;
use crate::ast::find_ast_dependencies_with_aliases;

const FEEDBACK_DECAY_STEP: usize = 2;

const FEEDBACK_CAP_MULTIPLIER: usize = 2;
const FEEDBACK_MIN_HEADROOM: usize = 20;

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq, Eq)]
pub struct HeuristicVelocity {
    pub ast_dependency: usize,
    pub symbol_hit_base: usize,
    pub symbol_hit_step: usize,
    pub git_dirty: usize,
    pub critical_file: usize,
    pub active_manifest: usize,
    pub active_parent: usize,
    pub task_term_matched: usize,
    pub task_term_ast_matched: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct HeuristicWeights {
    pub ast_dependency: usize,
    pub symbol_hit_base: usize,
    pub symbol_hit_step: usize,
    pub git_dirty: usize,
    pub critical_file: usize,
    pub active_manifest: usize,
    pub active_parent: usize,
    pub task_term_matched: usize,
    pub task_term_ast_matched: usize,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub velocity: Option<HeuristicVelocity>,
}

impl Default for HeuristicWeights {
    fn default() -> Self {
        Self {
            ast_dependency: 120,
            symbol_hit_base: 90,
            symbol_hit_step: 10,
            git_dirty: 80,
            critical_file: 30,
            active_manifest: 85,
            active_parent: 50,
            task_term_matched: 150,
            task_term_ast_matched: 220,
            velocity: None,
        }
    }
}

fn adaptive_cap(default: usize) -> usize {
    default
        .saturating_mul(FEEDBACK_CAP_MULTIPLIER)
        .max(default.saturating_add(FEEDBACK_MIN_HEADROOM))
}

fn decay_weight(current: &mut usize, default: usize) -> bool {
    let original = *current;
    let cap = adaptive_cap(default);
    if *current > cap {
        *current = cap;
    }

    if *current > default {
        *current = current.saturating_sub(FEEDBACK_DECAY_STEP).max(default);
    } else if *current < default {
        *current = current.saturating_add(FEEDBACK_DECAY_STEP).min(default);
    }

    *current != original
}

fn apply_momentum_boost(
    current: &mut usize,
    velocity: &mut usize,
    default: usize,
    s_avg: f64,
) -> bool {
    let original = *current;

    let new_vel = (9 * *velocity + (s_avg * 1000.0).round() as usize) / 10;
    *velocity = new_vel;

    let delta_w = (15 * new_vel + 500) / 1000;

    let cap = adaptive_cap(default);
    *current = current.saturating_add(delta_w).min(cap);

    *current != original
}

fn decay_momentum(current: &mut usize, velocity: &mut usize, default: usize) -> bool {
    *velocity = (9 * *velocity) / 10;
    decay_weight(current, default)
}

fn decay_weights_and_velocity_toward_defaults(weights: &mut HeuristicWeights) -> bool {
    let defaults = HeuristicWeights::default();
    let mut updated = false;
    let mut vel = weights.velocity.clone().unwrap_or_default();

    updated |= decay_momentum(&mut weights.ast_dependency, &mut vel.ast_dependency, defaults.ast_dependency);
    updated |= decay_momentum(&mut weights.symbol_hit_base, &mut vel.symbol_hit_base, defaults.symbol_hit_base);
    updated |= decay_momentum(&mut weights.symbol_hit_step, &mut vel.symbol_hit_step, defaults.symbol_hit_step);
    updated |= decay_momentum(&mut weights.git_dirty, &mut vel.git_dirty, defaults.git_dirty);
    updated |= decay_momentum(&mut weights.critical_file, &mut vel.critical_file, defaults.critical_file);
    updated |= decay_momentum(&mut weights.active_manifest, &mut vel.active_manifest, defaults.active_manifest);
    updated |= decay_momentum(&mut weights.active_parent, &mut vel.active_parent, defaults.active_parent);
    updated |= decay_momentum(&mut weights.task_term_matched, &mut vel.task_term_matched, defaults.task_term_matched);
    updated |= decay_momentum(&mut weights.task_term_ast_matched, &mut vel.task_term_ast_matched, defaults.task_term_ast_matched);

    weights.velocity = Some(vel);
    updated
}

impl HeuristicWeights {
    pub fn load_or_default(repo_root: &Path) -> Self {
        let weights_path = repo_root.join("bre_weights.json");
        if weights_path.exists() {
            if let Ok(content) = fs::read_to_string(&weights_path) {
                if let Ok(weights) = serde_json::from_str(&content) {
                    return weights;
                }
            }
        }
        Self::default()
    }

    pub fn save(&self, repo_root: &Path) -> std::io::Result<()> {
        let weights_path = repo_root.join("bre_weights.json");
        let content = serde_json::to_string_pretty(self)?;
        fs::write(weights_path, content)?;
        Ok(())
    }
}

pub fn apply_feedback_loop(
    diagnostics: &[DiagnosticError],
    tray: &ContextTray,
    file_traces: &[FileDecisionTrace],
    weights: &mut HeuristicWeights,
    repo_root: &Path,
    js_aliases: &HashMap<String, Vec<String>>,
) {
    let mut updated = false;

    if diagnostics.is_empty() {
        updated = decay_weights_and_velocity_toward_defaults(weights);
        if updated {
            if let Err(err) = weights.save(repo_root) {
                eprintln!("BRE: Failed to save decayed weights: {err}");
            }
        }
        return;
    }

    let defaults = HeuristicWeights::default();
    let background_files: HashSet<&str> = tray
        .background
        .iter()
        .map(|file| file.path.as_str())
        .collect();
    let background_traces: HashMap<&str, &FileDecisionTrace> = file_traces
        .iter()
        .filter(|trace| trace.tray == ContextTrayKind::Background)
        .map(|trace| (trace.path.as_str(), trace))
        .collect();

    let mut ast_sum = 0.0;
    let mut symbol_sum = 0.0;
    let mut manifest_sum = 0.0;
    let mut git_dirty_sum = 0.0;
    let mut active_parent_sum = 0.0;
    let mut task_term_sum = 0.0;
    let mut task_term_ast_sum = 0.0;
    let mut critical_sum = 0.0;
    let mut count = 0;

    let mut implicated_background_files = HashSet::new();

    for error in diagnostics {
        let error_path_raw = Path::new(&error.file);
        let absolute_error_path = if error_path_raw.is_absolute() {
            error_path_raw.to_path_buf()
        } else {
            repo_root.join(error_path_raw)
        };

        let Ok(rel) = absolute_error_path.strip_prefix(repo_root) else {
            continue;
        };
        let normalized_error_file = normalize_path(rel);

        if background_files.contains(normalized_error_file.as_str()) {
            implicated_background_files.insert(normalized_error_file);
        } else {
            let dependencies = find_ast_dependencies_with_aliases(
                &absolute_error_path,
                repo_root,
                js_aliases,
            );

            for dep_path in dependencies {
                if let Ok(dep_rel) = dep_path.strip_prefix(repo_root) {
                    let normalized_dep = normalize_path(dep_rel);
                    if background_files.contains(normalized_dep.as_str()) {
                        implicated_background_files.insert(normalized_dep);
                    }
                }
            }
        }
    }

    for file_path in &implicated_background_files {
        let Some(trace) = background_traces.get(file_path.as_str()) else {
            eprintln!(
                "BRE: Learning Loop - Implicated background file `{}` had no trace. Signal coverage gap.",
                file_path
            );
            continue;
        };

        let signals = &trace.signals;
        if !signals.any() {
            eprintln!(
                "BRE: Learning Loop - Implicated background file `{}` scored zero on all signals. No weight update possible.",
                file_path
            );
            continue;
        }

        eprintln!(
            "BRE: Learning Loop - Implicated background file `{}` was omitted but caused a compile failure. Fired signals: [{}].",
            file_path,
            signals.labels().join(", ")
        );

        if signals.ast_dependency {
            ast_sum += 1.0;
        }
        if signals.symbol_score > 0 {
            symbol_sum += 1.0;
        }
        if signals.active_manifest {
            manifest_sum += 1.0;
        }
        if signals.git_dirty {
            git_dirty_sum += 1.0;
        }
        if signals.active_parent {
            active_parent_sum += 1.0;
        }
        if signals.task_term_matched {
            task_term_sum += 1.0;
        }
        if signals.task_term_ast_matched {
            task_term_ast_sum += 1.0;
        }
        if signals.critical_file {
            critical_sum += 1.0;
        }
        count += 1;
    }

    if count > 0 {
        let n = count as f64;
        let s_ast = ast_sum / n;
        let s_symbol = symbol_sum / n;
        let s_manifest = manifest_sum / n;
        let s_git = git_dirty_sum / n;
        let s_parent = active_parent_sum / n;
        let s_term = task_term_sum / n;
        let s_term_ast = task_term_ast_sum / n;
        let s_critical = critical_sum / n;

        let mut vel = weights.velocity.clone().unwrap_or_default();

        updated |= apply_momentum_boost(&mut weights.ast_dependency, &mut vel.ast_dependency, defaults.ast_dependency, s_ast);
        updated |= apply_momentum_boost(&mut weights.symbol_hit_base, &mut vel.symbol_hit_base, defaults.symbol_hit_base, s_symbol);
        updated |= apply_momentum_boost(&mut weights.active_manifest, &mut vel.active_manifest, defaults.active_manifest, s_manifest);
        updated |= apply_momentum_boost(&mut weights.git_dirty, &mut vel.git_dirty, defaults.git_dirty, s_git);
        updated |= apply_momentum_boost(&mut weights.active_parent, &mut vel.active_parent, defaults.active_parent, s_parent);
        updated |= apply_momentum_boost(&mut weights.task_term_matched, &mut vel.task_term_matched, defaults.task_term_matched, s_term);
        updated |= apply_momentum_boost(&mut weights.task_term_ast_matched, &mut vel.task_term_ast_matched, defaults.task_term_ast_matched, s_term_ast);
        updated |= apply_momentum_boost(&mut weights.critical_file, &mut vel.critical_file, defaults.critical_file, s_critical);

        weights.velocity = Some(vel);

        if updated {
            if let Err(err) = weights.save(repo_root) {
                eprintln!("BRE: Failed to save updated weights: {err}");
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{ContextFile, FileDecisionTrace, FileRepresentation, FileSignals};

    fn background_trace(path: &str, signals: FileSignals) -> FileDecisionTrace {
        FileDecisionTrace {
            path: path.to_string(),
            tray: ContextTrayKind::Background,
            representation: FileRepresentation::Omitted,
            score: Some(120),
            signals,
        }
    }

    #[test]
    fn test_feedback_loop_boosts_weights() {
        let temp_dir = tempfile::tempdir().unwrap();
        let root = temp_dir.path();

        let mut weights = HeuristicWeights::default();
        let diagnostics = vec![DiagnosticError {
            file: "src/ignored_causing_error.rs".to_string(),
            line: 10,
            message: "compile error".to_string(),
            compiler: "rustc".to_string(),
        }];

        let tray = ContextTray {
            active: vec![],
            watching: vec![],
            background: vec![ContextFile {
                path: "src/ignored_causing_error.rs".to_string(),
                ..Default::default()
            }],
        };
        let traces = vec![background_trace(
            "src/ignored_causing_error.rs",
            FileSignals {
                ast_dependency: true,
                ..Default::default()
            },
        )];

        let original_ast_dependency = weights.ast_dependency;

        apply_feedback_loop(&diagnostics, &tray, &traces, &mut weights, root, &HashMap::new());

        assert!(weights.ast_dependency > original_ast_dependency);

        let weights_path = root.join("bre_weights.json");
        assert!(weights_path.exists());
        let saved_content = fs::read_to_string(weights_path).unwrap();
        let loaded: HeuristicWeights = serde_json::from_str(&saved_content).unwrap();
        assert_eq!(loaded.ast_dependency, weights.ast_dependency);
    }

    #[test]
    fn test_feedback_loop_decays_weights_after_clean_run() {
        let temp_dir = tempfile::tempdir().unwrap();
        let root = temp_dir.path();

        let defaults = HeuristicWeights::default();
        let mut weights = HeuristicWeights {
            ast_dependency: defaults.ast_dependency + 12,
            symbol_hit_base: defaults.symbol_hit_base + 8,
            git_dirty: defaults.git_dirty + 4,
            ..defaults.clone()
        };
        let tray = ContextTray {
            active: vec![],
            watching: vec![],
            background: vec![],
        };

        apply_feedback_loop(&[], &tray, &[], &mut weights, root, &HashMap::new());

        assert_eq!(weights.ast_dependency, defaults.ast_dependency + 10);
        assert_eq!(weights.symbol_hit_base, defaults.symbol_hit_base + 6);
        assert_eq!(weights.git_dirty, defaults.git_dirty + 2);

        let weights_path = root.join("bre_weights.json");
        assert!(weights_path.exists());
        let saved_content = fs::read_to_string(weights_path).unwrap();
        let loaded: HeuristicWeights = serde_json::from_str(&saved_content).unwrap();
        assert_eq!(loaded.ast_dependency, weights.ast_dependency);
    }

    #[test]
    fn test_feedback_loop_clean_defaults_do_not_write_weights() {
        let temp_dir = tempfile::tempdir().unwrap();
        let root = temp_dir.path();

        let mut weights = HeuristicWeights::default();
        let tray = ContextTray {
            active: vec![],
            watching: vec![],
            background: vec![],
        };

        apply_feedback_loop(&[], &tray, &[], &mut weights, root, &HashMap::new());

        assert_eq!(
            weights.ast_dependency,
            HeuristicWeights::default().ast_dependency
        );
        assert!(!root.join("bre_weights.json").exists());
    }

    #[test]
    fn test_feedback_loop_caps_overinflated_weights() {
        let temp_dir = tempfile::tempdir().unwrap();
        let root = temp_dir.path();

        let defaults = HeuristicWeights::default();
        let mut weights = HeuristicWeights {
            ast_dependency: usize::MAX,
            symbol_hit_base: usize::MAX,
            git_dirty: usize::MAX,
            ..defaults.clone()
        };
        let tray = ContextTray {
            active: vec![],
            watching: vec![],
            background: vec![],
        };

        apply_feedback_loop(&[], &tray, &[], &mut weights, root, &HashMap::new());

        assert_eq!(
            weights.ast_dependency,
            adaptive_cap(defaults.ast_dependency) - FEEDBACK_DECAY_STEP
        );
        assert_eq!(
            weights.symbol_hit_base,
            adaptive_cap(defaults.symbol_hit_base) - FEEDBACK_DECAY_STEP
        );
        assert_eq!(
            weights.git_dirty,
            adaptive_cap(defaults.git_dirty) - FEEDBACK_DECAY_STEP
        );
    }

    #[test]
    fn test_feedback_loop_boosts_only_fired_signals() {
        // The ejected file had ast_dependency + active_parent signals.
        // Only those two weights should be boosted — not git_dirty, symbol, etc.
        let temp_dir = tempfile::tempdir().unwrap();
        let root = temp_dir.path();

        let defaults = HeuristicWeights::default();
        let mut weights = defaults.clone();
        let diagnostics = vec![DiagnosticError {
            file: "src/feature.rs".to_string(),
            line: 10,
            message: "compile error".to_string(),
            compiler: "rustc".to_string(),
        }];

        let tray = ContextTray {
            active: vec![ContextFile {
                path: "src/main.rs".to_string(),
                ..Default::default()
            }],
            watching: vec![],
            background: vec![ContextFile {
                path: "src/feature.rs".to_string(),
                ..Default::default()
            }],
        };
        let traces = vec![background_trace(
            "src/feature.rs",
            FileSignals {
                ast_dependency: true,
                active_parent: true,
                ..Default::default()
            },
        )];

        apply_feedback_loop(&diagnostics, &tray, &traces, &mut weights, root, &HashMap::new());

        // Only signals that fired should be boosted.
        assert_eq!(
            weights.ast_dependency,
            defaults.ast_dependency + 2
        );
        assert_eq!(
            weights.active_parent,
            defaults.active_parent + 2
        );

        // Signals that did NOT fire must be unchanged.
        assert_eq!(weights.symbol_hit_base, defaults.symbol_hit_base);
        assert_eq!(
            weights.task_term_ast_matched,
            defaults.task_term_ast_matched
        );
        assert_eq!(weights.task_term_matched, defaults.task_term_matched);
        assert_eq!(weights.critical_file, defaults.critical_file);
        assert_eq!(weights.git_dirty, defaults.git_dirty);
        assert_eq!(weights.active_manifest, defaults.active_manifest);
    }

    #[test]
    fn test_feedback_loop_coverage_gap_does_not_boost_weights() {
        // A background file with no decision trace is an always-background file.
        // The engine had no basis to include it, so no weight update is possible.
        let temp_dir = tempfile::tempdir().unwrap();
        let root = temp_dir.path();

        let defaults = HeuristicWeights::default();
        let mut weights = defaults.clone();
        let diagnostics = vec![DiagnosticError {
            file: "generated/auto.rs".to_string(),
            line: 1,
            message: "some error".to_string(),
            compiler: "rustc".to_string(),
        }];

        let tray = ContextTray {
            active: vec![],
            watching: vec![],
            background: vec![ContextFile {
                path: "generated/auto.rs".to_string(),
                ..Default::default()
            }],
        };

        apply_feedback_loop(&diagnostics, &tray, &[], &mut weights, root, &HashMap::new());

        // Coverage gaps must never blindly inflate weights.
        assert_eq!(weights.ast_dependency, defaults.ast_dependency);
        assert_eq!(weights.symbol_hit_base, defaults.symbol_hit_base);
        assert_eq!(weights.git_dirty, defaults.git_dirty);
    }

    #[test]
    fn test_feedback_loop_graph_backprop_implicates_dependencies() {
        let temp_dir = tempfile::tempdir().unwrap();
        let root = temp_dir.path();

        fs::write(root.join("Cargo.toml"), "[package]\nname = \"test\"\nversion = \"0.1.0\"\n").unwrap();
        fs::create_dir_all(root.join("src")).unwrap();
        fs::write(root.join("src/main.rs"), "mod feature;\nuse crate::feature::Thing;\n").unwrap();
        fs::write(root.join("src/feature.rs"), "pub struct Thing;\n").unwrap();

        let defaults = HeuristicWeights::default();
        let mut weights = defaults.clone();

        let diagnostics = vec![DiagnosticError {
            file: root.join("src/main.rs").to_string_lossy().to_string(),
            line: 2,
            message: "type error".to_string(),
            compiler: "rustc".to_string(),
        }];

        let tray = ContextTray {
            active: vec![ContextFile {
                path: "src/main.rs".to_string(),
                ..Default::default()
            }],
            watching: vec![],
            background: vec![ContextFile {
                path: "src/feature.rs".to_string(),
                ..Default::default()
            }],
        };

        let traces = vec![background_trace(
            "src/feature.rs",
            FileSignals {
                ast_dependency: true,
                ..Default::default()
            },
        )];

        apply_feedback_loop(&diagnostics, &tray, &traces, &mut weights, root, &HashMap::new());

        assert!(weights.ast_dependency > defaults.ast_dependency);
    }

    #[test]
    fn test_loading_default_weights_does_not_write_repo_state() {
        let temp_dir = tempfile::tempdir().unwrap();
        let root = temp_dir.path();

        let weights = HeuristicWeights::load_or_default(root);

        assert_eq!(
            weights.ast_dependency,
            HeuristicWeights::default().ast_dependency
        );
        assert!(!root.join("bre_weights.json").exists());
    }
}
