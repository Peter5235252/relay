//! Sample data for `relay --demo`.
//!
//! This is intentionally not a fake provider: it writes ordinary messages to an
//! in-memory store so the real UI, Markdown renderer, sidebar, model picker, and
//! search can be reviewed without network access or API keys.

use relay_core::{ChatMessage, ContentBlock, Usage};

use crate::{Result, Store};

/// Seed a small, realistic conversation set and return the conversation that
/// should be opened first.
pub fn seed_demo(store: &Store) -> Result<String> {
    let main = store.create_conversation("claude-sonnet-5-5")?;
    append(
        store,
        &main,
        ChatMessage::user(
            "We're adding offline search to a note-taking app with about 200k notes. \
             Compare SQLite FTS5 with a Tantivy sidecar, then recommend one.",
        ),
    )?;

    let mut analyst = ChatMessage::assistant(
        "claude-sonnet-5-5",
        vec![
            ContentBlock::thinking(
                "The user has a moderate corpus (200k notes), a desktop target, and an existing \
                 SQLite dependency. FTS5 keeps transactions and search in one file; Tantivy offers \
                 better ranking and fuzzy matching but adds an index lifecycle. Recommend FTS5, \
                 with a ranked LIKE fallback for short prefixes.",
                "claude",
            ),
            ContentBlock::text(
                "## Recommendation\n\n\
                 **Use SQLite FTS5.** At 200k notes, it is comfortably inside FTS5's sweet spot, \
                 and keeping the index in the same database as your notes removes an entire class \
                 of consistency bugs.\n\n\
                 | | SQLite FTS5 | Tantivy sidecar |\n\
                 |---|---|---|\n\
                 | Consistency | Same transaction | Rebuild/reconcile loop |\n\
                 | Ranking | BM25 | BM25 + richer queries |\n\
                 | Binary size | Already linked | +2–4 MB |\n\
                 | Fuzzy matching | Prefix only | Strong |\n\n\
                 ### Shape I'd build\n\n\
                 ```sql\n\
                 CREATE VIRTUAL TABLE notes_fts USING fts5(\n\
                     title, body, note_id UNINDEXED,\n\
                     tokenize = 'unicode61 remove_diacritics 2'\n\
                 );\n\
                 ```\n\n\
                 - Keep FTS rows in sync with triggers inside the same transaction.\n\
                 - Debounce index writes during bulk import; wrap 500–2,000 notes per transaction.\n\
                 - Use `bm25(notes_fts, 10.0, 1.0)` so title matches outrank body matches.\n\
                 - Add a small in-memory prefix cache for instant results while the user types.\n\n\
                 Tantivy becomes the better answer when you need typo tolerance, phrase slop, or \
                 multi-language analyzers. You can add it later without changing the schema if you \
                 keep search behind a `SearchIndex` trait.",
            ),
        ],
        Some(Usage::new(1_840, 612, 1_024, 388)),
    );
    analyst
        .blocks
        .retain(|block| !matches!(block, ContentBlock::Unknown));
    append(store, &main, analyst)?;

    append(
        store,
        &main,
        ChatMessage::user("Now switch to GPT-6.1 Sol and turn that into a two-week plan."),
    )?;

    append(
        store,
        &main,
        ChatMessage::assistant(
            "gpt-6.1-sol",
            vec![ContentBlock::text(
                "## Two-week plan\n\n\
                 ### Week 1 — correctness first\n\n\
                 - **Day 1:** Add FTS5 migration, triggers, and a `rebuild_fts` command.\n\
                 - **Day 2:** Benchmark import and first-query latency on a 200k-note fixture.\n\
                 - **Day 3:** Implement title-weighted BM25 and snippet highlighting.\n\
                 - **Day 4:** Debounce indexing during edits; add crash-recovery test.\n\
                 - **Day 5:** Wire `SearchIndex` trait and measure memory at idle.\n\n\
                 ### Week 2 — product quality\n\n\
                 - **Day 6–7:** Incremental prefix results and keyboard navigation.\n\
                 - **Day 8:** Add filters (tag, folder, modified date) to the query AST.\n\
                 - **Day 9:** Profile worst-case queries; cap FTS `LIMIT` and add a result cache.\n\
                 - **Day 10:** Ship behind a flag, collect p50/p95 latency, and write the rollback note.\n\n\
                 > **Exit criteria:** p95 under 40 ms for a two-word query on 200k notes, zero \
                 orphaned FTS rows after 10k random edits, and no visible jank while typing.",
            )],
            Some(Usage::new(2_460, 344, 1_920, 96)),
        ),
    )?;

    let side = store.create_conversation("gemini-4-hydrogen")?;
    append(
        store,
        &side,
        ChatMessage::user(
            "Draft a short release note for the search update. Keep it under 120 words.",
        ),
    )?;
    append(
        store,
        &side,
        ChatMessage::assistant(
            "gemini-4-hydrogen",
            vec![ContentBlock::text(
                "**Search, rebuilt.** Notes now open with instant full-text results, title-weighted \
                 ranking, and highlighted matches. The index lives in the same SQLite transaction \
                 as your notes, so search never drifts out of sync, even after a crash. Results \
                 update as you type, and filters for tags and folders are available from the search \
                 bar. Existing notes are indexed automatically on first launch; large libraries may \
                 take a minute to finish in the background.",
            )],
            Some(Usage::new(96, 74, 0, 0)),
        ),
    )?;

    let third = store.create_conversation("claude-opus-5-5")?;
    append(
        store,
        &third,
        ChatMessage::user("Explain why my Wayland app flickers only on a 144 Hz display."),
    )?;
    append(
        store,
        &third,
        ChatMessage::assistant(
            "claude-opus-5-5",
            vec![ContentBlock::text(
                "Flicker that appears only at 144 Hz usually means the compositor is presenting a \
                 frame before the renderer has committed the new buffer, or the app is repainting \
                 on a timer that does not match the refresh cadence. Check three things: whether \
                 you request frame callbacks instead of sleeping, whether partial damage is being \
                 used correctly, and whether the swapchain has at least two buffers.",
            )],
            Some(Usage::new(410, 92, 0, 0)),
        ),
    )?;

    // Make the main conversation the most recent one.
    store.append_message(
        &main,
        &ChatMessage::user("Thanks — start with Day 1 and show me the migration."),
    )?;

    Ok(main)
}

fn append(store: &Store, conversation: &str, message: ChatMessage) -> Result<()> {
    store.append_message(conversation, &message)?;
    Ok(())
}
