//! What the model is told about the actions: argument shapes, the prompt list and the JSON schema
//! that constrains its answer (Ollama `format`). The registry stays the only authority: whatever
//! the model says still goes through `Executor::run`.

use jarvis_control::{ActionInfo, Risk};
use serde_json::{json, Map, Value};

#[derive(Debug, Clone, Copy)]
pub enum Kind {
    Int(i64, i64),
    Str,
    Enum(&'static [&'static str]),
}

#[derive(Debug, Clone, Copy)]
pub struct Arg {
    pub name: &'static str,
    pub kind: Kind,
    pub required: bool,
}

const fn req(name: &'static str, kind: Kind) -> Arg {
    Arg {
        name,
        kind,
        required: true,
    }
}

const fn opt(name: &'static str, kind: Kind) -> Arg {
    Arg {
        name,
        kind,
        required: false,
    }
}

const QUERY: [Arg; 1] = [req("query", Kind::Str)];
const FOLDERS: &[&str] = &[
    "downloads",
    "documents",
    "desktop",
    "pictures",
    "music",
    "videos",
];

/// Arguments of every action the brain knows (built-ins of `jarvis-control` and the brain's own).
pub const SPECS: &[(&str, &[Arg])] = &[
    ("app.open", &QUERY),
    ("app.refresh", &[]),
    ("window.list", &[]),
    ("window.focus", &QUERY),
    ("window.minimize", &QUERY),
    ("window.maximize", &QUERY),
    ("window.close", &QUERY),
    ("window.minimize_all", &[]),
    ("audio.get_volume", &[]),
    ("audio.set_volume", &[req("level", Kind::Int(0, 100))]),
    ("audio.change_volume", &[req("delta", Kind::Int(-100, 100))]),
    ("audio.mute", &[]),
    ("audio.unmute", &[]),
    (
        "audio.media",
        &[req("key", Kind::Enum(&["play_pause", "next", "prev"]))],
    ),
    (
        "files.open_folder",
        &[opt("name", Kind::Enum(FOLDERS)), opt("path", Kind::Str)],
    ),
    (
        "files.list",
        &[opt("name", Kind::Enum(FOLDERS)), opt("path", Kind::Str)],
    ),
    ("files.delete", &[req("path", Kind::Str)]),
    (
        "process.list",
        &[
            opt("sort", Kind::Enum(&["cpu", "ram"])),
            opt("limit", Kind::Int(1, 100)),
        ],
    ),
    ("process.hung", &[]),
    (
        "process.kill",
        &[opt("name", Kind::Str), opt("pid", Kind::Int(5, 4_000_000))],
    ),
    ("network.check", &[]),
    ("power.lock", &[]),
    ("power.sleep", &[]),
    ("power.hibernate", &[]),
    ("power.shutdown", &[]),
    ("power.restart", &[]),
    ("power.abort", &[]),
    ("info.time", &[]),
    ("info.date", &[]),
    (
        "memory.remember",
        &[
            req(
                "kind",
                Kind::Enum(&["preference", "project", "device", "scenario", "note"]),
            ),
            req("key", Kind::Str),
            req("value", Kind::Str),
        ],
    ),
    ("memory.forget", &[req("id", Kind::Int(1, 1_000_000_000))]),
    ("memory.recall", &[opt("query", Kind::Str)]),
    ("ai.unload", &[]),
    (
        "system.status",
        &[opt(
            "topic",
            Kind::Enum(&["gpu_temp", "gpu", "memory", "cpu", "slowdown", "overview"]),
        )],
    ),
    ("vision.describe_screen", &[opt("question", Kind::Str)]),
    ("vision.read_window", &[]),
    ("vision.explain_error", &[]),
    ("vision.save_screenshot", &[]),
];

pub fn args_of(name: &str) -> Option<&'static [Arg]> {
    SPECS
        .iter()
        .find(|(spec_name, _)| *spec_name == name)
        .map(|(_, args)| *args)
}

fn kind_schema(kind: Kind) -> Value {
    match kind {
        Kind::Int(min, max) => json!({"type": "integer", "minimum": min, "maximum": max}),
        Kind::Str => json!({"type": "string"}),
        Kind::Enum(values) => json!({"type": "string", "enum": values}),
    }
}

fn kind_hint(kind: Kind) -> String {
    match kind {
        Kind::Int(min, max) => format!("int {min}..{max}"),
        Kind::Str => "text".to_owned(),
        Kind::Enum(values) => values.join("|"),
    }
}

/// A compact hint such as `{level: int 0..100}` for the prompt.
pub fn args_hint(args: &[Arg]) -> String {
    let parts: Vec<String> = args
        .iter()
        .map(|arg| {
            format!(
                "{}{}: {}",
                arg.name,
                if arg.required { "" } else { "?" },
                kind_hint(arg.kind)
            )
        })
        .collect();
    format!("{{{}}}", parts.join(", "))
}

/// JSON schema of the `args` object of one action.
pub fn args_schema(name: &str) -> Value {
    let Some(args) = args_of(name) else {
        return json!({"type": "object"});
    };
    let mut properties = Map::new();
    let mut required = Vec::new();
    for arg in args {
        properties.insert(arg.name.to_owned(), kind_schema(arg.kind));
        if arg.required {
            required.push(arg.name);
        }
    }
    json!({
        "type": "object",
        "properties": properties,
        "required": required,
        "additionalProperties": false,
    })
}

fn risk_tag(risk: Risk) -> &'static str {
    match risk {
        Risk::Safe => "",
        Risk::Confirm => " [asks the user to confirm]",
        Risk::Forbidden => " [forbidden]",
    }
}

/// Actions the model may name: enabled and not forbidden.
pub fn offered(actions: &[ActionInfo]) -> Vec<&ActionInfo> {
    actions
        .iter()
        .filter(|info| info.enabled && info.risk != Risk::Forbidden)
        .collect()
}

/// The action list as it appears in the system prompt.
pub fn prompt_list(actions: &[ActionInfo]) -> String {
    offered(actions)
        .iter()
        .map(|info| {
            let hint = args_of(&info.name).map_or_else(|| "{}".to_owned(), args_hint);
            format!(
                "- {} {} : {}{}",
                info.name,
                hint,
                info.description_en,
                risk_tag(info.risk)
            )
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// The schema of the model's answer: an action with exact arguments, or a plain reply.
pub fn answer_schema(actions: &[ActionInfo]) -> Value {
    let mut branches: Vec<Value> = vec![reply_branch()];
    branches.extend(offered(actions).iter().map(|info| {
        json!({
            "type": "object",
            "properties": {
                "type": {"type": "string", "enum": ["action"]},
                "action": {"type": "string", "enum": [info.name]},
                "args": args_schema(&info.name),
            },
            "required": ["type", "action", "args"],
        })
    }));
    json!({"anyOf": branches})
}

fn reply_branch() -> Value {
    json!({
        "type": "object",
        "properties": {
            "type": {"type": "string", "enum": ["reply"]},
            "text": {"type": "string"},
        },
        "required": ["type", "text"],
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{atomic::AtomicBool, Arc};

    fn info(name: &str, risk: Risk, enabled: bool) -> ActionInfo {
        ActionInfo {
            name: name.into(),
            risk,
            description_ru: "ру".into(),
            description_en: "Set the master volume 0-100".into(),
            enabled,
        }
    }

    #[test]
    fn every_registry_action_has_a_spec() {
        let context = Arc::new(jarvis_control::actions::Context::new(
            jarvis_control::ControlConfig::default(),
            Arc::new(AtomicBool::new(false)),
        ));
        let registry = jarvis_control::actions::build_registry(context);
        for action in registry.list() {
            assert!(
                args_of(&action.name).is_some(),
                "no spec for {}",
                action.name
            );
        }
    }

    #[test]
    fn hints_and_schemas_describe_the_arguments() {
        assert_eq!(
            args_hint(args_of("audio.set_volume").expect("spec")),
            "{level: int 0..100}"
        );
        assert_eq!(
            args_hint(args_of("process.list").expect("spec")),
            "{sort?: cpu|ram, limit?: int 1..100}"
        );
        let schema = args_schema("audio.set_volume");
        assert_eq!(schema["properties"]["level"]["maximum"], 100);
        assert_eq!(schema["required"], json!(["level"]));
        assert_eq!(args_schema("unknown.thing"), json!({"type": "object"}));
    }

    #[test]
    fn only_enabled_non_forbidden_actions_are_offered() {
        let actions = vec![
            info("audio.mute", Risk::Safe, true),
            info("audio.unmute", Risk::Safe, false),
            info("power.shutdown", Risk::Confirm, true),
            info("x.forbidden", Risk::Forbidden, true),
        ];
        let list = prompt_list(&actions);
        assert!(list.contains("audio.mute"));
        assert!(list.contains("power.shutdown") && list.contains("confirm"));
        assert!(!list.contains("audio.unmute") && !list.contains("x.forbidden"));
        let schema = answer_schema(&actions);
        let branches = schema["anyOf"].as_array().expect("anyOf");
        // two actions plus the plain reply
        assert_eq!(branches.len(), 3);
        assert_eq!(branches[0]["properties"]["type"]["enum"], json!(["reply"]));
        // The model writes keys in schema order: "type" must come first, or it commits to an
        // action name before it has decided between acting and replying.
        for branch in branches {
            let first = branch["properties"]
                .as_object()
                .and_then(|p| p.keys().next());
            assert_eq!(first.map(String::as_str), Some("type"));
        }
    }
}
