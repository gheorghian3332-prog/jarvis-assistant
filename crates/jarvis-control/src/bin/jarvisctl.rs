//! `jarvisctl` - command-line front end of the Windows control engine.
//!
//! jarvisctl "<phrase>" [--lang ru|en] [--dry-run] [--yes]
//! jarvisctl --list-actions | --apps [query] | --windows | --processes

use std::process::ExitCode;
use std::sync::Arc;

use jarvis_control::apps::AppIndex;
use jarvis_control::procs::{self, SortKey};
use jarvis_control::{
    parse, windows_executor, wnd, ControlConfig, Intent, Lang, MemoryAudit, Outcome,
};

const USAGE: &str = "usage:\n  jarvisctl \"<phrase>\" [--lang ru|en] [--dry-run] [--yes]\n  jarvisctl --list-actions\n  jarvisctl --apps [query]\n  jarvisctl --windows\n  jarvisctl --processes";

fn print_json(value: &serde_json::Value) {
    match serde_json::to_string_pretty(value) {
        Ok(text) => println!("{text}"),
        Err(error) => eprintln!("cannot format output: {error}"),
    }
}

fn run(arguments: Vec<String>) -> Result<ExitCode, String> {
    let mut lang = Lang::Ru;
    let mut dry_run = false;
    let mut yes = false;
    let mut mode: Option<&str> = None;
    let mut positional: Vec<String> = Vec::new();
    let mut iterator = arguments.into_iter();
    while let Some(argument) = iterator.next() {
        match argument.as_str() {
            "--lang" => {
                lang = match iterator.next().as_deref() {
                    Some("ru") => Lang::Ru,
                    Some("en") => Lang::En,
                    _ => return Err("--lang expects ru or en".into()),
                }
            }
            "--dry-run" => dry_run = true,
            "--yes" => yes = true,
            "--list-actions" => mode = Some("list"),
            "--apps" => mode = Some("apps"),
            "--windows" => mode = Some("windows"),
            "--processes" => mode = Some("processes"),
            "--help" | "-h" => {
                println!("{USAGE}");
                return Ok(ExitCode::SUCCESS);
            }
            other if other.starts_with("--") => return Err(format!("unknown option {other}")),
            _ => positional.push(argument),
        }
    }
    let config = ControlConfig::default();
    match mode {
        Some("list") => {
            let executor = windows_executor(config, Arc::new(MemoryAudit::default()));
            let list = executor.registry().list();
            print_json(&serde_json::json!(list));
            Ok(ExitCode::SUCCESS)
        }
        Some("apps") => {
            let index = AppIndex::build(&config);
            if positional.is_empty() {
                println!("{} applications indexed", index.len());
                for entry in index.entries() {
                    println!(
                        "{:<10} {}  ->  {}",
                        entry.source.as_str(),
                        entry.name,
                        entry.launch
                    );
                }
            } else {
                let query = positional.join(" ");
                let matches = index.search(&query, 10);
                println!(
                    "{} matches for \"{query}\" (of {} indexed)",
                    matches.len(),
                    index.len()
                );
                for found in matches {
                    println!(
                        "{:>5.1}  {:<10} {}  ->  {}",
                        found.score,
                        found.source.as_str(),
                        found.name,
                        found.launch
                    );
                }
            }
            Ok(ExitCode::SUCCESS)
        }
        Some("windows") => {
            let windows = wnd::list_windows().map_err(|error| error.to_string())?;
            println!("{} windows", windows.len());
            for window in windows {
                println!(
                    "{:>8}  pid {:<6} {:<24} {}{}",
                    window.hwnd,
                    window.pid,
                    window.process,
                    window.title,
                    if window.hung {
                        "  [not responding]"
                    } else {
                        ""
                    }
                );
            }
            Ok(ExitCode::SUCCESS)
        }
        Some("processes") => {
            for sort in [SortKey::Cpu, SortKey::Ram] {
                println!("top 10 by {sort:?}");
                for process in procs::list(sort, 10) {
                    println!(
                        "{:>7}  {:<32} cpu {:>5.1}%  ram {} MB",
                        process.pid, process.name, process.cpu_percent, process.memory_mb
                    );
                }
            }
            Ok(ExitCode::SUCCESS)
        }
        _ => {
            if positional.is_empty() {
                return Err(USAGE.into());
            }
            let phrase = positional.join(" ");
            let Some(intent) = parse(&phrase, lang) else {
                println!("no fast-path match: this phrase would go to the LLM");
                return Ok(ExitCode::from(4));
            };
            let executor = windows_executor(config, Arc::new(MemoryAudit::default()));
            let risk = executor
                .registry()
                .get(&intent.action)
                .map(|action| action.effective_risk(&intent.args));
            print_json(&serde_json::json!({
                "intent": intent,
                "risk": risk.map(|r| r.as_str()),
            }));
            if dry_run {
                return Ok(ExitCode::SUCCESS);
            }
            let outcome = execute(&executor, intent, yes, lang);
            Ok(outcome)
        }
    }
}

fn execute(executor: &jarvis_control::Executor, intent: Intent, yes: bool, lang: Lang) -> ExitCode {
    let runtime = match tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
    {
        Ok(runtime) => runtime,
        Err(error) => {
            eprintln!("cannot start runtime: {error}");
            return ExitCode::FAILURE;
        }
    };
    runtime.block_on(async {
        let mut outcome = executor.run(intent).await;
        if let Outcome::NeedsConfirm { action_id, summary } = &outcome {
            if !yes {
                println!("confirmation required: {summary}\nrerun with --yes to proceed");
                return ExitCode::from(2);
            }
            let id = action_id.clone();
            outcome = executor.confirm(&id, true).await;
        }
        match outcome {
            Outcome::Done {
                reply_ru,
                reply_en,
                detail,
            } => {
                println!("{}", if lang == Lang::Ru { reply_ru } else { reply_en });
                if !detail.is_null() {
                    print_json(&detail);
                }
                ExitCode::SUCCESS
            }
            Outcome::Denied { reason } => {
                println!("denied: {reason}");
                ExitCode::from(3)
            }
            Outcome::Failed { code, message } => {
                println!("failed: {code}: {message}");
                ExitCode::FAILURE
            }
            Outcome::NeedsConfirm { summary, .. } => {
                println!("confirmation required: {summary}");
                ExitCode::from(2)
            }
        }
    })
}

fn main() -> ExitCode {
    match run(std::env::args().skip(1).collect()) {
        Ok(code) => code,
        Err(message) => {
            eprintln!("{message}");
            ExitCode::from(64)
        }
    }
}
