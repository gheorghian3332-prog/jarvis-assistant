//! The command service core: a phrase (typed or spoken) goes through the FAST PATH parser into the
//! control executor; confirmations, answers by voice, timeouts and the emergency stop live here.
//!
//! Everything the core touches is behind two small traits ([`Engine`] and [`Shell`]) so the logic is
//! tested with fakes; `control.rs` wires them to the real executor, the window and the speech output.
//! Nothing here calls a controller: every action goes through `Executor::run` / `Executor::confirm`.

use async_trait::async_trait;
use jarvis_brain::{Note, Route, Routed};
use jarvis_control::{parse, restyle, AuditRecord, AuditSink, Intent, Lang, Outcome, Style};
use serde::Serialize;
use serde_json::json;
use std::{
    sync::{
        atomic::{AtomicU64, Ordering},
        Arc, Mutex, MutexGuard,
    },
    time::{Duration, Instant},
};
use tokio::{task::JoinHandle, time::sleep};

/// How long a confirmation waits for an answer (button or voice).
pub const CONFIRM_WINDOW: Duration = Duration::from_secs(30);
/// How long the `ERROR` state is shown after a failed command.
pub const ERROR_HOLD: Duration = Duration::from_secs(2);
/// A phrase that was being recorded when the emergency stop hit is dropped if it arrives this soon.
const DISCARD_WINDOW: Duration = Duration::from_secs(15);

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// The executor as the core sees it.
#[async_trait]
pub trait Engine: Send + Sync {
    async fn run(&self, intent: Intent) -> Outcome;
    async fn confirm(&self, action_id: &str, approved: bool) -> Outcome;
    /// Cancels pending confirmations and a scheduled shutdown; returns the number of confirmations.
    fn emergency_stop(&self) -> usize;
}

/// `assistant://message`.
#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct AssistantMessage {
    pub text: String,
    pub lang: String,
    pub skill: Option<String>,
}

/// `confirm://request`.
#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct ConfirmRequest {
    pub action_id: String,
    pub action: String,
    pub summary: String,
    pub risk: String,
}

/// `action://result`.
#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct ActionReport {
    pub action_id: String,
    pub ok: bool,
    pub detail: String,
}

/// Everything with a side effect on the outside of the core: the window, the speech, the state.
pub trait Shell: Send + Sync {
    fn set_state(&self, state: &str);
    fn state(&self) -> String;
    /// Short spoken phrase; failures (no voice installed, no device) are the shell's to log.
    fn speak(&self, text: &str);
    fn message(&self, message: &AssistantMessage);
    fn confirm_request(&self, request: &ConfirmRequest);
    fn action_result(&self, report: &ActionReport);
    /// Stops the speech and releases push-to-talk (a recording in progress ends).
    fn silence(&self);
}

/// The AI side as the core sees it (the brain in the app, a fake in tests). It only *routes*:
/// every action it proposes has already gone through the executor when it is handed back.
#[async_trait]
pub trait Assistant: Send + Sync {
    /// LOCAL AI PATH for a phrase the FAST PATH did not understand.
    async fn route(&self, text: &str, lang: Lang) -> Routed;
    /// The wording variant of FAST PATH replies for the current personality.
    fn style(&self) -> Style;
    /// A finished exchange: the dialog buffer and the command history.
    async fn note(&self, note: Note);
}

#[async_trait]
impl Assistant for jarvis_brain::Brain {
    async fn route(&self, text: &str, lang: Lang) -> Routed {
        jarvis_brain::Brain::route(self, text, lang).await
    }

    fn style(&self) -> Style {
        jarvis_brain::Brain::style(self)
    }

    async fn note(&self, note: Note) {
        jarvis_brain::Brain::note(self, note).await;
    }
}

/// Where a phrase came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Source {
    Text,
    Voice,
}

/// A spoken or typed answer to a confirmation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Answer {
    Yes,
    No,
}

const YES_PHRASES: [&str; 6] = [
    "да",
    "подтверждаю",
    "выполняй",
    "yes",
    "confirm",
    "подтвердить",
];
const NO_PHRASES: [&str; 6] = ["нет", "отмена", "не надо", "no", "cancel", "отмени"];

/// Recognizes "да, подтверждаю, выполняй, yes, confirm" and "нет, отмена, не надо, no, cancel",
/// also after an address ("лёха, да"). Anything else, and any mixture, is `None` (nothing happens).
pub fn classify_answer(text: &str) -> Option<Answer> {
    let cleaned = parse::clean(text);
    let words: Vec<&str> = cleaned.split_whitespace().collect();
    if words.is_empty() {
        return None;
    }
    let mut answers = Vec::new();
    let mut index = 0;
    while index < words.len() {
        if index + 1 < words.len() {
            let pair = format!("{} {}", words[index], words[index + 1]);
            if NO_PHRASES.contains(&pair.as_str()) {
                answers.push(Answer::No);
                index += 2;
                continue;
            }
        }
        let word = words[index];
        if YES_PHRASES.contains(&word) {
            answers.push(Answer::Yes);
        } else if NO_PHRASES.contains(&word) {
            answers.push(Answer::No);
        } else {
            return None;
        }
        index += 1;
    }
    let first = *answers.first()?;
    answers
        .iter()
        .all(|answer| *answer == first)
        .then_some(first)
}

/// The language of a phrase: by script, then by the recognizer's hint, Russian by default.
pub fn lang_of(text: &str, hint: Option<&str>) -> Lang {
    let cyrillic = text.chars().any(|c| ('\u{0400}'..='\u{04FF}').contains(&c));
    if cyrillic {
        return Lang::Ru;
    }
    if text.chars().any(|c| c.is_ascii_alphabetic()) {
        return Lang::En;
    }
    match hint {
        Some("en") => Lang::En,
        _ => Lang::Ru,
    }
}

fn pick(lang: Lang, ru: &str, en: &str) -> String {
    match lang {
        Lang::Ru => ru.to_owned(),
        Lang::En => en.to_owned(),
    }
}

fn lang_code(lang: Lang) -> String {
    match lang {
        Lang::Ru => "ru".into(),
        Lang::En => "en".into(),
    }
}

struct Pending {
    action_id: String,
    action: String,
    lang: Lang,
    deadline: Instant,
    timer: JoinHandle<()>,
}

#[derive(Default)]
struct Inner {
    pending: Option<Pending>,
    discard_until: Option<Instant>,
}

/// Routes phrases to the executor and owns the confirmation round trip.
pub struct CommandCore {
    engine: Arc<dyn Engine>,
    shell: Arc<dyn Shell>,
    audit: Arc<dyn AuditSink>,
    ttl: Duration,
    error_hold: Duration,
    inner: Mutex<Inner>,
    /// Phrases are handled one at a time, in order.
    turn: tokio::sync::Mutex<()>,
    runs: AtomicU64,
    epoch: AtomicU64,
    /// Bumped by the emergency stop: an answer that was being thought out meanwhile is dropped.
    stops: AtomicU64,
    assistant: Option<Arc<dyn Assistant>>,
    /// The last text said, for the dialog buffer.
    last_spoken: Mutex<String>,
}

impl CommandCore {
    #[cfg(test)]
    pub fn new(
        engine: Arc<dyn Engine>,
        shell: Arc<dyn Shell>,
        audit: Arc<dyn AuditSink>,
    ) -> Arc<Self> {
        Self::with_timing(engine, shell, audit, CONFIRM_WINDOW, ERROR_HOLD)
    }

    /// The core with the AI path: phrases the FAST PATH does not know go to `assistant`.
    pub fn with_assistant(
        engine: Arc<dyn Engine>,
        shell: Arc<dyn Shell>,
        audit: Arc<dyn AuditSink>,
        assistant: Arc<dyn Assistant>,
    ) -> Arc<Self> {
        Self::build(
            engine,
            shell,
            audit,
            CONFIRM_WINDOW,
            ERROR_HOLD,
            Some(assistant),
        )
    }

    #[cfg(test)]
    pub fn with_timing(
        engine: Arc<dyn Engine>,
        shell: Arc<dyn Shell>,
        audit: Arc<dyn AuditSink>,
        ttl: Duration,
        error_hold: Duration,
    ) -> Arc<Self> {
        Self::build(engine, shell, audit, ttl, error_hold, None)
    }

    fn build(
        engine: Arc<dyn Engine>,
        shell: Arc<dyn Shell>,
        audit: Arc<dyn AuditSink>,
        ttl: Duration,
        error_hold: Duration,
        assistant: Option<Arc<dyn Assistant>>,
    ) -> Arc<Self> {
        Arc::new(Self {
            engine,
            shell,
            audit,
            ttl,
            error_hold,
            inner: Mutex::new(Inner::default()),
            turn: tokio::sync::Mutex::new(()),
            runs: AtomicU64::new(1),
            epoch: AtomicU64::new(0),
            stops: AtomicU64::new(0),
            assistant,
            last_spoken: Mutex::new(String::new()),
        })
    }

    /// The id of the confirmation waiting for an answer, if any.
    #[cfg(test)]
    pub fn pending_id(&self) -> Option<String> {
        lock(&self.inner)
            .pending
            .as_ref()
            .map(|pending| pending.action_id.clone())
    }

    /// Handles one phrase: an answer to a pending confirmation, a command, or something not understood.
    pub async fn submit(self: &Arc<Self>, text: &str, hint: Option<&str>, source: Source) {
        let text = text.trim();
        if text.is_empty() {
            return;
        }
        let _turn = self.turn.lock().await;
        if source == Source::Voice && self.take_discard() {
            tracing::info!("phrase dropped: it was being recorded when the emergency stop hit");
            return;
        }
        let lang = lang_of(text, hint);
        if let Some((action_id, pending_lang)) = self.live_pending() {
            if let Some(answer) = classify_answer(text) {
                self.resolve(&action_id, answer == Answer::Yes, pending_lang)
                    .await;
                return;
            }
        }
        lock(&self.last_spoken).clear();
        match parse(text, lang) {
            Some(intent) => {
                // A new command replaces a confirmation that nobody answered.
                self.cancel_pending("superseded by a new command").await;
                let action = intent.action.clone();
                let ok = self.run_intent(intent, lang).await;
                self.note(text, lang, Route::Fast, Some(action), ok).await;
            }
            None => self.fallback(text, lang).await,
        }
    }

    async fn note(&self, text: &str, lang: Lang, route: Route, action: Option<String>, ok: bool) {
        let Some(assistant) = &self.assistant else {
            return;
        };
        let reply = lock(&self.last_spoken).clone();
        assistant
            .note(Note {
                user: text.to_owned(),
                reply,
                lang,
                route,
                action,
                ok,
            })
            .await;
    }

    /// The single extension point for phrases the FAST PATH does not understand: the AI path.
    /// Without an assistant (or when no model is reachable) the answer is "did not understand".
    pub async fn fallback(self: &Arc<Self>, text: &str, lang: Lang) {
        let not_understood = pick(
            lang,
            "Не понял команду.",
            "I did not understand the command.",
        );
        let Some(assistant) = self.assistant.clone() else {
            self.say(&not_understood, lang, None);
            return;
        };
        let stops = self.stops.load(Ordering::SeqCst);
        self.shell.set_state("PROCESSING");
        let routed = assistant.route(text, lang).await;
        if self.stops.load(Ordering::SeqCst) != stops {
            tracing::info!("the answer was dropped: the emergency stop hit while it was prepared");
            return;
        }
        if self.shell.state() == "PROCESSING" {
            self.shell.set_state("IDLE");
        }
        match routed {
            Routed::Unavailable => {
                self.say(&not_understood, lang, None);
                self.note(text, lang, Route::None, None, false).await;
            }
            Routed::Reply {
                text: answer,
                route,
            } => {
                self.say(&answer, lang, Some("ai.reply"));
                self.note(text, lang, route, None, true).await;
            }
            Routed::Refused {
                text: answer,
                route,
            } => {
                self.say(&answer, lang, Some("ai.refused"));
                self.note(text, lang, route, None, false).await;
            }
            Routed::Executed {
                intent,
                outcome,
                route,
            } => {
                let ok = self
                    .finish(outcome, lang, &intent.action, None, false)
                    .await;
                self.note(text, lang, route, Some(intent.action), ok).await;
            }
        }
    }

    /// The UI button (or an answer given by voice): `false` when nothing is waiting under that id.
    pub async fn confirm_response(self: &Arc<Self>, action_id: &str, approved: bool) -> bool {
        let lang = match lock(&self.inner).pending.as_ref() {
            Some(pending) if pending.action_id == action_id => pending.lang,
            _ => return false,
        };
        self.resolve(action_id, approved, lang).await
    }

    /// Stops the speech, cancels confirmations and a scheduled shutdown, ends a recording, goes IDLE.
    pub async fn emergency_stop(self: &Arc<Self>) {
        let recording = matches!(self.shell.state().as_str(), "LISTENING" | "PROCESSING");
        self.shell.silence();
        let pending = lock(&self.inner).pending.take();
        let cancelled = self.engine.emergency_stop();
        if recording {
            lock(&self.inner).discard_until = Some(Instant::now() + DISCARD_WINDOW);
        }
        if let Some(pending) = pending {
            pending.timer.abort();
            self.shell.action_result(&ActionReport {
                action_id: pending.action_id,
                ok: false,
                detail: "emergency stop".into(),
            });
        }
        self.epoch.fetch_add(1, Ordering::SeqCst);
        self.stops.fetch_add(1, Ordering::SeqCst);
        self.shell.set_state("IDLE");
        self.audit
            .append(AuditRecord {
                action: "control.emergency_stop".into(),
                args: json!({"cancelled_confirmations": cancelled, "recording": recording}),
                risk: "safe".into(),
                ok: true,
                detail: format!("emergency stop: {cancelled} confirmation(s) cancelled"),
            })
            .await;
        self.shell.message(&AssistantMessage {
            text: "Аварийная остановка. / Emergency stop.".into(),
            lang: "ru".into(),
            skill: Some("control.emergency_stop".into()),
        });
    }

    /// Declines a waiting confirmation without speaking (settings changed, a new command came).
    pub async fn cancel_pending(self: &Arc<Self>, reason: &str) {
        let pending = lock(&self.inner).pending.take();
        let Some(pending) = pending else { return };
        pending.timer.abort();
        let _ = self.engine.confirm(&pending.action_id, false).await;
        self.shell.action_result(&ActionReport {
            action_id: pending.action_id,
            ok: false,
            detail: reason.into(),
        });
    }

    fn take_discard(&self) -> bool {
        let mut inner = lock(&self.inner);
        match inner.discard_until.take() {
            Some(until) => Instant::now() < until,
            None => false,
        }
    }

    /// The pending confirmation, unless its time is up (its timer closes it).
    fn live_pending(&self) -> Option<(String, Lang)> {
        let inner = lock(&self.inner);
        let pending = inner.pending.as_ref()?;
        (Instant::now() < pending.deadline).then(|| (pending.action_id.clone(), pending.lang))
    }

    fn next_run_id(&self) -> String {
        format!("run-{}", self.runs.fetch_add(1, Ordering::Relaxed))
    }

    fn executing(&self) {
        self.epoch.fetch_add(1, Ordering::SeqCst);
        self.shell.set_state("EXECUTING");
    }

    /// Back to IDLE, unless something else (a recording, a speech) already changed the state.
    fn idle(&self) {
        self.epoch.fetch_add(1, Ordering::SeqCst);
        if matches!(self.shell.state().as_str(), "EXECUTING" | "ERROR") {
            self.shell.set_state("IDLE");
        }
    }

    /// `ERROR` for [`ERROR_HOLD`], then IDLE (unless a newer command took over).
    fn error(self: &Arc<Self>) {
        let epoch = self.epoch.fetch_add(1, Ordering::SeqCst) + 1;
        self.shell.set_state("ERROR");
        let core = Arc::clone(self);
        let hold = self.error_hold;
        tokio::spawn(async move {
            sleep(hold).await;
            if core.epoch.load(Ordering::SeqCst) == epoch && core.shell.state() == "ERROR" {
                core.shell.set_state("IDLE");
            }
        });
    }

    fn say(&self, text: &str, lang: Lang, skill: Option<&str>) {
        *lock(&self.last_spoken) = text.to_owned();
        self.shell.message(&AssistantMessage {
            text: text.to_owned(),
            lang: lang_code(lang),
            skill: skill.map(str::to_owned),
        });
        self.shell.speak(text);
    }

    /// The full text goes to the chat, the short `speech` to the voice.
    fn say_split(&self, text: &str, speech: &str, lang: Lang, skill: Option<&str>) {
        *lock(&self.last_spoken) = speech.to_owned();
        self.shell.message(&AssistantMessage {
            text: text.to_owned(),
            lang: lang_code(lang),
            skill: skill.map(str::to_owned),
        });
        self.shell.speak(speech);
    }

    async fn run_intent(self: &Arc<Self>, intent: Intent, lang: Lang) -> bool {
        self.executing();
        let action = intent.action.clone();
        let outcome = self.engine.run(intent).await;
        self.finish(outcome, lang, &action, None, false).await
    }

    /// FAST PATH replies in the wording of the current personality.
    fn styled(&self, lang: Lang, text: &str) -> String {
        match &self.assistant {
            Some(assistant) => restyle(assistant.style(), lang, text),
            None => text.to_owned(),
        }
    }

    async fn resolve(self: &Arc<Self>, action_id: &str, approved: bool, lang: Lang) -> bool {
        let pending = {
            let mut inner = lock(&self.inner);
            match inner.pending.as_ref() {
                Some(pending) if pending.action_id == action_id => inner.pending.take(),
                _ => None,
            }
        };
        let Some(pending) = pending else {
            return false;
        };
        pending.timer.abort();
        if approved {
            self.executing();
        }
        let outcome = self.engine.confirm(action_id, approved).await;
        self.finish(outcome, lang, &pending.action, Some(action_id), !approved)
            .await;
        true
    }

    async fn expire(self: &Arc<Self>, action_id: &str) {
        let pending = {
            let mut inner = lock(&self.inner);
            match inner.pending.as_ref() {
                Some(pending) if pending.action_id == action_id => inner.pending.take(),
                _ => None,
            }
        };
        let Some(pending) = pending else { return };
        // The executor audits the expiry (or the refusal) itself.
        let _ = self.engine.confirm(action_id, false).await;
        self.shell.action_result(&ActionReport {
            action_id: pending.action_id.clone(),
            ok: false,
            detail: "confirmation expired".into(),
        });
        let text = pick(
            pending.lang,
            "Время подтверждения вышло. Команда отменена.",
            "The confirmation timed out. Command cancelled.",
        );
        self.say(&text, pending.lang, Some(&pending.action));
    }

    async fn finish(
        self: &Arc<Self>,
        outcome: Outcome,
        lang: Lang,
        action: &str,
        action_id: Option<&str>,
        declined: bool,
    ) -> bool {
        let id = action_id.map_or_else(|| self.next_run_id(), str::to_owned);
        match outcome {
            Outcome::Done {
                reply_ru,
                reply_en,
                detail,
            } => {
                self.idle();
                let text = self.styled(lang, &pick(lang, &reply_ru, &reply_en));
                self.shell.action_result(&ActionReport {
                    action_id: id,
                    ok: true,
                    detail: text.clone(),
                });
                // A long answer (vision) names a short `speech`: the chat shows everything, the
                // voice says only that part.
                match detail.get("speech").and_then(|value| value.as_str()) {
                    Some(speech) if !speech.is_empty() => {
                        self.say_split(&text, speech, lang, Some(action));
                    }
                    _ => self.say(&text, lang, Some(action)),
                }
                true
            }
            Outcome::NeedsConfirm { action_id, summary } => {
                self.idle();
                self.begin_confirm(action_id, summary, action, lang).await;
                true
            }
            Outcome::Denied { reason } => {
                self.idle();
                let text = denial_text(lang, &reason, declined);
                self.shell.action_result(&ActionReport {
                    action_id: id,
                    ok: false,
                    detail: reason,
                });
                self.say(&text, lang, Some(action));
                false
            }
            Outcome::Failed { code, message } => {
                self.error();
                let text = pick(
                    lang,
                    "Не удалось выполнить команду.",
                    "The command could not be carried out.",
                );
                self.shell.action_result(&ActionReport {
                    action_id: id,
                    ok: false,
                    detail: format!("{code}: {message}"),
                });
                self.say(&text, lang, Some(action));
                false
            }
        }
    }

    async fn begin_confirm(
        self: &Arc<Self>,
        action_id: String,
        summary: String,
        action: &str,
        lang: Lang,
    ) {
        // One dialog at a time: an older request is declined (and audited) first.
        self.cancel_pending("replaced by a newer confirmation")
            .await;
        let core = Arc::clone(self);
        let timer_id = action_id.clone();
        let ttl = self.ttl;
        let timer = tokio::spawn(async move {
            sleep(ttl).await;
            core.expire(&timer_id).await;
        });
        lock(&self.inner).pending = Some(Pending {
            action_id: action_id.clone(),
            action: action.to_owned(),
            lang,
            deadline: Instant::now() + self.ttl,
            timer,
        });
        self.shell.confirm_request(&ConfirmRequest {
            action_id,
            action: action.to_owned(),
            summary: summary.clone(),
            risk: "confirm".into(),
        });
        let prompt = pick(
            lang,
            &format!("Подтвердите: {summary}. Скажите «да» или «нет»."),
            &format!("Please confirm: {summary}. Say yes or no."),
        );
        self.say(&prompt, lang, Some(action));
    }
}

fn denial_text(lang: Lang, reason: &str, declined: bool) -> String {
    if declined {
        pick(lang, "Отменено.", "Cancelled.")
    } else if reason.contains("switched off") {
        pick(
            lang,
            "Это действие выключено в настройках.",
            "That action is switched off in the settings.",
        )
    } else if reason.contains("expired") {
        pick(
            lang,
            "Время подтверждения вышло.",
            "The confirmation timed out.",
        )
    } else {
        pick(lang, "Отказано.", "Denied.")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use jarvis_control::MemoryAudit;
    use serde_json::Value;
    use std::collections::VecDeque;
    use tauri::async_runtime::block_on;

    #[derive(Default)]
    struct FakeEngine {
        /// Outcomes handed out by `run`, in order.
        runs: Mutex<VecDeque<Outcome>>,
        intents: Mutex<Vec<Intent>>,
        confirms: Mutex<Vec<(String, bool)>>,
        confirm_outcome: Mutex<Option<Outcome>>,
        stops: AtomicU64,
        pending_cancelled: AtomicU64,
    }

    impl FakeEngine {
        fn queue(&self, outcome: Outcome) {
            lock(&self.runs).push_back(outcome);
        }
    }

    #[async_trait]
    impl Engine for FakeEngine {
        async fn run(&self, intent: Intent) -> Outcome {
            lock(&self.intents).push(intent);
            lock(&self.runs).pop_front().unwrap_or(Outcome::Failed {
                code: "NOT_FOUND".into(),
                message: "nothing queued".into(),
            })
        }

        async fn confirm(&self, action_id: &str, approved: bool) -> Outcome {
            lock(&self.confirms).push((action_id.to_owned(), approved));
            if !approved {
                return Outcome::Denied {
                    reason: "declined".into(),
                };
            }
            lock(&self.confirm_outcome).take().unwrap_or(Outcome::Done {
                reply_ru: "Готово.".into(),
                reply_en: "Done.".into(),
                detail: Value::Null,
            })
        }

        fn emergency_stop(&self) -> usize {
            self.stops.fetch_add(1, Ordering::SeqCst);
            self.pending_cancelled.load(Ordering::SeqCst) as usize
        }
    }

    #[derive(Default)]
    struct FakeShell {
        state: Mutex<String>,
        history: Mutex<Vec<String>>,
        spoken: Mutex<Vec<String>>,
        messages: Mutex<Vec<AssistantMessage>>,
        requests: Mutex<Vec<ConfirmRequest>>,
        reports: Mutex<Vec<ActionReport>>,
        silenced: AtomicU64,
    }

    impl FakeShell {
        fn set(&self, state: &str) {
            *lock(&self.state) = state.to_owned();
        }
        fn history(&self) -> Vec<String> {
            lock(&self.history).clone()
        }
        fn spoken(&self) -> Vec<String> {
            lock(&self.spoken).clone()
        }
        fn reports(&self) -> Vec<ActionReport> {
            lock(&self.reports).clone()
        }
    }

    impl Shell for FakeShell {
        fn set_state(&self, state: &str) {
            lock(&self.history).push(state.to_owned());
            self.set(state);
        }
        fn state(&self) -> String {
            lock(&self.state).clone()
        }
        fn speak(&self, text: &str) {
            lock(&self.spoken).push(text.to_owned());
        }
        fn message(&self, message: &AssistantMessage) {
            lock(&self.messages).push(message.clone());
        }
        fn confirm_request(&self, request: &ConfirmRequest) {
            lock(&self.requests).push(request.clone());
        }
        fn action_result(&self, report: &ActionReport) {
            lock(&self.reports).push(report.clone());
        }
        fn silence(&self) {
            self.silenced.fetch_add(1, Ordering::SeqCst);
        }
    }

    struct Rig {
        core: Arc<CommandCore>,
        engine: Arc<FakeEngine>,
        shell: Arc<FakeShell>,
        audit: Arc<MemoryAudit>,
    }

    fn rig(ttl: Duration, hold: Duration) -> Rig {
        let engine = Arc::new(FakeEngine::default());
        let shell = Arc::new(FakeShell::default());
        shell.set("IDLE");
        let audit = Arc::new(MemoryAudit::default());
        let core =
            CommandCore::with_timing(engine.clone(), shell.clone(), audit.clone(), ttl, hold);
        Rig {
            core,
            engine,
            shell,
            audit,
        }
    }

    fn done(ru: &str, en: &str) -> Outcome {
        Outcome::Done {
            reply_ru: ru.into(),
            reply_en: en.into(),
            detail: Value::Null,
        }
    }

    fn needs_confirm(id: &str, summary: &str) -> Outcome {
        Outcome::NeedsConfirm {
            action_id: id.into(),
            summary: summary.into(),
        }
    }

    #[test]
    fn a_done_command_replies_in_the_language_of_the_phrase() {
        block_on(async {
            let rig = rig(CONFIRM_WINDOW, ERROR_HOLD);
            rig.engine.queue(done("Сейчас 12:00.", "It is 12:00."));
            rig.core
                .submit("лёха, который час", Some("ru"), Source::Voice)
                .await;
            assert_eq!(lock(&rig.engine.intents)[0].action, "info.time");
            assert_eq!(rig.shell.spoken(), vec!["Сейчас 12:00.".to_owned()]);
            let message = lock(&rig.shell.messages)[0].clone();
            assert_eq!(message.lang, "ru");
            assert_eq!(message.skill.as_deref(), Some("info.time"));
            assert_eq!(rig.shell.history(), vec!["EXECUTING", "IDLE"]);
            assert!(rig.shell.reports()[0].ok);

            rig.engine.queue(done("Сейчас 12:00.", "It is 12:00."));
            rig.core
                .submit("what time is it", Some("en"), Source::Text)
                .await;
            assert_eq!(rig.shell.spoken()[1], "It is 12:00.");
            assert_eq!(lock(&rig.shell.messages)[1].lang, "en");
        });
    }

    #[test]
    fn a_phrase_the_fast_path_does_not_know_is_not_understood() {
        block_on(async {
            let rig = rig(CONFIRM_WINDOW, ERROR_HOLD);
            rig.core
                .submit("расскажи анекдот про космос", Some("ru"), Source::Text)
                .await;
            assert!(lock(&rig.engine.intents).is_empty());
            assert_eq!(rig.shell.spoken(), vec!["Не понял команду.".to_owned()]);
            assert!(rig.shell.history().is_empty());
        });
    }

    #[test]
    fn a_failure_shows_error_for_two_seconds_then_idle() {
        block_on(async {
            let rig = rig(CONFIRM_WINDOW, Duration::from_millis(40));
            rig.engine.queue(Outcome::Failed {
                code: "SYSTEM".into(),
                message: "boom".into(),
            });
            rig.core.submit("который час", None, Source::Text).await;
            assert_eq!(rig.shell.history(), vec!["EXECUTING", "ERROR"]);
            let report = rig.shell.reports()[0].clone();
            assert!(!report.ok);
            assert_eq!(report.detail, "SYSTEM: boom");
            sleep(Duration::from_millis(120)).await;
            assert_eq!(rig.shell.history(), vec!["EXECUTING", "ERROR", "IDLE"]);
        });
    }

    #[test]
    fn a_new_command_during_the_error_hold_keeps_its_own_state() {
        block_on(async {
            let rig = rig(CONFIRM_WINDOW, Duration::from_millis(80));
            rig.engine.queue(Outcome::Failed {
                code: "SYSTEM".into(),
                message: "boom".into(),
            });
            rig.core.submit("который час", None, Source::Text).await;
            rig.engine.queue(done("Сейчас.", "Now."));
            rig.core.submit("который час", None, Source::Text).await;
            sleep(Duration::from_millis(160)).await;
            // ERROR hold expiry must not add a second IDLE after the new command finished.
            assert_eq!(
                rig.shell.history(),
                vec!["EXECUTING", "ERROR", "EXECUTING", "IDLE"]
            );
        });
    }

    #[test]
    fn a_denied_command_is_refused_briefly() {
        block_on(async {
            let rig = rig(CONFIRM_WINDOW, ERROR_HOLD);
            rig.engine.queue(Outcome::Denied {
                reason: "action \"files.delete\" is switched off".into(),
            });
            rig.core.submit("который час", None, Source::Text).await;
            assert_eq!(
                rig.shell.spoken(),
                vec!["Это действие выключено в настройках.".to_owned()]
            );
            assert!(!rig.shell.reports()[0].ok);
            assert_eq!(rig.shell.state(), "IDLE");
        });
    }

    #[test]
    fn needs_confirm_asks_aloud_and_in_the_ui() {
        block_on(async {
            let rig = rig(CONFIRM_WINDOW, ERROR_HOLD);
            rig.engine
                .queue(needs_confirm("act-1", "Выключить компьютер через 60 с"));
            rig.core
                .submit("выключи компьютер", None, Source::Text)
                .await;
            let request = lock(&rig.shell.requests)[0].clone();
            assert_eq!(request.action_id, "act-1");
            assert_eq!(request.action, "power.shutdown");
            assert_eq!(request.risk, "confirm");
            assert_eq!(
                rig.shell.spoken(),
                vec![
                    "Подтвердите: Выключить компьютер через 60 с. Скажите «да» или «нет»."
                        .to_owned()
                ]
            );
            assert_eq!(rig.core.pending_id().as_deref(), Some("act-1"));
            assert_eq!(rig.shell.state(), "IDLE");
        });
    }

    #[test]
    fn the_button_approves_and_runs_the_action() {
        block_on(async {
            let rig = rig(CONFIRM_WINDOW, ERROR_HOLD);
            rig.engine.queue(needs_confirm("act-1", "s"));
            rig.core
                .submit("выключи компьютер", None, Source::Text)
                .await;
            assert!(rig.core.confirm_response("act-1", true).await);
            assert_eq!(
                lock(&rig.engine.confirms).clone(),
                vec![("act-1".to_owned(), true)]
            );
            assert_eq!(rig.core.pending_id(), None);
            assert_eq!(
                rig.shell.spoken().last().map(String::as_str),
                Some("Готово.")
            );
            assert_eq!(
                rig.shell.history(),
                vec!["EXECUTING", "IDLE", "EXECUTING", "IDLE"]
            );
            assert!(rig
                .shell
                .reports()
                .last()
                .is_some_and(|r| r.ok && r.action_id == "act-1"));
        });
    }

    #[test]
    fn the_button_can_decline() {
        block_on(async {
            let rig = rig(CONFIRM_WINDOW, ERROR_HOLD);
            rig.engine.queue(needs_confirm("act-1", "s"));
            rig.core
                .submit("выключи компьютер", None, Source::Text)
                .await;
            assert!(rig.core.confirm_response("act-1", false).await);
            assert_eq!(
                lock(&rig.engine.confirms).clone(),
                vec![("act-1".to_owned(), false)]
            );
            assert_eq!(
                rig.shell.spoken().last().map(String::as_str),
                Some("Отменено.")
            );
            let report = rig.shell.reports().last().cloned();
            assert_eq!(
                report.map(|r| (r.ok, r.action_id)),
                Some((false, "act-1".to_owned()))
            );
        });
    }

    #[test]
    fn an_unknown_or_used_id_is_not_found() {
        block_on(async {
            let rig = rig(CONFIRM_WINDOW, ERROR_HOLD);
            assert!(!rig.core.confirm_response("act-9", true).await);
            rig.engine.queue(needs_confirm("act-1", "s"));
            rig.core
                .submit("выключи компьютер", None, Source::Text)
                .await;
            assert!(!rig.core.confirm_response("act-2", true).await);
            assert!(rig.core.confirm_response("act-1", true).await);
            assert!(!rig.core.confirm_response("act-1", true).await);
            assert_eq!(lock(&rig.engine.confirms).len(), 1);
        });
    }

    #[test]
    fn voice_yes_confirms_with_and_without_the_address() {
        for phrase in [
            "да",
            "лёха, да",
            "Да, подтверждаю",
            "yes",
            "confirm",
            "выполняй",
            "алексей выполняй",
        ] {
            block_on(async {
                let rig = rig(CONFIRM_WINDOW, ERROR_HOLD);
                rig.engine.queue(needs_confirm("act-1", "s"));
                rig.core
                    .submit("выключи компьютер", None, Source::Text)
                    .await;
                rig.core.submit(phrase, Some("ru"), Source::Voice).await;
                assert_eq!(
                    lock(&rig.engine.confirms).clone(),
                    vec![("act-1".to_owned(), true)],
                    "{phrase}"
                );
            });
        }
    }

    #[test]
    fn voice_no_declines_with_and_without_the_address() {
        for phrase in [
            "нет",
            "лёха, нет",
            "отмена",
            "не надо",
            "no",
            "cancel",
            "лёха нет, отмена",
        ] {
            block_on(async {
                let rig = rig(CONFIRM_WINDOW, ERROR_HOLD);
                rig.engine.queue(needs_confirm("act-1", "s"));
                rig.core
                    .submit("выключи компьютер", None, Source::Text)
                    .await;
                rig.core.submit(phrase, Some("ru"), Source::Voice).await;
                assert_eq!(
                    lock(&rig.engine.confirms).clone(),
                    vec![("act-1".to_owned(), false)],
                    "{phrase}"
                );
                assert_eq!(
                    rig.shell.spoken().last().map(String::as_str),
                    Some("Отменено.")
                );
            });
        }
    }

    #[test]
    fn ambiguous_answers_never_confirm() {
        for phrase in ["да нет", "не выполняй", "может быть", "да подожди", "угу"]
        {
            assert_eq!(classify_answer(phrase), None, "{phrase}");
        }
        block_on(async {
            let rig = rig(CONFIRM_WINDOW, ERROR_HOLD);
            rig.engine.queue(needs_confirm("act-1", "s"));
            rig.core
                .submit("выключи компьютер", None, Source::Text)
                .await;
            rig.core.submit("может быть", None, Source::Voice).await;
            assert!(lock(&rig.engine.confirms).is_empty());
            assert_eq!(rig.core.pending_id().as_deref(), Some("act-1"));
        });
    }

    #[test]
    fn yes_without_a_pending_confirmation_is_just_an_unknown_phrase() {
        block_on(async {
            let rig = rig(CONFIRM_WINDOW, ERROR_HOLD);
            rig.core.submit("да", Some("ru"), Source::Voice).await;
            assert!(lock(&rig.engine.confirms).is_empty());
            assert!(lock(&rig.engine.intents).is_empty());
            assert_eq!(rig.shell.spoken(), vec!["Не понял команду.".to_owned()]);
        });
    }

    #[test]
    fn english_answers_get_english_replies() {
        block_on(async {
            let rig = rig(CONFIRM_WINDOW, ERROR_HOLD);
            rig.engine
                .queue(needs_confirm("act-1", "Shut down in 60 s"));
            rig.core
                .submit("shut down the computer", None, Source::Text)
                .await;
            assert_eq!(
                rig.shell.spoken(),
                vec!["Please confirm: Shut down in 60 s. Say yes or no.".to_owned()]
            );
            rig.core.submit("no", Some("en"), Source::Voice).await;
            assert_eq!(
                rig.shell.spoken().last().map(String::as_str),
                Some("Cancelled.")
            );
        });
    }

    #[test]
    fn an_unanswered_confirmation_times_out_and_is_declined() {
        block_on(async {
            let rig = rig(Duration::from_millis(60), ERROR_HOLD);
            rig.engine.queue(needs_confirm("act-1", "s"));
            rig.core
                .submit("выключи компьютер", None, Source::Text)
                .await;
            assert_eq!(rig.core.pending_id().as_deref(), Some("act-1"));
            sleep(Duration::from_millis(200)).await;
            assert_eq!(rig.core.pending_id(), None);
            assert_eq!(
                lock(&rig.engine.confirms).clone(),
                vec![("act-1".to_owned(), false)]
            );
            let report = rig.shell.reports().last().cloned();
            assert_eq!(
                report.map(|r| (r.ok, r.detail)),
                Some((false, "confirmation expired".to_owned()))
            );
            // Too late: the answer finds nothing to confirm.
            assert!(!rig.core.confirm_response("act-1", true).await);
            rig.core.submit("да", Some("ru"), Source::Voice).await;
            assert_eq!(lock(&rig.engine.confirms).len(), 1);
        });
    }

    #[test]
    fn an_answer_in_time_cancels_the_timer() {
        block_on(async {
            let rig = rig(Duration::from_millis(80), ERROR_HOLD);
            rig.engine.queue(needs_confirm("act-1", "s"));
            rig.core
                .submit("выключи компьютер", None, Source::Text)
                .await;
            rig.core.submit("да", Some("ru"), Source::Voice).await;
            sleep(Duration::from_millis(200)).await;
            // One confirm call (the approval), no late "expired" decline.
            assert_eq!(
                lock(&rig.engine.confirms).clone(),
                vec![("act-1".to_owned(), true)]
            );
        });
    }

    #[test]
    fn a_new_command_replaces_the_waiting_confirmation() {
        block_on(async {
            let rig = rig(CONFIRM_WINDOW, ERROR_HOLD);
            rig.engine.queue(needs_confirm("act-1", "s"));
            rig.engine.queue(done("Сейчас.", "Now."));
            rig.core
                .submit("выключи компьютер", None, Source::Text)
                .await;
            rig.core.submit("который час", None, Source::Text).await;
            assert_eq!(
                lock(&rig.engine.confirms).clone(),
                vec![("act-1".to_owned(), false)]
            );
            assert_eq!(rig.core.pending_id(), None);
        });
    }

    #[test]
    fn a_second_confirmation_declines_the_first() {
        block_on(async {
            let rig = rig(CONFIRM_WINDOW, ERROR_HOLD);
            rig.engine.queue(needs_confirm("act-1", "a"));
            rig.engine.queue(needs_confirm("act-2", "b"));
            rig.core
                .submit("выключи компьютер", None, Source::Text)
                .await;
            rig.core
                .submit("перезагрузи компьютер", None, Source::Text)
                .await;
            assert_eq!(
                lock(&rig.engine.confirms).clone(),
                vec![("act-1".to_owned(), false)]
            );
            assert_eq!(rig.core.pending_id().as_deref(), Some("act-2"));
        });
    }

    #[test]
    fn the_emergency_stop_clears_everything_and_is_audited() {
        block_on(async {
            let rig = rig(CONFIRM_WINDOW, ERROR_HOLD);
            rig.engine.pending_cancelled.store(1, Ordering::SeqCst);
            rig.engine.queue(needs_confirm("act-1", "s"));
            rig.core
                .submit("выключи компьютер", None, Source::Text)
                .await;
            rig.shell.set("SPEAKING");
            rig.core.emergency_stop().await;
            assert_eq!(rig.shell.silenced.load(Ordering::SeqCst), 1);
            assert_eq!(rig.engine.stops.load(Ordering::SeqCst), 1);
            assert_eq!(rig.core.pending_id(), None);
            assert_eq!(rig.shell.state(), "IDLE");
            let report = rig.shell.reports().last().cloned();
            assert_eq!(
                report.map(|r| (r.action_id, r.ok, r.detail)),
                Some(("act-1".to_owned(), false, "emergency stop".to_owned()))
            );
            let records = rig.audit.records();
            assert_eq!(records.len(), 1);
            assert_eq!(records[0].action, "control.emergency_stop");
            assert!(records[0].ok);
            assert_eq!(records[0].args["cancelled_confirmations"], 1);
            // The answer that follows finds nothing to confirm.
            rig.core.submit("да", Some("ru"), Source::Voice).await;
            assert!(lock(&rig.engine.confirms).is_empty());
        });
    }

    #[test]
    fn the_emergency_stop_drops_the_phrase_that_was_being_recorded() {
        block_on(async {
            let rig = rig(CONFIRM_WINDOW, ERROR_HOLD);
            rig.shell.set("LISTENING");
            rig.core.emergency_stop().await;
            rig.engine.queue(done("Сейчас.", "Now."));
            rig.core
                .submit("который час", Some("ru"), Source::Voice)
                .await;
            assert!(
                lock(&rig.engine.intents).is_empty(),
                "the stale phrase must not run"
            );
            // Only that one phrase: the next one works.
            rig.core
                .submit("который час", Some("ru"), Source::Voice)
                .await;
            assert_eq!(lock(&rig.engine.intents).len(), 1);
        });
    }

    #[test]
    fn the_emergency_stop_while_idle_does_not_swallow_the_next_phrase() {
        block_on(async {
            let rig = rig(CONFIRM_WINDOW, ERROR_HOLD);
            rig.core.emergency_stop().await;
            rig.engine.queue(done("Сейчас.", "Now."));
            rig.core
                .submit("который час", Some("ru"), Source::Voice)
                .await;
            assert_eq!(lock(&rig.engine.intents).len(), 1);
        });
    }

    #[test]
    fn cancel_pending_declines_through_the_engine() {
        block_on(async {
            let rig = rig(CONFIRM_WINDOW, ERROR_HOLD);
            rig.engine.queue(needs_confirm("act-1", "s"));
            rig.core
                .submit("выключи компьютер", None, Source::Text)
                .await;
            rig.core.cancel_pending("settings changed").await;
            assert_eq!(
                lock(&rig.engine.confirms).clone(),
                vec![("act-1".to_owned(), false)]
            );
            assert_eq!(rig.core.pending_id(), None);
            let detail = rig.shell.reports().last().map(|r| r.detail.clone());
            assert_eq!(detail.as_deref(), Some("settings changed"));
        });
    }

    #[test]
    fn language_follows_the_script_of_the_phrase() {
        assert_eq!(lang_of("открой блокнот", Some("en")), Lang::Ru);
        assert_eq!(lang_of("open notepad", Some("ru")), Lang::En);
        assert_eq!(lang_of("123", Some("en")), Lang::En);
        assert_eq!(lang_of("123", None), Lang::Ru);
    }

    #[test]
    fn answers_are_classified() {
        assert_eq!(classify_answer("Да."), Some(Answer::Yes));
        assert_eq!(classify_answer("лёха, да"), Some(Answer::Yes));
        assert_eq!(classify_answer("YES, confirm"), Some(Answer::Yes));
        assert_eq!(classify_answer("нет, отмена"), Some(Answer::No));
        assert_eq!(classify_answer("не надо"), Some(Answer::No));
        assert_eq!(classify_answer(""), None);
        assert_eq!(classify_answer("надо"), None);
        assert_eq!(classify_answer("открой блокнот"), None);
    }
}

#[cfg(test)]
#[path = "dispatch_ai_tests.rs"]
mod ai_tests;
