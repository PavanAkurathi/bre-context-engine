use bre_context_engine::config::{
    load_js_alias_map, serialize_prompt_payload, BreConfig, Cli, OutputFormat,
};
use bre_context_engine::engine::{run_bre, BreRunOptions};
use clap::Parser;
use std::fs;
use std::process;

fn main() {
    let cli = Cli::parse();

    if cli.mcp {
        if let Err(err) = bre_context_engine::mcp::run_mcp_server() {
            eprintln!("MCP server error: {err}");
            std::process::exit(1);
        }
        std::process::exit(0);
    }

    let config_path = cli.path.join("bre.json");

    let config_contents = fs::read_to_string(&config_path).unwrap_or_else(|err| {
        eprintln!("failed to read {}: {err}", config_path.display());
        process::exit(1);
    });

    let config: BreConfig = serde_json::from_str(&config_contents).unwrap_or_else(|err| {
        eprintln!("failed to parse {}: {err}", config_path.display());
        process::exit(1);
    });

    // Load JS sidecar alias map if specified
    let mut js_aliases = std::collections::HashMap::new();
    if let Some(ref alias_file) = cli.js_aliases {
        let alias_path = cli.path.join(alias_file);
        js_aliases = load_js_alias_map(&alias_path).unwrap_or_else(|err| {
            eprintln!("{err}");
            process::exit(1);
        });
    }

    // Prepare Run Options
    let run_options = BreRunOptions {
        max_tokens: cli.max_tokens,
        skip_diagnostics: cli.skip_diagnostics,
        include_report: cli.report,
        js_aliases,
        scoring_policy: cli
            .scoring_policy
            .clone()
            .map(|policy| policy.into_policy(cli.scoring_p)),
    };

    // Delegate execution to Reusable Engine API boundary
    let payload = match run_bre(&cli.path, config, run_options) {
        Ok(res) => res,
        Err(err) => {
            eprintln!("{err}");
            process::exit(1);
        }
    };

    let output = match cli.format {
        OutputFormat::Json => serde_json::to_string_pretty(&payload).unwrap_or_else(|err| {
            eprintln!("failed to serialize agent payload: {err}");
            process::exit(1);
        }),
        OutputFormat::Prompt => serialize_prompt_payload(&payload),
    };

    println!("{output}");
}
