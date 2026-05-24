use crate::config::{DiagnosticError, DiagnosticRunReport};
use regex::Regex;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

pub fn run_diagnostics(repo_root: &Path, ignored_patterns: &[String]) -> Vec<DiagnosticError> {
    run_diagnostics_with_report(repo_root, ignored_patterns).0
}

pub fn run_diagnostics_with_report(
    repo_root: &Path,
    ignored_patterns: &[String],
) -> (Vec<DiagnosticError>, DiagnosticRunReport) {
    let mut errors = Vec::new();
    let mut report = DiagnosticRunReport::new(ignored_patterns.len());

    // 1. Rust Diagnostics
    if repo_root.join("Cargo.toml").exists() {
        report.project_types.push("rust".to_string());
        report
            .adapters
            .push("cargo check --message-format=json".to_string());
        let before_count = errors.len();
        let output = std::process::Command::new("cargo")
            .arg("check")
            .arg("--message-format=json")
            .current_dir(repo_root)
            .output();

        match output {
            Ok(out) => {
                let stdout_str = String::from_utf8_lossy(&out.stdout);
                for line in stdout_str.lines() {
                    if let Ok(json) = serde_json::from_str::<serde_json::Value>(line) {
                        if json["reason"] == "compiler-message" {
                            let msg = &json["message"];
                            if msg["level"] == "error" {
                                let message = msg["rendered"]
                                    .as_str()
                                    .unwrap_or_else(|| msg["message"].as_str().unwrap_or(""))
                                    .to_string();

                                let mut file_path = String::new();
                                let mut line_num = 0;
                                if let Some(spans) = msg["spans"].as_array() {
                                    if let Some(first_span) = spans.first() {
                                        file_path = first_span["file_name"]
                                            .as_str()
                                            .unwrap_or("")
                                            .to_string();
                                        line_num =
                                            first_span["line_start"].as_u64().unwrap_or(0) as usize;
                                    }
                                }

                                errors.push(DiagnosticError {
                                    file: file_path,
                                    line: line_num,
                                    message,
                                    compiler: "rustc".to_string(),
                                });
                            }
                        }
                    }
                }

                if !out.status.success() && errors.len() == before_count {
                    errors.push(DiagnosticError {
                        file: "Cargo.toml".to_string(),
                        line: 0,
                        message: command_output_message(&out.stdout, &out.stderr),
                        compiler: "cargo".to_string(),
                    });
                }
            }
            Err(err) => {
                errors.push(DiagnosticError {
                    file: "Cargo.toml".to_string(),
                    line: 0,
                    message: format!("failed to run cargo check: {err}"),
                    compiler: "cargo".to_string(),
                });
            }
        }
    }

    // 2. TypeScript / JavaScript Diagnostics
    let has_ts_config =
        repo_root.join("tsconfig.json").exists() || repo_root.join("jsconfig.json").exists();
    let has_typescript_dependency = package_declares_typescript(repo_root);
    if is_svelte_project(repo_root) {
        report.project_types.push("svelte".to_string());
        report.adapters.push(svelte_diagnostic_adapter(repo_root));
        run_svelte_diagnostics(repo_root, &mut errors);
    } else if has_ts_config || has_typescript_dependency {
        report.project_types.push("typescript".to_string());
        let has_bun = repo_root.join("bun.lock").exists() || repo_root.join("bun.lockb").exists();
        let adapter = if has_bun {
            "bun x tsc --noEmit".to_string()
        } else {
            "local tsc --noEmit".to_string()
        };
        report.adapters.push(adapter);
        run_tsc_diagnostics(repo_root, &mut errors);
    }

    let raw_errors = errors.len();
    let errors = filter_diagnostics(errors, ignored_patterns);
    report.raw_errors = raw_errors;
    report.emitted_errors = errors.len();
    report.ignored_errors = raw_errors.saturating_sub(errors.len());

    (errors, report)
}

fn filter_diagnostics(
    errors: Vec<DiagnosticError>,
    ignored_patterns: &[String],
) -> Vec<DiagnosticError> {
    if ignored_patterns.is_empty() {
        return errors;
    }

    errors
        .into_iter()
        .filter(|error| !diagnostic_matches_ignore(error, ignored_patterns))
        .collect()
}

fn diagnostic_matches_ignore(error: &DiagnosticError, ignored_patterns: &[String]) -> bool {
    ignored_patterns.iter().any(|pattern| {
        let pattern = pattern.trim();
        !pattern.is_empty()
            && (error.file.contains(pattern)
                || error.message.contains(pattern)
                || error.compiler.contains(pattern))
    })
}

fn command_output_message(stdout: &[u8], stderr: &[u8]) -> String {
    let stdout = String::from_utf8_lossy(stdout);
    let stderr = String::from_utf8_lossy(stderr);
    let stdout = stdout.trim();
    let stderr = stderr.trim();

    match (stderr.is_empty(), stdout.is_empty()) {
        (false, false) => format!("{stderr}\n{stdout}"),
        (false, true) => stderr.to_string(),
        (true, false) => stdout.to_string(),
        (true, true) => "command failed without output".to_string(),
    }
}

fn run_tsc_diagnostics(repo_root: &Path, errors: &mut Vec<DiagnosticError>) {
    let tsc_path = local_tsc_path(repo_root);
    let has_bun = repo_root.join("bun.lock").exists() || repo_root.join("bun.lockb").exists();

    let output = if !tsc_path.is_file() {
        if has_bun {
            Command::new("bun")
                .arg("x")
                .arg("tsc")
                .arg("--noEmit")
                .current_dir(repo_root)
                .output()
        } else {
            errors.push(DiagnosticError {
                file: typescript_config_source(repo_root),
                line: 0,
                message: "local TypeScript compiler not found; run package install or add typescript to package.json".to_string(),
                compiler: "tsc".to_string(),
            });
            return;
        }
    } else {
        Command::new(&tsc_path)
            .arg("--noEmit")
            .current_dir(repo_root)
            .output()
    };

    let before_count = errors.len();

    match output {
        Ok(out) => {
            parse_tsc_errors(&out.stdout, &out.stderr, errors);

            if !out.status.success() && errors.len() == before_count {
                errors.push(DiagnosticError {
                    file: typescript_config_source(repo_root),
                    line: 0,
                    message: command_output_message(&out.stdout, &out.stderr),
                    compiler: "tsc".to_string(),
                });
            }
        }
        Err(err) => {
            let compiler = if !tsc_path.is_file() && has_bun { "bun" } else { "tsc" };
            errors.push(DiagnosticError {
                file: typescript_config_source(repo_root),
                line: 0,
                message: format!("failed to run TypeScript compiler: {err}"),
                compiler: compiler.to_string(),
            });
        }
    }
}

fn run_svelte_diagnostics(repo_root: &Path, errors: &mut Vec<DiagnosticError>) {
    let before_count = errors.len();
    let output = if package_script(repo_root, "check").is_some() {
        let (program, args) = package_run_command(repo_root, "check");
        let mut command = Command::new(program);
        command.args(args);
        command.current_dir(repo_root).output()
    } else {
        let svelte_check_path = local_svelte_check_path(repo_root);
        if !svelte_check_path.is_file() {
            errors.push(DiagnosticError {
                file: svelte_config_source(repo_root),
                line: 0,
                message:
                    "local Svelte checker not found; add a `check` script or install svelte-check"
                        .to_string(),
                compiler: "svelte-check".to_string(),
            });
            return;
        }

        Command::new(svelte_check_path)
            .arg("--output")
            .arg("human")
            .current_dir(repo_root)
            .output()
    };

    match output {
        Ok(out) => {
            parse_svelte_check_errors(&out.stdout, &out.stderr, errors);

            if !out.status.success() && errors.len() == before_count {
                errors.push(DiagnosticError {
                    file: svelte_config_source(repo_root),
                    line: 0,
                    message: command_output_message(&out.stdout, &out.stderr),
                    compiler: "svelte-check".to_string(),
                });
            }
        }
        Err(err) => {
            errors.push(DiagnosticError {
                file: svelte_config_source(repo_root),
                line: 0,
                message: format!("failed to run Svelte diagnostics: {err}"),
                compiler: "svelte-check".to_string(),
            });
        }
    }
}

fn svelte_diagnostic_adapter(repo_root: &Path) -> String {
    if package_script(repo_root, "check").is_some() {
        let (program, args) = package_run_command(repo_root, "check");
        let mut parts = vec![program.to_string()];
        parts.extend(args.into_iter().map(str::to_string));
        parts.join(" ")
    } else {
        "local svelte-check --output human".to_string()
    }
}

fn package_json(repo_root: &Path) -> Option<serde_json::Value> {
    let content = fs::read_to_string(repo_root.join("package.json")).ok()?;
    serde_json::from_str::<serde_json::Value>(&content).ok()
}

fn package_declares_dependency(repo_root: &Path, packages: &[&str]) -> bool {
    let Some(json) = package_json(repo_root) else {
        return false;
    };

    [
        "dependencies",
        "devDependencies",
        "peerDependencies",
        "optionalDependencies",
    ]
    .iter()
    .any(|section| {
        json.get(section)
            .and_then(|value| value.as_object())
            .is_some_and(|deps| packages.iter().any(|package| deps.contains_key(*package)))
    })
}

fn package_declares_typescript(repo_root: &Path) -> bool {
    package_declares_dependency(repo_root, &["typescript"])
}

fn is_svelte_project(repo_root: &Path) -> bool {
    [
        "svelte.config.js",
        "svelte.config.mjs",
        "svelte.config.cjs",
        "svelte.config.ts",
    ]
    .iter()
    .any(|config| repo_root.join(config).exists())
        || package_declares_dependency(
            repo_root,
            &["svelte", "@sveltejs/kit", "@sveltejs/vite-plugin-svelte"],
        )
}

fn package_script(repo_root: &Path, script: &str) -> Option<String> {
    package_json(repo_root)?
        .get("scripts")?
        .as_object()?
        .get(script)?
        .as_str()
        .map(str::to_string)
}

fn package_run_command<'a>(repo_root: &Path, script: &'a str) -> (&'static str, Vec<&'a str>) {
    if repo_root.join("pnpm-lock.yaml").exists() && !repo_root.join("package-lock.json").exists() {
        ("pnpm", vec!["run", script])
    } else if repo_root.join("yarn.lock").exists() && !repo_root.join("package-lock.json").exists()
    {
        ("yarn", vec![script])
    } else if repo_root.join("bun.lock").exists() || repo_root.join("bun.lockb").exists() {
        ("bun", vec!["run", script])
    } else {
        ("npm", vec!["run", script])
    }
}

fn local_tsc_path(repo_root: &Path) -> PathBuf {
    let executable = if cfg!(windows) { "tsc.cmd" } else { "tsc" };
    repo_root.join("node_modules").join(".bin").join(executable)
}

fn local_svelte_check_path(repo_root: &Path) -> PathBuf {
    let executable = if cfg!(windows) {
        "svelte-check.cmd"
    } else {
        "svelte-check"
    };
    repo_root.join("node_modules").join(".bin").join(executable)
}

fn typescript_config_source(repo_root: &Path) -> String {
    if repo_root.join("tsconfig.json").exists() {
        "tsconfig.json".to_string()
    } else if repo_root.join("jsconfig.json").exists() {
        "jsconfig.json".to_string()
    } else {
        "package.json".to_string()
    }
}

fn svelte_config_source(repo_root: &Path) -> String {
    for config in [
        "svelte.config.js",
        "svelte.config.mjs",
        "svelte.config.cjs",
        "svelte.config.ts",
    ] {
        if repo_root.join(config).exists() {
            return config.to_string();
        }
    }

    "package.json".to_string()
}

fn parse_tsc_errors(stdout: &[u8], stderr: &[u8], errors: &mut Vec<DiagnosticError>) {
    let stdout_str = String::from_utf8_lossy(stdout);
    let stderr_str = String::from_utf8_lossy(stderr);

    let re_tsc =
        Regex::new(r"([^(:\s]+)(?:\((\d+),\d+\)|:(\d+):\d+)\s*-\s*error\s+[A-Za-z0-9]+:\s*(.*)")
            .ok();
    let re_tsc_fallback =
        Regex::new(r"([^(:\s]+)\((\d+),\d+\):\s*error\s+[A-Za-z0-9]+:\s*(.*)").ok();

    for line in stdout_str.lines().chain(stderr_str.lines()) {
        if let Some(ref re) = re_tsc {
            if let Some(caps) = re.captures(line) {
                let file = caps
                    .get(1)
                    .map_or(String::new(), |m| m.as_str().to_string());
                let line_num = caps
                    .get(2)
                    .or_else(|| caps.get(3))
                    .and_then(|m| m.as_str().parse::<usize>().ok())
                    .unwrap_or(0);
                let message = caps
                    .get(4)
                    .map_or(String::new(), |m| m.as_str().to_string());
                errors.push(DiagnosticError {
                    file,
                    line: line_num,
                    message,
                    compiler: "tsc".to_string(),
                });
                continue;
            }
        }
        if let Some(ref re) = re_tsc_fallback {
            if let Some(caps) = re.captures(line) {
                let file = caps
                    .get(1)
                    .map_or(String::new(), |m| m.as_str().to_string());
                let line_num = caps
                    .get(2)
                    .and_then(|m| m.as_str().parse::<usize>().ok())
                    .unwrap_or(0);
                let message = caps
                    .get(3)
                    .map_or(String::new(), |m| m.as_str().to_string());
                errors.push(DiagnosticError {
                    file,
                    line: line_num,
                    message,
                    compiler: "tsc".to_string(),
                });
            }
        }
    }
}

fn parse_svelte_check_errors(stdout: &[u8], stderr: &[u8], errors: &mut Vec<DiagnosticError>) {
    let output = strip_ansi(&format!(
        "{}\n{}",
        String::from_utf8_lossy(stdout),
        String::from_utf8_lossy(stderr)
    ));
    let lines: Vec<&str> = output.lines().collect();

    let re_inline =
        Regex::new(r"^(.+\.(?:svelte|ts|js|tsx|jsx)):(\d+):\d+\s+(Error|Warn|Warning):\s*(.*)$")
            .ok();
    let re_location = Regex::new(r"^(.+\.(?:svelte|ts|js|tsx|jsx)):(\d+):\d+\s*$").ok();

    for (index, line) in lines.iter().enumerate() {
        let trimmed = line.trim();
        if let Some(ref re) = re_inline {
            if let Some(caps) = re.captures(trimmed) {
                let severity = caps.get(3).map_or("", |m| m.as_str());
                if severity == "Error" {
                    errors.push(DiagnosticError {
                        file: caps
                            .get(1)
                            .map_or(String::new(), |m| m.as_str().to_string()),
                        line: caps
                            .get(2)
                            .and_then(|m| m.as_str().parse::<usize>().ok())
                            .unwrap_or(0),
                        message: caps
                            .get(4)
                            .map_or(String::new(), |m| m.as_str().to_string()),
                        compiler: "svelte-check".to_string(),
                    });
                }
                continue;
            }
        }

        if let Some(ref re) = re_location {
            let Some(caps) = re.captures(trimmed) else {
                continue;
            };
            let Some(message) = lines
                .iter()
                .skip(index + 1)
                .take(4)
                .map(|line| line.trim())
                .find_map(|line| line.strip_prefix("Error: "))
            else {
                continue;
            };

            errors.push(DiagnosticError {
                file: caps
                    .get(1)
                    .map_or(String::new(), |m| m.as_str().to_string()),
                line: caps
                    .get(2)
                    .and_then(|m| m.as_str().parse::<usize>().ok())
                    .unwrap_or(0),
                message: message.to_string(),
                compiler: "svelte-check".to_string(),
            });
        }
    }
}

fn strip_ansi(input: &str) -> String {
    Regex::new(r"\x1b\[[0-9;]*m")
        .map(|re| re.replace_all(input, "").into_owned())
        .unwrap_or_else(|_| input.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::serialize_prompt_payload;
    use crate::config::AgentPayload;
    use crate::config::ContextTray;

    #[test]
    fn test_invalid_cargo_manifest_produces_cargo_diagnostic() {
        let temp_dir = tempfile::tempdir().unwrap();
        let root = temp_dir.path();
        fs::write(root.join("Cargo.toml"), "version = 2\n").unwrap();

        let diagnostics = run_diagnostics(root, &[]);

        assert!(diagnostics.iter().any(|diagnostic| {
            diagnostic.compiler == "cargo"
                && diagnostic.file == "Cargo.toml"
                && diagnostic.message.contains("failed to parse manifest")
        }));
    }

    #[test]
    fn test_ts_config_without_local_compiler_produces_tsc_diagnostic() {
        let temp_dir = tempfile::tempdir().unwrap();
        let root = temp_dir.path();
        fs::write(root.join("tsconfig.json"), "{}").unwrap();

        let diagnostics = run_diagnostics(root, &[]);

        assert_eq!(diagnostics.len(), 1);
        assert_eq!(diagnostics[0].compiler, "tsc");
        assert_eq!(diagnostics[0].file, "tsconfig.json");
        assert!(diagnostics[0]
            .message
            .contains("local TypeScript compiler not found"));
    }

    #[test]
    fn test_svelte_project_without_checker_produces_svelte_diagnostic() {
        let temp_dir = tempfile::tempdir().unwrap();
        let root = temp_dir.path();
        fs::write(root.join("svelte.config.js"), "export default {};\n").unwrap();
        fs::write(root.join("tsconfig.json"), "{}").unwrap();
        fs::write(
            root.join("package.json"),
            r#"{"devDependencies":{"svelte":"^5.0.0","typescript":"^5.0.0"}}"#,
        )
        .unwrap();

        let diagnostics = run_diagnostics(root, &[]);

        assert_eq!(diagnostics.len(), 1);
        assert_eq!(diagnostics[0].compiler, "svelte-check");
        assert_eq!(diagnostics[0].file, "svelte.config.js");
        assert!(diagnostics[0].message.contains("local Svelte checker"));
    }

    #[test]
    fn test_diagnostics_report_records_project_and_adapter() {
        let temp_dir = tempfile::tempdir().unwrap();
        let root = temp_dir.path();
        fs::write(root.join("svelte.config.js"), "export default {};\n").unwrap();
        fs::write(
            root.join("package.json"),
            r#"{"devDependencies":{"svelte":"^5.0.0"}}"#,
        )
        .unwrap();

        let (_diagnostics, report) =
            run_diagnostics_with_report(root, &["known-noise".to_string()]);

        assert_eq!(report.project_types, vec!["svelte"]);
        assert_eq!(report.adapters, vec!["local svelte-check --output human"]);
        assert_eq!(report.ignored_patterns, 1);
    }

    #[test]
    fn test_parse_svelte_check_errors() {
        let stdout = br#"
src/App.svelte:4:2
Error: Type 'number' is not assignable to type 'string'. (ts)

src/lib/stores.ts:8:11 Error: Cannot find name 'missingStore'. (ts)

====================================
svelte-check found 2 errors and 0 warnings in 2 files
"#;
        let mut diagnostics = Vec::new();

        parse_svelte_check_errors(stdout, b"", &mut diagnostics);

        assert_eq!(diagnostics.len(), 2);
        assert_eq!(diagnostics[0].compiler, "svelte-check");
        assert_eq!(diagnostics[0].file, "src/App.svelte");
        assert_eq!(diagnostics[0].line, 4);
        assert!(diagnostics[0].message.contains("Type 'number'"));
        assert_eq!(diagnostics[1].file, "src/lib/stores.ts");
        assert_eq!(diagnostics[1].line, 8);
        assert!(diagnostics[1].message.contains("missingStore"));
    }

    #[test]
    fn test_ignored_diagnostics_filter_by_file_or_message() {
        let diagnostics = vec![
            DiagnosticError {
                file: "src/lib/components/ui/button/index.ts".to_string(),
                line: 2,
                message: "Module has no exported member ButtonProps".to_string(),
                compiler: "tsc".to_string(),
            },
            DiagnosticError {
                file: "src/routes/api/webhooks/stripe/+server.ts".to_string(),
                line: 18,
                message: "Missing signature".to_string(),
                compiler: "tsc".to_string(),
            },
            DiagnosticError {
                file: "src/main.rs".to_string(),
                line: 7,
                message: "expected usize, found String".to_string(),
                compiler: "rustc".to_string(),
            },
        ];

        let ignored = vec![
            "src/lib/components/ui".to_string(),
            "expected usize".to_string(),
        ];
        let filtered = filter_diagnostics(diagnostics, &ignored);

        assert_eq!(filtered.len(), 1);
        assert_eq!(
            filtered[0].file,
            "src/routes/api/webhooks/stripe/+server.ts"
        );
    }

    #[test]
    fn test_run_diagnostics_filters_early_tsc_setup_diagnostic() {
        let temp_dir = tempfile::tempdir().unwrap();
        let root = temp_dir.path();
        fs::write(root.join("tsconfig.json"), "{}").unwrap();

        let diagnostics =
            run_diagnostics(root, &["local TypeScript compiler not found".to_string()]);

        assert!(diagnostics.is_empty());
    }

    #[test]
    fn test_package_only_js_repo_does_not_run_tsc() {
        let temp_dir = tempfile::tempdir().unwrap();
        let root = temp_dir.path();
        fs::write(root.join("package.json"), r#"{"name":"plain-js"}"#).unwrap();

        let diagnostics = run_diagnostics(root, &[]);

        assert!(diagnostics.is_empty());
    }

    #[test]
    fn test_diagnostics_formatting() {
        let payload = AgentPayload {
            goal: "Refactor auth".to_string(),
            acceptance_criteria: vec![],
            assumptions: vec![],
            non_goals: vec![],
            context: ContextTray {
                active: vec![],
                watching: vec![],
                background: vec![],
            },
            diagnostics: vec![DiagnosticError {
                file: "src/main.rs".to_string(),
                line: 42,
                message: "Mismatched types".to_string(),
                compiler: "rustc".to_string(),
            }],
            report: None,
        };

        let output = serialize_prompt_payload(&payload);
        assert!(output.contains("--- BUILD DIAGNOSTICS ---"));
        assert!(output.contains("[rustc] File: src/main.rs:42"));
        assert!(output.contains("Error: Mismatched types"));
    }

    #[test]
    fn test_diagnostics_report_records_bun_adapter() {
        let temp_dir = tempfile::tempdir().unwrap();
        let root = temp_dir.path();
        fs::write(root.join("tsconfig.json"), "{}").unwrap();
        fs::write(root.join("bun.lockb"), "").unwrap();

        let (_diagnostics, report) =
            run_diagnostics_with_report(root, &[]);

        assert_eq!(report.project_types, vec!["typescript"]);
        assert_eq!(report.adapters, vec!["bun x tsc --noEmit"]);
    }
}
