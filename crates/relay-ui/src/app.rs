//! Application state, actions, and streaming lifecycle.
//!
//! `RelayApp` owns the database handle, preferences, the open conversation, and
//! every focusable control. Rendering is split across `sidebar`, `chat`, and
//! `settings`; this module holds the behavior they share.

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex};

use gpui_kit::component::{
    ActiveTheme as _, StyledExt as _, TitleBar, WindowExt as _, h_flex,
    input::{InputEvent, InputState, TextareaState},
    notification::Notification,
    select::{SearchableVec, SelectEvent, SelectState},
    text::TextViewState,
    v_flex,
};
use gpui_kit::{
    AppContext as _, Context, Entity, InteractiveElement as _, IntoElement, KeyDownEvent,
    ParentElement as _, Render, SharedString, Styled as _, Subscription, Task, Window, div,
};
use relay_core::{
    AppSettings, ChatMessage, ContentBlock, ConversationSummary, ModelSelection, PersonalityPreset,
    Provider, ReasoningEffort, ReasoningKind, Role, compose_system_prompt, find_builtin_preset,
    fit_history, request_history,
};
use relay_providers::{ChatRequest, StreamEvent};
use relay_store::Store;
use tokio::runtime::Handle;
use tokio_util::sync::CancellationToken;

use crate::keys;
use crate::models::{self, ModelSelect};
use crate::settings::{SettingsEvent, SettingsView};
use crate::streaming::{self, StartedStream};
use crate::theme;

/// Which primary surface is showing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum View {
    /// Conversation transcript and composer.
    Chat,
    /// Full-page preferences.
    Settings,
}

/// Sidebar state kept separate from the transcript.
pub struct SidebarState {
    pub(crate) search: Entity<InputState>,
    pub(crate) scroll: gpui_kit::ScrollHandle,
}

/// Chat surface state: composer, pickers, and render caches.
pub struct ChatState {
    pub(crate) composer: Entity<TextareaState>,
    pub(crate) model_select: ModelSelect,
    pub(crate) reasoning_select: Option<Entity<SelectState<SearchableVec<SharedString>>>>,
    pub(crate) approval_select: Entity<SelectState<SearchableVec<SharedString>>>,
    /// Markdown state per assistant message.
    pub(crate) text_states: HashMap<String, Entity<TextViewState>>,
    /// Markdown state per reasoning block.
    pub(crate) thinking_states: HashMap<String, Entity<TextViewState>>,
    /// Assistant messages whose reasoning section is expanded.
    pub(crate) expanded: HashSet<String>,
    pub(crate) scroll: gpui_kit::ScrollHandle,
    pub(crate) presets: Vec<PersonalityPreset>,
    pub(crate) terminal_open: bool,
    pub(crate) terminal: Option<crate::terminal::ShellSession>,
    pub(crate) terminal_input: Entity<InputState>,
    pub(crate) terminal_scroll: gpui_kit::ScrollHandle,
    pub(crate) terminal_task: Option<Task<()>>,
}

impl ChatState {
    fn clear_caches(&mut self) {
        self.text_states.clear();
        self.thinking_states.clear();
        self.expanded.clear();
    }
}

/// A stream owned by the app. Dropping the task cancels the UI consumer; the
/// cancel token stops the network request.
struct ActiveStream {
    conversation_id: String,
    message_id: String,
    cancel: CancellationToken,
    _task: Task<()>,
}

/// Relay's main view.
pub struct RelayApp {
    pub(crate) store: Store,
    pub(crate) runtime: Handle,
    pub(crate) settings: AppSettings,
    pub(crate) view: View,
    pub(crate) conversations: Vec<ConversationSummary>,
    pub(crate) conversation_id: Option<String>,
    pub(crate) conversation_title: String,
    pub(crate) messages: Vec<ChatMessage>,
    /// Model used when no conversation has been saved yet.
    pub(crate) draft_model: String,
    pub(crate) sidebar: SidebarState,
    pub(crate) chat: ChatState,
    pub(crate) settings_view: Option<Entity<SettingsView>>,
    stream: Option<ActiveStream>,
    /// A dismissible message shown above the composer.
    pub(crate) notice: Option<String>,
    /// Message currently being edited: (message_id, input_state).
    pub(crate) editing: Option<(String, gpui_kit::Entity<InputState>)>,
    /// Whether any cloud provider has a usable credential.
    pub(crate) cloud_keys_ready: bool,
    model_subscription: Option<Subscription>,
    reasoning_subscription: Option<Subscription>,
    approval_subscription: Option<Subscription>,
    settings_subscription: Option<Subscription>,
    bundled: Arc<Mutex<crate::bundled::BundledState>>,
    bundled_applied: bool,
    bundled_needs_refresh: bool,
    pub(crate) pending_approval: Option<crate::streaming::ApprovalRequest>,
    _subscriptions: Vec<Subscription>,
}

impl RelayApp {
    /// Build the application, restoring the most recent conversation.
    pub fn new(
        store: Store,
        runtime: Handle,
        bundled: Arc<Mutex<crate::bundled::BundledState>>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let settings = store.load_settings().unwrap_or_default();
        let conversations = store.list_conversations().unwrap_or_default();
        let latest = store.latest_conversation().ok().flatten();
        let presets = store
            .presets()
            .unwrap_or_else(|_| relay_core::builtin_presets());

        let search = cx.new(|cx| InputState::new(window, cx).placeholder("Search conversations…"));
        let composer = cx.new(|cx| {
            TextareaState::new(window, cx)
                .placeholder("Message Relay…")
                .auto_grow(1, 8)
                .submit_on_enter(settings.send_on_enter)
        });

        let initial_model = latest
            .as_ref()
            .map(|summary| summary.model_slug.clone())
            .unwrap_or_else(|| settings.default_model.clone());
        let model_select = models::model_select(&settings, &initial_model, window, cx);
        let reasoning_select = if models::resolve(&initial_model).reasoning == ReasoningKind::None {
            None
        } else {
            Some(models::string_select(
                &reasoning_labels(),
                settings.reasoning.label(),
                window,
                cx,
            ))
        };
        let approval_select = models::string_select(
            &approval_labels(),
            settings.tool_approval.label(),
            window,
            cx,
        );
        let terminal_input =
            cx.new(|cx| InputState::new(window, cx).placeholder("Run a command in your shell…"));

        let mut app = Self {
            store,
            runtime,
            settings,
            view: View::Chat,
            conversations,
            conversation_id: None,
            conversation_title: "New chat".to_owned(),
            messages: Vec::new(),
            draft_model: initial_model,
            sidebar: SidebarState {
                search,
                scroll: gpui_kit::ScrollHandle::new(),
            },
            chat: ChatState {
                composer,
                model_select,
                reasoning_select,
                approval_select,
                text_states: HashMap::new(),
                thinking_states: HashMap::new(),
                expanded: HashSet::new(),
                scroll: gpui_kit::ScrollHandle::new(),
                presets,
                terminal_open: false,
                terminal: None,
                terminal_input,
                terminal_scroll: gpui_kit::ScrollHandle::new(),
                terminal_task: None,
            },
            settings_view: None,
            stream: None,
            notice: None,
            editing: None,
            cloud_keys_ready: Provider::CLOUD
                .iter()
                .any(|provider| keys::source(*provider).is_set()),
            model_subscription: None,
            reasoning_subscription: None,
            approval_subscription: None,
            settings_subscription: None,
            bundled,
            bundled_applied: false,
            bundled_needs_refresh: false,
            pending_approval: None,
            _subscriptions: Vec::new(),
        };

        app.attach_subscriptions(window, cx);
        // Bundled model is prepared lazily (on demand) to keep idle RAM low.
        app.spawn_bundled_watcher(cx);
        app.spawn_memory_summarizer();
        if let Some(summary) = latest {
            app.load_conversation_data(&summary.id, window, cx);
            app.rebuild_model_select(window, cx);
            app.rebuild_reasoning_select(window, cx);
        }

        app
    }

    fn spawn_memory_summarizer(&self) {
        let store = self.store.clone();
        let bundled = self.bundled.clone();
        let runtime = self.runtime.clone();
        runtime.spawn(async move {
            use std::time::Duration;
            loop {
                let mut endpoint = None;
                for _ in 0..360 {
                    let st = bundled.lock().map(|s| s.status()).unwrap_or_default();
                    if st.ready {
                        endpoint = st.endpoint;
                        break;
                    }
                    tokio::time::sleep(Duration::from_secs(10)).await;
                }
                let Some(endpoint) = endpoint else {
                    tokio::time::sleep(Duration::from_secs(60)).await;
                    continue;
                };

                let mut settings = store.load_settings().unwrap_or_default();
                let now = chrono::Utc::now().timestamp();
                if now - settings.last_memory_summary_at < 10 * 3600 {
                    tokio::time::sleep(Duration::from_secs(3600)).await;
                    continue;
                }

                let convs = store.list_conversations().unwrap_or_default();
                let mut text = String::new();
                for c in convs.iter().take(8) {
                    if let Ok(msgs) = store.messages(&c.id) {
                        text.push_str(&format!("Conversation: {}\n", c.title));
                        for m in msgs.iter().take(30) {
                            let t = m.text();
                            if t.trim().is_empty() {
                                continue;
                            }
                            text.push_str(&format!("{}: {}\n", m.role.as_str(), t.chars().take(800).collect::<String>()));
                        }
                    }
                }
                if text.trim().is_empty() {
                    settings.last_memory_summary_at = now;
                    let _ = store.save_settings(&settings);
                    tokio::time::sleep(Duration::from_secs(3600)).await;
                    continue;
                }

                let prompt = format!(
                    "Summarize the following conversation history into a concise cross-session memory \
                     of user preferences, facts, and ongoing tasks. Output only the summary, no preamble.\n\n{text}"
                );
                let client = reqwest::Client::new();
                let body = serde_json::json!({
                    "model": "local-model",
                    "messages": [{"role":"user","content":prompt}],
                    "max_tokens": 512,
                });
                let url = format!("{}/chat/completions", endpoint.trim_end_matches('/'));
                if let Ok(resp) = client.post(&url).json(&body).send().await {
                    if let Ok(json) = resp.json::<serde_json::Value>().await {
                        if let Some(summary) = json
                            .get("choices")
                            .and_then(|c| c.get(0))
                            .and_then(|c| c.get("message"))
                            .and_then(|m| m.get("content"))
                            .and_then(|x| x.as_str())
                        {
                            if let Ok(mems) = store.list_memories() {
                                for (id, content) in mems {
                                    if content.starts_with("[Auto-summary]") {
                                        let _ = store.delete_memory(&id);
                                    }
                                }
                            }
                            let _ = store.add_memory(&format!("[Auto-summary] {}", summary.trim()));
                            settings.last_memory_summary_at = chrono::Utc::now().timestamp();
                            let _ = store.save_settings(&settings);
                        }
                    }
                }
                tokio::time::sleep(Duration::from_secs(3600)).await;
            }
        });
    }

    pub(crate) fn bundled_status(&self) -> crate::bundled::BundledStatus {
        self.bundled.lock().map(|s| s.status()).unwrap_or_default()
    }

    fn selected_bundled_variant(&self) -> crate::bundled::BundledVariant {
        crate::bundled::BundledVariant::from_id(&self.settings.bundled_model_variant)
            .or_else(|| crate::bundled::BundledVariant::from_model_slug(&self.settings.local_model))
            .unwrap_or(crate::bundled::DEFAULT_VARIANT)
    }

    fn bundled_variant_for_slug(slug: &str) -> Option<crate::bundled::BundledVariant> {
        crate::bundled::BundledVariant::from_model_slug(slug)
    }

    fn start_bundled_for_settings(&self) {
        let variant = self.selected_bundled_variant();
        let ctx = self.settings.bundled_context_size;
        let bundled = self.bundled.clone();
        self.runtime.spawn(async move {
            crate::bundled::ensure_ready(bundled, variant, ctx).await;
        });
    }

    fn ensure_bundled_variant(
        &mut self,
        variant: crate::bundled::BundledVariant,
        cx: &mut Context<Self>,
    ) {
        let status = self.bundled.lock().map(|s| s.status()).unwrap_or_default();
        let needs = status.active_variant.as_deref() != Some(variant.id())
            || !status.ready
            || status.endpoint.is_none();
        if needs {
            let ctx = self.settings.bundled_context_size;
            let bundled = self.bundled.clone();
            self.runtime.spawn(async move {
                crate::bundled::ensure_ready(bundled, variant, ctx).await;
            });
            if let Ok(mut s) = self.bundled.lock() {
                if s.status.active_variant.as_deref() != Some(variant.id()) {
                    s.status.ready = false;
                    s.status.endpoint = None;
                    s.status.message = format!("Preparing {}…", variant.display_name());
                    s.status.error = None;
                }
            }
            cx.notify();
        }
    }

    fn spawn_bundled_watcher(&mut self, cx: &mut Context<Self>) {
        let bundled = self.bundled.clone();
        cx.spawn(async move |weak_self, cx| {
            loop {
                smol::Timer::after(std::time::Duration::from_secs(2)).await;
                let status = bundled.lock().map(|s| s.status()).unwrap_or_default();
                if !status.ready || status.endpoint.is_none() {
                    continue;
                }
                let endpoint = status.endpoint.clone().unwrap_or_default();
                let active = status.active_variant.clone().unwrap_or_default();
                let _ = weak_self.update(cx, |this, cx| {
                    if this.bundled_applied
                        && this.settings.local_base_url == endpoint
                        && this.settings.local_model == active
                    {
                        return;
                    }
                    this.bundled_applied = true;
                    let desired = this.selected_bundled_variant();
                    // If the ready variant matches desired/selected bundled, wire settings to it.
                    if active == desired.id()
                        || this.settings.local_model.trim().is_empty()
                        || crate::bundled::BundledVariant::from_id(&this.settings.local_model)
                            .is_some()
                    {
                        this.settings.local_base_url = endpoint.clone();
                        this.settings.local_model = active.clone();
                        this.settings.bundled_model_variant = active.clone();
                    }
                    if !this.cloud_keys_ready && this.conversation_id.is_none() {
                        let v = crate::bundled::BundledVariant::from_id(&active)
                            .unwrap_or(crate::bundled::DEFAULT_VARIANT);
                        this.draft_model = format!("local/{}", v.id());
                        this.settings.default_model = this.draft_model.clone();
                    }
                    this.persist_settings();
                    this.bundled_needs_refresh = true;
                    cx.notify();
                });
            }
        })
        .detach();
    }

    fn apply_bundled_ready(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.bundled_needs_refresh = false;
        self.rebuild_model_select(window, cx);
        self.rebuild_reasoning_select(window, cx);
        let status = self.bundled.lock().map(|s| s.status()).unwrap_or_default();
        if status.ready {
            self.notice = Some(status.message.clone());
        }
        cx.notify();
    }

    // MARK: Subscriptions

    fn attach_subscriptions(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.attach_model_subscription(window, cx);
        self.attach_reasoning_subscription(window, cx);
        self.attach_approval_subscription(window, cx);

        let terminal_input = self.chat.terminal_input.clone();
        self._subscriptions.push(cx.subscribe_in(
            &terminal_input,
            window,
            |this, _, event, window, cx| {
                if let gpui_kit::component::input::InputEvent::PressEnter { .. } = event {
                    this.send_terminal_command(window, cx);
                }
            },
        ));

        let search = self.sidebar.search.clone();
        self._subscriptions
            .push(cx.subscribe_in(&search, window, |this, _, event, _, cx| {
                if matches!(event, InputEvent::Change) {
                    this.refresh_conversations(cx);
                }
            }));

        let composer = self.chat.composer.clone();
        self._subscriptions.push(cx.subscribe_in(
            &composer,
            window,
            |this, _, event, window, cx| {
                if let InputEvent::PressEnter { shift, .. } = event
                    && !shift
                {
                    this.send(window, cx);
                }
            },
        ));
    }

    fn attach_model_subscription(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let select = self.chat.model_select.clone();
        self.model_subscription =
            Some(
                cx.subscribe_in(&select, window, |this, _, event, window, cx| {
                    if let SelectEvent::Confirm(Some(slug)) = event {
                        this.choose_model(slug.clone(), window, cx);
                    }
                }),
            );
    }

    fn attach_reasoning_subscription(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.reasoning_subscription = None;
        let Some(select) = self.chat.reasoning_select.clone() else {
            return;
        };
        self.reasoning_subscription =
            Some(
                cx.subscribe_in(&select, window, |this, _, event, _window, cx| {
                    if let SelectEvent::Confirm(Some(label)) = event
                        && let Some(effort) = ReasoningEffort::ALL
                            .iter()
                            .find(|effort| effort.label() == label.as_ref())
                    {
                        this.settings.reasoning = *effort;
                        this.persist_settings();
                        cx.notify();
                    }
                }),
            );
    }

    fn attach_approval_subscription(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let select = self.chat.approval_select.clone();
        self.approval_subscription = Some(cx.subscribe_in(
            &select,
            window,
            |this, _, event, _window, cx| {
                if let SelectEvent::Confirm(Some(label)) = event {
                    if label.as_ref() == relay_core::ToolApproval::None.label() {
                        this.set_tool_approval(relay_core::ToolApproval::None, cx);
                    } else if label.as_ref() == relay_core::ToolApproval::NonDestructive.label() {
                        this.set_tool_approval(relay_core::ToolApproval::NonDestructive, cx);
                    } else if label.as_ref() == relay_core::ToolApproval::All.label() {
                        this.set_tool_approval(relay_core::ToolApproval::All, cx);
                    }
                }
            },
        ));
    }

    // MARK: Conversation state

    /// Refresh the sidebar list, honoring the current search query.
    pub(crate) fn refresh_conversations(&mut self, cx: &mut Context<Self>) {
        let query = self.sidebar.search.read(cx).value().trim().to_owned();
        let result = if query.is_empty() {
            self.store.list_conversations()
        } else {
            self.store.search_conversations(&query)
        };
        match result {
            Ok(items) => self.conversations = items,
            Err(error) => tracing::warn!(%error, "could not refresh conversation list"),
        }
        cx.notify();
    }

    fn load_conversation_data(&mut self, id: &str, window: &mut Window, cx: &mut Context<Self>) {
        let summary = match self.store.conversation(id) {
            Ok(summary) => summary,
            Err(error) => {
                tracing::warn!(%error, "could not open conversation");
                return;
            }
        };
        let messages = match self.store.messages(id) {
            Ok(messages) => messages,
            Err(error) => {
                tracing::warn!(%error, "could not read conversation messages");
                return;
            }
        };

        self.conversation_id = Some(summary.id.clone());
        self.conversation_title = summary.title.clone();
        self.draft_model = summary.model_slug.clone();
        self.messages = messages;
        self.chat.clear_caches();
        self.build_render_caches(cx);
        self.notice = None;

        let slug = summary.model_slug.clone();
        self.chat.model_select.update(cx, |state, cx| {
            state.set_selected_value(&slug, window, cx);
        });
        cx.notify();
    }

    fn build_render_caches(&mut self, cx: &mut Context<Self>) {
        for index in 0..self.messages.len() {
            if self.messages[index].role != Role::Assistant {
                continue;
            }
            let id = self.messages[index].id.clone();
            let text = self.messages[index].text();
            let thinking = self.messages[index].thinking();
            let state = cx.new(|cx| TextViewState::markdown(&text, cx));
            self.chat.text_states.insert(id.clone(), state);
            if !thinking.trim().is_empty() {
                let state = cx.new(|cx| TextViewState::markdown(&thinking, cx));
                self.chat.thinking_states.insert(id, state);
            }
        }
    }

    /// Open a conversation and synchronize the pickers.
    pub(crate) fn open_conversation(
        &mut self,
        id: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.abandon_stream();
        self.load_conversation_data(&id, window, cx);
        self.rebuild_model_select(window, cx);
        self.rebuild_reasoning_select(window, cx);
        self.chat.scroll.scroll_to_bottom();
        cx.notify();
    }

    /// Start a blank conversation. Nothing is written until the first prompt.
    pub(crate) fn new_chat(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.abandon_stream();
        self.conversation_id = None;
        self.conversation_title = "New chat".to_owned();
        self.messages.clear();
        self.chat.clear_caches();
        self.draft_model = self.settings.default_model.clone();
        self.notice = None;
        self.rebuild_model_select(window, cx);
        self.rebuild_reasoning_select(window, cx);
        self.focus_composer(window, cx);
        cx.notify();
    }

    pub(crate) fn focus_composer(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.chat
            .composer
            .update(cx, |composer, cx| composer.focus(window, cx));
    }

    /// Fill the composer with a suggested prompt and focus it.
    pub(crate) fn set_prompt(
        &mut self,
        prompt: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.chat
            .composer
            .update(cx, |composer, cx| composer.set_value(prompt, window, cx));
        self.focus_composer(window, cx);
        cx.notify();
    }

    fn abandon_stream(&mut self) {
        if let Some(req) = self.pending_approval.take() {
            let _ = req.respond.send(false);
        }
        if let Some(stream) = self.stream.take() {
            stream.cancel.cancel();
        }
    }

    pub(crate) fn respond_tool_approval(&mut self, allow: bool, cx: &mut Context<Self>) {
        if let Some(req) = self.pending_approval.take() {
            let _ = req.respond.send(allow);
            cx.notify();
        }
    }

    pub(crate) fn toggle_terminal(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.chat.terminal_open = !self.chat.terminal_open;
        if self.chat.terminal_open {
            if self.chat.terminal.is_none() {
                match crate::terminal::ShellSession::spawn(24, 100) {
                    Ok(session) => {
                        self.chat.terminal = Some(session);
                        self.start_terminal_pump(cx);
                    }
                    Err(err) => {
                        self.notice = Some(format!("Could not start terminal: {err}"));
                        self.chat.terminal_open = false;
                    }
                }
            }
            self.chat
                .terminal_input
                .update(cx, |s, cx| s.focus(window, cx));
        } else {
            self.chat
                .composer
                .update(cx, |composer, cx| composer.focus(window, cx));
        }
        cx.notify();
    }

    fn start_terminal_pump(&mut self, cx: &mut Context<Self>) {
        let Some(term) = self.chat.terminal.as_ref() else {
            return;
        };
        let rx = term.events.clone();
        let task = cx.spawn(async move |weak, cx| {
            while let Ok(event) = rx.recv().await {
                let keep = weak
                    .update(cx, |this, cx| {
                        match event {
                            crate::terminal::ShellEvent::Output(chunk) => {
                                if let Some(t) = this.chat.terminal.as_mut() {
                                    t.push_output(&chunk);
                                }
                                this.chat.terminal_scroll.scroll_to_bottom();
                            }
                            crate::terminal::ShellEvent::Exited => {
                                if let Some(t) = this.chat.terminal.as_mut() {
                                    t.exited = true;
                                    t.push_output("\r\n[process exited]\r\n");
                                }
                                this.chat.terminal_task = None;
                            }
                        }
                        cx.notify();
                    })
                    .is_ok();
                if !keep {
                    break;
                }
            }
        });
        self.chat.terminal_task = Some(task);
    }

    pub(crate) fn send_terminal_command(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let cmd = self.chat.terminal_input.read(cx).value().to_string();
        if let Some(t) = self.chat.terminal.as_mut() {
            let mut data = cmd;
            if !data.ends_with('\n') {
                data.push('\n');
            }
            if let Err(err) = t.write(&data) {
                t.push_output(&format!("\r\n[write error: {err}]\r\n"));
            }
        }
        self.chat
            .terminal_input
            .update(cx, |s, cx| s.set_value("", window, cx));
        cx.notify();
    }

    pub(crate) fn close_terminal(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.chat.terminal_task = None;
        self.chat.terminal = None;
        self.chat.terminal_open = false;
        self.chat
            .composer
            .update(cx, |composer, cx| composer.focus(window, cx));
        cx.notify();
    }

    pub(crate) fn clear_terminal(&mut self, cx: &mut Context<Self>) {
        if let Some(t) = self.chat.terminal.as_mut() {
            t.clear();
        }
        cx.notify();
    }

    pub(crate) fn toggle_agent_mode(&mut self, _window: &mut Window, cx: &mut Context<Self>) {
        self.settings.agent_mode = match self.settings.agent_mode {
            relay_core::AgentMode::Plan => relay_core::AgentMode::Build,
            relay_core::AgentMode::Build => relay_core::AgentMode::Plan,
        };
        self.persist_settings();
        cx.notify();
    }

    pub(crate) fn set_tool_approval(
        &mut self,
        approval: relay_core::ToolApproval,
        cx: &mut Context<Self>,
    ) {
        self.settings.tool_approval = approval;
        self.persist_settings();
        cx.notify();
    }

    // MARK: Pickers

    /// The selected model slug, or the draft model before the first save.
    pub(crate) fn current_model_slug(&self) -> String {
        if let Some(id) = &self.conversation_id
            && let Some(summary) = self.conversations.iter().find(|item| &item.id == id)
        {
            return summary.model_slug.clone();
        }
        self.draft_model.clone()
    }

    /// Resolve the selected slug to full model metadata.
    pub(crate) fn current_model(&self) -> ModelSelection {
        models::resolve(&self.current_model_slug())
    }

    fn choose_model(&mut self, slug: String, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(id) = self.conversation_id.clone() {
            match self.store.set_conversation_model(&id, &slug) {
                Ok(()) => {
                    if let Some(summary) = self.conversations.iter_mut().find(|item| item.id == id)
                    {
                        summary.model_slug = slug.clone();
                    }
                }
                Err(error) => {
                    window.push_notification(
                        Notification::error(format!("Could not switch model: {error}")),
                        cx,
                    );
                    return;
                }
            }
        }
        self.draft_model = slug.clone();
        if let Some(variant) = Self::bundled_variant_for_slug(&slug) {
            self.settings.bundled_model_variant = variant.id().to_owned();
            self.persist_settings();
            self.ensure_bundled_variant(variant, cx);
        }
        self.rebuild_reasoning_select(window, cx);
        cx.notify();
    }

    pub(crate) fn rebuild_model_select(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let slug = self.current_model_slug();
        let select = models::model_select(&self.settings, &slug, window, cx);
        self.chat.model_select = select;
        self.attach_model_subscription(window, cx);
    }

    fn rebuild_reasoning_select(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let model = self.current_model();
        if model.reasoning == ReasoningKind::None {
            self.chat.reasoning_select = None;
            self.reasoning_subscription = None;
            return;
        }
        let select = models::string_select(
            &reasoning_labels(),
            self.settings.reasoning.label(),
            window,
            cx,
        );
        self.chat.reasoning_select = Some(select);
        self.attach_reasoning_subscription(window, cx);
    }

    fn rebuild_approval_select(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let select = models::string_select(
            &approval_labels(),
            self.settings.tool_approval.label(),
            window,
            cx,
        );
        self.chat.approval_select = select;
        self.attach_approval_subscription(window, cx);
    }

    // MARK: Settings

    pub(crate) fn open_settings(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let store = self.store.clone();
        let settings = self.settings.clone();
        let view = cx.new(|cx| SettingsView::new(store, settings, window, cx));
        self.settings_subscription =
            Some(
                cx.subscribe_in(&view, window, |this, _, event, window, cx| match event {
                    SettingsEvent::Close => {
                        this.settings_view = None;
                        this.view = View::Chat;
                        cx.notify();
                    }
                    SettingsEvent::Saved(settings) => {
                        this.apply_settings(settings.as_ref().clone(), window, cx);
                    }
                    SettingsEvent::KeysChanged => {
                        this.cloud_keys_ready = Provider::CLOUD
                            .iter()
                            .any(|provider| keys::source(*provider).is_set());
                        cx.notify();
                    }
                    SettingsEvent::Cleared => {
                        this.abandon_stream();
                        this.conversation_id = None;
                        this.conversation_title = "New chat".to_owned();
                        this.messages.clear();
                        this.chat.clear_caches();
                        this.draft_model = this.settings.default_model.clone();
                        this.refresh_conversations(cx);
                        this.rebuild_model_select(window, cx);
                        this.rebuild_reasoning_select(window, cx);
                        cx.notify();
                    }
                }),
            );
        self.settings_view = Some(view);
        self.view = View::Settings;
        cx.notify();
    }

    fn apply_settings(
        &mut self,
        settings: AppSettings,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.settings = settings;
        self.persist_settings();
        self.cloud_keys_ready = Provider::CLOUD
            .iter()
            .any(|provider| keys::source(*provider).is_set());
        if self.conversation_id.is_none() {
            self.draft_model = self.settings.default_model.clone();
        }
        self.chat.presets = self
            .store
            .presets()
            .unwrap_or_else(|_| relay_core::builtin_presets());
        self.chat.composer.update(cx, |composer, cx| {
            composer.set_submit_on_enter(self.settings.send_on_enter, cx);
        });
        theme::apply_theme(&self.settings, window, cx);
        self.rebuild_model_select(window, cx);
        self.rebuild_reasoning_select(window, cx);
        self.rebuild_approval_select(window, cx);
        self.start_bundled_for_settings();
        window.push_notification("Settings saved", cx);
        cx.notify();
    }

    fn persist_settings(&self) {
        if let Err(error) = self.store.save_settings(&self.settings) {
            tracing::warn!(%error, "could not save settings");
        }
    }

    // MARK: Sending

    /// Send the composer contents to the selected model.
    pub(crate) fn send(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.stream.is_some() {
            window.push_notification(
                Notification::info("Relay is still answering. Stop the response first."),
                cx,
            );
            return;
        }
        let prompt = self.chat.composer.read(cx).value().trim().to_owned();
        if prompt.is_empty() {
            return;
        }

        let model = self.current_model();
        if let Some(variant) = Self::bundled_variant_for_slug(&model.slug) {
            let status = self.bundled.lock().map(|s| s.status()).unwrap_or_default();
            let ready_for_variant =
                status.ready && status.active_variant.as_deref() == Some(variant.id());
            if !ready_for_variant {
                self.ensure_bundled_variant(variant, cx);
                let msg = if let Some(err) = status.error {
                    format!("Bundled model unavailable: {err}")
                } else {
                    let st = self.bundled.lock().map(|s| s.status()).unwrap_or_default();
                    format!(
                        "Bundled model is preparing: {}",
                        if st.message.is_empty() {
                            "downloading/starting…".into()
                        } else {
                            st.message
                        }
                    )
                };
                self.notice = Some(msg);
                cx.notify();
                return;
            }
            // ensure endpoint/model point to active bundled server
            if let Some(endpoint) = status.endpoint.clone() {
                self.settings.local_base_url = endpoint;
                self.settings.local_model = variant.id().to_owned();
                self.settings.bundled_model_variant = variant.id().to_owned();
                self.persist_settings();
            }
        }
        if model.provider.requires_api_key() && keys::resolve(model.provider).is_none() {
            self.notice = Some(format!(
                "Add a {} API key in Settings to talk to {}.",
                model.provider.label(),
                model.name
            ));
            cx.notify();
            return;
        }

        let conversation_id = match self.conversation_id.clone() {
            Some(id) => id,
            None => match self.store.create_conversation(&model.slug) {
                Ok(id) => {
                    self.conversation_id = Some(id.clone());
                    id
                }
                Err(error) => {
                    window.push_notification(
                        Notification::error(format!("Could not start a conversation: {error}")),
                        cx,
                    );
                    return;
                }
            },
        };

        let user = ChatMessage::user(prompt);
        let outcome = match self.store.append_message(&conversation_id, &user) {
            Ok(outcome) => outcome,
            Err(error) => {
                window.push_notification(
                    Notification::error(format!("Could not save your message: {error}")),
                    cx,
                );
                return;
            }
        };
        if let Some(title) = outcome.title {
            self.conversation_title = title;
        }
        self.messages.push(user);
        self.refresh_conversations(cx);

        self.generate(conversation_id, window, cx);
    }

    pub(crate) fn edit_user_message(
        &mut self,
        message_id: String,
        new_text: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.stream.is_some() {
            window.push_notification(Notification::info("Stop the response first."), cx);
            return;
        }
        let Some(conversation_id) = self.conversation_id.clone() else {
            return;
        };
        if let Err(error) = self
            .store
            .update_message_text(&conversation_id, &message_id, &new_text)
        {
            window.push_notification(Notification::error(format!("{error}")), cx);
            return;
        }
        if let Err(error) = self
            .store
            .delete_messages_after(&conversation_id, &message_id)
        {
            window.push_notification(Notification::error(format!("{error}")), cx);
            return;
        }
        // update in-memory
        if let Some(idx) = self.messages.iter().position(|m| m.id == message_id) {
            if let Some(block) = self.messages[idx].blocks.first_mut() {
                if let ContentBlock::Text { text } = block {
                    *text = new_text.clone();
                }
            } else {
                self.messages[idx].blocks = vec![ContentBlock::text(new_text.clone())];
            }
            self.messages.truncate(idx + 1);
        }
        self.refresh_conversations(cx);
        self.generate(conversation_id, window, cx);
    }

    pub(crate) fn generate(
        &mut self,
        conversation_id: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let model = self.current_model();
        let reasoning = if model.reasoning == ReasoningKind::None {
            ReasoningEffort::Off
        } else {
            self.settings.reasoning
        };
        let mut request = ChatRequest::new(model.clone(), Vec::new())
            .with_reasoning(reasoning)
            .with_temperature(self.settings.temperature)
            .with_max_output_tokens(self.settings.max_output_tokens)
            .with_tools({
                let mut specs = relay_core::tool_specs(self.settings.agent_mode);
                specs.extend(crate::mcp::global_tools());
                specs
            });
        let preset = self
            .chat
            .presets
            .iter()
            .find(|preset| preset.id == self.settings.active_preset_id)
            .cloned()
            .or_else(|| find_builtin_preset(&self.settings.active_preset_id));
        let mut custom = self.settings.custom_instructions.clone();
        if let Ok(memories) = self.store.list_memories() {
            if !memories.is_empty() {
                custom.push_str("\n\nCross-session memory:\n");
                for (_, content) in &memories {
                    custom.push_str("- ");
                    custom.push_str(content);
                    custom.push('\n');
                }
            }
        }
        let mut system = compose_system_prompt(preset.as_ref(), &custom)
            .unwrap_or_else(|| relay_core::BASE_SYSTEM_PROMPT.to_owned());
        if model.is_local() {
            system.push_str("\n\nLocal model tool-use rules: Use the provided function/tool calls exactly as specified—do not invent tool call syntax in plain text. Before claiming a file was created/changed, ensure a write tool call succeeded; before claiming contents, ensure read/list tool results show them. For Desktop on localized systems (e.g. Hungarian 'Asztal'), locate it via $HOME (e.g. run_command: echo $HOME, then list_dir) rather than guessing paths. If unsure, inspect first, then act, then verify with a tool.");
        }
        let system_opt = if system.trim().is_empty() {
            None
        } else {
            Some(system.clone())
        };
        request = request.with_system_prompt(system_opt.clone());
        let system_ref = system_opt.as_deref();
        let history = fit_history(
            request_history(&self.messages),
            model.context_window,
            request.output_limit(),
            system_ref,
        );
        request.messages = history;
        if model.provider.is_local() {
            let endpoint = match model.provider {
                Provider::Ollama => self.settings.ollama_base_url.clone(),
                _ => self.settings.local_base_url.clone(),
            };
            request = request.with_endpoint(endpoint);
        }
        if let Some(key) = keys::resolve(model.provider) {
            request = request.with_api_key(key);
        }

        let assistant = ChatMessage::assistant(model.slug.clone(), Vec::new(), None);
        let message_id = assistant.id.clone();
        let text_state = cx.new(|cx| TextViewState::markdown("", cx));
        let thinking_state = cx.new(|cx| TextViewState::markdown("", cx));
        self.chat.text_states.insert(message_id.clone(), text_state);
        self.chat
            .thinking_states
            .insert(message_id.clone(), thinking_state);
        self.messages.push(assistant);
        self.chat
            .composer
            .update(cx, |composer, cx| composer.set_value("", window, cx));
        self.focus_composer(window, cx);
        self.notice = None;

        let started = match streaming::start(
            &self.runtime,
            request,
            self.settings.agent_mode,
            self.settings.tool_approval,
            streaming::DelegateContext::from_settings(&self.settings),
        ) {
            Ok(started) => started,
            Err(error) => {
                self.messages.retain(|message| message.id != message_id);
                self.chat.text_states.remove(&message_id);
                self.chat.thinking_states.remove(&message_id);
                window.push_notification(
                    Notification::error(format!("Could not start the request: {error}")),
                    cx,
                );
                cx.notify();
                return;
            }
        };
        let StartedStream {
            events,
            error: error_rx,
            approvals,
            cancel,
        } = started;

        let consumer_id = message_id.clone();
        let task = cx.spawn(async move |weak_self, cx| {
            while let Ok(event) = events.recv().await {
                let id = consumer_id.clone();
                if weak_self
                    .update(cx, move |this, cx| {
                        this.handle_stream_event(&id, event, cx);
                    })
                    .is_err()
                {
                    return;
                }
            }
            let error = error_rx.recv().await.ok();
            weak_self
                .update(cx, |this, cx| {
                    this.finish_stream(&consumer_id, error, cx);
                })
                .ok();
        });

        cx.spawn(async move |weak_self, cx| {
            while let Ok(req) = approvals.recv().await {
                let _ = weak_self.update(cx, |this, cx| {
                    this.pending_approval = Some(req);
                    cx.notify();
                });
            }
        })
        .detach();

        self.stream = Some(ActiveStream {
            conversation_id,
            message_id,
            cancel,
            _task: task,
        });
        self.chat.scroll.scroll_to_bottom();
        cx.notify();
    }

    fn handle_stream_event(
        &mut self,
        message_id: &str,
        event: StreamEvent,
        cx: &mut Context<Self>,
    ) {
        let Some(index) = self
            .messages
            .iter()
            .position(|message| message.id == message_id)
        else {
            return;
        };
        match event {
            StreamEvent::Text(chunk) => {
                self.messages[index].append_text(&chunk);
                if let Some(state) = self.chat.text_states.get(message_id) {
                    state.update(cx, |state, cx| state.push_str(&chunk, cx));
                }
                self.chat.scroll.scroll_to_bottom();
                cx.notify();
            }
            StreamEvent::Thinking(chunk) => {
                let provider = self.messages[index]
                    .model
                    .as_deref()
                    .map(models::resolve)
                    .map(|model| model.provider.id())
                    .unwrap_or("model");
                self.messages[index].append_thinking(&chunk, provider);
                if let Some(state) = self.chat.thinking_states.get(message_id) {
                    state.update(cx, |state, cx| state.push_str(&chunk, cx));
                }
                self.chat.scroll.scroll_to_bottom();
                cx.notify();
            }
            StreamEvent::Usage(usage) => {
                self.messages[index].usage = Some(usage);
                cx.notify();
            }
            StreamEvent::Done { .. } => {}
            StreamEvent::ToolCall(call) => {
                self.messages[index].blocks.push(ContentBlock::ToolUse {
                    id: call.id,
                    name: call.name,
                    arguments: call.arguments,
                });
                cx.notify();
            }
            StreamEvent::ToolResult { id, output, .. } => {
                // replace matching ToolUse "running" concept by adding a ToolResult block
                self.messages[index].blocks.push(ContentBlock::ToolResult {
                    id,
                    output,
                    is_error: false,
                });
                self.chat.scroll.scroll_to_bottom();
                cx.notify();
            }
        }
    }

    fn finish_stream(&mut self, message_id: &str, error: Option<String>, cx: &mut Context<Self>) {
        let matches_current = self
            .stream
            .as_ref()
            .is_some_and(|stream| stream.message_id == message_id);
        if !matches_current {
            return;
        }
        let Some(stream) = self.stream.take() else {
            return;
        };
        if let Some(req) = self.pending_approval.take() {
            let _ = req.respond.send(false);
        }
        let cancelled = stream.cancel.is_cancelled();

        let Some(index) = self
            .messages
            .iter()
            .position(|message| message.id == message_id)
        else {
            cx.notify();
            return;
        };

        if let Some(error) = error
            && !cancelled
        {
            self.messages[index].error = Some(error);
        }

        let has_content = !self.messages[index].text().trim().is_empty()
            || !self.messages[index].thinking().trim().is_empty()
            || self.messages[index].error.is_some();

        if has_content {
            let message = self.messages[index].clone();
            if let Err(error) = self.store.append_message(&stream.conversation_id, &message) {
                tracing::warn!(%error, "could not save assistant message");
            }
        } else {
            self.messages.remove(index);
            self.chat.text_states.remove(message_id);
            self.chat.thinking_states.remove(message_id);
        }

        if let Ok(summary) = self.store.conversation(&stream.conversation_id) {
            self.conversation_title = summary.title;
        }
        self.refresh_conversations(cx);
        self.chat.scroll.scroll_to_bottom();
        cx.notify();
    }

    /// Ask the provider to stop. Partial output is kept.
    pub(crate) fn stop_stream(&mut self, cx: &mut Context<Self>) {
        if let Some(stream) = &self.stream {
            stream.cancel.cancel();
        }
        cx.notify();
    }

    pub(crate) fn is_streaming(&self) -> bool {
        self.stream.is_some()
    }

    pub(crate) fn streaming_message_id(&self) -> Option<&str> {
        self.stream
            .as_ref()
            .map(|stream| stream.message_id.as_str())
    }

    // MARK: Deletion

    pub(crate) fn confirm_delete_conversation(
        &mut self,
        id: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let title = self
            .conversations
            .iter()
            .find(|item| item.id == id)
            .map(|item| item.title.clone())
            .unwrap_or_else(|| "this conversation".to_owned());
        let store = self.store.clone();
        let this = cx.entity();
        let delete_id = id.clone();
        window.open_alert_dialog(cx, move |alert, _, _| {
            let store = store.clone();
            let this = this.clone();
            let delete_id = delete_id.clone();
            alert
                .title("Delete conversation")
                .description(format!(
                    "“{title}” and all of its messages will be removed. This cannot be undone."
                ))
                .confirm()
                .ok_text("Delete")
                .ok_variant(gpui_kit::component::button::ButtonVariant::Danger)
                .on_ok(move |_, window, cx| {
                    match store.delete_conversation(&delete_id) {
                        Ok(()) => {
                            let this = this.clone();
                            let delete_id = delete_id.clone();
                            this.update(cx, |app, cx| {
                                if app.conversation_id.as_deref() == Some(delete_id.as_str()) {
                                    app.view = View::Chat;
                                    app.settings_view = None;
                                    app.abandon_stream();
                                    app.conversation_id = None;
                                    app.conversation_title = "New chat".to_owned();
                                    app.messages.clear();
                                    app.chat.clear_caches();
                                    app.draft_model = app.settings.default_model.clone();
                                }
                                app.refresh_conversations(cx);
                            });
                            window.push_notification("Conversation deleted", cx);
                        }
                        Err(error) => {
                            window.push_notification(
                                Notification::error(format!("Could not delete: {error}")),
                                cx,
                            );
                        }
                    }
                    true
                })
        });
    }

    // MARK: Rendering

    fn render_main(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if self.view == View::Settings
            && let Some(view) = self.settings_view.clone()
        {
            return view.into_any_element();
        }
        self.render_chat(window, cx).into_any_element()
    }
}

impl Render for RelayApp {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if self.bundled_needs_refresh {
            self.apply_bundled_ready(window, cx);
        }
        let theme = cx.theme().clone();
        let sidebar = self.render_sidebar(window, cx).into_any_element();
        let main = self.render_main(window, cx).into_any_element();
        v_flex()
            .size_full()
            .bg(theme.background)
            .text_color(theme.foreground)
            .on_key_down(cx.listener(|this, event: &KeyDownEvent, window, cx| {
                let ks = &event.keystroke;
                if this.view == View::Chat
                    && ks.modifiers.control
                    && !ks.modifiers.shift
                    && ks.key == "`"
                {
                    window.prevent_default();
                    cx.stop_propagation();
                    this.toggle_terminal(window, cx);
                }
            }))
            .child(
                TitleBar::new().child(
                    h_flex()
                        .gap_2()
                        .items_center()
                        .child(
                            div()
                                .size_5()
                                .rounded(theme.radius)
                                .bg(theme.primary)
                                .flex()
                                .items_center()
                                .justify_center()
                                .text_xs()
                                .font_bold()
                                .text_color(theme.primary_foreground)
                                .child("R"),
                        )
                        .child(div().text_sm().font_semibold().child("Relay")),
                ),
            )
            .child(
                h_flex()
                    .flex_1()
                    .min_h_0()
                    .child(sidebar)
                    .child(div().flex_1().min_w_0().h_full().child(main)),
            )
    }
}

pub(crate) fn reasoning_labels() -> Vec<SharedString> {
    ReasoningEffort::ALL
        .iter()
        .map(|effort| SharedString::from(effort.label()))
        .collect()
}

pub(crate) fn approval_labels() -> Vec<SharedString> {
    vec![
        SharedString::from(relay_core::ToolApproval::None.label()),
        SharedString::from(relay_core::ToolApproval::NonDestructive.label()),
        SharedString::from(relay_core::ToolApproval::All.label()),
    ]
}
