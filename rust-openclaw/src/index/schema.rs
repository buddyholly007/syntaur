//! Index database schema and migrations.
//!
//! Single migration step for v1. Future migrations should append to the
//! `MIGRATIONS` array (each entry runs once, tracked in `schema_version`).

use rusqlite::Connection;

const MIGRATIONS: &[&str] = &[
    // v1: documents + chunks + chunks_fts + connector_state
    r#"
    CREATE TABLE IF NOT EXISTS documents (
        id           INTEGER PRIMARY KEY AUTOINCREMENT,
        source       TEXT NOT NULL,
        external_id  TEXT NOT NULL,
        title        TEXT NOT NULL,
        body         TEXT NOT NULL,
        updated_at   INTEGER NOT NULL,    -- unix epoch seconds (source mtime)
        indexed_at   INTEGER NOT NULL,    -- unix epoch seconds (when we indexed it)
        content_hash TEXT NOT NULL,       -- crc32 hex; skip re-ingest if unchanged
        metadata     TEXT NOT NULL DEFAULT '{}',
        UNIQUE(source, external_id)
    );

    CREATE INDEX IF NOT EXISTS idx_documents_source ON documents(source);
    CREATE INDEX IF NOT EXISTS idx_documents_updated ON documents(updated_at DESC);

    CREATE TABLE IF NOT EXISTS chunks (
        id      INTEGER PRIMARY KEY AUTOINCREMENT,
        doc_id  INTEGER NOT NULL REFERENCES documents(id) ON DELETE CASCADE,
        ord     INTEGER NOT NULL,
        text    TEXT NOT NULL
    );

    CREATE INDEX IF NOT EXISTS idx_chunks_doc ON chunks(doc_id);

    -- FTS5 virtual table over the chunks. content='chunks' uses external content
    -- mode so we don't duplicate the text — the FTS table holds only the index,
    -- text is fetched from the chunks row at query time.
    CREATE VIRTUAL TABLE IF NOT EXISTS chunks_fts USING fts5(
        text,
        content='chunks',
        content_rowid='id',
        tokenize='porter unicode61 remove_diacritics 2'
    );

    -- Triggers to keep FTS5 in sync with the chunks table
    CREATE TRIGGER IF NOT EXISTS chunks_ai AFTER INSERT ON chunks BEGIN
        INSERT INTO chunks_fts(rowid, text) VALUES (new.id, new.text);
    END;

    CREATE TRIGGER IF NOT EXISTS chunks_ad AFTER DELETE ON chunks BEGIN
        INSERT INTO chunks_fts(chunks_fts, rowid, text) VALUES('delete', old.id, old.text);
    END;

    CREATE TRIGGER IF NOT EXISTS chunks_au AFTER UPDATE ON chunks BEGIN
        INSERT INTO chunks_fts(chunks_fts, rowid, text) VALUES('delete', old.id, old.text);
        INSERT INTO chunks_fts(rowid, text) VALUES (new.id, new.text);
    END;

    -- Per-connector state for incremental polling
    CREATE TABLE IF NOT EXISTS connector_state (
        source         TEXT PRIMARY KEY,
        cursor         TEXT NOT NULL,            -- opaque JSON blob, connector-defined
        updated_at     INTEGER NOT NULL,
        last_full_load INTEGER,                  -- last successful Load (full snapshot)
        last_prune     INTEGER,                  -- last successful Slim/prune pass
        error_count    INTEGER NOT NULL DEFAULT 0,
        last_error     TEXT
    );

    CREATE TABLE IF NOT EXISTS schema_version (
        version INTEGER PRIMARY KEY,
        applied_at INTEGER NOT NULL
    );
    "#,
    // v2: research sessions for persistence + caching + streaming
    r#"
    CREATE TABLE IF NOT EXISTS research_sessions (
        id            TEXT PRIMARY KEY,
        agent         TEXT NOT NULL,
        query         TEXT NOT NULL,
        query_hash    TEXT NOT NULL,
        status        TEXT NOT NULL,            -- 'pending' | 'planning' | 'orchestrating' | 'reporting' | 'complete' | 'failed'
        plan_json     TEXT,
        evidence_json TEXT,
        report_text   TEXT,
        error         TEXT,
        created_at    INTEGER NOT NULL,
        started_at    INTEGER,
        completed_at  INTEGER,
        duration_ms   INTEGER
    );

    CREATE INDEX IF NOT EXISTS idx_research_sessions_query_hash ON research_sessions(query_hash);
    CREATE INDEX IF NOT EXISTS idx_research_sessions_agent ON research_sessions(agent);
    CREATE INDEX IF NOT EXISTS idx_research_sessions_status ON research_sessions(status);
    CREATE INDEX IF NOT EXISTS idx_research_sessions_created ON research_sessions(created_at DESC);
    "#,
    // v3: pending_actions table for the approval gate workflow
    r#"
    CREATE TABLE IF NOT EXISTS pending_actions (
        id          INTEGER PRIMARY KEY AUTOINCREMENT,
        agent       TEXT NOT NULL,
        tool_name   TEXT NOT NULL,
        args_json   TEXT NOT NULL,
        status      TEXT NOT NULL,           -- 'pending' | 'approved' | 'denied' | 'timed_out'
        created_at  INTEGER NOT NULL,
        resolved_at INTEGER,
        resolved_by TEXT
    );

    CREATE INDEX IF NOT EXISTS idx_pending_actions_status ON pending_actions(status);
    CREATE INDEX IF NOT EXISTS idx_pending_actions_agent ON pending_actions(agent);
    CREATE INDEX IF NOT EXISTS idx_pending_actions_created ON pending_actions(created_at DESC);
    "#,
    // v4: chunk_embeddings — pure-Rust vector store as little-endian f32 BLOBs
    r#"
    CREATE TABLE IF NOT EXISTS chunk_embeddings (
        chunk_id INTEGER PRIMARY KEY REFERENCES chunks(id) ON DELETE CASCADE,
        dim      INTEGER NOT NULL,
        vector   BLOB NOT NULL
    );

    CREATE INDEX IF NOT EXISTS idx_chunk_embeddings_dim ON chunk_embeddings(dim);
    "#,
    // v5: explicit conversation manager (separate from LCM's internal summary tables)
    r#"
    CREATE TABLE IF NOT EXISTS conversations_v2 (
        id          TEXT PRIMARY KEY,
        agent       TEXT NOT NULL,
        title       TEXT NOT NULL,
        created_at  INTEGER NOT NULL,
        updated_at  INTEGER NOT NULL
    );

    CREATE INDEX IF NOT EXISTS idx_conversations_v2_agent ON conversations_v2(agent);
    CREATE INDEX IF NOT EXISTS idx_conversations_v2_updated ON conversations_v2(updated_at DESC);

    CREATE TABLE IF NOT EXISTS conversation_messages_v2 (
        id              INTEGER PRIMARY KEY AUTOINCREMENT,
        conversation_id TEXT NOT NULL REFERENCES conversations_v2(id) ON DELETE CASCADE,
        role            TEXT NOT NULL,
        content         TEXT NOT NULL,
        created_at      INTEGER NOT NULL
    );

    CREATE INDEX IF NOT EXISTS idx_conv_messages_v2_conv ON conversation_messages_v2(conversation_id, id);
    "#,
    // v6: research session ↔ doc reference table for cache invalidation
    r#"
    CREATE TABLE IF NOT EXISTS research_session_doc_refs (
        session_id  TEXT NOT NULL,
        source      TEXT NOT NULL,
        external_id TEXT NOT NULL,
        PRIMARY KEY (session_id, source, external_id),
        FOREIGN KEY (session_id) REFERENCES research_sessions(id) ON DELETE CASCADE
    );

    CREATE INDEX IF NOT EXISTS idx_research_doc_refs_doc
        ON research_session_doc_refs(source, external_id);
    "#,
    // v7: per-user auth (v5 Item 3).
    //
    // The gateway up through v6 had one global API token; v7 introduces a
    // real user model + per-user tokens + Telegram chat → user links.
    //
    // The `user_id = 0` rows on existing tables represent the synthetic
    // "legacy admin" user — when the users table is empty the gateway
    // resolves the pre-existing `gateway.auth.token` to this user, so a
    // fresh install with no Item 3 setup keeps working unchanged.
    //
    // Token hashes are stored, not raw tokens. We use single-pass SHA256
    // because we hand the caller a token with >= 256 bits of entropy —
    // brute force against a uniform-random 32-byte secret is infeasible
    // and bcrypt-style work factors exist to protect low-entropy passwords.
    r#"
    CREATE TABLE IF NOT EXISTS users (
        id         INTEGER PRIMARY KEY AUTOINCREMENT,
        name       TEXT NOT NULL UNIQUE,
        created_at INTEGER NOT NULL,
        disabled   INTEGER NOT NULL DEFAULT 0
    );

    CREATE TABLE IF NOT EXISTS user_api_tokens (
        id            INTEGER PRIMARY KEY AUTOINCREMENT,
        user_id       INTEGER NOT NULL REFERENCES users(id) ON DELETE CASCADE,
        token_hash    TEXT NOT NULL UNIQUE,      -- hex-encoded SHA256 of the raw token
        name          TEXT NOT NULL,              -- human label, e.g. "laptop-cli"
        created_at    INTEGER NOT NULL,
        last_used_at  INTEGER,
        revoked_at    INTEGER
    );

    CREATE INDEX IF NOT EXISTS idx_user_api_tokens_user ON user_api_tokens(user_id);
    CREATE INDEX IF NOT EXISTS idx_user_api_tokens_hash ON user_api_tokens(token_hash);

    CREATE TABLE IF NOT EXISTS user_telegram_links (
        id           INTEGER PRIMARY KEY AUTOINCREMENT,
        user_id      INTEGER NOT NULL REFERENCES users(id) ON DELETE CASCADE,
        bot_token    TEXT NOT NULL,              -- identifies which bot account
        chat_id      INTEGER NOT NULL,           -- Telegram chat id (person or group)
        created_at   INTEGER NOT NULL,
        UNIQUE(bot_token, chat_id)
    );

    CREATE INDEX IF NOT EXISTS idx_user_telegram_links_user ON user_telegram_links(user_id);

    -- Scope existing data by owner. 0 = legacy admin.
    ALTER TABLE conversations_v2 ADD COLUMN user_id INTEGER NOT NULL DEFAULT 0;
    ALTER TABLE pending_actions  ADD COLUMN user_id INTEGER NOT NULL DEFAULT 0;
    ALTER TABLE research_sessions ADD COLUMN user_id INTEGER NOT NULL DEFAULT 0;

    CREATE INDEX IF NOT EXISTS idx_conversations_v2_user ON conversations_v2(user_id);
    CREATE INDEX IF NOT EXISTS idx_pending_actions_user ON pending_actions(user_id);
    CREATE INDEX IF NOT EXISTS idx_research_sessions_user ON research_sessions(user_id);
    "#,
    // v8: OAuth2 authorization_code tokens (v5 Item 4).
    //
    // Each row = one (user_id, provider) pair. Refresh tokens are persisted
    // alongside the access token so that a gateway restart doesn't force
    // the user to re-authorize. `expires_at` is unix seconds; refresh logic
    // kicks in ~30s before expiry (matches the OAuth2 client_credentials
    // cache from v4).
    r#"
    CREATE TABLE IF NOT EXISTS oauth_tokens (
        id            INTEGER PRIMARY KEY AUTOINCREMENT,
        user_id       INTEGER NOT NULL REFERENCES users(id) ON DELETE CASCADE,
        provider      TEXT NOT NULL,
        access_token  TEXT NOT NULL,
        refresh_token TEXT,
        expires_at    INTEGER,                    -- unix seconds, NULL = no expiry
        scope         TEXT NOT NULL DEFAULT '',
        created_at    INTEGER NOT NULL,
        updated_at    INTEGER NOT NULL,
        UNIQUE(user_id, provider)
    );

    CREATE INDEX IF NOT EXISTS idx_oauth_tokens_user ON oauth_tokens(user_id);
    "#,
    // v9: four-feature batch (tool_hooks + skills + plans + slash_commands).
    //
    // **tool_hooks** — user-configurable PreToolUse / PostToolUse hooks.
    // Distinct from the existing internal `HookBus` in `src/hooks.rs` which
    // is an in-process pub-sub for system events. tool_hooks fire on the
    // `dispatch_extension` funnel boundary and can block, notify, or
    // trigger downstream skills based on per-row config.
    //
    // **skills** — named, reusable workflows. Three kinds:
    //   - 'binary' — shell out to a configured executable (e.g. rust-social-manager bsky-engage)
    //   - 'prompt' — expand a template + run as a normal LLM turn
    //   - 'tool_chain' — JSON-encoded sequence of tool calls
    //
    // **plans + plan_steps** — multi-step approval-gated workflows. The
    // user (or Felix) calls `propose_plan` with a step list; the plan
    // is persisted and a Telegram inline keyboard is sent for approval.
    // Once approved, the plan executor runs the steps sequentially via
    // the existing tool dispatch funnel.
    //
    // **slash_commands** — short user-invocable shortcuts. Three kinds:
    //   - 'direct' — POST to a known internal endpoint, no LLM round-trip
    //   - 'text_prompt' — expand a template, post as a normal LLM message
    //   - 'skill_ref' — invoke a registered skill by name
    r#"
    CREATE TABLE IF NOT EXISTS tool_hooks (
        id                 INTEGER PRIMARY KEY AUTOINCREMENT,
        event              TEXT NOT NULL,            -- 'pre_tool_call' | 'post_tool_call'
        match_pattern_json TEXT NOT NULL,            -- {"tool":"browser_open","success":false}
        action             TEXT NOT NULL,            -- 'telegram_notify' | 'audit_log' | 'block' | 'run_skill'
        action_config_json TEXT NOT NULL,            -- per-action config (template, skill name, etc.)
        enabled            INTEGER NOT NULL DEFAULT 1,
        created_at         INTEGER NOT NULL
    );

    CREATE INDEX IF NOT EXISTS idx_tool_hooks_event ON tool_hooks(event) WHERE enabled = 1;

    CREATE TABLE IF NOT EXISTS skills (
        id                 INTEGER PRIMARY KEY AUTOINCREMENT,
        name               TEXT NOT NULL UNIQUE,
        description        TEXT NOT NULL,
        agent_id           TEXT NOT NULL DEFAULT 'main',
        kind               TEXT NOT NULL,            -- 'binary' | 'prompt' | 'tool_chain'
        body               TEXT NOT NULL,            -- binary path / template / json chain
        args_schema_json   TEXT,                     -- optional JSON schema for args
        requires_approval  INTEGER NOT NULL DEFAULT 0,
        created_at         INTEGER NOT NULL
    );

    CREATE INDEX IF NOT EXISTS idx_skills_agent ON skills(agent_id);

    CREATE TABLE IF NOT EXISTS plans (
        id            INTEGER PRIMARY KEY AUTOINCREMENT,
        user_id       INTEGER NOT NULL REFERENCES users(id) ON DELETE CASCADE,
        agent_id      TEXT NOT NULL,
        title         TEXT NOT NULL,
        rationale     TEXT NOT NULL DEFAULT '',     -- why this plan
        status        TEXT NOT NULL,                -- 'pending'|'approved'|'denied'|'executing'|'complete'|'failed'
        created_at    INTEGER NOT NULL,
        approved_at   INTEGER,
        completed_at  INTEGER,
        error         TEXT
    );

    CREATE INDEX IF NOT EXISTS idx_plans_user ON plans(user_id);
    CREATE INDEX IF NOT EXISTS idx_plans_status ON plans(status);

    CREATE TABLE IF NOT EXISTS plan_steps (
        id            INTEGER PRIMARY KEY AUTOINCREMENT,
        plan_id       INTEGER NOT NULL REFERENCES plans(id) ON DELETE CASCADE,
        ord           INTEGER NOT NULL,             -- 0-indexed
        step_kind     TEXT NOT NULL,                -- 'tool' | 'skill' | 'note'
        step_target   TEXT NOT NULL,                -- tool name OR skill name OR note text
        args_json     TEXT NOT NULL DEFAULT '{}',
        status        TEXT NOT NULL,                -- 'pending'|'running'|'complete'|'failed'|'skipped'
        result_text   TEXT,
        started_at    INTEGER,
        completed_at  INTEGER,
        UNIQUE(plan_id, ord)
    );

    CREATE INDEX IF NOT EXISTS idx_plan_steps_plan ON plan_steps(plan_id);

    CREATE TABLE IF NOT EXISTS slash_commands (
        id                 INTEGER PRIMARY KEY AUTOINCREMENT,
        name               TEXT NOT NULL UNIQUE,    -- WITHOUT the leading /
        description        TEXT NOT NULL,
        agent_filter       TEXT,                    -- agent_id or NULL for all agents
        kind               TEXT NOT NULL,           -- 'direct' | 'text_prompt' | 'skill_ref'
        body_template      TEXT NOT NULL,           -- endpoint path / prompt template / skill name
        args_schema_json   TEXT,
        created_at         INTEGER NOT NULL
    );
    "#,
];

pub fn migrate(conn: &Connection) -> rusqlite::Result<()> {
    // Ensure schema_version table exists so we can record migrations.
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS schema_version (version INTEGER PRIMARY KEY, applied_at INTEGER NOT NULL);"
    )?;

    let current: i64 = conn
        .query_row(
            "SELECT COALESCE(MAX(version), 0) FROM schema_version",
            [],
            |r| r.get(0),
        )
        .unwrap_or(0);

    for (i, sql) in MIGRATIONS.iter().enumerate() {
        let version = (i as i64) + 1;
        if version <= current {
            continue;
        }
        conn.execute_batch(sql)?;
        conn.execute(
            "INSERT INTO schema_version (version, applied_at) VALUES (?, ?)",
            rusqlite::params![version, chrono::Utc::now().timestamp()],
        )?;
    }
    Ok(())
}
