#![allow(clippy::expect_used, clippy::unwrap_used)]

use relay_core::{
    AppSettings, ChatMessage, ContentBlock, PersonalityPreset, ReasoningEffort, Usage,
};
use relay_store::Store;

fn store() -> Store {
    Store::open_in_memory().expect("open in-memory store")
}

#[test]
fn builtins_are_seeded_and_ordered() {
    let store = store();
    let presets = store.presets().expect("presets");
    assert_eq!(presets.len(), 6);
    assert!(presets.iter().all(|preset| preset.builtin));
    assert_eq!(presets[0].id, "balanced");
    assert_eq!(presets[5].id, "engineer");
}

#[test]
fn creating_and_listing_conversations_works() {
    let store = store();
    let id = store
        .create_conversation("claude-sonnet-5-5")
        .expect("create conversation");
    let summary = store.conversation(&id).expect("load conversation");
    assert_eq!(summary.title, "New chat");
    assert_eq!(summary.model_slug, "claude-sonnet-5-5");
    assert_eq!(summary.message_count, 0);

    let stored = store
        .append_message(
            &id,
            &ChatMessage::user("Explain SQLite WAL mode in two paragraphs."),
        )
        .expect("append");
    assert_eq!(stored.position, 0);
    assert_eq!(
        stored.title.as_deref(),
        Some("Explain SQLite WAL mode in two paragraphs.")
    );

    let summaries = store.list_conversations().expect("list");
    assert_eq!(summaries.len(), 1);
    assert_eq!(summaries[0].message_count, 1);
    assert_eq!(
        summaries[0].preview.as_deref(),
        Some("Explain SQLite WAL mode in two paragraphs.")
    );
}

#[test]
fn messages_round_trip_with_blocks_and_usage() {
    let store = store();
    let id = store
        .create_conversation("gpt-6.1-sol")
        .expect("create conversation");
    store
        .append_message(&id, &ChatMessage::user("Hello"))
        .expect("user message");

    let mut assistant = ChatMessage::assistant(
        "gpt-6.1-sol",
        vec![ContentBlock::text("Hi there")],
        Some(Usage::new(12, 4, 0, 2)),
    );
    assistant.append_thinking("A short internal note", "openai");
    assistant.error = Some("interrupted".to_owned());
    store
        .append_message(&id, &assistant)
        .expect("assistant message");

    let messages = store.messages(&id).expect("messages");
    assert_eq!(messages.len(), 2);
    assert_eq!(messages[0].role, relay_core::Role::User);
    assert_eq!(messages[1].text(), "Hi there");
    assert_eq!(messages[1].thinking(), "A short internal note");
    assert_eq!(messages[1].usage, Some(Usage::new(12, 4, 0, 2)));
    assert_eq!(messages[1].error.as_deref(), Some("interrupted"));
    assert_eq!(messages[1].model.as_deref(), Some("gpt-6.1-sol"));
}

#[test]
fn full_text_search_finds_message_text_and_titles() {
    let store = store();
    let first = store
        .create_conversation("claude-opus-5-5")
        .expect("conversation");
    store
        .append_message(
            &first,
            &ChatMessage::user("The borrow checker rejected my iterator."),
        )
        .expect("message");

    let second = store
        .create_conversation("claude-sonnet-5-5")
        .expect("conversation");
    store
        .append_message(&second, &ChatMessage::user("Kubernetes ingress notes"))
        .expect("message");

    let hits = store.search_conversations("borrow").expect("search");
    assert_eq!(hits.len(), 1);
    assert_eq!(hits[0].id, first);

    let hits = store.search_conversations("kubernetes").expect("search");
    assert_eq!(hits.len(), 1);
    assert_eq!(hits[0].id, second);

    let hits = store
        .search_conversations("does-not-exist")
        .expect("search");
    assert!(hits.is_empty());

    // Operators and punctuation must never produce an FTS syntax error.
    assert!(!store.search_conversations("OR").is_err());
    assert!(!store.search_conversations("\" * ( )").is_err());
}

#[test]
fn search_can_match_conversation_titles() {
    let store = store();
    let id = store
        .create_conversation("gemini-4-hydrogen")
        .expect("conversation");
    store
        .rename_conversation(&id, "Wayland flicker notes")
        .expect("rename");
    let hits = store.search_conversations("wayland").expect("search");
    assert_eq!(hits.len(), 1);
    assert_eq!(hits[0].id, id);
}

#[test]
fn deleting_a_conversation_removes_messages_from_search() {
    let store = store();
    let id = store
        .create_conversation("gemini-4-argon")
        .expect("conversation");
    store
        .append_message(&id, &ChatMessage::user("ephemeral search token"))
        .expect("message");
    assert_eq!(
        store
            .search_conversations("ephemeral")
            .expect("search")
            .len(),
        1
    );
    store.delete_conversation(&id).expect("delete");
    assert!(
        store
            .search_conversations("ephemeral")
            .expect("search")
            .is_empty()
    );
    assert_eq!(store.stats().expect("stats").messages, 0);
}

#[test]
fn settings_round_trip_and_normalize() {
    let store = store();
    let defaults = store.load_settings().expect("defaults");
    assert_eq!(defaults.reasoning, ReasoningEffort::Medium);

    let mut settings = AppSettings {
        custom_instructions: "Prefer short answers.".to_owned(),
        reasoning: ReasoningEffort::High,
        temperature: 1.4,
        send_on_enter: false,
        appearance: relay_core::Appearance::Dark,
        ..Default::default()
    };
    settings.normalize();
    store.save_settings(&settings).expect("save");
    let loaded = store.load_settings().expect("load");
    assert_eq!(loaded, settings);
}

#[test]
fn user_presets_can_be_created_updated_and_deleted() {
    let store = store();
    let mut preset = PersonalityPreset {
        id: "3d85a3c2-4ad4-4b0e-a3d8-21fdc338db2d".to_owned(),
        name: "Research memo".to_owned(),
        description: "Structured research notes".to_owned(),
        instructions: "Separate evidence from inference.".to_owned(),
        builtin: false,
    };
    store.save_preset(&preset).expect("save preset");
    let presets = store.presets().expect("presets");
    assert_eq!(presets.len(), 7);
    assert!(presets.iter().any(|item| item.id == preset.id));

    preset.name = "Research brief".to_owned();
    store.save_preset(&preset).expect("update preset");
    assert!(
        store
            .presets()
            .expect("presets")
            .iter()
            .any(|item| item.name == "Research brief")
    );

    store.delete_preset(&preset.id).expect("delete preset");
    assert_eq!(store.presets().expect("presets").len(), 6);

    let builtin = relay_core::find_builtin_preset("concise").expect("builtin");
    assert!(store.save_preset(&builtin).is_err());
    assert!(store.delete_preset("concise").is_err());
}

#[test]
fn markdown_export_contains_roles_and_metadata() {
    let store = store();
    let id = store
        .create_conversation("claude-fable-5-2")
        .expect("conversation");
    store
        .append_message(&id, &ChatMessage::user("Write a haiku"))
        .expect("user");
    store
        .append_message(
            &id,
            &ChatMessage::assistant(
                "claude-fable-5-2",
                vec![ContentBlock::text(
                    "Silent router hums\nmodels answer one by one\ncontext stays intact",
                )],
                Some(Usage::new(10, 20, 0, 0)),
            ),
        )
        .expect("assistant");
    let export = store.export_markdown(&id).expect("export");
    assert!(export.contains("Write a haiku"));
    assert!(export.contains("## Relay · claude-fable-5-2"));
    assert!(export.contains("Silent router hums"));
}

#[test]
fn stats_count_conversations_messages_and_tokens() {
    let store = store();
    let id = store
        .create_conversation("gpt-6-luna")
        .expect("conversation");
    store
        .append_message(&id, &ChatMessage::user("hello"))
        .expect("user");
    store
        .append_message(
            &id,
            &ChatMessage::assistant(
                "gpt-6-luna",
                vec![ContentBlock::text("hi")],
                Some(Usage::new(100, 50, 0, 0)),
            ),
        )
        .expect("assistant");
    let stats = store.stats().expect("stats");
    assert_eq!(stats.conversations, 1);
    assert_eq!(stats.messages, 2);
    assert_eq!(stats.tokens, 150);
}
