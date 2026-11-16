//! Chat surface: header pickers, transcript, welcome panel, and composer.

use gpui_kit::assets::IconName;
use gpui_kit::component::{
    ActiveTheme as _, Disableable as _, Icon, Sizable as _, StyledExt as _, WindowExt as _,
    button::{Button, ButtonVariants as _, Toggle},
    h_flex,
    input::{Input, InputState, Textarea, TextareaState},
    select::Select,
    spinner::Spinner,
    text::TextView,
    v_flex,
};
use gpui_kit::prelude::{FluentBuilder as _, StatefulInteractiveElement as _};
use gpui_kit::{
    AnyElement, AppContext as _, ClipboardItem, Context, InteractiveElement as _, IntoElement,
    KeyDownEvent, ParentElement as _, SharedString, Styled as _, Window, div, px,
};
use relay_core::{
    AgentMode, ChatMessage, ContentBlock, Role, ToolApproval, estimate_tokens, format_tokens,
    format_tokens_exact,
};

use crate::app::RelayApp;
use crate::models;

impl RelayApp {
    pub(crate) fn render_chat(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let theme = cx.theme().clone();
        let title = self.conversation_title.clone();
        let streaming = self.is_streaming();
        let model_select = self.chat.model_select.clone();
        let reasoning_select = self.chat.reasoning_select.clone();
        let composer = self.chat.composer.clone();
        let can_send = !composer.read(cx).value().trim().is_empty();
        let notice = self.notice.clone();
        let messages = self.messages.clone();
        let transcript_title = title.clone();

        let transcript = if messages.is_empty() {
            self.render_welcome(window, cx).into_any_element()
        } else {
            let mut column = v_flex()
                .max_w(px(900.))
                .w_full()
                .mx_auto()
                .px_6()
                .py_6()
                .gap_8();
            let messages: Vec<ChatMessage> = self.messages.clone();
            for message in &messages {
                column = column.child(self.render_message(message, cx));
            }
            div()
                .id("transcript")
                .flex_1()
                .min_h_0()
                .overflow_y_scroll()
                .track_scroll(&self.chat.scroll)
                .child(column)
                .into_any_element()
        };

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
                        div()
                            .flex_1()
                            .min_w_0()
                            .text_sm()
                            .font_medium()
                            .text_ellipsis()
                            .child(SharedString::from(transcript_title)),
                    )
                    .child(
                        h_flex()
                            .gap_2()
                            .items_center()
                            .child(
                                Button::new("mode-chat")
                                    .label("Chat")
                                    .px_4()
                                    .py_1p5()
                                    .when(self.settings.interaction_mode == "chat", |b| b.primary())
                                    .when(self.settings.interaction_mode != "chat", |b| b.outline())
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.settings.interaction_mode = "chat".to_owned();
                                        this.persist_settings();
                                        cx.notify();
                                    })),
                            )
                            .child(
                                Button::new("mode-work")
                                    .label("Work")
                                    .px_4()
                                    .py_1p5()
                                    .when(self.settings.interaction_mode == "work", |b| b.primary())
                                    .when(self.settings.interaction_mode != "work", |b| b.outline())
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.settings.interaction_mode = "work".to_owned();
                                        this.persist_settings();
                                        cx.notify();
                                    })),
                            )
                            .child(
                                Button::new("mode-info")
                                    .ghost()
                                    .small()
                                    .icon(Icon::new(IconName::Info))
                                    .tooltip(
                                        "Chat: daily chatting and brainstorming, no tools, no terminal — like ChatGPT. \
                                         Work: agentic mode with terminal tools, MCP servers, and file operations.",
                                    ),
                            ),
                    )
                    .child(
                        div()
                            .flex_1()
                            .flex()
                            .justify_end()
                            .items_center()
                            .gap_2()
                            .child(
                                div()
                                    .w(px(244.))
                                    .flex_shrink_0()
                                    .child(Select::new(&model_select).w_full()),
                            )
                    )
                    .when_some(reasoning_select, |this, select| {
                        this.child(
                            div()
                                .w(px(124.))
                                .flex_shrink_0()
                                .child(Select::new(&select).w_full()),
                        )
                    })
                    .child(
                        Button::new("copy-conversation")
                            .ghost()
                            .small()
                            .icon(Icon::new(IconName::Copy))
                            .tooltip("Copy conversation as Markdown")
                            .disabled(messages.is_empty())
                            .on_click(cx.listener(move |this, _, window, cx| {
                                this.copy_conversation(window, cx);
                            })),
                    ),
            )
            .child(transcript)
            .when(self.pending_ollama_install.is_some(), |this| {
                let tag = self.pending_ollama_install.clone().unwrap();
                this.child(
                    div()
                        .max_w(px(900.))
                        .w_full()
                        .mx_auto()
                        .px_6()
                        .py_3()
                        .rounded(theme.radius)
                        .border_1()
                        .border_color(theme.primary)
                        .bg(theme.input)
                        .child(
                            v_flex()
                                .gap_2()
                                .child(div().text_sm().font_medium().child(format!(
                                    "Install {} via Ollama? (ollama pull {})",
                                    tag, tag
                                )))
                                .child(
                                    v_flex()
                                        .gap_1()
                                        .child(guarantee_item("Local & private", "Runs on your machine; prompts never leave it.", &theme))
                                        .child(guarantee_item("Open weights", "Licensed for local use; see model card.", &theme)),
                                )
                                .child(
                                    h_flex()
                                        .gap_2()
                                        .child(
                                            Button::new("confirm-ollama-install")
                                                .primary()
                                                .label("Install")
                                                .on_click(cx.listener(move |this, _, _, cx| {
                                                    let t = tag.clone();
                                                    this.runtime.spawn(async move {
                                                        let _ = std::process::Command::new("ollama")
                                                            .arg("pull")
                                                            .arg(&t)
                                                            .status();
                                                    });
                                                    this.pending_ollama_install = None;
                                                    cx.notify();
                                                })),
                                        )
                                        .child(
                                            Button::new("cancel-ollama-install")
                                                .ghost()
                                                .label("Cancel")
                                                .on_click(cx.listener(|this, _, _, cx| {
                                                    this.pending_ollama_install = None;
                                                    cx.notify();
                                                })),
                                        ),
                                ),
                        )
                )
            })
            .when(self.pending_bundled_download.is_some(), |this| {
                let variant = self.pending_bundled_download.unwrap();
                this.child(
                    div()
                        .max_w(px(900.))
                        .w_full()
                        .mx_auto()
                        .px_6()
                        .py_3()
                        .rounded(theme.radius)
                        .border_1()
                        .border_color(theme.primary)
                        .bg(theme.input)
                        .child(
                            v_flex()
                                .gap_2()
                                .child(div().text_sm().font_medium().child(format!(
                                    "Download {}? (~4-5 GB, one-time)",
                                    variant.display_name()
                                )))
                                .child(
                                    v_flex()
                                        .gap_1()
                                        .child(guarantee_item("No telemetry", "No analytics, crash reporting, or usage pings.", &theme))
                                        .child(guarantee_item("Local model stays local", "The model runs on this machine; prompts never leave it.", &theme))
                                        .child(guarantee_item("Your data stays with you", "History is a local SQLCipher-encrypted database.", &theme))
                                        .child(guarantee_item("No middleman", "No Relay server; requests go straight to the provider.", &theme)),
                                )
                                .child(
                                    h_flex()
                                        .gap_2()
                                        .child(
                                            Button::new("confirm-bundled-download")
                                                .primary()
                                                .label("Download")
                                                .on_click(cx.listener(move |this, _, _, cx| {
                                                    this.ensure_bundled_variant(variant, cx);
                                                    this.pending_bundled_download = None;
                                                    cx.notify();
                                                })),
                                        )
                                        .child(
                                            Button::new("cancel-bundled-download")
                                                .ghost()
                                                .label("Cancel")
                                                .on_click(cx.listener(|this, _, _, cx| {
                                                    this.pending_bundled_download = None;
                                                    cx.notify();
                                                })),
                                        ),
                                ),
                        )
                )
            })
            .child(self.render_composer(window, cx, &composer, can_send, streaming, notice))
            .child(self.render_usage_footer(cx))
    }

    /// Persistent bottom strip with the current context meter and session totals.
    ///
    /// The context figure prefers provider-reported usage and falls back to a
    /// character-based estimate while a response is still in flight.
    fn render_usage_footer(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme().clone();
        let model = self.current_model();
        let window = relay_engine::agent::context_window(&self.settings, &model);

        let mut total_in = 0u64;
        let mut total_out = 0u64;
        let mut latest_context = 0u64;
        let mut estimated = 0u64;
        for message in &self.messages {
            estimated += estimate_message_tokens(message);
            if let Some(usage) = message.usage {
                total_in += u64::from(usage.input_tokens);
                total_out += u64::from(usage.output_tokens);
                latest_context = u64::from(usage.input_tokens) + u64::from(usage.output_tokens);
            }
        }
        let session_tokens = (total_in + total_out).max(estimated);
        let used = latest_context.max(estimated);
        let percent = if window > 0 {
            used as f64 / f64::from(window) * 100.0
        } else {
            0.0
        };
        let ratio = (percent / 100.0).clamp(0.0, 1.0);
        let percent_label = if used > 0 && percent < 0.05 {
            "<0.1%".to_owned()
        } else {
            format!("{percent:.1}%")
        };
        let bar_color = if percent >= 90.0 {
            theme.danger
        } else {
            theme.primary
        };
        let has_usage = total_in + total_out > 0;

        h_flex()
            .flex_none()
            .h(px(32.))
            .px_6()
            .gap_2()
            .items_center()
            .border_t_1()
            .border_color(theme.border)
            .text_xs()
            .text_color(theme.muted_foreground)
            .child(div().size(px(6.)).rounded_full().bg(if percent >= 90.0 {
                theme.danger
            } else {
                theme.success
            }))
            .child(div().child(SharedString::from(format!(
                "Context ~{} / {} tokens ({percent_label})",
                format_tokens(used),
                format_tokens_exact(u64::from(window))
            ))))
            .child(
                div()
                    .w(px(72.))
                    .h(px(4.))
                    .rounded_full()
                    .bg(theme.muted)
                    .overflow_hidden()
                    .child(
                        div()
                            .h_full()
                            .w(gpui_kit::relative(ratio as f32))
                            .bg(bar_color)
                            .rounded_full(),
                    ),
            )
            .child(div().flex_1())
            .child(div().child(SharedString::from(if has_usage {
                format!(
                    "Session ~{} tokens · {} in / {} out",
                    format_tokens(session_tokens),
                    format_tokens(total_in),
                    format_tokens(total_out)
                )
            } else {
                format!(
                    "Session ~{} tokens (estimated)",
                    format_tokens(session_tokens)
                )
            })))
    }

    fn render_welcome(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme().clone();
        let keys_ready = self.cloud_keys_ready;
        let local_ready = !self.settings.ollama_model.trim().is_empty()
            || !self.settings.local_model.trim().is_empty();

        v_flex()
            .flex_1()
            .min_h_0()
            .items_center()
            .justify_center()
            .gap_5()
            .p_8()
            .child(
                div()
                    .size(px(56.))
                    .rounded(px(16.))
                    .bg(theme.primary)
                    .flex()
                    .items_center()
                    .justify_center()
                    .child(
                        Icon::new(IconName::Sparkles)
                            .size_7()
                            .text_color(theme.primary_foreground),
                    ),
            )
            .child(
                div()
                    .text_2xl()
                    .font_semibold()
                    .text_center()
                    .child("One conversation, every frontier model."),
            )
            .child(
                div()
                    .max_w(px(540.))
                    .text_sm()
                    .text_center()
                    .text_color(theme.muted_foreground)
                    .child(
                        "Ask once, then switch between Gemini, Claude, GPT, Ollama, and local \
                         endpoints without losing the thread. Your history stays on this machine.",
                    ),
            )
            .when(!keys_ready && !local_ready, |this| {
                this.child(
                    h_flex()
                        .gap_3()
                        .items_center()
                        .rounded(theme.radius_lg)
                        .border_1()
                        .border_color(theme.border)
                        .bg(theme.input)
                        .px_4()
                        .py_3()
                        .child(Icon::new(IconName::KeyRound).text_color(theme.primary))
                        .child(
                            v_flex()
                                .gap_0p5()
                                .child(div().text_sm().font_medium().child("Add an API key"))
                                .child(div().text_xs().text_color(theme.muted_foreground).child(
                                    "Keys are encrypted in your desktop keyring, never in Relay's \
                                     database. History is encrypted locally.",
                                )),
                        )
                        .child(
                            Button::new("welcome-settings")
                                .outline()
                                .small()
                                .label("Open settings")
                                .on_click(cx.listener(|this, _, window, cx| {
                                    this.open_settings(window, cx);
                                })),
                        ),
                )
            })
            .child(
                h_flex()
                    .flex_wrap()
                    .gap_3()
                    .justify_center()
                    .max_w(px(660.))
                    .children([
                        suggestion(
                            "Compare the tradeoffs between SQLite FTS5 and a Tantivy sidecar for \
                             full-text search over 200,000 notes.",
                            "Compare search backends",
                            "Weigh setup, ranking, and maintenance.",
                            &theme,
                            cx,
                        ),
                        suggestion(
                            "Explain why this Rust function fails to compile and show the smallest \
                             fix: fn first<'a>(items: &'a [String]) -> &'a str { &items[0] }",
                            "Debug a Rust lifetime error",
                            "Get a precise diagnosis and a fix.",
                            &theme,
                            cx,
                        ),
                        suggestion(
                            "Design a three-day Kyoto itinerary for a first-time visitor who likes \
                             walking, temples, and small restaurants, with a budget per day.",
                            "Plan a slow weekend in Kyoto",
                            "Ask for a realistic, opinionated plan.",
                            &theme,
                            cx,
                        ),
                        suggestion(
                            "Write a short, vivid opening for a science-fiction novel set aboard a \
                             generation ship whose inhabitants have forgotten the destination.",
                            "Draft a story opening",
                            "Try a creative brief with a strong voice.",
                            &theme,
                            cx,
                        ),
                    ]),
            )
    }

    fn render_message(&mut self, message: &ChatMessage, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme().clone();
        if let Some((id, input)) = &self.editing
            && id == &message.id
        {
            let input = input.clone();
            let msg_id = message.id.clone();
            return v_flex()
                .w_full()
                .items_end()
                .gap_2()
                .child(Input::new(&input).w(px(480.)))
                .child(
                    h_flex()
                        .gap_2()
                        .child(Button::new("save-edit").primary().label("Save").on_click(
                            cx.listener(move |this, _, window, cx| {
                                let text = input.read(cx).value().to_string();
                                this.editing = None;
                                this.edit_user_message(msg_id.clone(), text, window, cx);
                            }),
                        ))
                        .child(Button::new("cancel-edit").ghost().label("Cancel").on_click(
                            cx.listener(|this, _, _, cx| {
                                this.editing = None;
                                cx.notify();
                            }),
                        )),
                )
                .into_any_element();
        }
        match message.role {
            Role::User => h_flex()
                .w_full()
                .justify_end()
                .gap_2()
                .items_end()
                .child(
                    v_flex()
                        .gap_1()
                        .items_end()
                        .child(
                            h_flex()
                                .gap_1()
                                .child(
                                    Button::new(SharedString::from(format!("copy-{}", message.id)))
                                        .ghost()
                                        .xsmall()
                                        .icon(Icon::new(IconName::Copy))
                                        .on_click(cx.listener({
                                            let text = message.text();
                                            move |_, _, _, cx| {
                                                cx.write_to_clipboard(
                                                    gpui_kit::ClipboardItem::new_string(
                                                        text.clone(),
                                                    ),
                                                );
                                            }
                                        })),
                                )
                                .child(
                                    Button::new(SharedString::from(format!("edit-{}", message.id)))
                                        .ghost()
                                        .xsmall()
                                        .icon(Icon::new(IconName::Pencil))
                                        .on_click(cx.listener({
                                            let id = message.id.clone();
                                            let text = message.text();
                                            move |this, _, window, cx| {
                                                let input = cx.new(|cx| {
                                                    InputState::new(window, cx)
                                                        .placeholder("Edit message…")
                                                });
                                                input.update(cx, |s, cx| {
                                                    s.set_value(text.clone(), window, cx)
                                                });
                                                this.editing = Some((id.clone(), input));
                                                cx.notify();
                                            }
                                        })),
                                ),
                        )
                        .child(
                            div()
                                .max_w(px(640.))
                                .rounded(theme.radius_lg)
                                .bg(theme.secondary)
                                .text_color(theme.secondary_foreground)
                                .px_4()
                                .py_3()
                                .child(
                                    TextView::markdown(
                                        SharedString::from(format!("user-{}", message.id)),
                                        SharedString::from(message.text()),
                                    )
                                    .selectable(true),
                                ),
                        ),
                )
                .into_any_element(),
            Role::Tool => div()
                .w_full()
                .px_4()
                .py_2()
                .text_xs()
                .text_color(theme.muted_foreground)
                .child(SharedString::from(format!(
                    "Tool result: {}",
                    message.text()
                )))
                .into_any_element(),
            Role::Assistant => {
                let model = message
                    .model
                    .as_deref()
                    .map(models::resolve)
                    .unwrap_or_else(|| self.current_model());
                let thinking = message.thinking();
                let expanded = self.chat.expanded.contains(&message.id);
                let streaming = self.streaming_message_id() == Some(message.id.as_str());
                let body = self.chat.text_states.get(&message.id).cloned();
                let thinking_state = self.chat.thinking_states.get(&message.id).cloned();
                let show_thinking = self.settings.show_thinking;
                let error = message.error.clone();
                let usage = message.usage;
                let message_id = message.id.clone();
                let copy_text = message.text();

                // Tool call status
                let tool_uses: Vec<_> = message
                    .blocks
                    .iter()
                    .filter_map(|b| {
                        if let ContentBlock::ToolUse {
                            id,
                            name,
                            arguments,
                        } = b
                        {
                            Some((id.clone(), name.clone(), arguments.clone()))
                        } else {
                            None
                        }
                    })
                    .collect();
                let tool_results: std::collections::HashMap<String, String> = message
                    .blocks
                    .iter()
                    .filter_map(|b| {
                        if let ContentBlock::ToolResult { id, output, .. } = b {
                            Some((id.clone(), output.clone()))
                        } else {
                            None
                        }
                    })
                    .collect();
                let running_tool = tool_uses
                    .iter()
                    .find(|(id, _, _)| !tool_results.contains_key(id))
                    .map(|(_, n, _)| n.clone());

                let mut block = v_flex().w_full().gap_3().child(
                    h_flex()
                        .gap_2()
                        .items_center()
                        .child(
                            div()
                                .size_6()
                                .rounded_full()
                                .bg(theme.primary)
                                .flex()
                                .items_center()
                                .justify_center()
                                .text_xs()
                                .font_bold()
                                .text_color(theme.primary_foreground)
                                .child("R"),
                        )
                        .child(
                            div()
                                .text_sm()
                                .font_semibold()
                                .child(SharedString::from(model.name.clone())),
                        )
                        .when(model.is_local(), |this| {
                            this.child(
                                div()
                                    .rounded(px(4.))
                                    .bg(theme.muted)
                                    .px_1p5()
                                    .py_0p5()
                                    .text_xs()
                                    .text_color(theme.muted_foreground)
                                    .child("Local"),
                            )
                        })
                        .when(streaming, |this| {
                            let label = if let Some(name) = &running_tool {
                                format!("Running {name}…")
                            } else if copy_text.trim().is_empty() && tool_uses.is_empty() {
                                "Thinking…".to_owned()
                            } else if copy_text.trim().is_empty() {
                                "Working…".to_owned()
                            } else {
                                "Writing…".to_owned()
                            };
                            this.child(
                                h_flex()
                                    .gap_1()
                                    .items_center()
                                    .child(Spinner::new().small())
                                    .child(
                                        div()
                                            .text_xs()
                                            .text_color(theme.muted_foreground)
                                            .child(label),
                                    ),
                            )
                        }),
                );

                if show_thinking && (!thinking.trim().is_empty() || thinking_state.is_some()) {
                    let toggle_id = message_id.clone();
                    block = block.child(
                        v_flex()
                            .gap_1()
                            .child(
                                Button::new(SharedString::from(format!("thinking-{}", message.id)))
                                    .ghost()
                                    .small()
                                    .icon(Icon::new(if expanded {
                                        IconName::ChevronDown
                                    } else {
                                        IconName::ChevronRight
                                    }))
                                    .label("Reasoning")
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        if !this.chat.expanded.remove(&toggle_id) {
                                            this.chat.expanded.insert(toggle_id.clone());
                                        }
                                        cx.notify();
                                    })),
                            )
                            .when(expanded, |this| {
                                this.child(
                                    div()
                                        .border_l_2()
                                        .border_color(theme.border)
                                        .pl_3()
                                        .py_1()
                                        .text_sm()
                                        .text_color(theme.muted_foreground)
                                        .when_some(thinking_state.clone(), |this, state| {
                                            this.child(
                                                TextView::new(&state)
                                                    .selectable(true)
                                                    .scrollable(false),
                                            )
                                        }),
                                )
                            }),
                    );
                }

                // Render tool calls/results
                if !tool_uses.is_empty() {
                    let mut tools_col = v_flex().gap_2();
                    for (id, name, args) in &tool_uses {
                        let result = tool_results.get(id);
                        let running = result.is_none();
                        let arg_summary = tool_arg_summary(name, args);
                        let out_text = result.cloned().unwrap_or_default();
                        let short_out = truncate_preview(&out_text, 600);
                        tools_col = tools_col.child(
                            div()
                                .w_full()
                                .rounded(theme.radius)
                                .border_1()
                                .border_color(if running { theme.primary } else { theme.border })
                                .bg(theme.muted)
                                .px_3()
                                .py_2()
                                .child(
                                    v_flex()
                                        .gap_1()
                                        .child(
                                            h_flex()
                                                .gap_2()
                                                .items_center()
                                                .child(
                                                    Icon::new(IconName::Terminal)
                                                        .small()
                                                        .text_color(theme.muted_foreground),
                                                )
                                                .child(
                                                    div()
                                                        .text_xs()
                                                        .font_semibold()
                                                        .child(SharedString::from(name.clone())),
                                                )
                                                .when(running, |this| {
                                                    this.child(Spinner::new().xsmall())
                                                })
                                                .when(!running, |this| {
                                                    this.child(
                                                        Icon::new(IconName::CircleCheck)
                                                            .small()
                                                            .text_color(theme.success),
                                                    )
                                                })
                                                .when(!arg_summary.is_empty(), |this| {
                                                    this.child(
                                                        div()
                                                            .text_xs()
                                                            .text_color(theme.muted_foreground)
                                                            .text_ellipsis()
                                                            .child(SharedString::from(arg_summary)),
                                                    )
                                                }),
                                        )
                                        .when(!short_out.is_empty(), |this| {
                                            this.child(
                                                div()
                                                    .mt_1()
                                                    .rounded(px(4.))
                                                    .bg(theme.background)
                                                    .px_2()
                                                    .py_1()
                                                    .text_xs()
                                                    .font_family(theme.mono_font_family.clone())
                                                    .whitespace_normal()
                                                    .child(SharedString::from(short_out)),
                                            )
                                        }),
                                ),
                        );
                    }
                    block = block.child(tools_col);
                }

                block = block.child(div().min_h(px(24.)).when_some(body, |this, state| {
                    this.child(
                        TextView::new(&state)
                            .selectable(true)
                            .scrollable(false)
                            .text_sm(),
                    )
                }));

                if let Some(error) = error {
                    block = block.child(
                        h_flex()
                            .gap_2()
                            .items_start()
                            .rounded(theme.radius)
                            .border_1()
                            .border_color(theme.danger)
                            .px_3()
                            .py_2()
                            .child(Icon::new(IconName::CircleAlert).text_color(theme.danger))
                            .child(
                                div()
                                    .text_sm()
                                    .text_color(theme.danger)
                                    .whitespace_normal()
                                    .child(SharedString::from(error)),
                            ),
                    );
                }

                let mut meta: Vec<String> = Vec::new();
                if let Some(usage) = usage {
                    meta.push(format!(
                        "{} in · {} out",
                        format_tokens(u64::from(usage.input_tokens)),
                        format_tokens(u64::from(usage.output_tokens))
                    ));
                    if let Some(cost) = model.estimate_cost(&usage) {
                        meta.push(format_cost(cost));
                    }
                }
                if let Some(note) = message
                    .model
                    .as_deref()
                    .and_then(relay_core::find_model)
                    .map(|spec| spec.tagline.to_owned())
                {
                    meta.push(note);
                }

                block = block.child(
                    h_flex()
                        .gap_2()
                        .items_center()
                        .child(
                            div()
                                .text_xs()
                                .text_color(theme.muted_foreground)
                                .text_ellipsis()
                                .child(SharedString::from(meta.join(" · "))),
                        )
                        .when(!copy_text.trim().is_empty(), |this| {
                            this.child(
                                Button::new(SharedString::from(format!("copy-{}", message.id)))
                                    .ghost()
                                    .small()
                                    .icon(Icon::new(IconName::Copy))
                                    .tooltip("Copy response")
                                    .on_click(cx.listener(move |_, _, window, cx| {
                                        cx.write_to_clipboard(ClipboardItem::new_string(
                                            copy_text.clone(),
                                        ));
                                        window.push_notification("Copied to clipboard", cx);
                                    })),
                            )
                        }),
                );

                block.into_any_element()
            }
            Role::System => div()
                .text_sm()
                .child(SharedString::from(message.text()))
                .into_any_element(),
        }
    }

    fn render_composer(
        &mut self,
        _window: &mut Window,
        cx: &mut Context<Self>,
        composer: &gpui_kit::Entity<TextareaState>,
        can_send: bool,
        streaming: bool,
        notice: Option<String>,
    ) -> impl IntoElement {
        let theme = cx.theme().clone();
        let bundled_progress = self.bundled_status().progress;
        let is_chat = self.settings.interaction_mode == "chat";
        let mode = self.settings.agent_mode;
        let approval = self.settings.tool_approval;
        let outline = match mode {
            AgentMode::Plan => gpui_kit::rgb(0xeab308),  // yellow
            AgentMode::Build => gpui_kit::rgb(0x3b82f6), // blue
        };
        let mode_label = mode.label();
        let approval_select = self.chat.approval_select.clone();

        let pending = self.pending_approval.as_ref().map(|req| {
            (
                req.call.name.clone(),
                tool_arg_summary(&req.call.name, &req.call.arguments),
            )
        });

        v_flex()
            .flex_none()
            .px_6()
            .pb_5()
            .pt_2()
            .gap_2()
            .child(
                div()
                    .max_w(px(900.))
                    .w_full()
                    .mx_auto()
                    .gap_2()
                    .when_some(notice, |this, notice| {
                        this.child(
                            h_flex()
                                .gap_2()
                                .items_start()
                                .rounded(theme.radius)
                                .border_1()
                                .border_color(theme.danger)
                                .bg(theme.input)
                                .px_3()
                                .py_2()
                                .child(Icon::new(IconName::CircleAlert).text_color(theme.danger))
                                .child(
                                    div()
                                        .flex_1()
                                        .text_sm()
                                        .whitespace_normal()
                                        .child(SharedString::from(notice)),
                                )
                                .child(
                                    Button::new("notice-settings")
                                        .outline()
                                        .small()
                                        .label("Settings")
                                        .on_click(cx.listener(|this, _, window, cx| {
                                            this.open_settings(window, cx);
                                        })),
                                )
                                .child(
                                    Button::new("notice-dismiss")
                                        .ghost()
                                        .small()
                                        .icon(Icon::new(IconName::Close))
                                        .on_click(cx.listener(|this, _, _, cx| {
                                            this.notice = None;
                                            cx.notify();
                                        })),
                                ),
                        )
                    })
                    .when_some(bundled_progress, |this, pct| {
                        this.child(
                            div()
                                .w_full()
                                .max_w(px(900.))
                                .mx_auto()
                                .h(px(6.))
                                .rounded(px(3.))
                                .bg(theme.muted)
                                .overflow_hidden()
                                .child(
                                    div()
                                        .h_full()
                                        .w(gpui_kit::relative(pct as f32 / 100.0))
                                        .bg(theme.primary)
                                        .rounded(px(3.)),
                                ),
                        )
                    })
                    .when_some(pending, |this, (name, arg)| {
                        let detail = if arg.is_empty() { name.clone() } else { format!("{name}: {arg}") };
                        this.child(
                            div()
                                .rounded(theme.radius_lg)
                                .border_1()
                                .border_color(gpui_kit::rgb(0xf59e0b))
                                .bg(theme.input)
                                .p_3()
                                .child(
                                    v_flex()
                                        .gap_2()
                                        .child(
                                            h_flex()
                                                .gap_2()
                                                .items_center()
                                                .child(Icon::new(IconName::TriangleAlert).text_color(gpui_kit::rgb(0xf59e0b)))
                                                .child(div().text_sm().font_semibold().child("Tool approval required"))
                                                .child(div().text_xs().text_color(theme.muted_foreground).child(SharedString::from(detail))),
                                        )
                                        .child(
                                            h_flex()
                                                .gap_2()
                                                .child(
                                                    Button::new("approve-tool")
                                                        .primary()
                                                        .small()
                                                        .label("Allow")
                                                        .on_click(cx.listener(|this, _, _, cx| {
                                                            this.respond_tool_approval(true, cx);
                                                        })),
                                                )
                                                .child(
                                                    Button::new("deny-tool")
                                                        .danger()
                                                        .small()
                                                        .label("Deny")
                                                        .on_click(cx.listener(|this, _, _, cx| {
                                                            this.respond_tool_approval(false, cx);
                                                        })),
                                                ),
                                        ),
                                ),
                        )
                    })
                    .child(
                        v_flex()
                            .id("composer-box")
                            .rounded(theme.radius_lg)
                            .border_1()
                            .border_color(theme.border)
                            .when(!is_chat, |this| this.border_2().border_color(outline))
                            .bg(theme.input)
                            .overflow_hidden()
                            .when(self.chat.terminal_open, |this| {
                                let output = self
                                    .chat
                                    .terminal
                                    .as_ref()
                                    .map(|t| t.output.clone())
                                    .unwrap_or_default();
                                let shell = self
                                    .chat
                                    .terminal
                                    .as_ref()
                                    .map(|t| t.shell_name().to_owned())
                                    .unwrap_or_else(|| "bash".to_owned());
                                let cwd = self
                                    .chat
                                    .terminal
                                    .as_ref()
                                    .and_then(|t| t.cwd_label())
                                    .unwrap_or_else(|| "~".to_owned());
                                let running = self
                                    .chat
                                    .terminal
                                    .as_ref()
                                    .is_some_and(|t| !t.exited);
                                let term_input = self.chat.terminal_input.clone();
                                let term_scroll = self.chat.terminal_scroll.clone();
                                this.child(
                                    v_flex()
                                        .id("terminal-panel")
                                        .w_full()
                                        .bg(theme.background)
                                        .border_b_1()
                                        .border_color(theme.border)
                                        .child(
                                            h_flex()
                                                .h(px(34.))
                                                .pl_3()
                                                .pr_1()
                                                .gap_2()
                                                .items_center()
                                                .child(
                                                    h_flex()
                                                        .flex_1()
                                                        .min_w_0()
                                                        .gap_2()
                                                        .items_center()
                                                        .child(
                                                            div()
                                                                .size(px(7.))
                                                                .rounded_full()
                                                                .bg(if running {
                                                                    theme.success
                                                                } else {
                                                                    theme.muted_foreground
                                                                }),
                                                        )
                                                        .child(
                                                            Icon::new(IconName::Terminal)
                                                                .xsmall()
                                                                .text_color(theme.primary),
                                                        )
                                                        .child(
                                                            div()
                                                                .text_xs()
                                                                .font_semibold()
                                                                .child("Shell"),
                                                        )
                                                        .child(
                                                            div()
                                                                .min_w_0()
                                                                .text_xs()
                                                                .text_color(theme.muted_foreground)
                                                                .text_ellipsis()
                                                                .child(SharedString::from(format!(
                                                                    "{shell} · {cwd}"
                                                                ))),
                                                        ),
                                                )
                                                .child(
                                                    h_flex()
                                                        .gap_1()
                                                        .items_center()
                                                        .child(
                                                            Button::new("clear-terminal")
                                                                .ghost()
                                                                .xsmall()
                                                                .icon(Icon::new(IconName::Trash))
                                                                .tooltip("Clear output")
                                                                .on_click(cx.listener(
                                                                    |this, _, _window, cx| {
                                                                        this.clear_terminal(cx);
                                                                    },
                                                                )),
                                                        )
                                                        .child(
                                                            Button::new("close-terminal")
                                                                .ghost()
                                                                .xsmall()
                                                                .icon(Icon::new(IconName::X))
                                                                .tooltip("Close shell")
                                                                .on_click(cx.listener(
                                                                    |this, _, window, cx| {
                                                                        this.close_terminal(window, cx);
                                                                    },
                                                                )),
                                                        ),
                                                ),
                                        )
                                        .child(
                                            div()
                                                .id("terminal-output")
                                                .h(px(150.))
                                                .min_h_0()
                                                .overflow_y_scroll()
                                                .track_scroll(&term_scroll)
                                                .px_3()
                                                .pb_2()
                                                .child(if output.trim().is_empty() {
                                                    div()
                                                        .text_xs()
                                                        .font_family(
                                                            theme.mono_font_family.clone(),
                                                        )
                                                        .text_color(theme.muted_foreground)
                                                        .child(SharedString::from(format!(
                                                            "{shell} — ready. This is your real shell; commands run locally as you."
                                                        )))
                                                } else {
                                                    div()
                                                        .text_xs()
                                                        .font_family(
                                                            theme.mono_font_family.clone(),
                                                        )
                                                        .text_color(theme.foreground)
                                                        .child(SharedString::from(output))
                                                }),
                                        )
                                        .child(
                                            h_flex()
                                                .mx_3()
                                                .mb_3()
                                                .px_2()
                                                .py_1()
                                                .gap_2()
                                                .items_center()
                                                .rounded(theme.radius)
                                                .border_1()
                                                .border_color(theme.border)
                                                .bg(theme.input)
                                                .child(
                                                    div()
                                                        .text_sm()
                                                        .font_family(
                                                            theme.mono_font_family.clone(),
                                                        )
                                                        .text_color(theme.primary)
                                                        .child("❯"),
                                                )
                                                .child(
                                                    Input::new(&term_input)
                                                        .flex_1()
                                                        .small()
                                                        .appearance(false),
                                                ),
                                        ),
                                )
                            })
                            .child(
                                v_flex()
                                    .p_2()
                                    .gap_2()
                                    .on_key_down(cx.listener(
                                        |this, event: &KeyDownEvent, window, cx| {
                                            let ks = &event.keystroke;
                                            if ks.modifiers.control
                                                && !ks.modifiers.shift
                                                && ks.key == "tab"
                                            {
                                                window.prevent_default();
                                                cx.stop_propagation();
                                                this.toggle_agent_mode(window, cx);
                                            }
                                        },
                                    ))
                                    .when(!is_chat, |this| {
                                        this.child(
                                            h_flex()
                                                .gap_2()
                                                .items_center()
                                                .px_1()
                                                .child({

                                                let mut btn = Button::new("mode-toggle")
                                                    .small()
                                                    .label(SharedString::from(format!(
                                                        "{mode_label} · Ctrl+Tab"
                                                    )))
                                                    .tooltip(if mode == AgentMode::Plan {
                                                        "Plan: read-only tools only"
                                                    } else {
                                                        "Build: can run tools with approval"
                                                    })
                                                    .on_click(cx.listener(
                                                        |this, _, window, cx| {
                                                            this.toggle_agent_mode(window, cx);
                                                        },
                                                    ));
                                                btn = if mode == AgentMode::Plan {
                                                    btn.ghost()
                                                } else {
                                                    btn.outline()
                                                };
                                                btn
                                            })
                                            .child(
                                                div()
                                                    .w(px(220.))
                                                    .flex_shrink_0()
                                                    .child(Select::new(&approval_select).w_full()),
                                            )
                                            .when(
                                                mode == AgentMode::Build
                                                    && approval == ToolApproval::All,
                                                |this| {
                                                    this.child(
                                                        h_flex()
                                                            .gap_1()
                                                            .items_center()
                                                            .flex_1()
                                                            .min_w_0()
                                                            .child(
                                                                Icon::new(
                                                                    IconName::TriangleAlert,
                                                                )
                                                                .xsmall()
                                                                .text_color(theme.danger),
                                                            )
                                                            .child(
                                                                div()
                                                                    .text_xs()
                                                                    .text_color(theme.danger)
                                                                    .text_ellipsis()
                                                                    .child("Auto-approve all: destructive commands may run without approval (prompt injection risk)."),
                                                            ),
                                                    )
                                                },
                                            ),
                                    )
                                    })
                                    .child(Textarea::new(composer).appearance(false).bordered(false))
                                    .child(
                                        h_flex()
                                            .justify_end()
                                            .items_center()
                                            .child(
                                                h_flex()
                                                    .gap_0p5()
                                                    .items_center()
                                                    .p_0p5()
                                                    .rounded(theme.radius)
                                                    .border_1()
                                                    .border_color(theme.border)
                                                    .bg(theme.background)
                                                    .when(!is_chat, |this| {
                                                        this.child(
                                                            Toggle::new("shell-toggle")
                                                                .icon(
                                                                    Icon::new(IconName::Terminal)
                                                                        .small(),
                                                                )
                                                                .label("Shell")
                                                                .checked(self.chat.terminal_open)
                                                                .tooltip("Toggle shell · Ctrl+`")
                                                                .on_click(cx.listener(
                                                                    |this, _, window, cx| {
                                                                        this.toggle_terminal(window, cx);
                                                                    },
                                                                )),
                                                        )
                                                        .child(
                                                            div()
                                                                .w(px(1.))
                                                                .h(px(14.))
                                                                .mx_0p5()
                                                                .bg(theme.border),
                                                        )
                                                    })
                                                    .child(if streaming {
                                                        Button::new("stop")
                                                            .danger()
                                                            .icon(Icon::new(IconName::Square))
                                                            .tooltip("Stop generating")
                                                            .on_click(cx.listener(
                                                                |this, _, _, cx| {
                                                                    this.stop_stream(cx);
                                                                },
                                                            ))
                                                            .into_any_element()
                                                    } else {
                                                        Button::new("send")
                                                            .primary()
                                                            .icon(Icon::new(IconName::ArrowUp))
                                                            .tooltip("Send")
                                                            .disabled(!can_send)
                                                            .on_click(cx.listener(
                                                                |this, _, window, cx| {
                                                                    this.send(window, cx);
                                                                },
                                                            ))
                                                            .into_any_element()
                                                    }),
                                            ),
                                    ),
                            ),
                    ),
            )
    }

    fn copy_conversation(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let markdown = match self.conversation_id.as_deref() {
            Some(id) => self.store.export_markdown(id).unwrap_or_else(|error| {
                tracing::warn!(%error, "could not export conversation");
                relay_core::conversation_markdown(&self.conversation_title, &self.messages)
            }),
            None => relay_core::conversation_markdown(&self.conversation_title, &self.messages),
        };
        if markdown.trim().is_empty() {
            return;
        }
        cx.write_to_clipboard(ClipboardItem::new_string(markdown));
        window.push_notification("Conversation copied as Markdown", cx);
    }
}

fn suggestion(
    prompt: &'static str,
    title: &'static str,
    detail: &'static str,
    theme: &gpui_kit::component::Theme,
    cx: &mut Context<RelayApp>,
) -> AnyElement {
    div()
        .id(title)
        .w(px(306.))
        .rounded(theme.radius_lg)
        .border_1()
        .border_color(theme.border)
        .bg(theme.input)
        .p_3()
        .cursor_pointer()
        .hover(|style| style.border_color(theme.primary))
        .on_click(cx.listener(move |this, _, window, cx| {
            this.set_prompt(prompt.to_owned(), window, cx);
        }))
        .child(
            v_flex()
                .gap_1()
                .child(div().text_sm().font_medium().child(title))
                .child(
                    div()
                        .text_xs()
                        .text_color(theme.muted_foreground)
                        .child(detail),
                ),
        )
        .into_any_element()
}

fn estimate_message_tokens(message: &ChatMessage) -> u64 {
    let mut tokens = u64::from(estimate_tokens(&message.text())) + 4;
    for block in &message.blocks {
        match block {
            ContentBlock::ToolUse { arguments, .. } => {
                tokens += u64::from(estimate_tokens(&arguments.to_string()));
            }
            ContentBlock::ToolResult { output, .. } => {
                tokens += u64::from(estimate_tokens(output));
            }
            _ => {}
        }
    }
    tokens
}

fn tool_arg_summary(name: &str, args: &serde_json::Value) -> String {
    match name {
        "run_command" => args
            .get("command")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .trim()
            .to_owned(),
        "read_file" | "list_dir" | "write_file" => args
            .get("path")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .trim()
            .to_owned(),
        _ => {
            let s = args.to_string();
            if s == "{}" || s == "null" {
                String::new()
            } else {
                truncate_preview(&s, 80)
            }
        }
    }
}

fn truncate_preview(s: &str, max: usize) -> String {
    let s = s.trim();
    if s.chars().count() <= max {
        return s.to_owned();
    }
    let mut out: String = s.chars().take(max).collect();
    out.push('…');
    out
}

fn format_cost(cost: f64) -> String {
    if cost >= 1.0 {
        format!("${cost:.2}")
    } else if cost >= 0.01 {
        format!("${cost:.3}")
    } else {
        format!("${cost:.5}")
    }
}

fn guarantee_item(
    title: &'static str,
    detail: &'static str,
    theme: &gpui_kit::component::Theme,
) -> gpui_kit::Div {
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
                .child(div().text_xs().font_medium().child(title))
                .child(
                    div()
                        .text_xs()
                        .text_color(theme.muted_foreground)
                        .child(detail),
                ),
        )
}
