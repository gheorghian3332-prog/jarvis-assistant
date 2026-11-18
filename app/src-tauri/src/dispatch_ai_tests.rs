//! The AI path of the command core, with a fake assistant (no model, no executor behind it).

use super::*;
use jarvis_control::MemoryAudit;
use serde_json::Value;
use std::collections::VecDeque;
use tauri::async_runtime::block_on;

#[derive(Default)]
struct Engine2 {
    runs: Mutex<Vec<Intent>>,
    confirms: Mutex<Vec<(String, bool)>>,
}

#[async_trait]
impl Engine for Engine2 {
    async fn run(&self, intent: Intent) -> Outcome {
        lock(&self.runs).push(intent);
        Outcome::Done {
            reply_ru: "Звук выключен.".into(),
            reply_en: "Sound muted.".into(),
            detail: Value::Null,
        }
    }
    async fn confirm(&self, action_id: &str, approved: bool) -> Outcome {
        lock(&self.confirms).push((action_id.to_owned(), approved));
        Outcome::Done {
            reply_ru: "Готово.".into(),
            reply_en: "Done.".into(),
            detail: Value::Null,
        }
    }
    fn emergency_stop(&self) -> usize {
        0
    }
}

#[derive(Default)]
struct Shell2 {
    state: Mutex<String>,
    spoken: Mutex<Vec<String>>,
    requests: Mutex<Vec<ConfirmRequest>>,
}

impl Shell for Shell2 {
    fn set_state(&self, state: &str) {
        *lock(&self.state) = state.to_owned();
    }
    fn state(&self) -> String {
        lock(&self.state).clone()
    }
    fn speak(&self, text: &str) {
        lock(&self.spoken).push(text.to_owned());
    }
    fn message(&self, _message: &AssistantMessage) {}
    fn confirm_request(&self, request: &ConfirmRequest) {
        lock(&self.requests).push(request.clone());
    }
    fn action_result(&self, _report: &ActionReport) {}
    fn silence(&self) {}
}

struct FakeAssistant {
    answers: Mutex<VecDeque<Routed>>,
    routed: AtomicU64,
    notes: Mutex<Vec<Note>>,
    style: Style,
    delay: Duration,
}

#[async_trait]
impl Assistant for FakeAssistant {
    async fn route(&self, _text: &str, _lang: Lang) -> Routed {
        self.routed.fetch_add(1, Ordering::SeqCst);
        sleep(self.delay).await;
        lock(&self.answers)
            .pop_front()
            .unwrap_or(Routed::Unavailable)
    }
    fn style(&self) -> Style {
        self.style
    }
    async fn note(&self, note: Note) {
        lock(&self.notes).push(note);
    }
}

struct Rig {
    core: Arc<CommandCore>,
    engine: Arc<Engine2>,
    shell: Arc<Shell2>,
    assistant: Arc<FakeAssistant>,
}

fn rig(answers: Vec<Routed>, style: Style, delay: Duration) -> Rig {
    let engine = Arc::new(Engine2::default());
    let shell = Arc::new(Shell2::default());
    shell.set_state("IDLE");
    let assistant = Arc::new(FakeAssistant {
        answers: Mutex::new(answers.into()),
        routed: AtomicU64::new(0),
        notes: Mutex::new(Vec::new()),
        style,
        delay,
    });
    let core = CommandCore::with_assistant(
        engine.clone(),
        shell.clone(),
        Arc::new(MemoryAudit::default()),
        assistant.clone(),
    );
    Rig {
        core,
        engine,
        shell,
        assistant,
    }
}

fn spoken(rig: &Rig) -> Vec<String> {
    lock(&rig.shell.spoken).clone()
}

#[test]
fn the_fast_path_never_asks_the_assistant_and_is_noted() {
    block_on(async {
        let rig = rig(vec![], Style::Classic, Duration::ZERO);
        rig.core.submit("выключи звук", None, Source::Text).await;
        assert_eq!(rig.assistant.routed.load(Ordering::SeqCst), 0);
        assert_eq!(lock(&rig.engine.runs).len(), 1);
        let notes = lock(&rig.assistant.notes).clone();
        assert_eq!(notes.len(), 1);
        assert_eq!(notes[0].route, Route::Fast);
        assert_eq!(notes[0].action.as_deref(), Some("audio.mute"));
        assert!(notes[0].ok);
        assert_eq!(notes[0].reply, "Звук выключен.");
    });
}

#[test]
fn an_unknown_phrase_gets_the_assistants_reply() {
    block_on(async {
        let answer = Routed::Reply {
            text: "Всегда рад помочь.".into(),
            route: Route::Local,
        };
        let rig = rig(vec![answer], Style::Classic, Duration::ZERO);
        rig.core
            .submit("спасибо тебе большое", None, Source::Text)
            .await;
        assert_eq!(rig.assistant.routed.load(Ordering::SeqCst), 1);
        assert_eq!(spoken(&rig), vec!["Всегда рад помочь."]);
        assert_eq!(rig.shell.state(), "IDLE");
        let notes = lock(&rig.assistant.notes).clone();
        assert_eq!(notes[0].route, Route::Local);
        assert!(notes[0].action.is_none());
    });
}

#[test]
fn without_a_model_the_old_answer_stays() {
    block_on(async {
        let rig = rig(vec![Routed::Unavailable], Style::Classic, Duration::ZERO);
        rig.core.submit("расскажи шутку", None, Source::Text).await;
        assert_eq!(spoken(&rig), vec!["Не понял команду."]);
        assert!(!lock(&rig.assistant.notes)[0].ok);
        let rig = self::rig(vec![], Style::Classic, Duration::ZERO);
        rig.core.submit("tell me a joke", None, Source::Text).await;
        assert_eq!(spoken(&rig), vec!["I did not understand the command."]);
    });
}

#[test]
fn a_model_action_that_needs_confirmation_uses_the_normal_dialog() {
    block_on(async {
        let routed = Routed::Executed {
            intent: Intent::new("power.shutdown", serde_json::json!({})),
            outcome: Outcome::NeedsConfirm {
                action_id: "act-7".into(),
                summary: "Shut down the computer in 30 seconds".into(),
            },
            route: Route::Local,
        };
        let rig = rig(vec![routed], Style::Classic, Duration::ZERO);
        rig.core
            .submit("пора на боковую", None, Source::Voice)
            .await;
        assert_eq!(rig.core.pending_id().as_deref(), Some("act-7"));
        assert_eq!(lock(&rig.shell.requests).len(), 1);
        assert!(lock(&rig.engine.runs).is_empty());
        // A spoken "no" declines; nothing ran.
        rig.core.submit("нет", None, Source::Voice).await;
        assert_eq!(
            lock(&rig.engine.confirms).clone(),
            vec![("act-7".to_owned(), false)]
        );
        assert_eq!(rig.assistant.routed.load(Ordering::SeqCst), 1);
    });
}

#[test]
fn replies_follow_the_personality() {
    block_on(async {
        let rig = rig(vec![], Style::Formal, Duration::ZERO);
        rig.core.submit("выключи звук", None, Source::Text).await;
        assert_eq!(spoken(&rig), vec!["Звук выключен, сэр."]);
        let rig = rig_style(Style::Friendly);
        rig.core.submit("mute", None, Source::Text).await;
        assert_eq!(spoken(&rig), vec!["Sure! Sound muted."]);
    });
}

fn rig_style(style: Style) -> Rig {
    rig(vec![], style, Duration::ZERO)
}

#[test]
fn the_emergency_stop_drops_an_answer_that_was_being_prepared() {
    block_on(async {
        let answer = Routed::Reply {
            text: "Поздно.".into(),
            route: Route::Local,
        };
        let rig = rig(vec![answer], Style::Classic, Duration::from_millis(1500));
        let core = Arc::clone(&rig.core);
        let task = tokio::spawn(async move {
            core.submit("расскажи длинную историю", None, Source::Text)
                .await;
        });
        // The first parse compiles the rule set (slow in a debug build): give the task time to get past it.
        sleep(Duration::from_millis(700)).await;
        rig.core.emergency_stop().await;
        task.await.expect("join");
        assert!(spoken(&rig).is_empty(), "{:?}", spoken(&rig));
        assert_eq!(rig.shell.state(), "IDLE");
    });
}
