#![allow(clippy::expect_used, clippy::unwrap_used)]

use std::{fs, path::Path};

use relay_core::ChatMessage;
use relay_store::{DatabaseKey, Store, StoreError};
use tempfile::tempdir;

const SQLITE_HEADER: &[u8] = b"SQLite format 3\0";

fn key(byte: u8) -> DatabaseKey {
    DatabaseKey::new([byte; 32])
}

fn contains(haystack: &[u8], needle: &[u8]) -> bool {
    haystack
        .windows(needle.len())
        .any(|window| window == needle)
}

fn files(dir: &Path) -> Vec<Vec<u8>> {
    fs::read_dir(dir)
        .expect("read data dir")
        .map(|entry| fs::read(entry.expect("entry").path()).expect("read data file"))
        .collect()
}

#[test]
fn new_database_is_encrypted_at_rest() {
    let dir = tempdir().expect("temp dir");
    let path = dir.path().join("relay.db");
    let marker = "unique-plaintext-marker-9f3a";

    {
        let store = Store::open(&path, &key(1)).expect("open encrypted store");
        let id = store
            .create_conversation("gpt-6.1-sol")
            .expect("create conversation");
        store
            .append_message(&id, &ChatMessage::user(marker))
            .expect("append message");
    }

    let database = fs::read(&path).expect("read database");
    assert!(!database.starts_with(SQLITE_HEADER));
    for file in files(dir.path()) {
        assert!(!file.starts_with(SQLITE_HEADER), "SQLite header leaked");
        assert!(!contains(&file, marker.as_bytes()), "plaintext leaked");
    }

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;

        assert_eq!(
            fs::metadata(&path)
                .expect("database metadata")
                .permissions()
                .mode()
                & 0o777,
            0o600
        );
        assert_eq!(
            fs::metadata(dir.path())
                .expect("data dir metadata")
                .permissions()
                .mode()
                & 0o777,
            0o700
        );
    }

    let store = Store::open(&path, &key(1)).expect("reopen encrypted store");
    let summaries = store.list_conversations().expect("list conversations");
    assert_eq!(summaries.len(), 1);
    assert_eq!(summaries[0].preview.as_deref(), Some(marker));
}

#[test]
fn wrong_key_is_rejected() {
    let dir = tempdir().expect("temp dir");
    let path = dir.path().join("relay.db");

    {
        let store = Store::open(&path, &key(1)).expect("open encrypted store");
        let id = store
            .create_conversation("gpt-6.1-sol")
            .expect("create conversation");
        store
            .append_message(&id, &ChatMessage::user("private data"))
            .expect("append message");
    }

    let error = Store::open(&path, &key(2)).expect_err("wrong key must fail");
    assert!(matches!(error, StoreError::Unlock(_)), "{error:?}");
}

#[test]
fn plaintext_database_is_encrypted_in_place() {
    let dir = tempdir().expect("temp dir");
    let path = dir.path().join("relay.db");
    let marker = "Legacy plaintext title";

    {
        let conn = rusqlite::Connection::open(&path).expect("open plaintext database");
        conn.execute_batch(&format!(
            "CREATE TABLE conversations (
                id TEXT PRIMARY KEY NOT NULL,
                title TEXT NOT NULL,
                model_slug TEXT NOT NULL,
                created_at TEXT NOT NULL,
                updated_at TEXT NOT NULL
            );
            CREATE INDEX conversations_updated ON conversations(updated_at DESC);
            INSERT INTO conversations (id, title, model_slug, created_at, updated_at)
            VALUES ('legacy-id', '{marker}', 'gpt-6.1-sol',
                    '2026-11-15T08:00:00.000Z', '2026-11-15T08:00:00.000Z');"
        ))
        .expect("seed plaintext database");
    }
    assert!(
        fs::read(&path)
            .expect("read plaintext")
            .starts_with(SQLITE_HEADER)
    );

    {
        let store = Store::open(&path, &key(3)).expect("migrate plaintext database");
        let summaries = store.list_conversations().expect("list conversations");
        assert_eq!(summaries.len(), 1);
        assert_eq!(summaries[0].title, marker);
    }

    assert!(
        !fs::read(&path)
            .expect("read encrypted")
            .starts_with(SQLITE_HEADER)
    );
    for file in files(dir.path()) {
        assert!(!file.starts_with(SQLITE_HEADER), "SQLite header leaked");
        assert!(!contains(&file, marker.as_bytes()), "plaintext leaked");
    }

    let store = Store::open(&path, &key(3)).expect("reopen migrated database");
    assert_eq!(store.list_conversations().expect("list").len(), 1);
}

#[test]
fn plaintext_full_text_index_survives_migration() {
    let dir = tempdir().expect("temp dir");
    let path = dir.path().join("relay.db");
    let marker = "magnesium";

    {
        let conn = rusqlite::Connection::open(&path).expect("open plaintext database");
        conn.execute_batch(&format!(
            "CREATE TABLE conversations (
                id TEXT PRIMARY KEY NOT NULL,
                title TEXT NOT NULL,
                model_slug TEXT NOT NULL,
                created_at TEXT NOT NULL,
                updated_at TEXT NOT NULL
            );
            CREATE TABLE messages (
                id TEXT PRIMARY KEY NOT NULL,
                conversation_id TEXT NOT NULL REFERENCES conversations(id) ON DELETE CASCADE,
                role TEXT NOT NULL,
                content_json TEXT NOT NULL,
                search_text TEXT NOT NULL DEFAULT '',
                model_slug TEXT,
                usage_json TEXT,
                error TEXT,
                created_at TEXT NOT NULL,
                position INTEGER NOT NULL,
                UNIQUE(conversation_id, position)
            );
            CREATE VIRTUAL TABLE messages_fts USING fts5(
                search_text,
                conversation_id UNINDEXED,
                message_id UNINDEXED,
                tokenize = 'unicode61 remove_diacritics 2'
            );
            CREATE TRIGGER messages_fts_insert AFTER INSERT ON messages BEGIN
                INSERT INTO messages_fts(rowid, search_text, conversation_id, message_id)
                VALUES (new.rowid, new.search_text, new.conversation_id, new.id);
            END;
            INSERT INTO conversations (id, title, model_slug, created_at, updated_at)
            VALUES ('legacy-fts', 'Legacy search', 'gpt-6.1-sol',
                    '2026-11-15T08:00:00.000Z', '2026-11-15T08:00:00.000Z');
            INSERT INTO messages
                (id, conversation_id, role, content_json, search_text, created_at, position)
            VALUES ('legacy-message', 'legacy-fts', 'user',
                    '[{{\"type\":\"text\",\"text\":\"A note about {marker}.\"}}]',
                    'A note about {marker}.',
                    '2026-11-15T08:00:00.000Z', 0);"
        ))
        .expect("seed plaintext database with FTS");
    }

    let store = Store::open(&path, &key(4)).expect("migrate plaintext database");
    let results = store
        .search_conversations(marker)
        .expect("search migrated database");
    assert_eq!(results.len(), 1);
    assert_eq!(results[0].id, "legacy-fts");
    assert_eq!(
        store.messages("legacy-fts").expect("load messages").len(),
        1
    );
}
