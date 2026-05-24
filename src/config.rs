use clap::Parser;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs;
use std::path::Path;
use std::path::PathBuf;

#[derive(clap::ValueEnum, Clone, Debug, PartialEq)]
pub enum OutputFormat {
    Json,
    Prompt,
}

#[derive(clap::ValueEnum, Clone, Debug, PartialEq)]
pub enum CliScoringPolicy {
    Max,
    Additive,
    PNorm,
}

impl CliScoringPolicy {
    pub fn into_policy(self, p: Option<u32>) -> ScoringPolicy {
        match self {
            Self::Max => ScoringPolicy::max(),
            Self::Additive => ScoringPolicy::additive(),
            Self::PNorm => ScoringPolicy::p_norm(p.unwrap_or(2)),
        }
    }
}

#[derive(Parser, Debug)]
pub struct Cli {
    #[arg(default_value = ".")]
    pub path: PathBuf,

    #[arg(short, long, value_enum, default_value_t = OutputFormat::Json)]
    pub format: OutputFormat,

    #[arg(long)]
    pub max_tokens: Option<usize>,

    #[arg(long)]
    pub skip_diagnostics: bool,

    #[arg(long)]
    pub js_aliases: Option<String>,

    #[arg(long, value_enum)]
    pub scoring_policy: Option<CliScoringPolicy>,

    #[arg(long)]
    pub scoring_p: Option<u32>,

    #[arg(long)] // Added: Request an evidence report summary
    pub report: bool,

    #[arg(long)]
    pub mcp: bool,
}

pub type JsAliasMap = HashMap<String, Vec<String>>;

#[derive(Debug, Serialize, Deserialize)]
pub struct BreConfig {
    pub goal: String,
    pub acceptance_criteria: Vec<String>,
    pub assumptions: Vec<String>,
    pub non_goals: Vec<String>,
    pub active_files: Vec<String>,
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub js_aliases: JsAliasMap,
    #[serde(default)]
    pub max_tokens: Option<usize>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ignored_diagnostics: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "ScoringPolicy::is_default")]
    pub scoring_policy: ScoringPolicy,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ScoringPolicyMode {
    #[default]
    Max,
    Additive,
    PNorm,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ScoringPolicy {
    #[serde(default)]
    pub mode: ScoringPolicyMode,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub p: Option<u32>,
}

impl Default for ScoringPolicy {
    fn default() -> Self {
        Self::max()
    }
}

impl ScoringPolicy {
    pub fn max() -> Self {
        Self {
            mode: ScoringPolicyMode::Max,
            p: None,
        }
    }

    pub fn additive() -> Self {
        Self {
            mode: ScoringPolicyMode::Additive,
            p: None,
        }
    }

    pub fn p_norm(p: u32) -> Self {
        Self {
            mode: ScoringPolicyMode::PNorm,
            p: Some(p),
        }
    }

    pub fn is_default(&self) -> bool {
        self == &Self::default()
    }

    pub fn validate(&self) -> Result<(), String> {
        if self.mode == ScoringPolicyMode::PNorm && self.p.unwrap_or(2) == 0 {
            return Err("scoring_policy p_norm requires `p` to be at least 1".to_string());
        }
        Ok(())
    }

    pub fn p_value(&self) -> u32 {
        self.p.unwrap_or(2).max(1)
    }

    pub fn label(&self) -> String {
        match self.mode {
            ScoringPolicyMode::Max => "max".to_string(),
            ScoringPolicyMode::Additive => "additive".to_string(),
            ScoringPolicyMode::PNorm => format!("p_norm(p={})", self.p_value()),
        }
    }
}

pub fn load_js_alias_map(path: &Path) -> Result<JsAliasMap, String> {
    let content = fs::read_to_string(path)
        .map_err(|err| format!("failed to read {}: {err}", path.display()))?;
    let value: serde_json::Value = serde_json::from_str(&content)
        .map_err(|err| format!("failed to parse {}: {err}", path.display()))?;
    parse_js_alias_map(&value)
}

pub fn parse_js_alias_map(value: &serde_json::Value) -> Result<JsAliasMap, String> {
    let alias_value = value.get("js_aliases").unwrap_or(value);
    let Some(object) = alias_value.as_object() else {
        return Err("JS alias map must be an object or contain a `js_aliases` object".to_string());
    };

    let mut aliases = JsAliasMap::new();
    for (alias, targets) in object {
        if let Some(target) = targets.as_str() {
            aliases.insert(alias.clone(), vec![target.to_string()]);
            continue;
        }

        let Some(targets) = targets.as_array() else {
            return Err(format!(
                "JS alias `{alias}` must map to a string or string array"
            ));
        };

        let mut parsed_targets = Vec::new();
        for target in targets {
            let Some(target) = target.as_str() else {
                return Err(format!("JS alias `{alias}` contains a non-string target"));
            };
            parsed_targets.push(target.to_string());
        }
        aliases.insert(alias.clone(), parsed_targets);
    }

    Ok(aliases)
}

pub fn merge_js_alias_maps(target: &mut JsAliasMap, source: JsAliasMap) {
    for (alias, source_targets) in source {
        let target_targets = target.entry(alias).or_default();
        for source_target in source_targets {
            if !target_targets.contains(&source_target) {
                target_targets.push(source_target);
            }
        }
    }
}

/// Per-file signal trace: which heuristics fired and contributed to this file's score.
/// Only present in the payload when `--report` is requested.
#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq, Eq)]
pub struct FileSignals {
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub ast_dependency: bool,
    /// Raw symbol score (0 = no match). Encodes both hit presence and match count.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub symbol_score: usize,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub active_manifest: bool,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub git_dirty: bool,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub active_parent: bool,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub task_term_matched: bool,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub task_term_ast_matched: bool,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub critical_file: bool,
}

fn is_zero(v: &usize) -> bool {
    *v == 0
}

impl FileSignals {
    /// Returns true if at least one signal fired for this file.
    pub fn any(&self) -> bool {
        self.ast_dependency
            || self.symbol_score > 0
            || self.active_manifest
            || self.git_dirty
            || self.active_parent
            || self.task_term_matched
            || self.task_term_ast_matched
            || self.critical_file
    }

    /// Human-readable label list for prompt output.
    pub fn labels(&self) -> Vec<&'static str> {
        let mut out = Vec::new();
        if self.ast_dependency {
            out.push("ast-import");
        }
        if self.symbol_score > 0 {
            out.push("symbol-match");
        }
        if self.active_manifest {
            out.push("manifest");
        }
        if self.git_dirty {
            out.push("git-dirty");
        }
        if self.active_parent {
            out.push("proximity");
        }
        if self.task_term_ast_matched {
            out.push("task-term+ast");
        } else if self.task_term_matched {
            out.push("task-term");
        }
        if self.critical_file {
            out.push("critical");
        }
        out
    }
}

#[derive(Debug, Serialize, Deserialize, Clone, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ContextTrayKind {
    Active,
    Watching,
    Background,
}

#[derive(Debug, Serialize, Deserialize, Clone, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum FileRepresentation {
    Full,
    Skeleton,
    Omitted,
}

#[derive(Debug, Serialize, Deserialize, Clone, PartialEq, Eq)]
pub struct FileDecisionTrace {
    pub path: String,
    pub tray: ContextTrayKind,
    pub representation: FileRepresentation,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub score: Option<usize>,
    pub signals: FileSignals,
}

#[derive(Debug, Serialize, Deserialize, Clone, Default, PartialEq, Eq)]
pub struct ContextFile {
    pub path: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub content: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub is_skeleton: Option<bool>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct ContextTray {
    pub active: Vec<ContextFile>,
    pub watching: Vec<ContextFile>,
    pub background: Vec<ContextFile>,
}

#[derive(Debug, Serialize, Deserialize, Clone, PartialEq, Eq)]
pub struct DiagnosticError {
    pub file: String,
    pub line: usize,
    pub message: String,
    pub compiler: String,
}

#[derive(Debug, Serialize, Deserialize, Clone, PartialEq, Eq)]
pub struct ContextCounts {
    pub active: usize,
    pub watching: usize,
    pub background: usize,
}

impl ContextCounts {
    pub fn from_tray(tray: &ContextTray) -> Self {
        Self {
            active: tray.active.len(),
            watching: tray.watching.len(),
            background: tray.background.len(),
        }
    }
}

#[derive(Debug, Serialize, Deserialize, Clone, PartialEq, Eq)]
pub struct ContextBuildReport {
    pub counts: ContextCounts,
    pub scoring_policy: ScoringPolicy,
    pub budget_action: String,
    pub max_tokens: usize,
    pub initial_tokens: usize,
    pub final_tokens: usize,
    pub active_tokens: usize,
    pub skeletonized_files: usize,
    pub ejected_files: usize,
    pub max_loaded_files: usize,
    pub loaded_files: usize,
}

#[derive(Debug, Serialize, Deserialize, Clone, PartialEq, Eq)]
pub struct DiagnosticRunReport {
    pub skipped: bool,
    pub project_types: Vec<String>,
    pub adapters: Vec<String>,
    pub ignored_patterns: usize,
    pub raw_errors: usize,
    pub ignored_errors: usize,
    pub emitted_errors: usize,
}

impl DiagnosticRunReport {
    pub fn new(ignored_patterns: usize) -> Self {
        Self {
            skipped: false,
            project_types: Vec::new(),
            adapters: Vec::new(),
            ignored_patterns,
            raw_errors: 0,
            ignored_errors: 0,
            emitted_errors: 0,
        }
    }

    pub fn skipped(ignored_patterns: usize) -> Self {
        Self {
            skipped: true,
            ..Self::new(ignored_patterns)
        }
    }
}

#[derive(Debug, Serialize, Deserialize, Clone, PartialEq, Eq)]
pub struct WeightRunReport {
    pub weights_path: String,
    pub changed: bool,
    pub file_exists_after_run: bool,
    pub feedback_applied: bool,
}

#[derive(Debug, Serialize, Deserialize, Clone, PartialEq, Eq)]
pub struct RunReport {
    pub repo_root: String,
    pub diagnostics: DiagnosticRunReport,
    pub context: ContextBuildReport,
    pub weights: WeightRunReport,
    pub file_traces: Vec<FileDecisionTrace>,
}

#[derive(Debug, Serialize)]
pub struct AgentPayload {
    pub goal: String,
    pub acceptance_criteria: Vec<String>,
    pub assumptions: Vec<String>,
    pub non_goals: Vec<String>,
    pub context: ContextTray,
    pub diagnostics: Vec<DiagnosticError>,
    #[serde(skip_serializing_if = "Option::is_none")] // Added: Only include report when requested
    pub report: Option<RunReport>,
}

pub fn serialize_prompt_payload(payload: &AgentPayload) -> String {
    let mut out = String::new();
    let trace_by_path: HashMap<&str, &FileDecisionTrace> = payload
        .report
        .as_ref()
        .map(|report| {
            report
                .file_traces
                .iter()
                .map(|trace| (trace.path.as_str(), trace))
                .collect()
        })
        .unwrap_or_default();

    if let Some(report) = &payload.report {
        out.push_str("--- BRE RUN REPORT ---\n");
        out.push_str(&format!("Repo Root: {}\n", report.repo_root));
        let project_types = format_report_list(&report.diagnostics.project_types);
        let adapters = format_report_list(&report.diagnostics.adapters);
        out.push_str(&format!("Project Types: {project_types}\n"));
        out.push_str(&format!("Diagnostics Adapters: {adapters}\n"));
        out.push_str(&format!(
            "Diagnostics: {} emitted, {} ignored, {} raw{}\n",
            report.diagnostics.emitted_errors,
            report.diagnostics.ignored_errors,
            report.diagnostics.raw_errors,
            if report.diagnostics.skipped {
                " (skipped)"
            } else {
                ""
            }
        ));
        out.push_str(&format!(
            "Context: {} Active, {} Watching, {} Background\n",
            report.context.counts.active,
            report.context.counts.watching,
            report.context.counts.background
        ));
        out.push_str(&format!(
            "Scoring Policy: {}\n",
            report.context.scoring_policy.label()
        ));
        out.push_str(&format!(
            "Tokens: {} -> {}\n",
            report.context.initial_tokens, report.context.final_tokens
        ));
        out.push_str(&format!(
            "Skeletonized Files Count: {}, Ejected Files Count: {}\n",
            report.context.skeletonized_files, report.context.ejected_files
        ));
        out.push_str(&format!(
            "Adaptive Weights Changed: {}\n\n",
            if report.weights.changed { "yes" } else { "no" }
        ));
    }

    if !payload.diagnostics.is_empty() {
        out.push_str("--- BUILD DIAGNOSTICS ---\n");
        out.push_str("The following compilation/type check errors were found in the codebase. Fix them first:\n\n");
        for err in &payload.diagnostics {
            out.push_str(&format!(
                "[{}] File: {}:{}\n",
                err.compiler, err.file, err.line
            ));
            out.push_str(&format!("Error: {}\n\n", err.message));
        }
        out.push('\n');
    }

    out.push_str("--- TASK CONTRACT ---\n");
    out.push_str(&format!("Goal: {}\n\n", payload.goal));

    if !payload.acceptance_criteria.is_empty() {
        out.push_str("Acceptance Criteria:\n");
        for criterion in &payload.acceptance_criteria {
            out.push_str(&format!("- {}\n", criterion));
        }
        out.push('\n');
    }

    if !payload.assumptions.is_empty() {
        out.push_str("Assumptions:\n");
        for assumption in &payload.assumptions {
            out.push_str(&format!("- {}\n", assumption));
        }
        out.push('\n');
    }

    if !payload.non_goals.is_empty() {
        out.push_str("Non-Goals:\n");
        for non_goal in &payload.non_goals {
            out.push_str(&format!("- {}\n", non_goal));
        }
        out.push('\n');
    }

    out.push_str("--- CONTEXT TRAY ---\n\n");

    out.push_str("Active Files:\n");
    if payload.context.active.is_empty() {
        out.push_str("(none)\n\n");
    } else {
        for file in &payload.context.active {
            out.push_str("================================================================================\n");
            out.push_str(&format!("File: {}\n", file.path));
            out.push_str("================================================================================\n");
            if let Some(ref content) = file.content {
                out.push_str(content);
            }
            if !out.ends_with('\n') {
                out.push('\n');
            }
            out.push('\n');
        }
    }

    out.push_str("Watching Files:\n");
    if payload.context.watching.is_empty() {
        out.push_str("(none)\n\n");
    } else {
        for file in &payload.context.watching {
            let label = if file.is_skeleton.unwrap_or(false) {
                format!("{} (Skeletonized)", file.path)
            } else {
                file.path.clone()
            };
            out.push_str("================================================================================\n");
            out.push_str(&format!("File: {}\n", label));
            if let Some(trace) = trace_by_path.get(file.path.as_str()) {
                let signals = &trace.signals;
                let labels = signals.labels();
                if !labels.is_empty() {
                    let score = trace
                        .score
                        .map(|score| score.to_string())
                        .unwrap_or_else(|| "n/a".to_string());
                    out.push_str(&format!("Why: {} (score: {score})\n", labels.join(", ")));
                }
            }
            out.push_str("================================================================================\n");
            if let Some(ref content) = file.content {
                out.push_str(content);
            }
            if !out.ends_with('\n') {
                out.push('\n');
            }
            out.push('\n');
        }
    }

    out.push_str("Background Files (Excluded from context):\n");
    if payload.context.background.is_empty() {
        out.push_str("(none)\n");
    } else {
        for file in &payload.context.background {
            if let Some(trace) = trace_by_path.get(file.path.as_str()) {
                let signals = &trace.signals;
                let labels = signals.labels();
                if labels.is_empty() {
                    out.push_str(&format!("- {} (no signals — coverage gap)\n", file.path));
                } else {
                    let score = trace
                        .score
                        .map(|score| score.to_string())
                        .unwrap_or_else(|| "n/a".to_string());
                    out.push_str(&format!(
                        "- {} (ejected; score: {score}; signals: {})\n",
                        file.path,
                        labels.join(", ")
                    ));
                }
            } else {
                out.push_str(&format!("- {}\n", file.path));
            }
        }
    }

    out
}

fn format_report_list(values: &[String]) -> String {
    if values.is_empty() {
        "(none)".to_string()
    } else {
        values.join(", ")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_serialize_prompt_payload() {
        let payload = AgentPayload {
            goal: "Refactor auth".to_string(),
            acceptance_criteria: vec!["Secure validation".to_string()],
            assumptions: vec!["DB is up to date".to_string()],
            non_goals: vec!["Do not rewrite UI".to_string()],
            context: ContextTray {
                active: vec![ContextFile {
                    path: "src/main.rs".to_string(),
                    content: Some("pub fn main() {}".to_string()),
                    is_skeleton: Some(false),
                }],
                watching: vec![ContextFile {
                    path: "Cargo.toml".to_string(),
                    content: Some("[package]\nname = \"bre\"".to_string()),
                    is_skeleton: Some(false),
                }],
                background: vec![ContextFile {
                    path: "bre.json".to_string(),
                    ..Default::default()
                }],
            },
            diagnostics: vec![],
            report: None,
        };

        let output = serialize_prompt_payload(&payload);
        assert!(output.contains("--- TASK CONTRACT ---"));
        assert!(output.contains("Goal: Refactor auth"));
        assert!(output.contains("- Secure validation"));
        assert!(output.contains("- DB is up to date"));
        assert!(output.contains("- Do not rewrite UI"));
        assert!(output.contains("--- CONTEXT TRAY ---"));
        assert!(output.contains("File: src/main.rs"));
        assert!(output.contains("pub fn main() {}"));
        assert!(output.contains("File: Cargo.toml"));
        assert!(output.contains("[package]"));
        assert!(output.contains("- bre.json"));
    }

    #[test]
    fn test_serialize_prompt_payload_includes_run_report_when_enabled() {
        let payload = AgentPayload {
            goal: "Audit release readiness".to_string(),
            acceptance_criteria: vec![],
            assumptions: vec![],
            non_goals: vec![],
            context: ContextTray {
                active: vec![],
                watching: vec![ContextFile {
                    path: "src/auth.rs".to_string(),
                    content: Some("pub fn check() {}".to_string()),
                    is_skeleton: Some(false),
                }],
                background: vec![ContextFile {
                    path: "src/rate_limit.rs".to_string(),
                    ..Default::default()
                }],
            },
            diagnostics: vec![],
            report: Some(RunReport {
                repo_root: "/tmp/project".to_string(),
                diagnostics: DiagnosticRunReport {
                    skipped: false,
                    project_types: vec!["svelte".to_string()],
                    adapters: vec!["npm run check".to_string()],
                    ignored_patterns: 1,
                    raw_errors: 2,
                    ignored_errors: 1,
                    emitted_errors: 1,
                },
                context: ContextBuildReport {
                    counts: ContextCounts {
                        active: 2,
                        watching: 3,
                        background: 5,
                    },
                    scoring_policy: ScoringPolicy::max(),
                    budget_action: "trimmed_to_token_budget".to_string(),
                    max_tokens: 30000,
                    initial_tokens: 42000,
                    final_tokens: 28000,
                    active_tokens: 12000,
                    skeletonized_files: 1,
                    ejected_files: 4,
                    max_loaded_files: 48,
                    loaded_files: 5,
                },
                weights: WeightRunReport {
                    weights_path: "/tmp/project/bre_weights.json".to_string(),
                    changed: false,
                    file_exists_after_run: true,
                    feedback_applied: true,
                },
                file_traces: vec![
                    FileDecisionTrace {
                        path: "src/auth.rs".to_string(),
                        tray: ContextTrayKind::Watching,
                        representation: FileRepresentation::Full,
                        score: Some(120),
                        signals: FileSignals {
                            ast_dependency: true,
                            ..Default::default()
                        },
                    },
                    FileDecisionTrace {
                        path: "src/rate_limit.rs".to_string(),
                        tray: ContextTrayKind::Background,
                        representation: FileRepresentation::Omitted,
                        score: Some(50),
                        signals: FileSignals {
                            active_parent: true,
                            ..Default::default()
                        },
                    },
                ],
            }),
        };

        let output = serialize_prompt_payload(&payload);
        assert!(output.contains("--- BRE RUN REPORT ---"));
        assert!(output.contains("Repo Root: /tmp/project"));
        assert!(output.contains("Project Types: svelte"));
        assert!(output.contains("Diagnostics Adapters: npm run check"));
        assert!(output.contains("Diagnostics: 1 emitted, 1 ignored, 2 raw"));
        assert!(output.contains("Context: 2 Active, 3 Watching, 5 Background"));
        assert!(output.contains("Scoring Policy: max"));
        assert!(output.contains("Tokens: 42000 -> 28000"));
        assert!(output.contains("Adaptive Weights Changed: no"));
        assert!(output.contains("Why: ast-import (score: 120)"));
        assert!(output.contains("- src/rate_limit.rs (ejected; score: 50; signals: proximity)"));
    }

    #[test]
    fn test_file_signal_labels_are_stable() {
        let signals = FileSignals {
            ast_dependency: true,
            symbol_score: 100,
            active_manifest: true,
            git_dirty: true,
            active_parent: true,
            task_term_matched: true,
            task_term_ast_matched: true,
            critical_file: true,
        };

        assert_eq!(
            signals.labels(),
            vec![
                "ast-import",
                "symbol-match",
                "manifest",
                "git-dirty",
                "proximity",
                "task-term+ast",
                "critical"
            ]
        );
    }

    #[test]
    fn test_run_report_serializes_nested_shape_with_traces() {
        let report = RunReport {
            repo_root: "/tmp/project".to_string(),
            diagnostics: DiagnosticRunReport::skipped(0),
            context: ContextBuildReport {
                counts: ContextCounts {
                    active: 1,
                    watching: 1,
                    background: 1,
                },
                scoring_policy: ScoringPolicy::max(),
                budget_action: "within_budget".to_string(),
                max_tokens: 30000,
                initial_tokens: 20,
                final_tokens: 20,
                active_tokens: 10,
                skeletonized_files: 0,
                ejected_files: 0,
                max_loaded_files: 48,
                loaded_files: 2,
            },
            weights: WeightRunReport {
                weights_path: "/tmp/project/bre_weights.json".to_string(),
                changed: false,
                file_exists_after_run: false,
                feedback_applied: false,
            },
            file_traces: vec![FileDecisionTrace {
                path: "src/main.rs".to_string(),
                tray: ContextTrayKind::Active,
                representation: FileRepresentation::Full,
                score: None,
                signals: FileSignals::default(),
            }],
        };

        let value = serde_json::to_value(&report).unwrap();
        assert_eq!(value["repo_root"], "/tmp/project");
        assert_eq!(value["diagnostics"]["skipped"], true);
        assert_eq!(value["context"]["counts"]["active"], 1);
        assert_eq!(value["context"]["scoring_policy"]["mode"], "max");
        assert_eq!(value["weights"]["feedback_applied"], false);
        assert_eq!(value["file_traces"][0]["tray"], "active");
        assert_eq!(value["file_traces"][0]["representation"], "full");
    }

    #[test]
    fn test_bre_config_defaults_to_max_scoring_policy() {
        let raw = serde_json::json!({
            "goal": "Inspect",
            "acceptance_criteria": [],
            "assumptions": [],
            "non_goals": [],
            "active_files": []
        });

        let config: BreConfig = serde_json::from_value(raw).unwrap();

        assert_eq!(config.scoring_policy, ScoringPolicy::max());
    }

    #[test]
    fn test_bre_config_accepts_additive_and_p_norm_scoring_policies() {
        let additive = serde_json::json!({
            "goal": "Inspect",
            "acceptance_criteria": [],
            "assumptions": [],
            "non_goals": [],
            "active_files": [],
            "scoring_policy": {
                "mode": "additive"
            }
        });
        let p_norm = serde_json::json!({
            "goal": "Inspect",
            "acceptance_criteria": [],
            "assumptions": [],
            "non_goals": [],
            "active_files": [],
            "scoring_policy": {
                "mode": "p_norm",
                "p": 3
            }
        });

        let additive: BreConfig = serde_json::from_value(additive).unwrap();
        let p_norm: BreConfig = serde_json::from_value(p_norm).unwrap();

        assert_eq!(additive.scoring_policy, ScoringPolicy::additive());
        assert_eq!(p_norm.scoring_policy, ScoringPolicy::p_norm(3));
    }

    #[test]
    fn test_parse_js_alias_map_accepts_raw_or_wrapped_maps() {
        let raw = serde_json::json!({
            "@/*": ["src/*"],
            "~": "src"
        });
        let parsed = parse_js_alias_map(&raw).unwrap();
        assert_eq!(parsed.get("@/*").unwrap(), &vec!["src/*".to_string()]);
        assert_eq!(parsed.get("~").unwrap(), &vec!["src".to_string()]);

        let wrapped = serde_json::json!({
            "js_aliases": {
                "@components/*": ["src/components/*"]
            }
        });
        let parsed = parse_js_alias_map(&wrapped).unwrap();
        assert_eq!(
            parsed.get("@components/*").unwrap(),
            &vec!["src/components/*".to_string()]
        );
    }

    #[test]
    fn test_merge_js_alias_maps_dedupes_targets() {
        let mut target = JsAliasMap::from([(
            "@/*".to_string(),
            vec!["src/*".to_string(), "app/*".to_string()],
        )]);
        let source = JsAliasMap::from([(
            "@/*".to_string(),
            vec!["src/*".to_string(), "packages/app/src/*".to_string()],
        )]);

        merge_js_alias_maps(&mut target, source);

        assert_eq!(
            target.get("@/*").unwrap(),
            &vec![
                "src/*".to_string(),
                "app/*".to_string(),
                "packages/app/src/*".to_string()
            ]
        );
    }
}
