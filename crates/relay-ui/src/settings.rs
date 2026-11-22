//! Full-page settings: general preferences, credentials, presets, data, about.

use gpui_kit::assets::IconName;
use gpui_kit::component::{
    ActiveTheme as _, Disableable as _, Icon, Selectable as _, Sizable as _, StyledExt as _, Theme,
    WindowExt as _,
    button::{Button, ButtonVariant, ButtonVariants as _},
    h_flex,
    input::{Input, InputState, Textarea, TextareaState},
    notification::Notification,
    select::{SearchableVec, Select, SelectState},
    switch::Switch,
    v_flex,
};
use gpui_kit::prelude::{FluentBuilder as _, StatefulInteractiveElement as _};
use gpui_kit::{
    AnyElement, AppContext as _, Context, Entity, EventEmitter, InteractiveElement as _,
    IntoElement, ParentElement as _, SharedString, Styled as _, Window, div, px,
};
use relay_core::{
    AppSettings, Appearance, ColorPreset, PersonalityPreset, Provider, ReasoningEffort,
    format_tokens,
};
use relay_store::{Store, StoreStats};

use crate::app::reasoning_labels;
use crate::keys::{self, KeySource};
use crate::models::{self, ModelSelect};

type StringSelect = Entity<SelectState<SearchableVec<SharedString>>>;

/// Events sent to the main view.
#[derive(Debug, Clone)]
pub enum SettingsEvent {
    /// Return to the chat without saving.
    Close,
    /// Preferences were saved.
    Saved(Box<AppSettings>),
    /// A credential was added or removed.
    KeysChanged,
    /// All conversations were deleted.
    Cleared,
}

/// Settings tabs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SettingsTab {
    General,
    Models,
    Presets,
    Data,
    About,
}

impl SettingsTab {
    const ALL: [Self; 5] = [
        Self::General,
        Self::Models,
        Self::Presets,
        Self::Data,
        Self::About,
    ];

    fn label(self) -> &'static str {
        match self {
            Self::General => "General",
            Self::Models => "Models & Keys",
            Self::Presets => "Presets",
            Self::Data => "Data",
            Self::About => "About",
        }
    }

    fn icon(self) -> IconName {
        match self {
            Self::General => IconName::Settings,
            Self::Models => IconName::KeyRound,
            Self::Presets => IconName::Sparkles,
            Self::Data => IconName::Database,
            Self::About => IconName::Info,
        }
    }
}

/// The settings page.
pub struct SettingsView {
    store: Store,
    settings: AppSettings,
    tab: SettingsTab,
    default_model: ModelSelect,
    appearance: StringSelect,
    reasoning: StringSelect,
    temperature: Entity<InputState>,
    max_output: Entity<InputState>,
    instructions: Entity<TextareaState>,
    font_family: StringSelect,
    mono_font_family: StringSelect,
    font_size: Entity<InputState>,
    color_preset: StringSelect,
    ollama_url: Entity<InputState>,
    ollama_model: Entity<InputState>,
    local_url: Entity<InputState>,
    local_model: Entity<InputState>,
    bundled_context: Entity<InputState>,
    mcp_config: Entity<TextareaState>,
    key_inputs: Vec<(Provider, Entity<InputState>)>,
    key_sources: Vec<(Provider, KeySource)>,
    presets: Vec<PersonalityPreset>,
    preset_selected: Option<String>,
    preset_name: Entity<InputState>,
    preset_description: Entity<InputState>,
    preset_instructions: Entity<TextareaState>,
    stats: StoreStats,
    memories: Vec<(String, String)>,
    memory_input: Entity<InputState>,
    scroll: gpui_kit::ScrollHandle,
}

impl EventEmitter<SettingsEvent> for SettingsView {}

impl SettingsView {
    pub fn new(
        store: Store,
        settings: AppSettings,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let _presets = store
            .presets()
            .unwrap_or_else(|_| relay_core::builtin_presets());
        let default_model = models::model_select(&settings, &settings.default_model, window, cx);
        let appearance = models::string_select(
            &["System", "Light", "Dark"]
                .iter()
                .map(|value| SharedString::from(*value))
                .collect::<Vec<_>>(),
            settings.appearance.label(),
            window,
            cx,
        );
        let reasoning =
            models::string_select(&reasoning_labels(), settings.reasoning.label(), window, cx);

        let temperature = text_input(window, cx, "0.7", &format!("{:.2}", settings.temperature));
        let max_output = text_input(window, cx, "Model default", &{
            if settings.max_output_tokens == 0 {
                String::new()
            } else {
                settings.max_output_tokens.to_string()
            }
        });
        let instructions = textarea(
            window,
            cx,
            "Standing instructions for every conversation…",
            &settings.custom_instructions,
        );
        let font_family = models::string_select(
            &crate::fonts::ui_font_labels(),
            &settings.font_family,
            window,
            cx,
        );
        let mono_font_family = models::string_select(
            &crate::fonts::mono_font_labels(),
            &settings.mono_font_family,
            window,
            cx,
        );
        let font_size = text_input(window, cx, "15", &format!("{:.1}", settings.font_size));
        let color_preset = models::string_select(
            &ColorPreset::ALL
                .iter()
                .map(|p| SharedString::from(p.label()))
                .collect::<Vec<_>>(),
            settings.color_preset.label(),
            window,
            cx,
        );
        let ollama_url = text_input(
            window,
            cx,
            "http://localhost:11434",
            &settings.ollama_base_url,
        );
        let ollama_model = text_input(window, cx, "llama3.2", &settings.ollama_model);
        let local_url = text_input(
            window,
            cx,
            "http://localhost:8000/v1",
            &settings.local_base_url,
        );
        let local_model = text_input(window, cx, "Model name", &settings.local_model);
        let bundled_context = text_input(
            window,
            cx,
            "4096",
            &settings.bundled_context_size.to_string(),
        );
        let mcp_json =
            serde_json::to_string_pretty(&settings.mcp_servers).unwrap_or_else(|_| "[]".into());
        let mcp_config = textarea(window, cx, "JSON array of MCP servers", &mcp_json);

        let mut key_inputs = Vec::new();
        for provider in key_providers() {
            let input = cx.new(|cx| {
                InputState::new(window, cx)
                    .placeholder("Paste an API key…")
                    .masked(true)
            });
            key_inputs.push((provider, input));
        }
        let key_sources = key_providers()
            .into_iter()
            .map(|provider| (provider, keys::source(provider)))
            .collect();

        let preset_name = text_input(window, cx, "Preset name", "");
        let preset_description = text_input(window, cx, "One-line description", "");
        let preset_instructions = textarea(window, cx, "Instructions for this personality…", "");
        let memory_input = text_input(window, cx, "Remember something across sessions…", "");
        let memories = store.list_memories().unwrap_or_default();
        let presets = store
            .presets()
            .unwrap_or_else(|_| relay_core::builtin_presets());

        Self {
            stats: store.stats().unwrap_or_default(),
            memories,
            store,
            settings,
            tab: SettingsTab::General,
            default_model,
            appearance,
            reasoning,
            temperature,
            max_output,
            instructions,
            font_family,
            mono_font_family,
            font_size,
            color_preset,
            ollama_url,
            ollama_model,
            local_url,
            local_model,
            bundled_context,
            mcp_config,
            key_inputs,
            key_sources,
            presets,
            preset_selected: None,
            preset_name,
            preset_description,
            preset_instructions,
            memory_input,
            scroll: gpui_kit::ScrollHandle::new(),
        }
    }

    // MARK: Saving

    fn save(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let default_model = self.default_model.read(cx).selected_value().cloned();
        if let Some(slug) = default_model {
            self.settings.default_model = slug;
        }
        let appearance = self.appearance.read(cx).selected_value().cloned();
        if let Some(appearance) = appearance {
            self.settings.appearance = match appearance.as_ref() {
                "Light" => Appearance::Light,
                "Dark" => Appearance::Dark,
                _ => Appearance::System,
            };
        }
        let reasoning = self.reasoning.read(cx).selected_value().cloned();
        if let Some(reasoning) = reasoning
            && let Some(effort) = ReasoningEffort::ALL
                .iter()
                .find(|effort| effort.label() == reasoning.as_ref())
        {
            self.settings.reasoning = *effort;
        }
        let temperature = self
            .temperature
            .read(cx)
            .value()
            .trim()
            .parse::<f64>()
            .unwrap_or(0.7);
        self.settings.temperature = temperature;
        self.settings.max_output_tokens = self
            .max_output
            .read(cx)
            .value()
            .trim()
            .parse::<u32>()
            .unwrap_or(0);
        self.settings.custom_instructions = self.instructions.read(cx).value().to_string();
        if let Some(v) = self.font_family.read(cx).selected_value().cloned() {
            self.settings.font_family = v.to_string();
        }
        if let Some(v) = self.mono_font_family.read(cx).selected_value().cloned() {
            self.settings.mono_font_family = v.to_string();
        }
        self.settings.font_size = self
            .font_size
            .read(cx)
            .value()
            .trim()
            .parse::<f32>()
            .unwrap_or(self.settings.font_size);
        if let Some(v) = self.color_preset.read(cx).selected_value().cloned() {
            self.settings.color_preset = ColorPreset::from_str(v.as_ref());
        }
        self.settings.ollama_base_url = self.ollama_url.read(cx).value().to_string();
        self.settings.ollama_model = self.ollama_model.read(cx).value().to_string();
        self.settings.local_base_url = self.local_url.read(cx).value().to_string();
        self.settings.local_model = self.local_model.read(cx).value().to_string();
        if let Ok(n) = self.bundled_context.read(cx).value().parse::<u32>() {
            self.settings.bundled_context_size = n;
        }
        if let Ok(servers) = serde_json::from_str::<Vec<relay_core::McpServerConfig>>(
            self.mcp_config.read(cx).value().as_ref(),
        ) {
            self.settings.mcp_servers = servers;
        }
        self.settings.normalize();

        if let Err(error) = self.store.save_settings(&self.settings) {
            window.push_notification(
                Notification::error(format!("Could not save settings: {error}")),
                cx,
            );
            return;
        }
        cx.emit(SettingsEvent::Saved(Box::new(self.settings.clone())));
        crate::mcp::init_global(&self.settings.mcp_servers);
        self.stats = self.store.stats().unwrap_or_default();
        cx.notify();
    }

    // MARK: Keys

    fn refresh_key_sources(&mut self) {
        self.key_sources = key_providers()
            .into_iter()
            .map(|provider| (provider, keys::source(provider)))
            .collect();
    }

    fn key_source(&self, provider: Provider) -> KeySource {
        self.key_sources
            .iter()
            .find(|(candidate, _)| *candidate == provider)
            .map(|(_, source)| *source)
            .unwrap_or(KeySource::Missing)
    }

    fn key_input(&self, provider: Provider) -> Option<Entity<InputState>> {
        self.key_inputs
            .iter()
            .find(|(candidate, _)| *candidate == provider)
            .map(|(_, input)| input.clone())
    }

    fn save_key(&mut self, provider: Provider, window: &mut Window, cx: &mut Context<Self>) {
        let Some(input) = self.key_input(provider) else {
            return;
        };
        let value = input.read(cx).value().to_string();
        match keys::save(provider, &value) {
            Ok(()) => {
                input.update(cx, |state, cx| state.set_value("", window, cx));
                self.refresh_key_sources();
                cx.emit(SettingsEvent::KeysChanged);
                window.push_notification(
                    format!("{} key saved to the keyring", provider.label()),
                    cx,
                );
                cx.notify();
            }
            Err(error) => {
                window.push_notification(
                    Notification::error(format!("Could not save the key: {error}")),
                    cx,
                );
            }
        }
    }

    fn clear_key(&mut self, provider: Provider, window: &mut Window, cx: &mut Context<Self>) {
        match keys::clear(provider) {
            Ok(()) => {
                self.refresh_key_sources();
                cx.emit(SettingsEvent::KeysChanged);
                window.push_notification(
                    format!("{} key removed from the keyring", provider.label()),
                    cx,
                );
                cx.notify();
            }
            Err(error) => {
                window.push_notification(
                    Notification::error(format!("Could not remove the key: {error}")),
                    cx,
                );
            }
        }
    }

    // MARK: Presets

    fn selected_preset(&self) -> Option<&PersonalityPreset> {
        let id = self.preset_selected.as_deref()?;
        self.presets.iter().find(|preset| preset.id == id)
    }

    fn select_preset(&mut self, id: String, window: &mut Window, cx: &mut Context<Self>) {
        let Some(preset) = self.presets.iter().find(|preset| preset.id == id).cloned() else {
            return;
        };
        self.preset_selected = Some(id);
        self.preset_name.update(cx, |state, cx| {
            state.set_value(preset.name, window, cx);
        });
        self.preset_description.update(cx, |state, cx| {
            state.set_value(preset.description, window, cx);
        });
        self.preset_instructions.update(cx, |state, cx| {
            state.set_value(preset.instructions, window, cx);
        });
        cx.notify();
    }

    fn new_preset(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.preset_selected = None;
        self.preset_name
            .update(cx, |state, cx| state.set_value("", window, cx));
        self.preset_description
            .update(cx, |state, cx| state.set_value("", window, cx));
        self.preset_instructions
            .update(cx, |state, cx| state.set_value("", window, cx));
        self.preset_name
            .update(cx, |state, cx| state.focus(window, cx));
        cx.notify();
    }

    fn duplicate_preset(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(preset) = self.selected_preset().cloned() else {
            return;
        };
        self.preset_selected = None;
        self.preset_name.update(cx, |state, cx| {
            state.set_value(format!("{} copy", preset.name), window, cx);
        });
        self.preset_description.update(cx, |state, cx| {
            state.set_value(preset.description, window, cx);
        });
        self.preset_instructions.update(cx, |state, cx| {
            state.set_value(preset.instructions, window, cx);
        });
        cx.notify();
    }

    fn save_preset(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let name = self.preset_name.read(cx).value().trim().to_owned();
        if name.is_empty() {
            window.push_notification(Notification::error("A preset needs a name"), cx);
            return;
        }
        if self.selected_preset().is_some_and(|preset| preset.builtin) {
            window.push_notification(
                Notification::error(
                    "Built-in presets are read-only. Duplicate one to make it editable.",
                ),
                cx,
            );
            return;
        }
        let id = self
            .preset_selected
            .clone()
            .unwrap_or_else(|| uuid::Uuid::new_v4().to_string());
        let preset = PersonalityPreset {
            id: id.clone(),
            name,
            description: self.preset_description.read(cx).value().to_string(),
            instructions: self.preset_instructions.read(cx).value().to_string(),
            builtin: false,
        };
        match self.store.save_preset(&preset) {
            Ok(()) => {
                self.reload_presets();
                self.preset_selected = Some(id);
                window.push_notification("Preset saved", cx);
                cx.notify();
            }
            Err(error) => {
                window.push_notification(
                    Notification::error(format!("Could not save the preset: {error}")),
                    cx,
                );
            }
        }
    }

    fn delete_preset(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(preset) = self.selected_preset().cloned() else {
            return;
        };
        if preset.builtin {
            window.push_notification(
                Notification::error("Built-in presets cannot be deleted"),
                cx,
            );
            return;
        }
        match self.store.delete_preset(&preset.id) {
            Ok(()) => {
                if self.settings.active_preset_id == preset.id {
                    self.settings.active_preset_id = "balanced".to_owned();
                    let _ = self.store.save_settings(&self.settings);
                    cx.emit(SettingsEvent::Saved(Box::new(self.settings.clone())));
                }
                self.preset_selected = None;
                self.reload_presets();
                self.new_preset(window, cx);
                window.push_notification("Preset deleted", cx);
                cx.notify();
            }
            Err(error) => {
                window.push_notification(
                    Notification::error(format!("Could not delete the preset: {error}")),
                    cx,
                );
            }
        }
    }

    fn use_preset(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(id) = self.preset_selected.clone() else {
            return;
        };
        self.settings.active_preset_id = id;
        match self.store.save_settings(&self.settings) {
            Ok(()) => {
                cx.emit(SettingsEvent::Saved(Box::new(self.settings.clone())));
                window.push_notification("Active preset updated", cx);
                cx.notify();
            }
            Err(error) => {
                window.push_notification(
                    Notification::error(format!("Could not update the preset: {error}")),
                    cx,
                );
            }
        }
    }

    fn reload_presets(&mut self) {
        self.presets = self
            .store
            .presets()
            .unwrap_or_else(|_| relay_core::builtin_presets());
    }

    fn reset_appearance(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let defaults = AppSettings::default();
        self.settings.font_family = defaults.font_family.clone();
        self.settings.mono_font_family = defaults.mono_font_family.clone();
        self.settings.font_size = defaults.font_size;
        self.settings.color_preset = defaults.color_preset;

        self.font_family.update(cx, |s, cx| {
            s.set_selected_value(
                &SharedString::from(defaults.font_family.clone()),
                window,
                cx,
            );
        });
        self.mono_font_family.update(cx, |s, cx| {
            s.set_selected_value(
                &SharedString::from(defaults.mono_font_family.clone()),
                window,
                cx,
            );
        });
        self.font_size.update(cx, |s, cx| {
            s.set_value(format!("{:.1}", defaults.font_size), window, cx)
        });
        self.color_preset.update(cx, |s, cx| {
            s.set_selected_value(
                &SharedString::from(defaults.color_preset.label()),
                window,
                cx,
            );
        });
        cx.notify();
    }

    // MARK: Data

    fn confirm_delete_all(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let store = self.store.clone();
        let this = cx.entity();
        window.open_alert_dialog(cx, move |alert, _, _| {
            let store = store.clone();
            let this = this.clone();
            alert
                .title("Delete all conversations")
                .description(
                    "Every conversation and message in Relay's local database will be removed. \
                     Settings, presets, and API keys are kept. This cannot be undone.",
                )
                .confirm()
                .ok_text("Delete everything")
                .ok_variant(ButtonVariant::Danger)
                .on_ok(move |_, window, cx| {
                    match store.delete_all_conversations() {
                        Ok(()) => {
                            let stats = store.stats().unwrap_or_default();
                            let this = this.clone();
                            this.update(cx, |view, cx| {
                                view.stats = stats;
                                cx.emit(SettingsEvent::Cleared);
                                cx.notify();
                            });
                            window.push_notification("All conversations deleted", cx);
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

    fn render_tabs(&self, theme: &Theme, cx: &mut Context<Self>) -> impl IntoElement {
        v_flex()
            .gap_1()
            .p_2()
            .children(SettingsTab::ALL.map(|tab| {
                Button::new(SharedString::from(format!("tab-{}", tab.label())))
                    .ghost()
                    .w_full()
                    .justify_start()
                    .icon(Icon::new(tab.icon()))
                    .label(tab.label())
                    .when(self.tab == tab, |this| this.selected(true))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.tab = tab;
                        cx.notify();
                    }))
            }))
            .child(div().flex_1())
            .child(
                v_flex()
                    .gap_1()
                    .px_2()
                    .py_3()
                    .text_xs()
                    .text_color(theme.muted_foreground)
                    .child("API keys never enter the database.")
                    .child("They live in the OS keyring."),
            )
    }

    fn render_general(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme().clone();
        let appearance = self.appearance.clone();
        let default_model = self.default_model.clone();
        let reasoning = self.reasoning.clone();
        let temperature = self.temperature.clone();
        let max_output = self.max_output.clone();
        let instructions = self.instructions.clone();
        let font_family = self.font_family.clone();
        let mono_font_family = self.mono_font_family.clone();
        let font_size = self.font_size.clone();
        let color_preset = self.color_preset.clone();

        v_flex()
            .gap_6()
            .child(section(
                "Defaults",
                "Choose how Relay looks and which model starts a new conversation.",
                &theme,
            ))
            .child(
                card(&theme)
                    .child(field(
                        "Appearance",
                        "Follow the desktop or force a light or dark theme.",
                        div().w(px(200.)).child(Select::new(&appearance).w_full()),
                    ))
                    .child(field(
                        "Default model",
                        "Used for new conversations. You can switch models at any time.",
                        div().w_full().child(Select::new(&default_model).w_full()),
                    ))
                    .child(field(
                        "Reasoning effort",
                        "Requested depth for models that expose a reasoning control.",
                        div().w(px(200.)).child(Select::new(&reasoning).w_full()),
                    ))
                    .child(toggle_row(
                        "Show reasoning summaries",
                        "Display provider-provided reasoning above answers when available.",
                        Switch::new("show-thinking")
                            .checked(self.settings.show_thinking)
                            .on_click(cx.listener(|this, checked, _, cx| {
                                this.settings.show_thinking = *checked;
                                cx.notify();
                            })),
                    ))
                    .child(toggle_row(
                        "Send with Enter",
                        "When off, Enter inserts a new line and only the send button submits.",
                        Switch::new("send-on-enter")
                            .checked(self.settings.send_on_enter)
                            .on_click(cx.listener(|this, checked, _, cx| {
                                this.settings.send_on_enter = *checked;
                                cx.notify();
                            })),
                    )),
            )
            .child(section(
                "Appearance",
                "Customize fonts and choose a color preset. Presets are tuned to stay readable in both light and dark mode.",
                &theme,
            ))
            .child(
                card(&theme)
                    .child(field(
                        "UI font family",
                        "Font used across the app interface (bundled developer fonts included).",
                        div().w(px(320.)).child(Select::new(&font_family).w_full()),
                    ))
                    .child(field(
                        "Monospace font family",
                        "Font used for code and tool output (bundled developer fonts included).",
                        div().w(px(320.)).child(Select::new(&mono_font_family).w_full()),
                    ))
                    .child(field(
                        "Font size (px)",
                        "Base UI font size.",
                        Input::new(&font_size).w(px(120.)),
                    ))
                    .child(field(
                        "Color preset",
                        "Applies accent colors that look right in both Light and Dark mode.",
                        div().w(px(240.)).child(Select::new(&color_preset).w_full()),
                    ))
                    .child(
                        h_flex().gap_2().child(
                            Button::new("reset-appearance")
                                .outline()
                                .small()
                                .label("Reset appearance")
                                .on_click(cx.listener(|this, _, window, cx| {
                                    this.reset_appearance(window, cx);
                                })),
                        ),
                    ),
            )
            .child(section(
                "Sampling",
                "Optional limits applied to every request. Model-specific limits always win.",
                &theme,
            ))
            .child(
                card(&theme)
                    .child(field(
                        "Temperature",
                        "0.0 to 2.0. Some providers ignore temperature while reasoning is active.",
                        h_flex()
                            .gap_2()
                            .items_center()
                            .child(Input::new(&temperature).w(px(140.)))
                            .child(
                                div()
                                    .text_xs()
                                    .text_color(theme.muted_foreground)
                                    .child("Default 0.7"),
                            ),
                    ))
                    .child(field(
                        "Maximum output tokens",
                        "Leave empty or 0 to use the model's default output limit.",
                        Input::new(&max_output).w(px(180.)),
                    )),
            )
            .child(section(
                "Custom instructions",
                "Appended to the active personality preset for every conversation.",
                &theme,
            ))
            .child(card(&theme).child(Textarea::new(&instructions).h(px(120.))))
            .into_any_element()
    }

    fn render_models(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme().clone();
        let ollama_url = self.ollama_url.clone();
        let ollama_model = self.ollama_model.clone();
        let local_url = self.local_url.clone();
        let local_model = self.local_model.clone();
        let bundled_context = self.bundled_context.clone();
        let mcp_config = self.mcp_config.clone();

        let mut key_cards = v_flex().gap_4();
        for provider in key_providers() {
            key_cards = key_cards.child(self.render_key_card(provider, &theme, cx));
        }

        v_flex()
            .gap_6()
            .child(section(
                "Cloud API keys",
                "Keys are sealed with ChaCha20-Poly1305 before they are stored in your desktop \
                 keyring. Environment variables are used as a fallback when no keyring entry \
                 exists.",
                &theme,
            ))
            .child(key_cards)
            .child(section(
                "Local Ollama",
                "Relay talks to the Ollama daemon directly. Install models with `ollama pull`.",
                &theme,
            ))
            .child(
                card(&theme)
                    .child(field(
                        "Endpoint",
                        "Base URL of the Ollama server.",
                        Input::new(&ollama_url).w_full(),
                    ))
                    .child(field(
                        "Model",
                        "The installed Ollama model new chats should use, for example `llama3.2`.",
                        Input::new(&ollama_model).w(px(280.)),
                    )),
            )
            .child(section(
                "OpenAI-compatible endpoint",
                "Any local server that exposes /v1/chat/completions and /v1/models: llama.cpp, \
                 vLLM, LM Studio, or a self-hosted gateway.",
                &theme,
            ))
            .child(
                card(&theme)
                    .child(field(
                        "Base URL",
                        "Include the version prefix, usually /v1.",
                        Input::new(&local_url).w_full(),
                    ))
                    .child(field(
                        "Model",
                        "The model identifier the endpoint expects.",
                        Input::new(&local_model).w(px(280.)),
                    )),
            )
            .child(section(
                "Bundled Nemotron",
                "Context window for the auto-managed local Nemotron model (requires restart of the bundled server).",
                &theme,
            ))
            .child(
                card(&theme).child(field(
                    "Context size (tokens)",
                    "Tokens of context for the bundled llama-server (512-32768).",
                    Input::new(&bundled_context).w(px(120.)),
                )),
            )
            .child(section(
                "MCP servers (stdio)",
                "JSON array of {name, command, args, env}. Saved servers are spawned at startup.",
                &theme,
            ))
            .child(card(&theme).child(Textarea::new(&mcp_config).h(px(160.))))
            .into_any_element()
    }

    fn render_key_card(
        &mut self,
        provider: Provider,
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let source = self.key_source(provider);
        let input = self.key_input(provider);
        let hint = keys::environment_hint(provider);

        card(theme)
            .child(
                h_flex()
                    .justify_between()
                    .items_center()
                    .child(
                        v_flex()
                            .gap_0p5()
                            .child(div().text_sm().font_semibold().child(provider.label()))
                            .child(div().text_xs().text_color(theme.muted_foreground).child(
                                if hint.is_empty() {
                                    "Optional bearer token.".to_owned()
                                } else {
                                    format!("Environment fallback: {hint}")
                                },
                            )),
                    )
                    .child(status_chip(source.label(), source.is_set(), theme)),
            )
            .when_some(input, |this, input| {
                this.child(
                    h_flex()
                        .gap_2()
                        .items_center()
                        .child(Input::new(&input).mask_toggle().w_full())
                        .child(
                            Button::new(SharedString::from(format!("save-key-{}", provider.id())))
                                .secondary()
                                .label("Save")
                                .on_click(cx.listener(move |this, _, window, cx| {
                                    this.save_key(provider, window, cx);
                                })),
                        )
                        .child(
                            Button::new(SharedString::from(format!("clear-key-{}", provider.id())))
                                .ghost()
                                .label("Clear")
                                .disabled(source == KeySource::Missing)
                                .on_click(cx.listener(move |this, _, window, cx| {
                                    this.clear_key(provider, window, cx);
                                })),
                        ),
                )
                .child(div().text_xs().text_color(theme.muted_foreground).child(
                    "The key is read from the keyring first, then from the environment. \
                             Nothing is written to Relay's database.",
                ))
            })
            .when(source == KeySource::Environment, |this| {
                this.child(
                    h_flex()
                        .gap_2()
                        .items_center()
                        .child(Icon::new(IconName::TriangleAlert).text_color(theme.warning))
                        .child(div().text_xs().text_color(theme.muted_foreground).child(
                            "Using an environment variable. Save a key here to keep it \
                                     in the keyring instead.",
                        )),
                )
            })
            .into_any_element()
    }

    fn render_presets(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme().clone();
        let presets = self.presets.clone();
        let selected = self.preset_selected.clone();
        let active = self.settings.active_preset_id.clone();
        let name = self.preset_name.clone();
        let description = self.preset_description.clone();
        let instructions = self.preset_instructions.clone();
        let is_builtin = self.selected_preset().is_some_and(|preset| preset.builtin);
        let has_selection = self.preset_selected.is_some();

        let mut list = v_flex().gap_1();
        for preset in &presets {
            let id = preset.id.clone();
            let is_selected = selected.as_deref() == Some(preset.id.as_str());
            let is_active = active == preset.id;
            list = list.child(
                div()
                    .id(SharedString::from(format!("preset-{}", preset.id)))
                    .rounded(theme.radius)
                    .px_2()
                    .py_1p5()
                    .cursor_pointer()
                    .when(is_selected, |this| this.bg(theme.sidebar_accent))
                    .when(!is_selected, |this| {
                        this.hover(|style| style.bg(theme.list_hover))
                    })
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.select_preset(id.clone(), window, cx);
                    }))
                    .child(
                        h_flex()
                            .justify_between()
                            .items_center()
                            .gap_2()
                            .child(
                                div()
                                    .text_sm()
                                    .font_medium()
                                    .text_ellipsis()
                                    .child(SharedString::from(preset.name.clone())),
                            )
                            .when(is_active, |this| {
                                this.child(
                                    div()
                                        .rounded(px(4.))
                                        .bg(theme.success)
                                        .px_1p5()
                                        .py_0p5()
                                        .text_xs()
                                        .text_color(theme.background)
                                        .child("Active"),
                                )
                            }),
                    )
                    .child(
                        div()
                            .text_xs()
                            .text_color(theme.muted_foreground)
                            .text_ellipsis()
                            .child(SharedString::from(preset.description.clone())),
                    ),
            );
        }

        v_flex()
            .gap_6()
            .child(section(
                "Personality presets",
                "Presets shape tone and approach. Custom instructions are appended on top.",
                &theme,
            ))
            .child(
                h_flex()
                    .gap_6()
                    .items_start()
                    .child(
                        v_flex()
                            .gap_2()
                            .w(px(270.))
                            .flex_shrink_0()
                            .child(
                                Button::new("new-preset")
                                    .secondary()
                                    .w_full()
                                    .icon(Icon::new(IconName::Plus))
                                    .label("New preset")
                                    .on_click(cx.listener(|this, _, window, cx| {
                                        this.new_preset(window, cx);
                                    })),
                            )
                            .child(
                                div()
                                    .id("preset-list")
                                    .max_h(px(420.))
                                    .overflow_y_scroll()
                                    .child(list),
                            ),
                    )
                    .child(
                        card(&theme)
                            .flex_1()
                            .min_w_0()
                            .when(is_builtin, |this| {
                                this.child(
                                h_flex()
                                    .gap_2()
                                    .items_center()
                                    .rounded(theme.radius)
                                    .bg(theme.muted)
                                    .px_3()
                                    .py_2()
                                    .child(Icon::new(IconName::Info))
                                    .child(
                                        div().text_xs().text_color(theme.muted_foreground).child(
                                            "Built-in presets are read-only. Duplicate this \
                                                     preset to customize it.",
                                        ),
                                    ),
                            )
                            })
                            .child(field("Name", "", Input::new(&name).w_full()))
                            .child(field(
                                "Description",
                                "Shown in this list and the preset picker.",
                                Input::new(&description).w_full(),
                            ))
                            .child(field(
                                "Instructions",
                                "Appended to Relay's base prompt for every conversation.",
                                Textarea::new(&instructions).h(px(140.)),
                            ))
                            .child(
                                h_flex()
                                    .gap_2()
                                    .items_center()
                                    .child(
                                        Button::new("save-preset")
                                            .primary()
                                            .label("Save preset")
                                            .on_click(cx.listener(|this, _, window, cx| {
                                                this.save_preset(window, cx);
                                            })),
                                    )
                                    .child(
                                        Button::new("use-preset")
                                            .secondary()
                                            .label("Use preset")
                                            .disabled(!has_selection)
                                            .on_click(cx.listener(|this, _, window, cx| {
                                                this.use_preset(window, cx);
                                            })),
                                    )
                                    .child(
                                        Button::new("duplicate-preset")
                                            .outline()
                                            .label("Duplicate")
                                            .disabled(!has_selection)
                                            .on_click(cx.listener(|this, _, window, cx| {
                                                this.duplicate_preset(window, cx);
                                            })),
                                    )
                                    .child(div().flex_1())
                                    .child(
                                        Button::new("delete-preset")
                                            .ghost()
                                            .label("Delete")
                                            .disabled(!has_selection || is_builtin)
                                            .on_click(cx.listener(|this, _, window, cx| {
                                                this.delete_preset(window, cx);
                                            })),
                                    ),
                            ),
                    ),
            )
            .into_any_element()
    }

    fn render_data(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme().clone();
        let stats = self.stats;
        let path = self.store.path().display().to_string();
        let path_for_copy = path.clone();

        v_flex()
            .gap_6()
            .child(section(
                "Local data",
                "Everything Relay stores lives in one SQLCipher-encrypted SQLite database on this \
                 machine. The database key is held in your OS keyring, or supplied by \
                 RELAY_MASTER_KEY for headless setups.",
                &theme,
            ))
            .child(
                card(&theme)
                    .child(
                        h_flex()
                            .gap_4()
                            .child(stat(
                                "Conversations",
                                stats.conversations.to_string(),
                                &theme,
                            ))
                            .child(stat("Messages", stats.messages.to_string(), &theme))
                            .child(stat("Tokens", format_tokens(stats.tokens), &theme)),
                    )
                    .child(
                        v_flex()
                            .gap_1()
                            .child(div().text_sm().font_medium().child("Database"))
                            .child(
                                div()
                                    .text_xs()
                                    .font_family("DejaVu Sans Mono")
                                    .text_color(theme.muted_foreground)
                                    .child(SharedString::from(path.clone())),
                            ),
                    )
                    .child(
                        h_flex()
                            .justify_between()
                            .items_center()
                            .child(div().text_sm().font_medium().child("Encryption"))
                            .child(
                                div()
                                    .text_xs()
                                    .text_color(theme.muted_foreground)
                                    .child("SQLCipher · AES-256 · key in keyring or env"),
                            ),
                    )
                    .child(
                        h_flex()
                            .gap_2()
                            .child(
                                Button::new("refresh-stats")
                                    .secondary()
                                    .icon(Icon::new(IconName::RefreshCw))
                                    .label("Refresh")
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.stats = this.store.stats().unwrap_or_default();
                                        cx.notify();
                                    })),
                            )
                            .child(
                                Button::new("copy-path")
                                    .ghost()
                                    .icon(Icon::new(IconName::Copy))
                                    .label("Copy path")
                                    .on_click(cx.listener(move |_, _, window, cx| {
                                        cx.write_to_clipboard(gpui_kit::ClipboardItem::new_string(
                                            path_for_copy.clone(),
                                        ));
                                        window.push_notification("Database path copied", cx);
                                    })),
                            ),
                    ),
            )
            .child(section(
                "Cross-session memory",
                "Facts Relay remembers and includes in every conversation.",
                &theme,
            ))
            .child(
                card(&theme)
                    .child(
                        h_flex()
                            .gap_2()
                            .items_center()
                            .child(Input::new(&self.memory_input).flex_1())
                            .child(Button::new("add-memory").primary().label("Add").on_click(
                                cx.listener(|this, _, window, cx| {
                                    let text = this.memory_input.read(cx).value().trim().to_owned();
                                    if text.is_empty() {
                                        return;
                                    }
                                    if let Err(e) = this.store.add_memory(&text) {
                                        window.push_notification(
                                            Notification::error(format!("{e}")),
                                            cx,
                                        );
                                        return;
                                    }
                                    this.memory_input
                                        .update(cx, |s, cx| s.set_value("", window, cx));
                                    this.memories = this.store.list_memories().unwrap_or_default();
                                    window.push_notification("Memory saved", cx);
                                    cx.notify();
                                }),
                            )),
                    )
                    .children(self.memories.iter().map(|(id, content)| {
                        let id = id.clone();
                        h_flex()
                            .w_full()
                            .justify_between()
                            .items_center()
                            .py_1()
                            .child(
                                div()
                                    .text_sm()
                                    .flex_1()
                                    .child(SharedString::from(content.clone())),
                            )
                            .child(
                                Button::new(SharedString::from(format!("del-mem-{id}")))
                                    .ghost()
                                    .xsmall()
                                    .icon(Icon::new(IconName::Trash))
                                    .on_click(cx.listener(move |this, _, _window, cx| {
                                        let _ = this.store.delete_memory(&id);
                                        this.memories =
                                            this.store.list_memories().unwrap_or_default();
                                        cx.notify();
                                    })),
                            )
                            .into_any_element()
                    })),
            )
            .child(section(
                "Danger zone",
                "These actions are local and immediate.",
                &theme,
            ))
            .child(
                card(&theme).child(
                    h_flex()
                        .justify_between()
                        .items_center()
                        .gap_4()
                        .child(
                            v_flex()
                                .gap_0p5()
                                .child(
                                    div()
                                        .text_sm()
                                        .font_medium()
                                        .child("Delete all conversations"),
                                )
                                .child(div().text_xs().text_color(theme.muted_foreground).child(
                                    "Removes history, messages, and search indexes. \
                                                 Settings, presets, and keys are kept.",
                                )),
                        )
                        .child(
                            Button::new("delete-all")
                                .danger()
                                .label("Delete all")
                                .on_click(cx.listener(|this, _, window, cx| {
                                    this.confirm_delete_all(window, cx);
                                })),
                        ),
                ),
            )
            .into_any_element()
    }

    fn render_about(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme().clone();
        v_flex()
            .gap_6()
            .child(
                v_flex()
                    .gap_3()
                    .items_center()
                    .py_4()
                    .child(
                        div()
                            .size(px(56.))
                            .rounded(px(16.))
                            .bg(theme.primary)
                            .flex()
                            .items_center()
                            .justify_center()
                            .text_2xl()
                            .font_bold()
                            .text_color(theme.primary_foreground)
                            .child("R"),
                    )
                    .child(div().text_2xl().font_semibold().child("Relay"))
                    .child(div().text_sm().text_color(theme.muted_foreground).child(
                        SharedString::from(format!("Version {} · Apache-2.0", relay_core::VERSION)),
                    )),
            )
            .child(section(
                "One conversation, every frontier model",
                "Relay is a local-first desktop chat client for Gemini, Claude, GPT, Ollama, and \
                 OpenAI-compatible endpoints. Conversations are stored as provider-neutral \
                 messages, so you can switch models mid-thread without losing context.",
                &theme,
            ))
            .child(section(
                "Guarantees",
                "What Relay controls — and the one thing it cannot.",
                &theme,
            ))
            .child(
                card(&theme)
                    .gap_3()
                    .child(check_item(
                        "No ads",
                        "No sponsored placements, promoted models, or paid ranking.",
                        &theme,
                    ))
                    .child(check_item(
                        "Open source, Apache-2.0",
                        "The full source is public, with an express patent grant.",
                        &theme,
                    ))
                    .child(check_item(
                        "Non-profit project",
                        "No company, investors, or paid Relay tier. Relay is not monetized.",
                        &theme,
                    ))
                    .child(check_item(
                        "Zero telemetry",
                        "No analytics, crash reporting, or usage pings. Relay collects nothing, and local models send nothing.",
                        &theme,
                    ))
                    .child(check_item(
                        "Local models stay local",
                        "Bundled Nemotron models run on this machine. Their prompts never leave it.",
                        &theme,
                    ))
                    .child(check_item(
                        "Your data stays with you",
                        "History is a local SQLCipher-encrypted database. API keys are sealed in \
                         the OS keyring or read from your environment, never the database.",
                        &theme,
                    ))
                    .child(check_item(
                        "No middleman",
                        "Cloud requests go straight from Relay to the provider with your own key. There is no Relay server.",
                        &theme,
                    ))
                    .child(
                        h_flex()
                            .items_start()
                            .gap_2()
                            .border_t_1()
                            .border_color(theme.border)
                            .pt_3()
                            .child(
                                div()
                                    .mt(px(1.))
                                    .flex_shrink_0()
                                    .child(
                                        Icon::new(IconName::Info)
                                            .text_color(theme.muted_foreground),
                                    ),
                            )
                            .child(
                                div()
                                    .flex_1()
                                    .min_w_0()
                                    .text_xs()
                                    .text_color(theme.muted_foreground)
                                    .child(
                                        "The one thing Relay cannot guarantee: what a cloud provider \
                                         does with a prompt after it leaves this machine — \
                                         whether they retain it or train on it. Those terms are \
                                         theirs, not ours.",
                                    ),
                            ),
                    ),
            )
            .child(section(
                "Cloud models & data training",
                "How OpenAI, Anthropic, and Google treat API data for the cloud models Relay \
                 ships, as of November 2026. These are provider policies; Relay cannot override \
                 them.",
                &theme,
            ))
            .child(
                card(&theme)
                    .child(provider_note(
                        "Google Gemini",
                        "Gemini 4 Hydrogen · Argon · Carbon · Neon",
                        "Paid Gemini API tier: prompts and responses are not used to improve \
                         Google products; logs are kept briefly for abuse prevention. Unpaid tier \
                         and AI Studio may use content to improve products, including human \
                         review. In the EEA, Switzerland, and the UK, paid-tier data terms apply \
                         to all Gemini API and AI Studio use.",
                        &theme,
                    ))
                    .child(
                        provider_note(
                            "Anthropic Claude",
                            "Claude Haiku, Sonnet, and Opus 5.5 · Fable and Mythos 5.2",
                            "Commercial Terms: Anthropic may not train models on Customer \
                             Content submitted through the API. Consumer Claude plans are \
                             separate and have their own controls. Zero data retention is \
                             available for eligible enterprise arrangements.",
                            &theme,
                        )
                        .border_t_1()
                        .border_color(theme.border)
                        .pt_4(),
                    )
                    .child(
                        provider_note(
                            "OpenAI GPT",
                            "GPT-6.2 Astra · GPT-6.2 Sol · GPT-6.1 Terra · GPT-6 Luna",
                            "API data is not used to train or improve OpenAI models unless you \
                             explicitly opt in (since March 1, 2023). Abuse-monitoring logs are \
                             kept for up to 30 days, longer only when legally required; approved \
                             organizations can enable zero data retention. ChatGPT consumer \
                             plans differ.",
                            &theme,
                        )
                        .border_t_1()
                        .border_color(theme.border)
                        .pt_4(),
                    )
                    .child(
                        v_flex()
                            .gap_2()
                            .border_t_1()
                            .border_color(theme.border)
                            .pt_3()
                            .child(
                                h_flex()
                                    .gap_2()
                                    .items_center()
                                    .child(
                                        div()
                                            .text_xs()
                                            .text_color(theme.muted_foreground)
                                            .child("Official policies"),
                                    )
                                    .child(
                                        Button::new("openai-policy")
                                            .ghost()
                                            .small()
                                            .label("OpenAI")
                                            .on_click(cx.listener(|_, _, _, cx| {
                                                cx.open_url(
                                                    "https://developers.openai.com/api/docs/guides/your-data",
                                                );
                                            })),
                                    )
                                    .child(
                                        Button::new("anthropic-policy")
                                            .ghost()
                                            .small()
                                            .label("Anthropic")
                                            .on_click(cx.listener(|_, _, _, cx| {
                                                cx.open_url(
                                                    "https://www.anthropic.com/legal/commercial-terms",
                                                );
                                            })),
                                    )
                                    .child(
                                        Button::new("gemini-policy")
                                            .ghost()
                                            .small()
                                            .label("Gemini")
                                            .on_click(cx.listener(|_, _, _, cx| {
                                                cx.open_url("https://ai.google.dev/gemini-api/terms");
                                            })),
                                    ),
                            )
                            .child(
                                div()
                                    .text_xs()
                                    .text_color(theme.muted_foreground)
                                    .child(
                                        "Last checked November 2026. Terms can change; the \
                                         provider's current policy governs. Relay cannot control \
                                         provider-side retention or training.",
                                    ),
                            ),
                    ),
            )
            .child(
                card(&theme)
                    .child(
                        h_flex()
                            .justify_between()
                            .items_center()
                            .child(
                                v_flex()
                                    .gap_0p5()
                                    .child(div().text_sm().font_medium().child("Project"))
                                    .child(
                                        div()
                                            .text_xs()
                                            .text_color(theme.muted_foreground)
                                            .child(relay_core::PROJECT_URL),
                                    ),
                            )
                            .child(
                                Button::new("open-project")
                                    .outline()
                                    .icon(Icon::new(IconName::ExternalLink))
                                    .label("Open")
                                    .on_click(cx.listener(|_, _, _, cx| {
                                        cx.open_url(relay_core::PROJECT_URL);
                                    })),
                            ),
                    )
                    .child(
                        h_flex()
                            .justify_between()
                            .items_center()
                            .child(div().text_sm().font_medium().child("Model catalog"))
                            .child(
                                div()
                                    .text_xs()
                                    .text_color(theme.muted_foreground)
                                    .child("13 cloud models · Ollama · OpenAI-compatible"),
                            ),
                    )
                    .child(
                        h_flex()
                            .justify_between()
                            .items_center()
                            .child(div().text_sm().font_medium().child("Built with"))
                            .child(
                                div()
                                    .text_xs()
                                    .text_color(theme.muted_foreground)
                                    .child("Rust · GPUI Kit · SQLite · Tokio"),
                            ),
                    ),
            )
            .into_any_element()
    }
}

impl gpui_kit::Render for SettingsView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme().clone();
        let content = match self.tab {
            SettingsTab::General => self.render_general(window, cx),
            SettingsTab::Models => self.render_models(window, cx),
            SettingsTab::Presets => self.render_presets(window, cx),
            SettingsTab::Data => self.render_data(window, cx),
            SettingsTab::About => self.render_about(window, cx),
        };
        let tabs = self.render_tabs(&theme, cx).into_any_element();

        v_flex()
            .size_full()
            .child(
                h_flex()
                    .flex_none()
                    .h(px(54.))
                    .px_4()
                    .gap_2()
                    .items_center()
                    .border_b_1()
                    .border_color(theme.border)
                    .child(
                        Button::new("settings-back")
                            .ghost()
                            .small()
                            .icon(Icon::new(IconName::ArrowLeft))
                            .tooltip("Back to chat")
                            .on_click(cx.listener(|_, _, _, cx| {
                                cx.emit(SettingsEvent::Close);
                            })),
                    )
                    .child(div().text_sm().font_semibold().child("Settings"))
                    .child(div().flex_1())
                    .child(
                        Button::new("settings-save")
                            .primary()
                            .icon(Icon::new(IconName::Check))
                            .label("Save")
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.save(window, cx);
                            })),
                    ),
            )
            .child(
                h_flex()
                    .flex_1()
                    .min_h_0()
                    .child(
                        div()
                            .w(px(210.))
                            .h_full()
                            .flex_shrink_0()
                            .border_r_1()
                            .border_color(theme.border)
                            .child(tabs),
                    )
                    .child(
                        div()
                            .id("settings-content")
                            .flex_1()
                            .min_w_0()
                            .h_full()
                            .overflow_y_scroll()
                            .track_scroll(&self.scroll)
                            .child(
                                v_flex()
                                    .max_w(px(820.))
                                    .w_full()
                                    .p_6()
                                    .gap_6()
                                    .child(content),
                            ),
                    ),
            )
    }
}

fn key_providers() -> [Provider; 4] {
    [
        Provider::Gemini,
        Provider::Anthropic,
        Provider::OpenAi,
        Provider::OpenAiCompatible,
    ]
}

fn text_input(
    window: &mut Window,
    cx: &mut Context<SettingsView>,
    placeholder: &str,
    value: &str,
) -> Entity<InputState> {
    let input = cx.new(|cx| InputState::new(window, cx).placeholder(placeholder.to_owned()));
    if !value.is_empty() {
        input.update(cx, |state, cx| {
            state.set_value(value.to_owned(), window, cx)
        });
    }
    input
}

fn textarea(
    window: &mut Window,
    cx: &mut Context<SettingsView>,
    placeholder: &str,
    value: &str,
) -> Entity<TextareaState> {
    let input = cx.new(|cx| {
        TextareaState::new(window, cx)
            .placeholder(placeholder.to_owned())
            .auto_grow(2, 8)
    });
    if !value.is_empty() {
        input.update(cx, |state, cx| {
            state.set_value(value.to_owned(), window, cx)
        });
    }
    input
}

fn section(title: &'static str, description: &'static str, theme: &Theme) -> impl IntoElement {
    v_flex()
        .gap_1()
        .child(div().text_lg().font_semibold().child(title))
        .when(!description.is_empty(), |this| {
            this.child(
                div()
                    .max_w(px(700.))
                    .text_sm()
                    .text_color(theme.muted_foreground)
                    .child(description),
            )
        })
}

fn card(theme: &Theme) -> gpui_kit::Div {
    v_flex()
        .gap_4()
        .rounded(theme.radius_lg)
        .border_1()
        .border_color(theme.border)
        .p_4()
}

fn check_item(title: &'static str, detail: &'static str, theme: &Theme) -> gpui_kit::Div {
    h_flex()
        .items_start()
        .gap_2()
        .child(
            div()
                .mt(px(1.))
                .flex_shrink_0()
                .child(Icon::new(IconName::CircleCheck).text_color(theme.success)),
        )
        .child(
            v_flex()
                .gap_0p5()
                .child(div().text_sm().font_medium().child(title))
                .child(
                    div()
                        .text_xs()
                        .text_color(theme.muted_foreground)
                        .child(detail),
                ),
        )
}

fn provider_note(
    provider: &'static str,
    models: &'static str,
    detail: &'static str,
    theme: &Theme,
) -> gpui_kit::Div {
    v_flex()
        .gap_1()
        .child(div().text_sm().font_medium().child(provider))
        .child(
            div()
                .text_xs()
                .text_color(theme.muted_foreground)
                .child(models),
        )
        .child(
            div()
                .text_xs()
                .text_color(theme.muted_foreground)
                .child(detail),
        )
}

fn field(label: &'static str, hint: &'static str, control: impl IntoElement) -> impl IntoElement {
    v_flex()
        .gap_1()
        .child(div().text_sm().font_medium().child(label))
        .when(!hint.is_empty(), |this| {
            this.child(div().text_xs().opacity(0.65).child(hint))
        })
        .child(control)
}

fn toggle_row(
    label: &'static str,
    hint: &'static str,
    control: impl IntoElement,
) -> impl IntoElement {
    h_flex()
        .justify_between()
        .items_center()
        .gap_6()
        .child(
            v_flex()
                .gap_0p5()
                .child(div().text_sm().font_medium().child(label))
                .child(div().text_xs().opacity(0.65).child(hint)),
        )
        .child(control)
}

fn status_chip(label: &str, good: bool, theme: &Theme) -> impl IntoElement {
    div()
        .rounded(px(999.))
        .border_1()
        .border_color(if good { theme.success } else { theme.border })
        .px_2()
        .py_0p5()
        .text_xs()
        .text_color(if good {
            theme.success
        } else {
            theme.muted_foreground
        })
        .child(SharedString::from(label.to_owned()))
}

fn stat(label: &'static str, value: String, theme: &Theme) -> impl IntoElement {
    v_flex()
        .gap_0p5()
        .rounded(theme.radius)
        .bg(theme.muted)
        .px_4()
        .py_2()
        .child(div().text_lg().font_semibold().child(value))
        .child(
            div()
                .text_xs()
                .text_color(theme.muted_foreground)
                .child(label),
        )
}
