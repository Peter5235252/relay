//! Conversation sidebar: search, grouped history, and settings entry.

use chrono::{Local, Utc};
use gpui_kit::assets::IconName;
use gpui_kit::component::{
    ActiveTheme as _, Icon, Sizable as _, StyledExt as _, WindowExt as _,
    button::{Button, ButtonVariants as _},
    h_flex,
    input::{Input, InputState},
    v_flex,
};
use gpui_kit::prelude::{FluentBuilder as _, StatefulInteractiveElement as _};
use gpui_kit::{
    AppContext as _, Context, InteractiveElement as _, IntoElement, ParentElement as _, SharedString,
    Styled as _, Window, div, px,
};
use relay_core::{ConversationSummary, find_model};

use crate::app::RelayApp;

impl RelayApp {
    pub(crate) fn render_sidebar(
        &mut self,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let theme = cx.theme().clone();
        let search = self.sidebar.search.clone();
        let conversations = self.conversations.clone();
        let current = self.conversation_id.clone();

        let mut list = v_flex().gap_0p5().px_2().pb_4();
        let mut last_group = "";
        for summary in &conversations {
            let group = group_label(summary.updated_at);
            if group != last_group {
                last_group = group;
                list = list.child(
                    div()
                        .px_2()
                        .pt_4()
                        .pb_1()
                        .text_xs()
                        .font_semibold()
                        .text_color(theme.muted_foreground)
                        .child(group),
                );
            }
            list = list.child(self.render_conversation_row(
                summary,
                current.as_deref() == Some(summary.id.as_str()),
                &theme,
                cx,
            ));
        }

        if conversations.is_empty() {
            let message = if self.sidebar.search.read(cx).value().trim().is_empty() {
                "No conversations yet. Start one below."
            } else {
                "No conversations match your search."
            };
            list = list.child(
                div()
                    .px_3()
                    .py_6()
                    .text_sm()
                    .text_color(theme.muted_foreground)
                    .child(message),
            );
        }

        v_flex()
            .w(px(286.))
            .h_full()
            .flex_shrink_0()
            .bg(theme.sidebar)
            .border_r_1()
            .border_color(theme.sidebar_border)
            .child(
                v_flex()
                    .gap_3()
                    .p_3()
                    .child(
                        h_flex()
                            .justify_between()
                            .items_center()
                            .child(
                                div()
                                    .text_sm()
                                    .font_semibold()
                                    .text_color(theme.sidebar_foreground)
                                    .child("Chats"),
                            )
                            .child(
                                h_flex()
                                    .gap_1()
                                    .child(
                                        Button::new("bulk-select")
                                            .ghost()
                                            .small()
                                            .icon(Icon::new(IconName::Check))
                                            .tooltip("Select multiple")
                                            .on_click(cx.listener(|this, _, _, cx| {
                                                this.bulk_select_mode = !this.bulk_select_mode;
                                                if !this.bulk_select_mode {
                                                    this.selected_conversations.clear();
                                                }
                                                cx.notify();
                                            })),
                                    )
                                    .child(
                                        Button::new("export-all")
                                            .ghost()
                                            .small()
                                            .icon(Icon::new(IconName::Download))
                                            .tooltip("Export all conversations")
                                            .on_click(cx.listener(|this, _, window, cx| {
                                                if let Ok(md) = this.store.export_all_markdown() {
                                                    let path = this.store.path().parent().unwrap_or(std::path::Path::new(".")).join("relay-export-all.md");
                                                    let _ = std::fs::write(&path, md);
                                                    window.push_notification(format!("Exported to {}", path.display()), cx);
                                                }
                                            })),
                                    )
                                    .child(
                                        Button::new("new-chat")
                                            .ghost()
                                            .small()
                                            .icon(Icon::new(IconName::Plus))
                                            .tooltip("New chat")
                                            .on_click(cx.listener(|this, _, window, cx| {
                                                this.new_chat(window, cx);
                                            })),
                                    ),
                            ),
                    )
                    .child(
                        Input::new(&search)
                            .prefix(Icon::new(IconName::Search))
                            .cleanable(true),
                    )
                    .when(self.bulk_select_mode, |this| {
                        this.child(
                            h_flex()
                                .gap_2()
                                .child(
                                    Button::new("delete-selected")
                                        .danger()
                                        .small()
                                        .label(format!("Delete ({})", self.selected_conversations.len()))
                                        .on_click(cx.listener(|this, _, window, cx| {
                                            for id in this.selected_conversations.clone() {
                                                let _ = this.store.delete_conversation(&id);
                                            }
                                            this.selected_conversations.clear();
                                            this.bulk_select_mode = false;
                                            this.refresh_conversations(cx);
                                            window.push_notification("Deleted selected", cx);
                                            cx.notify();
                                        })),
                                )
                                .child(
                                    Button::new("cancel-select")
                                        .ghost()
                                        .small()
                                        .label("Cancel")
                                        .on_click(cx.listener(|this, _, _, cx| {
                                            this.bulk_select_mode = false;
                                            this.selected_conversations.clear();
                                            cx.notify();
                                        })),
                                ),
                        )
                    }),
            )
            .child(
                div()
                    .id("conversation-list")
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .track_scroll(&self.sidebar.scroll)
                    .child(list),
            )
            .child(
                v_flex()
                    .p_2()
                    .border_t_1()
                    .border_color(theme.sidebar_border)
                    .child(
                        Button::new("open-settings")
                            .ghost()
                            .w_full()
                            .justify_start()
                            .icon(Icon::new(IconName::Settings))
                            .label("Settings")
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.open_settings(window, cx);
                            })),
                    )
                    .child(
                        div()
                            .px_2()
                            .pt_1()
                            .text_xs()
                            .text_color(theme.muted_foreground)
                            .child(SharedString::from(format!(
                                "Relay {} · Apache-2.0",
                                relay_core::VERSION
                            ))),
                    ),
            )
    }

    fn render_conversation_row(
        &self,
        summary: &ConversationSummary,
        selected: bool,
        theme: &gpui_kit::component::Theme,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let id = summary.id.clone();
        let delete_id = summary.id.clone();
        let group = format!("conversation-{}", summary.id);
        let model = find_model(&summary.model_slug)
            .map(|spec| spec.name.to_owned())
            .unwrap_or_else(|| summary.model_label());
        let subtitle = summary
            .preview
            .as_deref()
            .map(str::trim)
            .filter(|preview| !preview.is_empty())
            .map(|preview| preview.chars().take(90).collect::<String>())
            .unwrap_or_else(|| model.clone());

        h_flex()
            .id(SharedString::from(format!("row-{}", summary.id)))
            .group(group.clone())
            .gap_1()
            .items_start()
            .rounded(theme.radius)
            .px_2()
            .py_1p5()
            .when(selected, |this| this.bg(theme.sidebar_accent))
            .when(!selected, |this| {
                this.hover(|style| style.bg(theme.list_hover))
            })
            .cursor_pointer()
            .on_click(cx.listener(move |this, _: &gpui_kit::ClickEvent, window, cx| {
                if this.bulk_select_mode {
                    let id = id.clone();
                    if this.selected_conversations.contains(&id) {
                        this.selected_conversations.remove(&id);
                    } else {
                        this.selected_conversations.insert(id);
                    }
                    cx.notify();
                    return;
                }
                this.open_conversation(id.clone(), window, cx);
            }))
            .when(self.bulk_select_mode, |this| {
                let id = summary.id.clone();
                this.child(
                    gpui_kit::component::switch::Switch::new(SharedString::from(format!("sel-{}", summary.id)))
                        .checked(self.selected_conversations.contains(&id))
                        .on_click(cx.listener(move |this, _, _, cx| {
                            if this.selected_conversations.contains(&id) {
                                this.selected_conversations.remove(&id);
                            } else {
                                this.selected_conversations.insert(id.clone());
                            }
                            cx.notify();
                        })),
                )
            })
            .child(
                v_flex()
                    .flex_1()
                    .min_w_0()
                    .gap_0p5()
                    .child(if self.renaming_conversation.as_ref().map(|(id, _)| id) == Some(&summary.id) {
                        let input = self.renaming_conversation.as_ref().unwrap().1.clone();
                        let id = summary.id.clone();
                        h_flex()
                            .gap_1()
                            .child(Input::new(&input).flex_1())
                            .child(
                                Button::new("save-rename")
                                    .ghost()
                                    .xsmall()
                                    .icon(Icon::new(IconName::Check))
                                    .on_click(cx.listener(move |this, _, window, cx| {
                                        let new_title = input.read(cx).value().trim().to_owned();
                                        if !new_title.is_empty() {
                                            let _ = this.store.rename_conversation(&id, &new_title);
                                        }
                                        this.renaming_conversation = None;
                                        this.refresh_conversations(cx);
                                        cx.notify();
                                    })),
                            )
                            .into_any_element()
                    } else {
                        div()
                            .text_sm()
                            .when(selected, |this| this.font_medium())
                            .text_color(if selected {
                                theme.sidebar_accent_foreground
                            } else {
                                theme.sidebar_foreground
                            })
                            .text_ellipsis()
                            .child(SharedString::from(summary.title.clone()))
                            .into_any_element()
                    })
                    .child(
                        div()
                            .text_xs()
                            .text_color(theme.muted_foreground)
                            .text_ellipsis()
                            .child(SharedString::from(subtitle)),
                    ),
            )
            .child(
                h_flex()
                    .gap_0p5()
                    .opacity(if selected { 1.0 } else { 0.0 })
                    .group_hover(group, |style| style.opacity(1.0))
                    .child(
                        Button::new(SharedString::from(format!("pin-{}", summary.id)))
                            .ghost()
                            .xsmall()
                            .icon(Icon::new(if summary.pinned { IconName::Pin } else { IconName::Pin }))
                            .tooltip(if summary.pinned { "Unpin" } else { "Pin" })
                            .on_click(cx.listener({
                                let id = summary.id.clone();
                                let pinned = summary.pinned;
                                move |this, _: &gpui_kit::ClickEvent, _, cx| {
                                    cx.stop_propagation();
                                    let _ = this.store.set_pinned(&id, !pinned);
                                    this.refresh_conversations(cx);
                                }
                            })),
                    )
                    .child(
                        Button::new(SharedString::from(format!("archive-{}", summary.id)))
                            .ghost()
                            .xsmall()
                            .icon(Icon::new(IconName::Archive))
                            .tooltip("Archive")
                            .on_click(cx.listener({
                                let id = summary.id.clone();
                                move |this, _: &gpui_kit::ClickEvent, _, cx| {
                                    cx.stop_propagation();
                                    let _ = this.store.set_archived(&id, true);
                                    this.refresh_conversations(cx);
                                }
                            })),
                    )
                    .child(
                        Button::new(SharedString::from(format!("rename-{}", summary.id)))
                            .ghost()
                            .xsmall()
                            .icon(Icon::new(IconName::Pencil))
                            .tooltip("Rename")
                            .on_click(cx.listener({
                                let id = summary.id.clone();
                                let title = summary.title.clone();
                                move |this, _: &gpui_kit::ClickEvent, window, cx| {
                                    cx.stop_propagation();
                                    let input = cx.new(|cx| {
                                        InputState::new(window, cx).placeholder("New title…")
                                    });
                                    input.update(cx, |s, cx| s.set_value(title.clone(), window, cx));
                                    this.renaming_conversation = Some((id.clone(), input));
                                    cx.notify();
                                }
                            })),
                    )
                    .child(
                        Button::new(SharedString::from(format!("export-{}", summary.id)))
                            .ghost()
                            .xsmall()
                            .icon(Icon::new(IconName::Download))
                            .tooltip("Export")
                            .on_click(cx.listener({
                                let id = summary.id.clone();
                                let title = summary.title.clone();
                                move |this, _: &gpui_kit::ClickEvent, window, cx| {
                                    cx.stop_propagation();
                                    if let Ok(md) = this.store.export_markdown(&id) {
                                        let path = this.store.path().parent().unwrap_or(std::path::Path::new(".")).join(format!("{}.md", title.replace('/', "-")));
                                        let _ = std::fs::write(&path, md);
                                        window.push_notification(format!("Exported to {}", path.display()), cx);
                                    }
                                }
                            })),
                    )
                    .child(
                        Button::new(SharedString::from(format!("delete-{}", summary.id)))
                            .ghost()
                            .xsmall()
                            .icon(Icon::new(IconName::Trash))
                            .tooltip("Delete conversation")
                            .on_click(cx.listener({
                                let delete_id = summary.id.clone();
                                move |this, _: &gpui_kit::ClickEvent, window, cx| {
                                    cx.stop_propagation();
                                    this.confirm_delete_conversation(delete_id.clone(), window, cx);
                                }
                            })),
                    ),
            )
    }
}

fn group_label(updated_at: chrono::DateTime<Utc>) -> &'static str {
    let today = Local::now().date_naive();
    let day = updated_at.with_timezone(&Local).date_naive();
    let age = today.signed_duration_since(day).num_days();
    match age {
        i64::MIN..=0 => "Today",
        1 => "Yesterday",
        2..=7 => "Previous 7 days",
        _ => "Older",
    }
}
