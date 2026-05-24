use crate::config::{
    AgentPayload, BreConfig, DiagnosticRunReport, RunReport, ScoringPolicy, WeightRunReport,
};
use crate::context::build_context_tray_with_report;
use crate::diagnostics::run_diagnostics_with_report;
use crate::weights::{apply_feedback_loop, HeuristicWeights};
use std::collections::HashMap;
use std::path::Path;

#[derive(Debug, Clone, Default)]
pub struct BreRunOptions {
    pub max_tokens: Option<usize>,
    pub skip_diagnostics: bool,
    pub include_report: bool,
    pub js_aliases: HashMap<String, Vec<String>>,
    pub scoring_policy: Option<ScoringPolicy>,
}

/// Runs the Boundary Reasoning Environment (BRE) context-retrieval pipeline.
/// Returns a structured AgentPayload containing the context tray, compiler diagnostics, and optional reports.
pub fn run_bre(
    repo_root: &Path,
    config: BreConfig,
    options: BreRunOptions,
) -> Result<AgentPayload, String> {
    let mut config = config;

    // 0. Merge JS sidecar alias map
    for (alias, source_targets) in options.js_aliases.clone() {
        let target_targets = config.js_aliases.entry(alias).or_default();
        for source_target in source_targets {
            if !target_targets.contains(&source_target) {
                target_targets.push(source_target);
            }
        }
    }
    if let Some(scoring_policy) = options.scoring_policy.clone() {
        config.scoring_policy = scoring_policy;
    }

    // 1. Load Heuristic Weights
    let mut weights = HeuristicWeights::load_or_default(repo_root);
    let original_weights = weights.clone();

    // 2. Compile-time Diagnostics Run with Ignored Exclusions
    let ignored_patterns = config.ignored_diagnostics.clone().unwrap_or_default();
    let (diagnostics, diagnostic_report) = if options.skip_diagnostics {
        (
            Vec::new(),
            DiagnosticRunReport::skipped(ignored_patterns.len()),
        )
    } else {
        run_diagnostics_with_report(repo_root, &ignored_patterns)
    };

    // 3. Context Tray Builder
    let max_tokens = options.max_tokens.or(config.max_tokens).unwrap_or(30000);
    let (tray, context_report, file_traces) =
        build_context_tray_with_report(&config, repo_root, max_tokens, &weights)?;

    // 4. Execute Compile-time Heuristic Learning Feedback Loop.
    // Signal traces drive targeted weight updates for ejected/background files.
    let feedback_applied = !options.skip_diagnostics;
    if feedback_applied {
        apply_feedback_loop(&diagnostics, &tray, &file_traces, &mut weights, repo_root, &config.js_aliases);
    }

    // 5. Compile Report Summary
    let report = if options.include_report {
        let repo_root_display = repo_root
            .canonicalize()
            .unwrap_or_else(|_| repo_root.to_path_buf());
        let weights_path = repo_root.join("bre_weights.json");
        Some(RunReport {
            repo_root: repo_root_display.display().to_string(),
            diagnostics: diagnostic_report,
            context: context_report,
            weights: WeightRunReport {
                weights_path: weights_path.display().to_string(),
                changed: weights != original_weights,
                file_exists_after_run: weights_path.exists(),
                feedback_applied,
            },
            file_traces,
        })
    } else {
        None
    };

    Ok(AgentPayload {
        goal: config.goal.clone(),
        acceptance_criteria: config.acceptance_criteria.clone(),
        assumptions: config.assumptions.clone(),
        non_goals: config.non_goals.clone(),
        context: tray,
        diagnostics,
        report,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_run_bre_returns_payload_with_optional_report() {
        let temp_dir = tempfile::tempdir().unwrap();
        let root = temp_dir.path();
        std::fs::create_dir_all(root.join("src")).unwrap();
        std::fs::write(root.join("src/main.rs"), "fn main() {}\n").unwrap();

        let config = BreConfig {
            goal: "Inspect main".to_string(),
            acceptance_criteria: vec![],
            assumptions: vec![],
            non_goals: vec![],
            active_files: vec!["src/main.rs".to_string()],
            js_aliases: Default::default(),
            max_tokens: None,
            ignored_diagnostics: None,
            scoring_policy: Default::default(),
        };

        let payload = run_bre(
            root,
            config,
            BreRunOptions {
                max_tokens: None,
                skip_diagnostics: true,
                include_report: true,
                js_aliases: Default::default(),
                scoring_policy: None,
            },
        )
        .unwrap();

        assert_eq!(payload.context.active.len(), 1);
        assert!(payload.diagnostics.is_empty());
        let report = payload.report.unwrap();
        assert_eq!(report.context.counts.active, 1);
        assert_eq!(report.context.counts.watching, 0);
        assert_eq!(report.context.counts.background, 0);
        assert_eq!(report.context.scoring_policy, ScoringPolicy::max());
        assert!(!report.weights.changed);
        assert!(!report.weights.feedback_applied);
        assert_eq!(report.file_traces.len(), 1);
        assert_eq!(report.file_traces[0].path, "src/main.rs");
    }

    #[test]
    fn test_run_bre_omits_report_when_not_requested() {
        let temp_dir = tempfile::tempdir().unwrap();
        let root = temp_dir.path();
        std::fs::create_dir_all(root.join("src")).unwrap();
        std::fs::write(root.join("src/main.rs"), "fn main() {}\n").unwrap();

        let config = BreConfig {
            goal: "Inspect main".to_string(),
            acceptance_criteria: vec![],
            assumptions: vec![],
            non_goals: vec![],
            active_files: vec!["src/main.rs".to_string()],
            js_aliases: Default::default(),
            max_tokens: None,
            ignored_diagnostics: None,
            scoring_policy: Default::default(),
        };

        let payload = run_bre(
            root,
            config,
            BreRunOptions {
                max_tokens: None,
                skip_diagnostics: true,
                include_report: false,
                js_aliases: Default::default(),
                scoring_policy: None,
            },
        )
        .unwrap();

        assert!(payload.report.is_none());
        let value = serde_json::to_value(&payload).unwrap();
        assert!(value.get("report").is_none());
        assert!(value["context"]["active"][0].get("signals").is_none());
    }
}
