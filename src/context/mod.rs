pub mod heuristics;
pub mod skeleton;

use std::collections::{BTreeSet, HashMap, HashSet};
use std::path::{Path, PathBuf};

use crate::ast::find_ast_dependencies_with_aliases;
use crate::config::{
    BreConfig, ContextBuildReport, ContextCounts, ContextFile, ContextTray, ContextTrayKind,
    FileDecisionTrace, FileRepresentation, FileSignals, ScoringPolicy, ScoringPolicyMode,
};
use crate::utils::normalize_path;
use crate::weights::HeuristicWeights;

use heuristics::{
    context_symbol_candidates, file_defines_any_symbol, find_active_manifest_files,
    find_symbol_dependencies, get_critical_hidden_files, get_git_dirty_files, is_broad_context_dir,
    is_critical_file, is_matching_active_test, is_test_like_file, path_matches_task_terms,
    read_context_file, should_descend, task_mentions_tests, task_path_terms,
    task_symbol_candidates, validate_active_files,
};
use skeleton::{
    max_context_file_count, prepare_watching_content_for_budget,
    skeletonize_for_budget,
};



struct CandidateFile {
    path: String,
    score: usize,
    full_content: String,
    full_tokens: usize,
    skel_content: Option<String>,
    skel_tokens: Option<usize>,
    signals: FileSignals,
    was_pre_skeletonized: bool,
}

fn get_utility(score: usize, state: usize) -> usize {
    match state {
        0 => 0,
        1 => (0.6 * score as f64).round() as usize,
        2 => score,
        _ => 0,
    }
}

fn get_tokens(cand: &CandidateFile, state: usize) -> usize {
    match state {
        0 => 0,
        1 => cand.skel_tokens.unwrap_or(cand.full_tokens),
        2 => cand.full_tokens,
        _ => 0,
    }
}

fn get_loaded(state: usize) -> usize {
    match state {
        0 => 0,
        1 => 1,
        2 => 1,
        _ => 0,
    }
}

fn solve_mmkp_fixed_lambdas(
    candidates: &[CandidateFile],
    token_budget: usize,
    file_budget: usize,
    lambda_1: f64,
    lambda_2: f64,
) -> Vec<usize> {
    let mut states = vec![0; candidates.len()];
    let mut current_tokens = 0;
    let mut current_files = 0;

    loop {
        let mut best_candidate_idx: Option<usize> = None;
        let mut best_next_state = 0;
        let mut best_ratio = -1.0;

        for (i, cand) in candidates.iter().enumerate() {
            let curr_state = states[i];
            if curr_state == 2 {
                continue;
            }

            let possible_next_states = if curr_state == 0 {
                if cand.skel_tokens.is_some() {
                    vec![1, 2]
                } else {
                    vec![2]
                }
            } else {
                vec![2]
            };

            for next_state in possible_next_states {
                let delta_v = get_utility(cand.score, next_state) as f64 - get_utility(cand.score, curr_state) as f64;
                if delta_v <= 0.0 {
                    continue;
                }
                let delta_t = get_tokens(cand, next_state) as f64 - get_tokens(cand, curr_state) as f64;
                let delta_l = get_loaded(next_state) as f64 - get_loaded(curr_state) as f64;

                if current_tokens + delta_t as usize <= token_budget && current_files + delta_l as usize <= file_budget {
                    let cost_term = lambda_1 * delta_t + lambda_2 * delta_l;
                    let ratio = delta_v / (cost_term + 1e-9);

                    let is_better = match best_candidate_idx {
                        None => true,
                        Some(best_idx) => {
                            if (ratio - best_ratio).abs() < 1e-9 {
                                if cand.path != candidates[best_idx].path {
                                    cand.path < candidates[best_idx].path
                                } else {
                                    next_state > best_next_state
                                }
                            } else {
                                ratio > best_ratio
                            }
                        }
                    };

                    if is_better {
                        best_candidate_idx = Some(i);
                        best_next_state = next_state;
                        best_ratio = ratio;
                    }
                }
            }
        }

        if let Some(idx) = best_candidate_idx {
            let cand = &candidates[idx];
            let delta_t = get_tokens(cand, best_next_state) - get_tokens(cand, states[idx]);
            let delta_l = get_loaded(best_next_state) - get_loaded(states[idx]);

            current_tokens += delta_t;
            current_files += delta_l;
            states[idx] = best_next_state;
        } else {
            break;
        }
    }

    states
}

fn solve_mmkp(
    candidates: &[CandidateFile],
    token_budget: usize,
    file_budget: usize,
) -> Vec<usize> {
    let mut best_states = vec![0; candidates.len()];
    let mut best_total_utility = 0;

    let lambda_1 = 1.0 / (token_budget as f64).max(1.0);
    let sweep_factors = [0.0, 0.01, 0.05, 0.1, 0.5, 1.0, 2.0, 5.0, 10.0, 50.0, 100.0];

    for &factor in &sweep_factors {
        let lambda_2 = factor / (file_budget as f64).max(1.0);
        let states = solve_mmkp_fixed_lambdas(candidates, token_budget, file_budget, lambda_1, lambda_2);

        let mut total_tokens = 0;
        let mut total_files = 0;
        let mut total_utility = 0;
        for (i, &state) in states.iter().enumerate() {
            if state > 0 {
                total_files += 1;
                total_utility += get_utility(candidates[i].score, state);
                total_tokens += get_tokens(&candidates[i], state);
            }
        }

        if total_tokens <= token_budget && total_files <= file_budget {
            if total_utility > best_total_utility {
                best_total_utility = total_utility;
                best_states = states.clone();
            } else if total_utility == best_total_utility {
                let mut best_tokens = 0;
                for (i, &s) in best_states.iter().enumerate() {
                    best_tokens += get_tokens(&candidates[i], s);
                }
                if total_tokens < best_tokens {
                    best_states = states.clone();
                }
            }
        }
    }

    best_states
}

struct SignalInputs<'a> {
    ast_dependencies: &'a HashSet<String>,
    symbol_dependencies: &'a HashMap<String, usize>,
    active_manifest_files: &'a HashSet<String>,
    git_dirty_files: &'a HashSet<String>,
    active_parent_dirs: &'a HashSet<PathBuf>,
    task_path_score_terms: &'a HashSet<String>,
    task_includes_tests: bool,
}

fn compute_signals(file_name: &str, inputs: &SignalInputs<'_>) -> FileSignals {
    let file_path = Path::new(file_name);
    let is_ast = inputs.ast_dependencies.contains(file_name);
    let is_broad_unrequested = is_broad_context_dir(file_path) && !inputs.task_includes_tests;
    let task_term_hit = !is_broad_unrequested
        && !inputs.task_path_score_terms.is_empty()
        && path_matches_task_terms(file_path, inputs.task_path_score_terms);
    let is_root_manifest = file_path.components().count() == 1
        && matches!(
            file_path.file_name().and_then(|n| n.to_str()),
            Some("package.json" | "Cargo.toml")
        );
    FileSignals {
        ast_dependency: is_ast,
        symbol_score: inputs
            .symbol_dependencies
            .get(file_name)
            .copied()
            .unwrap_or(0),
        active_manifest: inputs.active_manifest_files.contains(file_name),
        git_dirty: inputs.git_dirty_files.contains(file_name),
        active_parent: file_path
            .parent()
            .map(|p| inputs.active_parent_dirs.contains(p))
            .unwrap_or(false),
        task_term_matched: task_term_hit && !is_ast,
        task_term_ast_matched: task_term_hit && is_ast,
        critical_file: is_critical_file(file_name) || is_root_manifest,
    }
}

fn signal_contributions(signals: &FileSignals, weights: &HeuristicWeights) -> Vec<usize> {
    let mut contributions = Vec::new();
    if signals.ast_dependency {
        contributions.push(weights.ast_dependency);
    }
    if signals.symbol_score > 0 {
        contributions.push(signals.symbol_score);
    }
    if signals.active_manifest {
        contributions.push(weights.active_manifest);
    }
    if signals.git_dirty {
        contributions.push(weights.git_dirty);
    }
    if signals.active_parent {
        contributions.push(weights.active_parent);
    }
    if signals.task_term_ast_matched {
        contributions.push(weights.task_term_ast_matched);
    } else if signals.task_term_matched {
        contributions.push(weights.task_term_matched);
    }
    if signals.critical_file {
        contributions.push(weights.critical_file);
    }
    contributions
}

fn file_score(
    signals: &FileSignals,
    weights: &HeuristicWeights,
    scoring_policy: &ScoringPolicy,
) -> usize {
    let contributions = signal_contributions(signals, weights);
    match scoring_policy.mode {
        ScoringPolicyMode::Max => contributions.into_iter().max().unwrap_or(0),
        ScoringPolicyMode::Additive => contributions
            .into_iter()
            .fold(0usize, |score, contribution| {
                score.saturating_add(contribution)
            }),
        ScoringPolicyMode::PNorm => {
            let p = scoring_policy.p_value() as f64;
            let sum = contributions
                .into_iter()
                .map(|contribution| (contribution as f64).powf(p))
                .sum::<f64>();
            sum.powf(1.0 / p).round() as usize
        }
    }
}

pub fn build_context_tray(
    config: &BreConfig,
    root_path: &Path,
    max_tokens: usize,
    weights: &HeuristicWeights,
) -> Result<ContextTray, String> {
    build_context_tray_with_report(config, root_path, max_tokens, weights).map(|(tray, _, _)| tray)
}

pub fn build_context_tray_with_report(
    config: &BreConfig,
    root_path: &Path,
    max_tokens: usize,
    weights: &HeuristicWeights,
) -> Result<(ContextTray, ContextBuildReport, Vec<FileDecisionTrace>), String> {
    config.scoring_policy.validate()?;
    validate_active_files(config, root_path)?;

    let active: BTreeSet<String> = config.active_files.iter().cloned().collect();

    let active_parent_dirs: HashSet<PathBuf> = config
        .active_files
        .iter()
        .filter_map(|file| Path::new(file).parent().map(Path::to_path_buf))
        .collect();
    let task_includes_tests = task_mentions_tests(config);

    let mut watching = BTreeSet::new();
    let mut background = BTreeSet::new();

    let mut ast_dependencies = HashSet::new();
    // 1. Resolve AST dependencies for all active files
    for active_file in &config.active_files {
        let active_path = root_path.join(active_file);
        for dep in find_ast_dependencies_with_aliases(&active_path, root_path, &config.js_aliases) {
            if let Ok(rel) = dep.strip_prefix(root_path) {
                let rel_str = normalize_path(rel);
                if !active.contains(&rel_str) {
                    ast_dependencies.insert(rel_str.clone());
                    watching.insert(rel_str);
                }
            }
        }
    }
    let task_prelude_symbols = task_symbol_candidates(config);
    let fallback_prelude_symbols = context_symbol_candidates(config, root_path, 1);
    let first_order_ast_dependencies = ast_dependencies.clone();
    for dep in first_order_ast_dependencies {
        if !dep.ends_with("prelude.rs") {
            continue;
        }
        let dep_path = root_path.join(&dep);
        let mut transitive_deps = Vec::new();
        for transitive_dep in
            find_ast_dependencies_with_aliases(&dep_path, root_path, &config.js_aliases)
        {
            if let Ok(rel) = transitive_dep.strip_prefix(root_path) {
                let rel_str = normalize_path(rel);
                if !active.contains(&rel_str) {
                    transitive_deps.push(rel_str);
                }
            }
        }

        let task_matched_deps: Vec<String> = transitive_deps
            .iter()
            .filter(|rel| file_defines_any_symbol(root_path, rel, &task_prelude_symbols))
            .cloned()
            .collect();

        let fallback_matched_deps: Vec<String> = if task_matched_deps.is_empty() {
            transitive_deps
                .iter()
                .filter(|rel| file_defines_any_symbol(root_path, rel, &fallback_prelude_symbols))
                .cloned()
                .collect()
        } else {
            Vec::new()
        };

        let deps_to_include = if !task_matched_deps.is_empty() {
            task_matched_deps
        } else if !fallback_matched_deps.is_empty() {
            fallback_matched_deps
        } else {
            transitive_deps
        };

        for rel_str in deps_to_include {
            ast_dependencies.insert(rel_str.clone());
            watching.insert(rel_str);
        }
    }

    let symbol_dependencies = find_symbol_dependencies(config, root_path, weights);
    for dep in symbol_dependencies.keys() {
        if !active.contains(dep) {
            watching.insert(dep.clone());
        }
    }

    let active_manifest_files = find_active_manifest_files(config, root_path);
    for manifest_file in &active_manifest_files {
        if !active.contains(manifest_file) {
            watching.insert(manifest_file.clone());
        }
    }

    let git_dirty_files: HashSet<String> = get_git_dirty_files(root_path).into_iter().collect();
    for dirty_file in &git_dirty_files {
        if dirty_file != "bre.json"
            && dirty_file != "bre_weights.json"
            && !active.contains(dirty_file)
        {
            watching.insert(dirty_file.clone());
        }
    }

    for critical_file in get_critical_hidden_files(root_path) {
        if !active.contains(&critical_file) {
            watching.insert(critical_file);
        }
    }

    for entry in walkdir::WalkDir::new(root_path)
        .into_iter()
        .filter_entry(should_descend)
        .filter_map(Result::ok)
        .filter(|entry| entry.file_type().is_file())
    {
        let relative_path = match entry.path().strip_prefix(root_path) {
            Ok(path) => path,
            Err(_) => continue,
        };

        let relative = normalize_path(relative_path);

        if active.contains(&relative) {
            continue;
        }

        let is_critical = is_critical_file(&relative);
        let is_root_manifest = relative_path.components().count() == 1
            && matches!(
                relative_path.file_name().and_then(|name| name.to_str()),
                Some("package.json" | "Cargo.toml")
            );

        let shares_active_parent = relative_path
            .parent()
            .map(|parent| {
                active_parent_dirs.contains(parent)
                    && !is_broad_context_dir(parent)
                    && (!is_test_like_file(relative_path)
                        || is_matching_active_test(relative_path, &config.active_files))
            })
            .unwrap_or(false);

        if is_critical || is_root_manifest || shares_active_parent {
            watching.insert(relative);
        } else if !watching.contains(&relative) {
            background.insert(relative);
        }
    }

    // Token management & Boundary Enforcement
    let bpe = tiktoken_rs::cl100k_base().unwrap();

    let mut final_active = Vec::new();
    let mut file_traces = Vec::new();
    let mut active_tokens = 0;
    for active_file in &config.active_files {
        let content = read_context_file(root_path, active_file)?;
        let tokens = bpe.encode_with_special_tokens(&content).len();
        active_tokens += tokens;
        file_traces.push(FileDecisionTrace {
            path: active_file.clone(),
            tray: ContextTrayKind::Active,
            representation: FileRepresentation::Full,
            score: None,
            signals: FileSignals::default(),
        });
        final_active.push(ContextFile {
            path: active_file.clone(),
            content: Some(content),
            is_skeleton: Some(false),
        });
    }

    if active_tokens > max_tokens {
        return Err(format!(
            "context boundary exceeded: Active files consume {active_tokens} tokens, which exceeds the limit of {max_tokens}"
        ));
    }

    let task_path_score_terms = task_path_terms(config);
    let signal_inputs = SignalInputs {
        ast_dependencies: &ast_dependencies,
        symbol_dependencies: &symbol_dependencies,
        active_manifest_files: &active_manifest_files,
        git_dirty_files: &git_dirty_files,
        active_parent_dirs: &active_parent_dirs,
        task_path_score_terms: &task_path_score_terms,
        task_includes_tests,
    };



    let mut candidates = Vec::new();
    for path in watching {
        let content = read_context_file(root_path, &path).unwrap_or_default();
        let signals = compute_signals(&path, &signal_inputs);
        let score = file_score(&signals, weights, &config.scoring_policy);

        let ext = Path::new(&path)
            .extension()
            .and_then(|e| e.to_str())
            .unwrap_or("");

        let (full_content, was_pre_skeletonized) = prepare_watching_content_for_budget(&path, content);
        let full_tokens = bpe.encode_with_special_tokens(&full_content).len();

        let (skel_content, skel_tokens) = if was_pre_skeletonized || !skeleton::is_skeletonizable_extension(ext) {
            (None, None)
        } else {
            let skel = skeletonize_for_budget(&full_content, ext);
            let sk_tokens = bpe.encode_with_special_tokens(&skel).len();
            if sk_tokens < full_tokens {
                (Some(skel), Some(sk_tokens))
            } else {
                (None, None)
            }
        };

        candidates.push(CandidateFile {
            path,
            score,
            full_content,
            full_tokens,
            skel_content,
            skel_tokens,
            signals,
            was_pre_skeletonized,
        });
    }

    let initial_total = active_tokens + candidates.iter().map(|c| c.full_tokens).sum::<usize>();
    let max_loaded_files = max_context_file_count(max_tokens);
    let active_files_count = final_active.len();

    let token_budget = max_tokens.saturating_sub(active_tokens);
    let file_budget = max_loaded_files.saturating_sub(active_files_count);



    let states = solve_mmkp(&candidates, token_budget, file_budget);

    let mut final_watching = Vec::new();
    let mut ejected_count = 0;
    let mut skeletonized_count = 0;
    let mut total_tokens = active_tokens;

    for (i, cand) in candidates.into_iter().enumerate() {
        let state = states[i];
        if state == 0 {
            ejected_count += 1;
            file_traces.push(FileDecisionTrace {
                path: cand.path.clone(),
                tray: ContextTrayKind::Background,
                representation: FileRepresentation::Omitted,
                score: Some(cand.score),
                signals: cand.signals,
            });
            background.insert(cand.path);
        } else {
            let (content, is_skeleton, repr) = if state == 1 {
                skeletonized_count += 1;
                let c = cand.skel_content.clone().unwrap_or_else(|| cand.full_content.clone());
                (c, true, FileRepresentation::Skeleton)
            } else {
                if cand.was_pre_skeletonized {
                    skeletonized_count += 1;
                    (cand.full_content.clone(), true, FileRepresentation::Skeleton)
                } else {
                    (cand.full_content.clone(), false, FileRepresentation::Full)
                }
            };
            total_tokens += bpe.encode_with_special_tokens(&content).len();

            file_traces.push(FileDecisionTrace {
                path: cand.path.clone(),
                tray: ContextTrayKind::Watching,
                representation: repr,
                score: Some(cand.score),
                signals: cand.signals,
            });
            final_watching.push(ContextFile {
                path: cand.path,
                content: Some(content),
                is_skeleton: Some(is_skeleton),
            });
        }
    }

    final_watching.sort_by(|a, b| a.path.cmp(&b.path));

    let has_ejections = ejected_count > 0;
    let initial_candidates_count = final_active.len() + final_watching.len() + ejected_count;

    let budget_action = if has_ejections && initial_total > max_tokens {
        "trimmed_to_token_budget"
    } else if has_ejections && initial_candidates_count > max_loaded_files {
        "trimmed_to_loaded_file_cap"
    } else if has_ejections {
        "trimmed_to_file_budget"
    } else if skeletonized_count > 0 {
        let has_pre_skel = final_watching.iter().any(|w| w.is_skeleton.unwrap_or(false));
        if has_pre_skel && initial_total <= max_tokens {
            "pre_skeletonized_large_files"
        } else {
            "trimmed_to_token_budget"
        }
    } else {
        "within_budget"
    };

    if skeletonized_count > 0 || ejected_count > 0 {
        let budget_label = match budget_action {
            "trimmed_to_token_budget" => "Token limit exceeded",
            "trimmed_to_loaded_file_cap" => "Loaded-file cap applied",
            "pre_skeletonized_large_files" => "Large files pre-skeletonized",
            _ => "Context budget adjusted",
        };
        eprintln!(
            "BRE: {}. Total tokens: {} -> {} (Limit: {}). Skeletonized {} files, Ejected {} files to Background.",
            budget_label, initial_total, total_tokens, max_tokens, skeletonized_count, ejected_count
        );
    } else {
        eprintln!(
            "BRE: Total tokens: {} (Limit: {}). Ejected 0 files, Skeletonized 0 files.",
            total_tokens, max_tokens
        );
    }

    let mut final_background = Vec::new();
    for bg_file in background {
        final_background.push(ContextFile {
            path: bg_file,
            content: None,
            is_skeleton: None,
        });
    }

    let tray = ContextTray {
        active: final_active,
        watching: final_watching,
        background: final_background,
    };
    let report = ContextBuildReport {
        counts: ContextCounts::from_tray(&tray),
        scoring_policy: config.scoring_policy.clone(),
        budget_action: budget_action.to_string(),
        max_tokens,
        initial_tokens: initial_total,
        final_tokens: total_tokens,
        active_tokens,
        skeletonized_files: skeletonized_count,
        ejected_files: ejected_count,
        max_loaded_files,
        loaded_files: tray.active.len() + tray.watching.len(),
    };

    file_traces.sort_by(|a, b| {
        trace_tray_rank(&a.tray)
            .cmp(&trace_tray_rank(&b.tray))
            .then_with(|| a.path.cmp(&b.path))
    });

    Ok((tray, report, file_traces))
}

fn trace_tray_rank(tray: &ContextTrayKind) -> u8 {
    match tray {
        ContextTrayKind::Active => 0,
        ContextTrayKind::Watching => 1,
        ContextTrayKind::Background => 2,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn test_scoring_policies_aggregate_signal_contributions() {
        let weights = HeuristicWeights::default();
        let signals = FileSignals {
            ast_dependency: true,
            git_dirty: true,
            critical_file: true,
            ..Default::default()
        };

        let max_score = file_score(&signals, &weights, &ScoringPolicy::max());
        let additive_score = file_score(&signals, &weights, &ScoringPolicy::additive());
        let p_norm_score = file_score(&signals, &weights, &ScoringPolicy::p_norm(2));

        assert_eq!(max_score, weights.ast_dependency);
        assert_eq!(
            additive_score,
            weights.ast_dependency + weights.git_dirty + weights.critical_file
        );
        assert!(p_norm_score > max_score);
        assert!(p_norm_score < additive_score);
    }

    #[test]
    fn test_invalid_p_norm_policy_fails_closed() {
        let temp_dir = tempfile::tempdir().unwrap();
        let root = temp_dir.path();
        fs::write(root.join("main.rs"), "fn main() {}\n").unwrap();

        let config = BreConfig {
            goal: "Invalid scoring policy".to_string(),
            acceptance_criteria: vec![],
            assumptions: vec![],
            non_goals: vec![],
            active_files: vec!["main.rs".to_string()],
            js_aliases: Default::default(),
            max_tokens: None,
            ignored_diagnostics: None,
            scoring_policy: ScoringPolicy::p_norm(0),
        };

        let err =
            build_context_tray(&config, root, 5000, &HeuristicWeights::default()).unwrap_err();

        assert!(err.contains("p_norm requires `p` to be at least 1"));
    }

    #[test]
    fn test_large_watching_file_skeletonized_before_budgeting() {
        let temp_dir = tempfile::tempdir().unwrap();
        let root = temp_dir.path();
        fs::create_dir_all(root.join("src")).unwrap();

        fs::write(root.join("src/main.rs"), "fn main() {}\n").unwrap();
        let large_body = "let value = value.wrapping_add(1);\n".repeat(1_000);
        let large_helper =
            format!("pub fn large_helper(mut value: usize) -> usize {{\n{large_body}value\n}}\n");
        assert!(large_helper.len() > skeleton::PRETOKENIZE_SKELETONIZE_BYTES);
        fs::write(root.join("src/helper.rs"), large_helper).unwrap();

        let config = BreConfig {
            goal: "Keep nearby helper context".to_string(),
            acceptance_criteria: vec![],
            assumptions: vec![],
            non_goals: vec![],
            active_files: vec!["src/main.rs".to_string()],
            js_aliases: Default::default(),
            max_tokens: None,
            ignored_diagnostics: None,
            scoring_policy: Default::default(),
        };

        let tray =
            build_context_tray(&config, root, 100_000, &HeuristicWeights::default()).unwrap();
        let helper = tray
            .watching
            .iter()
            .find(|file| file.path == "src/helper.rs")
            .expect("nearby helper should stay in watching");

        assert_eq!(helper.is_skeleton, Some(true));
        assert!(helper.content.as_ref().unwrap().contains("/* ... */"));
    }

    #[test]
    fn test_two_stage_ejection() {
        let temp_dir = tempfile::tempdir().unwrap();
        let root = temp_dir.path();

        let active_file = "active_file.rs";
        let watching_file = "watching.rs";

        let active_content = "fn main() {}"; // ~4 tokens
        let watching_content = "fn heavy_helper() { println!(\"long content line 1\"); println!(\"long content line 2\"); }"; // ~20 tokens

        fs::write(root.join(active_file), active_content).unwrap();
        fs::write(root.join(watching_file), watching_content).unwrap();

        let config = BreConfig {
            goal: "Test two stage".to_string(),
            acceptance_criteria: vec![],
            assumptions: vec![],
            non_goals: vec![],
            active_files: vec![active_file.to_string()],
            js_aliases: Default::default(),
            max_tokens: None,
            ignored_diagnostics: None,
            scoring_policy: Default::default(),
        };

        // We run with limit = 15.
        // Initial tokens = active (4) + watching (20) = 24.
        // Stage 1 should skeletonize watching_file -> "fn heavy_helper() { /* ... */ }" (~7 tokens).
        // Total becomes 4 + 7 = 11 <= 15.
        // So watching_file should be skeletonized and kept, NOT ejected!
        let tray = build_context_tray(&config, root, 15, &HeuristicWeights::default()).unwrap();
        assert!(tray.active.iter().any(|f| f.path == active_file));
        let watch_cf = tray
            .watching
            .iter()
            .find(|f| f.path == watching_file)
            .unwrap();
        assert_eq!(watch_cf.is_skeleton, Some(true));

        let tray_low = build_context_tray(&config, root, 8, &HeuristicWeights::default()).unwrap();
        assert!(tray_low.watching.is_empty());
        assert!(tray_low.background.iter().any(|f| f.path == watching_file));
    }

    #[test]
    fn test_context_boundary_enforcement() {
        let temp_dir = tempfile::tempdir().unwrap();
        let root = temp_dir.path();

        let active_file = "active_file.rs";
        let ast_file = "watching_ast.rs";
        let proximity_file = "watching_proximity.rs";
        let critical_file = ".env";

        // Write content:
        // active_file imports watching_ast.rs via 'mod watching_ast;'
        fs::write(root.join(active_file), "mod watching_ast; fn main() {}").unwrap();
        fs::write(root.join(ast_file), "struct AstDependency;").unwrap();
        fs::write(root.join(proximity_file), "struct ProximityDependency;").unwrap();
        fs::write(root.join(critical_file), "API_KEY=1234567890").unwrap();

        let config = BreConfig {
            goal: "Test token limit".to_string(),
            acceptance_criteria: vec![],
            assumptions: vec![],
            non_goals: vec![],
            active_files: vec![active_file.to_string()],
            js_aliases: Default::default(),
            max_tokens: None,
            ignored_diagnostics: None,
            scoring_policy: Default::default(),
        };

        // We run with limit = 15.
        // active_file is ~10 tokens.
        // ast_file is ~5 tokens.
        // proximity_file is ~5 tokens.
        // critical_file is ~7 tokens.
        // Total is ~27 tokens, which is > 15.
        // Coldest is critical_file (heat 30) -> should be ejected.
        // Next coldest is proximity_file (heat 50) -> should be ejected.
        // ast_file is AST dependency (heat 100) -> should be kept.
        let tray = build_context_tray(&config, root, 15, &HeuristicWeights::default()).unwrap();

        assert!(tray.active.iter().any(|f| f.path == active_file));
        assert!(tray.watching.iter().any(|f| f.path == ast_file));
        assert!(!tray.watching.iter().any(|f| f.path == proximity_file));
        assert!(!tray.watching.iter().any(|f| f.path == critical_file));
        assert!(tray.background.iter().any(|f| f.path == proximity_file));
        assert!(tray.background.iter().any(|f| f.path == critical_file));
    }

    #[test]
    fn test_context_report_includes_file_decision_traces() {
        let temp_dir = tempfile::tempdir().unwrap();
        let root = temp_dir.path();

        fs::write(root.join("main.rs"), "mod helper; fn main() {}\n").unwrap();
        fs::write(root.join("helper.rs"), "pub fn helper() {}\n").unwrap();

        let config = BreConfig {
            goal: "Change helper behavior".to_string(),
            acceptance_criteria: vec![],
            assumptions: vec![],
            non_goals: vec![],
            active_files: vec!["main.rs".to_string()],
            js_aliases: Default::default(),
            max_tokens: None,
            ignored_diagnostics: None,
            scoring_policy: Default::default(),
        };

        let (tray, report, traces) =
            build_context_tray_with_report(&config, root, 5000, &HeuristicWeights::default())
                .unwrap();

        assert_eq!(report.counts, ContextCounts::from_tray(&tray));
        let active_trace = traces
            .iter()
            .find(|trace| trace.path == "main.rs")
            .expect("active file should have a trace");
        assert_eq!(active_trace.tray, ContextTrayKind::Active);
        assert_eq!(active_trace.representation, FileRepresentation::Full);
        assert_eq!(active_trace.score, None);

        let helper_trace = traces
            .iter()
            .find(|trace| trace.path == "helper.rs")
            .expect("AST dependency should have a trace");
        assert_eq!(helper_trace.tray, ContextTrayKind::Watching);
        assert_eq!(helper_trace.representation, FileRepresentation::Full);
        assert!(helper_trace.signals.ast_dependency);
        assert!(helper_trace.score.is_some());
    }

    #[test]
    fn test_pruning_backfills_small_files_when_budget_remains() {
        let temp_dir = tempfile::tempdir().unwrap();
        let root = temp_dir.path();

        fs::create_dir_all(root.join("src")).unwrap();
        fs::create_dir_all(root.join(".github/workflows")).unwrap();
        fs::write(root.join("src/main.rs"), "fn main() {}\n").unwrap();
        fs::write(root.join(".env"), "A=1\n").unwrap();
        fs::write(
            root.join(".github/workflows/ci.yml"),
            "name: ci\n".repeat(200),
        )
        .unwrap();

        let config = BreConfig {
            goal: "Keep small critical context".to_string(),
            acceptance_criteria: vec![],
            assumptions: vec![],
            non_goals: vec![],
            active_files: vec!["src/main.rs".to_string()],
            js_aliases: Default::default(),
            max_tokens: None,
            ignored_diagnostics: None,
            scoring_policy: Default::default(),
        };

        let tray = build_context_tray(&config, root, 25, &HeuristicWeights::default()).unwrap();

        assert!(tray.watching.iter().any(|file| file.path == ".env"));
        assert!(tray
            .background
            .iter()
            .any(|file| file.path == ".github/workflows/ci.yml"));
    }

    #[test]
    fn test_active_file_nearest_manifest_is_watching_context() {
        let temp_dir = tempfile::tempdir().unwrap();
        let root = temp_dir.path();
        fs::write(
            root.join("Cargo.toml"),
            "[workspace]\nmembers = [\"crates/game\"]\n",
        )
        .unwrap();
        fs::create_dir_all(root.join("crates/game/examples")).unwrap();
        fs::write(
            root.join("crates/game/Cargo.toml"),
            "[package]\nname = \"game\"\nversion = \"0.1.0\"\n",
        )
        .unwrap();
        fs::write(
            root.join("crates/game/examples/collision.rs"),
            "fn main() {}\n",
        )
        .unwrap();
        let config = BreConfig {
            goal: "Change example behavior".to_string(),
            acceptance_criteria: vec![],
            assumptions: vec![],
            non_goals: vec![],
            active_files: vec!["crates/game/examples/collision.rs".to_string()],
            js_aliases: Default::default(),
            max_tokens: None,
            ignored_diagnostics: None,
            scoring_policy: Default::default(),
        };

        let tray = build_context_tray(&config, root, 5000, &HeuristicWeights::default()).unwrap();

        assert!(tray
            .watching
            .iter()
            .any(|file| file.path == "crates/game/Cargo.toml"));
    }

    #[test]
    fn test_rust_prelude_glob_expands_one_hop_dependencies() {
        let temp_dir = tempfile::tempdir().unwrap();
        let root = temp_dir.path();
        fs::write(
            root.join("Cargo.toml"),
            "[package]\nname = \"mini-game\"\nversion = \"0.1.0\"\n",
        )
        .unwrap();
        fs::create_dir_all(root.join("examples")).unwrap();
        fs::create_dir_all(root.join("src")).unwrap();
        fs::write(
            root.join("examples/input.rs"),
            "use mini_game::prelude::*;\nfn main() { is_key_down(KeyCode::A); }\n",
        )
        .unwrap();
        fs::write(
            root.join("src/lib.rs"),
            "pub mod input;\npub mod prelude;\n",
        )
        .unwrap();
        fs::write(
            root.join("src/prelude.rs"),
            "pub use crate::input::*;\npub use crate::audio::*;\npub use crate::rendering::*;\n",
        )
        .unwrap();
        fs::write(
            root.join("src/input.rs"),
            "pub enum KeyCode { A }\npub fn is_key_down(_: KeyCode) -> bool { false }\n",
        )
        .unwrap();
        fs::write(
            root.join("src/audio.rs"),
            "pub struct SoundEffect;\npub fn play_sound(_: SoundEffect) {}\n",
        )
        .unwrap();
        fs::write(
            root.join("src/rendering.rs"),
            "pub struct SpriteBatch;\npub fn draw_sprite(_: SpriteBatch) {}\n",
        )
        .unwrap();
        let config = BreConfig {
            goal: "Change input behavior".to_string(),
            acceptance_criteria: vec![],
            assumptions: vec![],
            non_goals: vec![],
            active_files: vec!["examples/input.rs".to_string()],
            js_aliases: Default::default(),
            max_tokens: None,
            ignored_diagnostics: None,
            scoring_policy: Default::default(),
        };

        let tray = build_context_tray(&config, root, 5000, &HeuristicWeights::default()).unwrap();
        let watching: HashSet<&str> = tray
            .watching
            .iter()
            .map(|file| file.path.as_str())
            .collect();

        assert!(watching.contains("src/prelude.rs"));
        assert!(watching.contains("src/input.rs"));
        assert!(!watching.contains("src/audio.rs"));
        assert!(!watching.contains("src/rendering.rs"));
    }

    #[test]
    fn test_symbol_definitions_from_active_context_are_watching_context() {
        let temp_dir = tempfile::tempdir().unwrap();
        let root = temp_dir.path();
        fs::write(
            root.join("Cargo.toml"),
            "[package]\nname = \"physics-game\"\nversion = \"0.1.0\"\n",
        )
        .unwrap();
        fs::create_dir_all(root.join("examples")).unwrap();
        fs::create_dir_all(root.join("src/collision/collider")).unwrap();
        fs::write(
            root.join("examples/collision_layers.rs"),
            r#"
                use physics_game::prelude::*;

                #[derive(PhysicsLayer, Default)]
                enum Layer { Default }

                fn main() {
                    let _ = CollisionLayers::new([Layer::Default], LayerMask::ALL);
                }
            "#,
        )
        .unwrap();
        fs::write(
            root.join("src/collision/collider/layers.rs"),
            r#"
                pub trait PhysicsLayer {}
                pub struct LayerMask;
                impl LayerMask { pub const ALL: Self = Self; }
                pub struct CollisionLayers;
                impl CollisionLayers {
                    pub fn new(_: impl Into<LayerMask>, _: impl Into<LayerMask>) -> Self {
                        Self
                    }
                }
            "#,
        )
        .unwrap();

        let config = BreConfig {
            goal: "Change collision layer behavior".to_string(),
            acceptance_criteria: vec![
                "Inspect CollisionLayers and LayerMask before editing.".to_string()
            ],
            assumptions: vec![],
            non_goals: vec![],
            active_files: vec!["examples/collision_layers.rs".to_string()],
            js_aliases: Default::default(),
            max_tokens: None,
            ignored_diagnostics: None,
            scoring_policy: Default::default(),
        };

        let tray = build_context_tray(&config, root, 5000, &HeuristicWeights::default()).unwrap();

        assert!(tray
            .watching
            .iter()
            .any(|file| file.path == "src/collision/collider/layers.rs"));
    }

    #[test]
    fn test_broad_example_directory_does_not_pull_all_siblings() {
        let temp_dir = tempfile::tempdir().unwrap();
        let root = temp_dir.path();
        fs::create_dir_all(root.join("examples")).unwrap();
        fs::write(
            root.join("examples/input_test.rs"),
            "struct MainState;\nfn main() { let _state = MainState; }\n",
        )
        .unwrap();
        fs::write(
            root.join("examples/unrelated.rs"),
            "struct MainState;\nfn main() {}\n",
        )
        .unwrap();
        let config = BreConfig {
            goal: "Focus example".to_string(),
            acceptance_criteria: vec![],
            assumptions: vec![],
            non_goals: vec![],
            active_files: vec!["examples/input_test.rs".to_string()],
            js_aliases: Default::default(),
            max_tokens: None,
            ignored_diagnostics: None,
            scoring_policy: Default::default(),
        };

        let tray = build_context_tray(&config, root, 5000, &HeuristicWeights::default()).unwrap();

        assert!(!tray
            .watching
            .iter()
            .any(|file| file.path == "examples/unrelated.rs"));
    }

    #[test]
    fn test_js_parent_imports_are_normalized_in_context_paths() {
        let temp_dir = tempfile::tempdir().unwrap();
        let root = temp_dir.path();
        fs::create_dir_all(root.join("src/core")).unwrap();
        fs::write(
            root.join("src/core/reducer.ts"),
            "import type { State } from '../types';\nexport const reducer = (state: State) => state;\n",
        )
        .unwrap();
        fs::write(
            root.join("src/types.ts"),
            "export type State = { id: string };\n",
        )
        .unwrap();
        let config = BreConfig {
            goal: "Change reducer state handling".to_string(),
            acceptance_criteria: vec![],
            assumptions: vec![],
            non_goals: vec![],
            active_files: vec!["src/core/reducer.ts".to_string()],
            js_aliases: Default::default(),
            max_tokens: None,
            ignored_diagnostics: None,
            scoring_policy: Default::default(),
        };

        let tray = build_context_tray(&config, root, 5000, &HeuristicWeights::default()).unwrap();
        let watching: HashSet<&str> = tray
            .watching
            .iter()
            .map(|file| file.path.as_str())
            .collect();

        assert!(watching.contains("src/types.ts"));
    }

    #[test]
    fn test_configured_js_aliases_feed_context_dependency_resolution() {
        let temp_dir = tempfile::tempdir().unwrap();
        let root = temp_dir.path();
        fs::create_dir_all(root.join("src/components")).unwrap();
        fs::write(
            root.join("src/App.tsx"),
            "import { Card } from '@/components/Card';\nexport const App = () => <Card />;\n",
        )
        .unwrap();
        fs::write(
            root.join("src/components/Card.tsx"),
            "export const Card = () => null;\n",
        )
        .unwrap();
        fs::write(
            root.join("vite.config.ts"),
            "const srcDir = path.resolve(__dirname, './src');\nexport default { resolve: { alias: { '@': srcDir } } };\n",
        )
        .unwrap();

        let config = BreConfig {
            goal: "Change card rendering".to_string(),
            acceptance_criteria: vec![],
            assumptions: vec![],
            non_goals: vec![],
            active_files: vec!["src/App.tsx".to_string()],
            js_aliases: [("@/*".to_string(), vec!["src/*".to_string()])]
                .into_iter()
                .collect(),
            max_tokens: None,
            ignored_diagnostics: None,
            scoring_policy: Default::default(),
        };

        let tray = build_context_tray(&config, root, 5000, &HeuristicWeights::default()).unwrap();
        let watching: HashSet<&str> = tray
            .watching
            .iter()
            .map(|file| file.path.as_str())
            .collect();

        assert!(watching.contains("src/components/Card.tsx"));
    }

    #[test]
    fn test_missing_active_file_fails_validation() {
        let temp_dir = tempfile::tempdir().unwrap();
        let root = temp_dir.path();
        let config = BreConfig {
            goal: "Missing active".to_string(),
            acceptance_criteria: vec![],
            assumptions: vec![],
            non_goals: vec![],
            active_files: vec!["src/missing.rs".to_string()],
            js_aliases: Default::default(),
            max_tokens: None,
            ignored_diagnostics: None,
            scoring_policy: Default::default(),
        };

        let err =
            build_context_tray(&config, root, 5000, &HeuristicWeights::default()).unwrap_err();

        assert!(err.contains("active file `src/missing.rs` does not exist"));
    }

    #[test]
    fn test_active_files_over_token_limit_fail_closed() {
        let temp_dir = tempfile::tempdir().unwrap();
        let root = temp_dir.path();
        fs::write(
            root.join("active.rs"),
            "fn main() { println!(\"too large\"); }\n",
        )
        .unwrap();
        let config = BreConfig {
            goal: "Over budget".to_string(),
            acceptance_criteria: vec![],
            assumptions: vec![],
            non_goals: vec![],
            active_files: vec!["active.rs".to_string()],
            js_aliases: Default::default(),
            max_tokens: None,
            ignored_diagnostics: None,
            scoring_policy: Default::default(),
        };

        let err = build_context_tray(&config, root, 1, &HeuristicWeights::default()).unwrap_err();

        assert!(err.contains("context boundary exceeded"));
    }

    #[test]
    fn test_symbol_scan_skips_fixture_directories_unless_tests_are_requested() {
        let temp_dir = tempfile::tempdir().unwrap();
        let root = temp_dir.path();
        fs::create_dir_all(root.join("src/platform")).unwrap();
        fs::create_dir_all(root.join("fixtures")).unwrap();
        fs::write(root.join("src/main.ts"), "export const main = true;\n").unwrap();
        fs::write(
            root.join("src/platform/keybindingResolver.ts"),
            "export class KeybindingResolver {}\n",
        )
        .unwrap();
        fs::write(
            root.join("fixtures/keybindingParser.ts"),
            "export class KeybindingResolver {}\n",
        )
        .unwrap();

        let config = BreConfig {
            goal: "Improve KeybindingResolver behavior".to_string(),
            acceptance_criteria: vec![],
            assumptions: vec![],
            non_goals: vec![],
            active_files: vec!["src/main.ts".to_string()],
            js_aliases: Default::default(),
            max_tokens: None,
            ignored_diagnostics: None,
            scoring_policy: Default::default(),
        };

        let tray = build_context_tray(&config, root, 5000, &HeuristicWeights::default()).unwrap();
        let watching: HashSet<&str> = tray
            .watching
            .iter()
            .map(|file| file.path.as_str())
            .collect();

        assert!(watching.contains("src/platform/keybindingResolver.ts"));
        assert!(!watching.contains("fixtures/keybindingParser.ts"));
    }

    #[test]
    fn test_unrelated_sibling_tests_do_not_load_by_proximity() {
        let temp_dir = tempfile::tempdir().unwrap();
        let root = temp_dir.path();
        fs::create_dir_all(root.join("src/core")).unwrap();
        fs::write(
            root.join("src/core/reducer.ts"),
            "export function reducer() { return 1; }\n",
        )
        .unwrap();
        fs::write(
            root.join("src/core/reducer.test.ts"),
            "test('reducer', () => expect(true).toBe(true));\n",
        )
        .unwrap();
        fs::write(
            root.join("src/core/flow.test.ts"),
            "test('flow', () => expect(true).toBe(true));\n",
        )
        .unwrap();
        let config = BreConfig {
            goal: "Focus reducer".to_string(),
            acceptance_criteria: vec![],
            assumptions: vec![],
            non_goals: vec![],
            active_files: vec!["src/core/reducer.ts".to_string()],
            js_aliases: Default::default(),
            max_tokens: None,
            ignored_diagnostics: None,
            scoring_policy: Default::default(),
        };

        let tray = build_context_tray(&config, root, 5000, &HeuristicWeights::default()).unwrap();
        let watching: HashSet<&str> = tray
            .watching
            .iter()
            .map(|file| file.path.as_str())
            .collect();

        assert!(watching.contains("src/core/reducer.test.ts"));
        assert!(!watching.contains("src/core/flow.test.ts"));
    }

    #[test]
    fn test_yaml_github_workflow_is_critical_watching_context() {
        let temp_dir = tempfile::tempdir().unwrap();
        let root = temp_dir.path();
        fs::create_dir_all(root.join(".github/workflows")).unwrap();
        fs::write(root.join(".github/workflows/ci.yaml"), "name: ci\n").unwrap();
        let config = BreConfig {
            goal: "Check workflow".to_string(),
            acceptance_criteria: vec![],
            assumptions: vec![],
            non_goals: vec![],
            active_files: vec![],
            js_aliases: Default::default(),
            max_tokens: None,
            ignored_diagnostics: None,
            scoring_policy: Default::default(),
        };

        let tray = build_context_tray(&config, root, 5000, &HeuristicWeights::default()).unwrap();

        assert!(tray
            .watching
            .iter()
            .any(|file| file.path == ".github/workflows/ci.yaml"));
    }

    #[test]
    fn test_action_and_typescript_config_are_critical_watching_context() {
        let temp_dir = tempfile::tempdir().unwrap();
        let root = temp_dir.path();
        fs::create_dir_all(root.join("src")).unwrap();
        fs::create_dir_all(root.join("fixtures")).unwrap();
        fs::write(root.join("src/main.ts"), "export const main = () => {}\n").unwrap();
        fs::write(root.join("action.yml"), "runs:\n  using: node24\n").unwrap();
        fs::write(root.join("tsconfig.json"), "{}\n").unwrap();
        fs::write(root.join("fixtures/tsconfig.json"), "{}\n").unwrap();
        let config = BreConfig {
            goal: "Update action input handling".to_string(),
            acceptance_criteria: vec![],
            assumptions: vec![],
            non_goals: vec![],
            active_files: vec!["src/main.ts".to_string()],
            js_aliases: Default::default(),
            max_tokens: None,
            ignored_diagnostics: None,
            scoring_policy: Default::default(),
        };

        let tray = build_context_tray(&config, root, 5000, &HeuristicWeights::default()).unwrap();
        let watching: HashSet<&str> = tray
            .watching
            .iter()
            .map(|file| file.path.as_str())
            .collect();

        assert!(watching.contains("action.yml"));
        assert!(watching.contains("tsconfig.json"));
        assert!(!watching.contains("fixtures/tsconfig.json"));
    }

    #[test]
    fn test_enterprise_framework_and_platform_files_are_watching_context() {
        let temp_dir = tempfile::tempdir().unwrap();
        let root = temp_dir.path();
        fs::create_dir_all(root.join("src/main/java/com/acme")).unwrap();
        fs::create_dir_all(root.join("src/main/java/com/acme/service")).unwrap();
        fs::create_dir_all(root.join("src/main/resources")).unwrap();
        fs::create_dir_all(root.join("deploy/k8s")).unwrap();
        fs::create_dir_all(root.join("kafka")).unwrap();
        fs::write(
            root.join("src/main/java/com/acme/App.java"),
            "package com.acme;\nimport com.acme.service.UserService;\nclass App {}\n",
        )
        .unwrap();
        fs::write(
            root.join("src/main/java/com/acme/service/UserService.java"),
            "package com.acme.service;\npublic class UserService {}\n",
        )
        .unwrap();
        fs::write(root.join("pom.xml"), "<project />\n").unwrap();
        fs::write(
            root.join("src/main/resources/application.yml"),
            "spring:\n  application:\n    name: app\n",
        )
        .unwrap();
        fs::write(
            root.join("deploy/k8s/service.yaml"),
            "apiVersion: v1\nkind: Service\n",
        )
        .unwrap();
        fs::write(root.join("kafka/producer.properties"), "acks=all\n").unwrap();
        let config = BreConfig {
            goal: "Update service startup behavior".to_string(),
            acceptance_criteria: vec![],
            assumptions: vec![],
            non_goals: vec![],
            active_files: vec!["src/main/java/com/acme/App.java".to_string()],
            js_aliases: Default::default(),
            max_tokens: None,
            ignored_diagnostics: None,
            scoring_policy: Default::default(),
        };

        let tray = build_context_tray(&config, root, 5000, &HeuristicWeights::default()).unwrap();
        let watching: HashSet<&str> = tray
            .watching
            .iter()
            .map(|file| file.path.as_str())
            .collect();

        assert!(watching.contains("pom.xml"));
        assert!(watching.contains("src/main/java/com/acme/service/UserService.java"));
        assert!(watching.contains("src/main/resources/application.yml"));
        assert!(watching.contains("deploy/k8s/service.yaml"));
        assert!(watching.contains("kafka/producer.properties"));
    }

    #[test]
    fn test_dotnet_project_file_is_active_manifest_context() {
        let temp_dir = tempfile::tempdir().unwrap();
        let root = temp_dir.path();
        fs::create_dir_all(root.join("src/App")).unwrap();
        fs::write(
            root.join("src/App/Program.cs"),
            "namespace App; public class Program {}\n",
        )
        .unwrap();
        fs::write(root.join("src/App/App.csproj"), "<Project />\n").unwrap();
        fs::write(root.join("src/App/appsettings.json"), "{}\n").unwrap();
        let config = BreConfig {
            goal: "Update ASP.NET startup behavior".to_string(),
            acceptance_criteria: vec![],
            assumptions: vec![],
            non_goals: vec![],
            active_files: vec!["src/App/Program.cs".to_string()],
            js_aliases: Default::default(),
            max_tokens: None,
            ignored_diagnostics: None,
            scoring_policy: Default::default(),
        };

        let tray = build_context_tray(&config, root, 5000, &HeuristicWeights::default()).unwrap();
        let watching: HashSet<&str> = tray
            .watching
            .iter()
            .map(|file| file.path.as_str())
            .collect();

        assert!(watching.contains("src/App/App.csproj"));
        assert!(watching.contains("src/App/appsettings.json"));
    }

    #[test]
    fn test_nested_env_files_are_not_global_critical_context() {
        let temp_dir = tempfile::tempdir().unwrap();
        let root = temp_dir.path();
        fs::create_dir_all(root.join("src")).unwrap();
        fs::create_dir_all(root.join("examples/react-web")).unwrap();
        fs::write(root.join("src/main.ts"), "export const main = () => {}\n").unwrap();
        fs::write(root.join(".env"), "ROOT_SECRET=1\n").unwrap();
        fs::write(root.join("examples/react-web/.env"), "DEMO_SECRET=1\n").unwrap();
        let config = BreConfig {
            goal: "Update main".to_string(),
            acceptance_criteria: vec![],
            assumptions: vec![],
            non_goals: vec![],
            active_files: vec!["src/main.ts".to_string()],
            js_aliases: Default::default(),
            max_tokens: None,
            ignored_diagnostics: None,
            scoring_policy: Default::default(),
        };

        let tray = build_context_tray(&config, root, 5000, &HeuristicWeights::default()).unwrap();
        let watching: HashSet<&str> = tray
            .watching
            .iter()
            .map(|file| file.path.as_str())
            .collect();

        assert!(watching.contains(".env"));
        assert!(!watching.contains("examples/react-web/.env"));
    }

    #[test]
    fn test_solve_mmkp_optimal_selection() {
        let candidates = vec![
            CandidateFile {
                path: "file_a.rs".to_string(),
                score: 100,
                full_content: "".to_string(),
                full_tokens: 100,
                skel_content: Some("".to_string()),
                skel_tokens: Some(50),
                signals: FileSignals::default(),
                was_pre_skeletonized: false,
            },
            CandidateFile {
                path: "file_b.rs".to_string(),
                score: 80,
                full_content: "".to_string(),
                full_tokens: 80,
                skel_content: Some("".to_string()),
                skel_tokens: Some(40),
                signals: FileSignals::default(),
                was_pre_skeletonized: false,
            },
            CandidateFile {
                path: "file_c.rs".to_string(),
                score: 0,
                full_content: "".to_string(),
                full_tokens: 50,
                skel_content: Some("".to_string()),
                skel_tokens: Some(25),
                signals: FileSignals::default(),
                was_pre_skeletonized: false,
            },
            CandidateFile {
                path: "file_d.rs".to_string(),
                score: 1, // Will test that zero-utility upgrades are avoided
                full_content: "".to_string(),
                full_tokens: 20,
                skel_content: Some("".to_string()),
                skel_tokens: Some(10), // utility(1) = round(0.6 * 1) = 1, utility(2) = 1 (delta_v = 0 from 1 to 2)
                signals: FileSignals::default(),
                was_pre_skeletonized: false,
            },
        ];

        // Case 1: Ample budgets. Everything except zero-score is selected fully, and file_d is kept at skeleton (1) because upgrading to 2 yields 0 extra utility.
        let states = solve_mmkp(&candidates, 1000, 10);
        assert_eq!(states[0], 2); // file_a: Full (Score 100)
        assert_eq!(states[1], 2); // file_b: Full (Score 80)
        assert_eq!(states[2], 0); // file_c: Omitted (Score 0)
        assert_eq!(states[3], 1); // file_d: Skeleton (Score 1, kept at 1 because 1 -> 2 yields 0 utility)

        // Case 2: Tight Token Budget. Total tokens cannot exceed 100.
        // Option 1: File A (Full) = 100 tokens, 1 file, utility = 100
        // Option 2: File A (Skel) + File B (Skel) = 50 + 40 = 90 tokens, 2 files, utility = 60 + 48 = 108
        // Option 3: File B (Full) = 80 tokens, 1 file, utility = 80
        // MMKP should choose Option 2 to maximize utility (108).
        let states_tight_tokens = solve_mmkp(&candidates, 100, 10);
        assert_eq!(states_tight_tokens[0], 1); // file_a: Skeleton (utility 60)
        assert_eq!(states_tight_tokens[1], 1); // file_b: Skeleton (utility 48)
        assert_eq!(states_tight_tokens[2], 0);
        assert_eq!(states_tight_tokens[3], 1); // file_d is also skeletonized (utility 1, 10 tokens) -> 50+40+10 = 100 tokens <= 100, utility = 109

        // Case 3: Tight File Budget. File budget is 1. Token budget is ample (1000).
        // File A (Full) has utility 100, File B (Full) has utility 80.
        // MMKP must choose File A (Full) and omit File B.
        let states_tight_files = solve_mmkp(&candidates, 1000, 1);
        assert_eq!(states_tight_files[0], 2); // file_a: Full (utility 100)
        assert_eq!(states_tight_files[1], 0); // file_b: Omitted (utility 0)
        assert_eq!(states_tight_files[2], 0);
        assert_eq!(states_tight_files[3], 0);

        // Case 4: Zero Budgets.
        let states_zero_tokens = solve_mmkp(&candidates, 0, 10);
        assert_eq!(states_zero_tokens, vec![0, 0, 0, 0]);

        let states_zero_files = solve_mmkp(&candidates, 1000, 0);
        assert_eq!(states_zero_files, vec![0, 0, 0, 0]);
    }
}
