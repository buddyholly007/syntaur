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
    // v10: bug reports.
    r#"
    CREATE TABLE IF NOT EXISTS bug_reports (
        id          INTEGER PRIMARY KEY AUTOINCREMENT,
        user_id     INTEGER,
        user_name   TEXT,
        description TEXT NOT NULL,
        system_info TEXT,
        page_url    TEXT,
        status      TEXT NOT NULL DEFAULT 'open',
        created_at  DATETIME DEFAULT CURRENT_TIMESTAMP
    );
    "#,
    // v11: todos + calendar events — server-side storage for dashboard widgets.
    // Agents can create/complete todos and add calendar events via tools.
    // Cross-device: state lives on the server, all devices see the same data.
    r#"
    CREATE TABLE IF NOT EXISTS todos (
        id          INTEGER PRIMARY KEY AUTOINCREMENT,
        user_id     INTEGER NOT NULL DEFAULT 0,
        text        TEXT NOT NULL,
        done        INTEGER NOT NULL DEFAULT 0,
        due_date    TEXT,
        created_at  INTEGER NOT NULL,
        completed_at INTEGER
    );
    CREATE INDEX IF NOT EXISTS idx_todos_user ON todos(user_id);

    CREATE TABLE IF NOT EXISTS calendar_events (
        id          INTEGER PRIMARY KEY AUTOINCREMENT,
        user_id     INTEGER NOT NULL DEFAULT 0,
        title       TEXT NOT NULL,
        description TEXT,
        start_time  TEXT NOT NULL,
        end_time    TEXT,
        all_day     INTEGER NOT NULL DEFAULT 0,
        source      TEXT NOT NULL DEFAULT 'manual',
        created_at  INTEGER NOT NULL
    );
    CREATE INDEX IF NOT EXISTS idx_calendar_user ON calendar_events(user_id);
    CREATE INDEX IF NOT EXISTS idx_calendar_start ON calendar_events(start_time);
    "#,
    // v12: tax module — receipts, expenses, categories (premium module).
    r#"
    CREATE TABLE IF NOT EXISTS expense_categories (
        id              INTEGER PRIMARY KEY AUTOINCREMENT,
        name            TEXT NOT NULL UNIQUE,
        entity          TEXT NOT NULL DEFAULT 'personal',
        tax_deductible  INTEGER NOT NULL DEFAULT 0,
        parent_category TEXT
    );

    CREATE TABLE IF NOT EXISTS receipts (
        id              INTEGER PRIMARY KEY AUTOINCREMENT,
        user_id         INTEGER NOT NULL DEFAULT 0,
        image_path      TEXT NOT NULL,
        vendor          TEXT,
        amount_cents    INTEGER,
        category_id     INTEGER REFERENCES expense_categories(id),
        receipt_date    TEXT,
        description     TEXT,
        raw_ocr         TEXT,
        status          TEXT NOT NULL DEFAULT 'pending',
        created_at      INTEGER NOT NULL
    );
    CREATE INDEX IF NOT EXISTS idx_receipts_user ON receipts(user_id);
    CREATE INDEX IF NOT EXISTS idx_receipts_date ON receipts(receipt_date);

    CREATE TABLE IF NOT EXISTS expenses (
        id              INTEGER PRIMARY KEY AUTOINCREMENT,
        user_id         INTEGER NOT NULL DEFAULT 0,
        amount_cents    INTEGER NOT NULL,
        vendor          TEXT NOT NULL,
        category_id     INTEGER REFERENCES expense_categories(id),
        expense_date    TEXT NOT NULL,
        description     TEXT,
        entity          TEXT NOT NULL DEFAULT 'personal',
        receipt_id      INTEGER REFERENCES receipts(id),
        created_at      INTEGER NOT NULL
    );
    CREATE INDEX IF NOT EXISTS idx_expenses_user ON expenses(user_id);
    CREATE INDEX IF NOT EXISTS idx_expenses_date ON expenses(expense_date);
    CREATE INDEX IF NOT EXISTS idx_expenses_category ON expenses(category_id);

    -- Seed default categories
    INSERT OR IGNORE INTO expense_categories (name, entity, tax_deductible) VALUES
        ('Advertising & Marketing', 'business', 1),
        ('Equipment & Tools', 'business', 1),
        ('Hardware & Supplies', 'business', 1),
        ('Lumber & Raw Materials', 'business', 1),
        ('Office Supplies', 'business', 1),
        ('Professional Services', 'business', 1),
        ('Rent & Utilities', 'business', 1),
        ('Insurance', 'business', 1),
        ('Software & Subscriptions', 'business', 1),
        ('Shipping & Packaging', 'business', 1),
        ('Vehicle & Mileage', 'business', 1),
        ('Education & Training', 'business', 1),
        ('Meals & Entertainment', 'business', 1),
        ('Travel', 'business', 1),
        ('Tools - Consumables', 'business', 1),
        ('Safety Gear', 'business', 1),
        ('Miscellaneous Business', 'business', 1),
        ('Medical', 'personal', 1),
        ('Mortgage', 'personal', 1),
        ('Vehicle', 'personal', 0),
        ('Donations', 'personal', 1),
        ('Education', 'personal', 1),
        ('Home Improvement', 'personal', 0),
        ('Utilities', 'personal', 0),
        ('Groceries', 'personal', 0),
        ('Dining', 'personal', 0),
        ('Entertainment', 'personal', 0),
        ('Other', 'personal', 0);
    "#,
    // v13: tax documents — smart classifier for W-2, 1099, statements, etc.
    // Extracted fields stored as JSON so each doc type can have its own schema.
    r#"
    CREATE TABLE IF NOT EXISTS tax_documents (
        id              INTEGER PRIMARY KEY AUTOINCREMENT,
        user_id         INTEGER NOT NULL DEFAULT 0,
        doc_type        TEXT NOT NULL,
        tax_year        INTEGER,
        issuer          TEXT,
        extracted_fields TEXT,
        image_path      TEXT NOT NULL,
        status          TEXT NOT NULL DEFAULT 'pending',
        created_at      INTEGER NOT NULL
    );
    CREATE INDEX IF NOT EXISTS idx_tax_docs_user ON tax_documents(user_id);
    CREATE INDEX IF NOT EXISTS idx_tax_docs_type ON tax_documents(doc_type);
    CREATE INDEX IF NOT EXISTS idx_tax_docs_year ON tax_documents(tax_year);

    -- Also ensure tax_income table exists (may have been created ad-hoc)
    CREATE TABLE IF NOT EXISTS tax_income (
        id          INTEGER PRIMARY KEY AUTOINCREMENT,
        user_id     INTEGER NOT NULL DEFAULT 0,
        source      TEXT NOT NULL,
        amount_cents INTEGER NOT NULL,
        tax_year    INTEGER NOT NULL,
        category    TEXT,
        description TEXT,
        created_at  INTEGER NOT NULL
    );
    "#,
    // v14: statement transactions + property profiles + insurance classifications.
    //
    // **statement_transactions** — individual line items extracted from bank/credit
    // card statements via AI vision. Each row links back to the source tax_document.
    //
    // **property_profiles** — centralized property data (sqft, assessor values,
    // mortgage, building/land split). Auto-populated from scanned docs.
    //
    // **insurance_classifications** — disambiguate same-vendor insurance payments
    // (car vs home vs health) based on amount, frequency, and document context.
    r#"
    CREATE TABLE IF NOT EXISTS statement_transactions (
        id              INTEGER PRIMARY KEY AUTOINCREMENT,
        user_id         INTEGER NOT NULL DEFAULT 0,
        document_id     INTEGER REFERENCES tax_documents(id),
        transaction_date TEXT NOT NULL,
        description     TEXT NOT NULL,
        amount_cents    INTEGER NOT NULL,
        category_id     INTEGER REFERENCES expense_categories(id),
        vendor          TEXT,
        insurance_type  TEXT,
        is_deductible   INTEGER NOT NULL DEFAULT 0,
        status          TEXT NOT NULL DEFAULT 'extracted',
        created_at      INTEGER NOT NULL
    );
    CREATE INDEX IF NOT EXISTS idx_stmt_txn_user ON statement_transactions(user_id);
    CREATE INDEX IF NOT EXISTS idx_stmt_txn_date ON statement_transactions(transaction_date);
    CREATE INDEX IF NOT EXISTS idx_stmt_txn_doc ON statement_transactions(document_id);
    CREATE INDEX IF NOT EXISTS idx_stmt_txn_vendor ON statement_transactions(vendor);

    CREATE TABLE IF NOT EXISTS property_profiles (
        id                  INTEGER PRIMARY KEY AUTOINCREMENT,
        user_id             INTEGER NOT NULL DEFAULT 0,
        address             TEXT NOT NULL,
        total_sqft          INTEGER,
        workshop_sqft       INTEGER,
        purchase_price_cents INTEGER,
        purchase_date       TEXT,
        building_value_cents INTEGER,
        land_value_cents    INTEGER,
        land_ratio          REAL,
        assessor_total_cents INTEGER,
        assessor_land_cents  INTEGER,
        annual_property_tax_cents INTEGER,
        annual_insurance_cents    INTEGER,
        mortgage_lender     TEXT,
        mortgage_interest_cents   INTEGER,
        mortgage_principal_cents  INTEGER,
        depreciation_basis_cents  INTEGER,
        depreciation_annual_cents INTEGER,
        notes               TEXT,
        created_at          INTEGER NOT NULL,
        updated_at          INTEGER NOT NULL
    );
    CREATE INDEX IF NOT EXISTS idx_property_user ON property_profiles(user_id);

    CREATE TABLE IF NOT EXISTS insurance_classifications (
        id              INTEGER PRIMARY KEY AUTOINCREMENT,
        user_id         INTEGER NOT NULL DEFAULT 0,
        vendor          TEXT NOT NULL,
        amount_cents    INTEGER,
        insurance_type  TEXT NOT NULL,
        confidence      REAL NOT NULL DEFAULT 0.5,
        evidence        TEXT,
        created_at      INTEGER NOT NULL
    );
    CREATE INDEX IF NOT EXISTS idx_insurance_class_user ON insurance_classifications(user_id);
    CREATE INDEX IF NOT EXISTS idx_insurance_class_vendor ON insurance_classifications(vendor);
    "#,
    // v15: module licensing — free tier + $49 pro unlock + 3-day trials.
    //
    // **user_licenses**: one row per user who has purchased Pro. The $49
    // payment unlocks ALL modules. license_key is a receipt/txn ID from
    // the payment processor.
    //
    // **module_trials**: per-user, per-module trial tracking. Each module
    // gets one 3-day trial. trial_started_at is set on first access;
    // trial_expires_at = started + 3 days. After expiry the module locks
    // until the user buys Pro.
    //
    // Free features (never gated): chat, todos, calendar, dashboard.
    r#"
    CREATE TABLE IF NOT EXISTS user_licenses (
        id              INTEGER PRIMARY KEY AUTOINCREMENT,
        user_id         INTEGER NOT NULL REFERENCES users(id) ON DELETE CASCADE,
        license_type    TEXT NOT NULL DEFAULT 'pro',
        purchased_at    INTEGER NOT NULL,
        payment_id      TEXT,
        amount_cents    INTEGER NOT NULL DEFAULT 4900,
        UNIQUE(user_id, license_type)
    );
    CREATE INDEX IF NOT EXISTS idx_user_licenses_user ON user_licenses(user_id);

    CREATE TABLE IF NOT EXISTS module_trials (
        id                INTEGER PRIMARY KEY AUTOINCREMENT,
        user_id           INTEGER NOT NULL DEFAULT 0,
        module_name       TEXT NOT NULL,
        trial_started_at  INTEGER NOT NULL,
        trial_expires_at  INTEGER NOT NULL,
        UNIQUE(user_id, module_name)
    );
    CREATE INDEX IF NOT EXISTS idx_module_trials_user ON module_trials(user_id);

    -- Seed the module registry
    CREATE TABLE IF NOT EXISTS modules (
        name            TEXT PRIMARY KEY,
        display_name    TEXT NOT NULL,
        description     TEXT,
        icon            TEXT,
        trial_days      INTEGER NOT NULL DEFAULT 3,
        enabled         INTEGER NOT NULL DEFAULT 1
    );
    INSERT OR IGNORE INTO modules (name, display_name, description, icon) VALUES
        ('tax', 'Tax & Expenses', 'Receipt scanning, expense tracking, tax document management, deduction calculator, and year-end tax prep wizard.', '&#128176;');
    "#,
    // v16: financial integrations — Plaid, Alpaca, Coinbase, SimpleFIN
    // connections + investment data + email connections for receipt parsing.
    r#"
    CREATE TABLE IF NOT EXISTS connected_accounts (
        id              INTEGER PRIMARY KEY AUTOINCREMENT,
        user_id         INTEGER NOT NULL DEFAULT 0,
        provider        TEXT NOT NULL,
        institution_name TEXT,
        institution_id  TEXT,
        access_token    TEXT NOT NULL,
        item_id         TEXT,
        cursor          TEXT,
        account_ids     TEXT,
        status          TEXT NOT NULL DEFAULT 'active',
        last_sync_at    INTEGER,
        error           TEXT,
        created_at      INTEGER NOT NULL
    );
    CREATE INDEX IF NOT EXISTS idx_connected_accounts_user ON connected_accounts(user_id);

    CREATE TABLE IF NOT EXISTS investment_accounts (
        id              INTEGER PRIMARY KEY AUTOINCREMENT,
        user_id         INTEGER NOT NULL DEFAULT 0,
        broker          TEXT NOT NULL,
        api_key         TEXT NOT NULL,
        api_secret      TEXT,
        base_url        TEXT,
        nickname        TEXT,
        status          TEXT NOT NULL DEFAULT 'active',
        last_sync_at    INTEGER,
        created_at      INTEGER NOT NULL
    );
    CREATE INDEX IF NOT EXISTS idx_investment_accounts_user ON investment_accounts(user_id);

    CREATE TABLE IF NOT EXISTS investment_transactions (
        id              INTEGER PRIMARY KEY AUTOINCREMENT,
        user_id         INTEGER NOT NULL DEFAULT 0,
        account_id      INTEGER REFERENCES investment_accounts(id),
        broker          TEXT NOT NULL,
        activity_type   TEXT NOT NULL,
        symbol          TEXT,
        qty             REAL,
        price_cents     INTEGER,
        amount_cents    INTEGER NOT NULL,
        side            TEXT,
        realized_pl_cents INTEGER,
        cost_basis_cents INTEGER,
        transaction_date TEXT NOT NULL,
        description     TEXT,
        external_id     TEXT,
        created_at      INTEGER NOT NULL
    );
    CREATE INDEX IF NOT EXISTS idx_inv_txn_user ON investment_transactions(user_id);
    CREATE INDEX IF NOT EXISTS idx_inv_txn_date ON investment_transactions(transaction_date);
    CREATE INDEX IF NOT EXISTS idx_inv_txn_symbol ON investment_transactions(symbol);
    CREATE INDEX IF NOT EXISTS idx_inv_txn_external ON investment_transactions(external_id);

    CREATE TABLE IF NOT EXISTS email_connections (
        id              INTEGER PRIMARY KEY AUTOINCREMENT,
        user_id         INTEGER NOT NULL DEFAULT 0,
        provider        TEXT NOT NULL DEFAULT 'gmail',
        email_address   TEXT,
        oauth_token     TEXT,
        refresh_token   TEXT,
        token_expires_at INTEGER,
        last_scan_at    INTEGER,
        status          TEXT NOT NULL DEFAULT 'active',
        created_at      INTEGER NOT NULL
    );
    CREATE INDEX IF NOT EXISTS idx_email_conn_user ON email_connections(user_id);
    "#,
    // v17: calendar extensions — recurrence, reminders, edit tracking.
    r#"
    ALTER TABLE calendar_events ADD COLUMN recurrence_rule TEXT;
    ALTER TABLE calendar_events ADD COLUMN recurrence_end_date TEXT;
    ALTER TABLE calendar_events ADD COLUMN reminder_minutes INTEGER;
    ALTER TABLE calendar_events ADD COLUMN updated_at INTEGER;

    CREATE TABLE IF NOT EXISTS calendar_reminders_sent (
        id              INTEGER PRIMARY KEY AUTOINCREMENT,
        event_id        INTEGER NOT NULL,
        occurrence_date TEXT NOT NULL,
        sent_at         INTEGER NOT NULL,
        UNIQUE(event_id, occurrence_date)
    );
    CREATE INDEX IF NOT EXISTS idx_cal_rem_event ON calendar_reminders_sent(event_id);
    "#,
    // v18: generic sync connections (Stripe, Bluesky, ICS subscriptions,
    // generic providers) + Telegram pairing codes.
    r#"
    CREATE TABLE IF NOT EXISTS sync_connections (
        id              INTEGER PRIMARY KEY AUTOINCREMENT,
        user_id         INTEGER NOT NULL DEFAULT 0,
        provider        TEXT NOT NULL,
        display_name    TEXT,
        credential      TEXT NOT NULL,
        metadata        TEXT,
        status          TEXT NOT NULL DEFAULT 'active',
        last_sync_at    INTEGER,
        last_check_at   INTEGER,
        last_error      TEXT,
        created_at      INTEGER NOT NULL,
        updated_at      INTEGER,
        UNIQUE(user_id, provider)
    );
    CREATE INDEX IF NOT EXISTS idx_sync_conn_user ON sync_connections(user_id);
    CREATE INDEX IF NOT EXISTS idx_sync_conn_provider ON sync_connections(provider);

    CREATE TABLE IF NOT EXISTS telegram_pairings (
        id              INTEGER PRIMARY KEY AUTOINCREMENT,
        user_id         INTEGER NOT NULL,
        code            TEXT NOT NULL UNIQUE,
        bot_token       TEXT NOT NULL,
        expires_at      INTEGER NOT NULL,
        created_at      INTEGER NOT NULL,
        consumed_at     INTEGER
    );
    CREATE INDEX IF NOT EXISTS idx_tg_pair_code ON telegram_pairings(code);
    CREATE INDEX IF NOT EXISTS idx_tg_pair_expires ON telegram_pairings(expires_at);
    "#,
    // v19: deduction questionnaire + auto-scanner candidates
    r#"
    CREATE TABLE IF NOT EXISTS deduction_questionnaire (
        id              INTEGER PRIMARY KEY AUTOINCREMENT,
        user_id         INTEGER NOT NULL DEFAULT 0,
        tax_year        INTEGER NOT NULL,
        answers_json    TEXT NOT NULL DEFAULT '{}',
        completed       INTEGER NOT NULL DEFAULT 0,
        created_at      INTEGER NOT NULL,
        updated_at      INTEGER NOT NULL,
        UNIQUE(user_id, tax_year)
    );
    CREATE INDEX IF NOT EXISTS idx_ded_quest_user ON deduction_questionnaire(user_id);

    CREATE TABLE IF NOT EXISTS deduction_candidates (
        id              INTEGER PRIMARY KEY AUTOINCREMENT,
        user_id         INTEGER NOT NULL DEFAULT 0,
        tax_year        INTEGER NOT NULL,
        source_type     TEXT NOT NULL,
        source_id       INTEGER NOT NULL,
        deduction_type  TEXT NOT NULL,
        vendor          TEXT,
        description     TEXT,
        amount_cents    INTEGER NOT NULL,
        transaction_date TEXT,
        category_suggestion TEXT,
        entity_suggestion TEXT NOT NULL DEFAULT 'personal',
        confidence      REAL NOT NULL DEFAULT 0.5,
        match_rule      TEXT,
        status          TEXT NOT NULL DEFAULT 'pending',
        reviewed_at     INTEGER,
        expense_id      INTEGER,
        created_at      INTEGER NOT NULL,
        UNIQUE(user_id, source_type, source_id, deduction_type)
    );
    CREATE INDEX IF NOT EXISTS idx_ded_cand_user ON deduction_candidates(user_id);
    CREATE INDEX IF NOT EXISTS idx_ded_cand_status ON deduction_candidates(status);
    CREATE INDEX IF NOT EXISTS idx_ded_cand_year ON deduction_candidates(tax_year);
    CREATE INDEX IF NOT EXISTS idx_ded_cand_source ON deduction_candidates(source_type, source_id);
    "#,
    // v19: runtime-configured OAuth apps, keyed by IDENTITY PROVIDER
    // (google/microsoft/spotify/etc) so one Google config unlocks Gmail,
    // Calendar, YouTube Music, Drive without re-pasting credentials.
    r#"
    CREATE TABLE IF NOT EXISTS oauth_config (
        identity_provider TEXT PRIMARY KEY,
        client_id         TEXT NOT NULL,
        client_secret     TEXT NOT NULL,
        configured_at     INTEGER NOT NULL,
        updated_at        INTEGER,
        configured_by     INTEGER
    );
    "#,
    // v21: music preferences — persistent memory for the AI DJ.
    r#"
    CREATE TABLE IF NOT EXISTS user_music_preferences (
        id              INTEGER PRIMARY KEY AUTOINCREMENT,
        user_id         INTEGER NOT NULL DEFAULT 0,
        category        TEXT NOT NULL,
        kind            TEXT,
        value           TEXT NOT NULL,
        track_id        TEXT,
        provider        TEXT,
        weight          REAL NOT NULL DEFAULT 1.0,
        source          TEXT,
        created_at      INTEGER NOT NULL
    );
    CREATE INDEX IF NOT EXISTS idx_mpref_user ON user_music_preferences(user_id);
    CREATE INDEX IF NOT EXISTS idx_mpref_created ON user_music_preferences(created_at);
    "#,
    // v22: tax extension filing workflow with state tracking
    r#"
    CREATE TABLE IF NOT EXISTS tax_extensions (
        id                INTEGER PRIMARY KEY AUTOINCREMENT,
        user_id           INTEGER NOT NULL DEFAULT 0,
        tax_year          INTEGER NOT NULL,
        total_tax_cents   INTEGER NOT NULL,
        total_paid_cents  INTEGER NOT NULL,
        balance_due_cents INTEGER NOT NULL,
        payment_cents     INTEGER NOT NULL DEFAULT 0,
        filing_method     TEXT,
        status            TEXT NOT NULL DEFAULT 'draft',
        confirmation_id   TEXT,
        filed_at          INTEGER,
        confirmed_at      INTEGER,
        document_id       INTEGER,
        form_text         TEXT NOT NULL,
        created_at        INTEGER NOT NULL,
        updated_at        INTEGER NOT NULL,
        UNIQUE(user_id, tax_year)
    );
    CREATE INDEX IF NOT EXISTS idx_tax_ext_user ON tax_extensions(user_id);
    "#,
    // v23: taxpayer profile + dependents for tax filing
    r#"
    CREATE TABLE IF NOT EXISTS taxpayer_profiles (
        id              INTEGER PRIMARY KEY AUTOINCREMENT,
        user_id         INTEGER NOT NULL,
        tax_year        INTEGER NOT NULL,
        first_name      TEXT,
        last_name       TEXT,
        ssn_encrypted   TEXT,
        date_of_birth   TEXT,
        address_line1   TEXT,
        address_line2   TEXT,
        city            TEXT,
        state           TEXT,
        zip             TEXT,
        phone           TEXT,
        email           TEXT,
        filing_status   TEXT NOT NULL DEFAULT 'single',
        spouse_first    TEXT,
        spouse_last     TEXT,
        spouse_ssn_encrypted TEXT,
        spouse_dob      TEXT,
        occupation      TEXT,
        spouse_occupation TEXT,
        created_at      INTEGER NOT NULL,
        updated_at      INTEGER NOT NULL,
        UNIQUE(user_id, tax_year)
    );
    CREATE INDEX IF NOT EXISTS idx_tp_user ON taxpayer_profiles(user_id);

    CREATE TABLE IF NOT EXISTS dependents (
        id              INTEGER PRIMARY KEY AUTOINCREMENT,
        user_id         INTEGER NOT NULL DEFAULT 0,
        tax_year        INTEGER NOT NULL,
        first_name      TEXT NOT NULL,
        last_name       TEXT NOT NULL,
        ssn_encrypted   TEXT,
        date_of_birth   TEXT,
        relationship    TEXT NOT NULL DEFAULT 'child',
        months_lived    INTEGER NOT NULL DEFAULT 12,
        qualifies_ctc   INTEGER NOT NULL DEFAULT 1,
        qualifies_odc   INTEGER NOT NULL DEFAULT 0,
        is_student      INTEGER NOT NULL DEFAULT 0,
        is_disabled     INTEGER NOT NULL DEFAULT 0,
        created_at      INTEGER NOT NULL
    );
    CREATE INDEX IF NOT EXISTS idx_dep_user ON dependents(user_id, tax_year);
    "#,
    // v24: expanded default expense categories + custom category support
    r#"
    INSERT OR IGNORE INTO expense_categories (name, entity, tax_deductible) VALUES
        -- Business: common categories users need
        ('Power Tools', 'business', 1),
        ('Shop Maintenance', 'business', 1),
        ('Fuel - Equipment', 'business', 1),
        ('Furniture & Equipment', 'business', 1),
        ('Hardware & Fasteners', 'business', 1),
        ('Dust Collection', 'business', 1),
        ('Backup Power', 'business', 1),
        ('Home Office', 'business', 1),
        ('Supplies - General', 'business', 1),
        ('Contract Labor', 'business', 1),
        ('Commissions & Fees', 'business', 1),
        ('Depreciation (Sec 179)', 'business', 1),
        ('Business Revenue', 'business', 0),
        -- Personal: tax-relevant categories
        ('Real Estate Taxes', 'personal', 1),
        ('Mortgage Interest', 'personal', 1),
        ('Construction Loan Interest', 'personal', 1),
        ('Closing Costs', 'personal', 0),
        ('Homeowner''s Insurance', 'personal', 0),
        ('Student Loan Interest', 'personal', 1),
        ('Health Insurance', 'personal', 1),
        ('Childcare', 'personal', 1),
        ('Retirement Contributions', 'personal', 1),
        ('HSA Contributions', 'personal', 1),
        -- Personal: tracking categories
        ('FICA - Social Security', 'personal', 0),
        ('FICA - Medicare', 'personal', 0),
        ('Federal Income Tax Paid', 'personal', 0),
        ('State/Local Tax Paid', 'personal', 0),
        ('Bank Fees', 'personal', 0),
        ('Personal Expenses', 'personal', 0),
        ('Wages Income', 'personal', 0),
        ('Interest Income', 'personal', 0),
        ('Dividend Income', 'personal', 0),
        ('Capital Gains', 'personal', 0);

    -- Allow user_id-scoped custom categories (added flag)
    ALTER TABLE expense_categories ADD COLUMN user_id INTEGER NOT NULL DEFAULT 0;
    ALTER TABLE expense_categories ADD COLUMN is_custom INTEGER NOT NULL DEFAULT 0;
    "#,

    // Migration 25: Token expiry support
    r#"
    ALTER TABLE user_api_tokens ADD COLUMN expires_at INTEGER;
    "#,

    // Migration 26: Terminal / Coders module tables
    r#"
    CREATE TABLE IF NOT EXISTS terminal_hosts (
        id            INTEGER PRIMARY KEY AUTOINCREMENT,
        name          TEXT NOT NULL UNIQUE,
        hostname      TEXT NOT NULL,
        port          INTEGER NOT NULL DEFAULT 22,
        username      TEXT NOT NULL DEFAULT 'sean',
        auth_method   TEXT NOT NULL DEFAULT 'key',
        private_key   TEXT,
        password      TEXT,
        jump_host_id  INTEGER REFERENCES terminal_hosts(id),
        default_shell TEXT DEFAULT '/bin/bash',
        group_name    TEXT DEFAULT '',
        tags          TEXT DEFAULT '[]',
        color         TEXT DEFAULT '#0ea5e9',
        sort_order    INTEGER NOT NULL DEFAULT 0,
        is_local      INTEGER NOT NULL DEFAULT 0,
        favorite      INTEGER NOT NULL DEFAULT 0,
        created_at    INTEGER NOT NULL,
        updated_at    INTEGER NOT NULL
    );

    CREATE TABLE IF NOT EXISTS terminal_sessions (
        id            TEXT PRIMARY KEY,
        host_id       INTEGER NOT NULL REFERENCES terminal_hosts(id),
        title         TEXT NOT NULL DEFAULT 'bash',
        cols          INTEGER NOT NULL DEFAULT 80,
        rows          INTEGER NOT NULL DEFAULT 24,
        status        TEXT NOT NULL DEFAULT 'alive',
        recording     INTEGER NOT NULL DEFAULT 0,
        created_at    INTEGER NOT NULL,
        last_active   INTEGER NOT NULL
    );

    CREATE TABLE IF NOT EXISTS terminal_snippets (
        id            INTEGER PRIMARY KEY AUTOINCREMENT,
        name          TEXT NOT NULL,
        command       TEXT NOT NULL,
        variables     TEXT DEFAULT '[]',
        tags          TEXT DEFAULT '[]',
        folder        TEXT DEFAULT '',
        created_at    INTEGER NOT NULL
    );

    CREATE TABLE IF NOT EXISTS terminal_recordings (
        id            INTEGER PRIMARY KEY AUTOINCREMENT,
        session_id    TEXT NOT NULL REFERENCES terminal_sessions(id),
        started_at    INTEGER NOT NULL,
        ended_at      INTEGER,
        size_bytes    INTEGER NOT NULL DEFAULT 0,
        file_path     TEXT NOT NULL
    );

    CREATE TABLE IF NOT EXISTS terminal_port_forwards (
        id            INTEGER PRIMARY KEY AUTOINCREMENT,
        host_id       INTEGER NOT NULL REFERENCES terminal_hosts(id),
        direction     TEXT NOT NULL,
        bind_host     TEXT NOT NULL DEFAULT '127.0.0.1',
        bind_port     INTEGER NOT NULL,
        target_host   TEXT NOT NULL,
        target_port   INTEGER NOT NULL,
        auto_start    INTEGER NOT NULL DEFAULT 0,
        created_at    INTEGER NOT NULL
    );
    "#,

    // Migration 27: Per-agent knowledge scoping.
    // Each document belongs to an agent (or 'shared' for cross-agent access).
    // Main agent can search all; other agents see only their own + shared.
    r#"
    ALTER TABLE documents ADD COLUMN agent_id TEXT NOT NULL DEFAULT 'shared';
    CREATE INDEX IF NOT EXISTS idx_documents_agent ON documents(agent_id);
    "#,

    // Migration 28: Multi-user accounts.
    // Role-based admin (replaces hardcoded id==1), per-user passwords,
    // user-owned agents, data sharing controls, invite system.
    r#"
    ALTER TABLE users ADD COLUMN role TEXT NOT NULL DEFAULT 'user';
    ALTER TABLE users ADD COLUMN password_hash TEXT;

    -- Existing user 1 becomes admin
    UPDATE users SET role = 'admin' WHERE id = 1;

    -- Per-user agents (overlay on system agents from config)
    CREATE TABLE IF NOT EXISTS user_agents (
        id           INTEGER PRIMARY KEY AUTOINCREMENT,
        user_id      INTEGER NOT NULL REFERENCES users(id) ON DELETE CASCADE,
        agent_id     TEXT NOT NULL,
        display_name TEXT NOT NULL,
        base_agent   TEXT NOT NULL DEFAULT 'main',
        system_prompt TEXT,
        workspace    TEXT,
        tool_profile TEXT DEFAULT 'full',
        enabled      INTEGER NOT NULL DEFAULT 1,
        created_at   INTEGER NOT NULL,
        updated_at   INTEGER NOT NULL,
        UNIQUE(user_id, agent_id)
    );
    CREATE INDEX IF NOT EXISTS idx_user_agents_user ON user_agents(user_id);

    -- Data sharing mode: shared (legacy), isolated, or selective
    CREATE TABLE IF NOT EXISTS sharing_config (
        id         INTEGER PRIMARY KEY AUTOINCREMENT,
        mode       TEXT NOT NULL DEFAULT 'shared',
        updated_at INTEGER NOT NULL,
        updated_by INTEGER
    );
    INSERT INTO sharing_config (mode, updated_at) VALUES ('shared', strftime('%s','now'));

    -- Selective sharing grants (admin → other users)
    CREATE TABLE IF NOT EXISTS sharing_grants (
        id              INTEGER PRIMARY KEY AUTOINCREMENT,
        grantor_user_id INTEGER NOT NULL REFERENCES users(id),
        grantee_user_id INTEGER NOT NULL REFERENCES users(id),
        resource_type   TEXT NOT NULL,
        resource_id     TEXT,
        created_at      INTEGER NOT NULL,
        UNIQUE(grantor_user_id, grantee_user_id, resource_type, resource_id)
    );
    CREATE INDEX IF NOT EXISTS idx_sharing_grants_grantee ON sharing_grants(grantee_user_id);

    -- Per-user knowledge isolation
    ALTER TABLE documents ADD COLUMN user_id INTEGER NOT NULL DEFAULT 0;
    CREATE INDEX IF NOT EXISTS idx_documents_user ON documents(user_id);

    -- Invite codes for adding new users
    CREATE TABLE IF NOT EXISTS user_invites (
        id          INTEGER PRIMARY KEY AUTOINCREMENT,
        code        TEXT NOT NULL UNIQUE,
        created_by  INTEGER NOT NULL REFERENCES users(id),
        name_hint   TEXT,
        role        TEXT NOT NULL DEFAULT 'user',
        expires_at  INTEGER NOT NULL,
        consumed_at INTEGER,
        consumed_by INTEGER REFERENCES users(id),
        created_at  INTEGER NOT NULL
    );
    CREATE INDEX IF NOT EXISTS idx_user_invites_code ON user_invites(code);
    "#,

    // Migration 29: Personality docs, invite sharing presets, onboarding flag.
    r#"
    -- Per-user AI personality documents (bio, preferences, writing samples)
    CREATE TABLE IF NOT EXISTS user_personality_docs (
        id         INTEGER PRIMARY KEY AUTOINCREMENT,
        user_id    INTEGER NOT NULL REFERENCES users(id) ON DELETE CASCADE,
        agent_id   TEXT NOT NULL,
        doc_type   TEXT NOT NULL,
        title      TEXT NOT NULL,
        content    TEXT NOT NULL,
        created_at INTEGER NOT NULL,
        updated_at INTEGER NOT NULL
    );
    CREATE INDEX IF NOT EXISTS idx_personality_user ON user_personality_docs(user_id, agent_id);

    -- Sharing preset on invites (JSON array of grants to auto-create)
    ALTER TABLE user_invites ADD COLUMN sharing_preset TEXT;

    -- Onboarding completion flag
    ALTER TABLE users ADD COLUMN onboarding_complete INTEGER NOT NULL DEFAULT 0;
    -- Existing users are already onboarded
    UPDATE users SET onboarding_complete = 1;
    "#,

    // Migration 30: Per-user data directory.
    r#"
    ALTER TABLE users ADD COLUMN data_dir TEXT;
    "#,

    // Migration 31: Tax credits + quarterly estimated payments + planning.
    r#"
    -- Computed tax credits
    CREATE TABLE IF NOT EXISTS tax_credits (
        id                INTEGER PRIMARY KEY AUTOINCREMENT,
        user_id           INTEGER NOT NULL,
        tax_year          INTEGER NOT NULL,
        credit_type       TEXT NOT NULL,
        amount_cents      INTEGER NOT NULL DEFAULT 0,
        phase_out_cents   INTEGER NOT NULL DEFAULT 0,
        qualifying_data   TEXT DEFAULT '{}',
        form_ref          TEXT,
        created_at        INTEGER NOT NULL,
        UNIQUE(user_id, tax_year, credit_type)
    );

    -- Education expenses for AOTC / LLC
    CREATE TABLE IF NOT EXISTS education_expenses (
        id                INTEGER PRIMARY KEY AUTOINCREMENT,
        user_id           INTEGER NOT NULL,
        tax_year          INTEGER NOT NULL,
        student_name      TEXT NOT NULL,
        institution       TEXT NOT NULL,
        tuition_cents     INTEGER NOT NULL DEFAULT 0,
        fees_cents        INTEGER NOT NULL DEFAULT 0,
        books_cents       INTEGER NOT NULL DEFAULT 0,
        form_1098t_doc_id INTEGER,
        created_at        INTEGER NOT NULL
    );

    -- Dependent care expenses for CDCC (Form 2441)
    CREATE TABLE IF NOT EXISTS dependent_care_expenses (
        id                INTEGER PRIMARY KEY AUTOINCREMENT,
        user_id           INTEGER NOT NULL,
        tax_year          INTEGER NOT NULL,
        provider_name     TEXT NOT NULL,
        provider_tin      TEXT,
        amount_cents      INTEGER NOT NULL DEFAULT 0,
        dependent_id      INTEGER,
        created_at        INTEGER NOT NULL
    );

    -- Energy improvements for residential energy credit
    CREATE TABLE IF NOT EXISTS energy_improvements (
        id                INTEGER PRIMARY KEY AUTOINCREMENT,
        user_id           INTEGER NOT NULL,
        tax_year          INTEGER NOT NULL,
        improvement_type  TEXT NOT NULL,
        cost_cents        INTEGER NOT NULL DEFAULT 0,
        qualifying_cents  INTEGER NOT NULL DEFAULT 0,
        property_id       INTEGER,
        vendor            TEXT,
        created_at        INTEGER NOT NULL
    );

    -- Quarterly estimated tax payments
    CREATE TABLE IF NOT EXISTS estimated_tax_payments (
        id                INTEGER PRIMARY KEY AUTOINCREMENT,
        user_id           INTEGER NOT NULL,
        tax_year          INTEGER NOT NULL,
        quarter           INTEGER NOT NULL,
        amount_cents      INTEGER NOT NULL DEFAULT 0,
        payment_date      TEXT,
        payment_method    TEXT,
        confirmation_id   TEXT,
        status            TEXT NOT NULL DEFAULT 'pending',
        created_at        INTEGER NOT NULL,
        UNIQUE(user_id, tax_year, quarter)
    );

    -- Tax projections / what-if scenarios
    CREATE TABLE IF NOT EXISTS tax_projections (
        id                INTEGER PRIMARY KEY AUTOINCREMENT,
        user_id           INTEGER NOT NULL,
        tax_year          INTEGER NOT NULL,
        scenario_name     TEXT NOT NULL DEFAULT 'baseline',
        parameters_json   TEXT DEFAULT '{}',
        result_json       TEXT DEFAULT '{}',
        created_at        INTEGER NOT NULL
    );
    "#,

    // Migration 32: MACRS depreciation engine.
    r#"
    -- Depreciable assets (equipment, vehicles, buildings, improvements)
    CREATE TABLE IF NOT EXISTS depreciable_assets (
        id                  INTEGER PRIMARY KEY AUTOINCREMENT,
        user_id             INTEGER NOT NULL,
        description         TEXT NOT NULL,
        asset_class         TEXT NOT NULL,
        macrs_life_years    INTEGER NOT NULL,
        convention          TEXT NOT NULL DEFAULT 'half_year',
        cost_basis_cents    INTEGER NOT NULL,
        placed_in_service   TEXT NOT NULL,
        business_use_pct    INTEGER NOT NULL DEFAULT 100,
        section_179_cents   INTEGER NOT NULL DEFAULT 0,
        bonus_depr_cents    INTEGER NOT NULL DEFAULT 0,
        prior_depr_cents    INTEGER NOT NULL DEFAULT 0,
        property_id         INTEGER,
        is_vehicle          INTEGER NOT NULL DEFAULT 0,
        disposed_date       TEXT,
        disposition_type    TEXT,
        sale_proceeds_cents INTEGER,
        status              TEXT NOT NULL DEFAULT 'active',
        created_at          INTEGER NOT NULL,
        updated_at          INTEGER NOT NULL
    );
    CREATE INDEX IF NOT EXISTS idx_assets_user ON depreciable_assets(user_id);

    -- Per-year depreciation schedule (auto-computed)
    CREATE TABLE IF NOT EXISTS depreciation_schedule (
        id                  INTEGER PRIMARY KEY AUTOINCREMENT,
        asset_id            INTEGER NOT NULL REFERENCES depreciable_assets(id) ON DELETE CASCADE,
        tax_year            INTEGER NOT NULL,
        depreciation_cents  INTEGER NOT NULL,
        method              TEXT NOT NULL DEFAULT 'MACRS_GDS',
        accumulated_cents   INTEGER NOT NULL DEFAULT 0,
        remaining_cents     INTEGER NOT NULL DEFAULT 0,
        UNIQUE(asset_id, tax_year)
    );

    -- Vehicle mileage log
    CREATE TABLE IF NOT EXISTS vehicle_usage (
        id                  INTEGER PRIMARY KEY AUTOINCREMENT,
        user_id             INTEGER NOT NULL,
        tax_year            INTEGER NOT NULL,
        vehicle_description TEXT NOT NULL,
        total_miles         INTEGER NOT NULL DEFAULT 0,
        business_miles      INTEGER NOT NULL DEFAULT 0,
        commuting_miles     INTEGER NOT NULL DEFAULT 0,
        personal_miles      INTEGER NOT NULL DEFAULT 0,
        standard_rate_cents INTEGER NOT NULL DEFAULT 70,
        actual_expenses_cents INTEGER NOT NULL DEFAULT 0,
        method_used         TEXT NOT NULL DEFAULT 'standard',
        asset_id            INTEGER,
        created_at          INTEGER NOT NULL,
        UNIQUE(user_id, tax_year, vehicle_description)
    );
    "#,

    // Migration 33: Investment tax engine — cost basis, wash sales, K-1, carryforward.
    r#"
    -- Per-lot cost basis tracking
    CREATE TABLE IF NOT EXISTS tax_lots (
        id                  INTEGER PRIMARY KEY AUTOINCREMENT,
        user_id             INTEGER NOT NULL,
        symbol              TEXT NOT NULL,
        asset_type          TEXT NOT NULL DEFAULT 'stock',
        quantity            REAL NOT NULL,
        cost_per_unit_cents INTEGER NOT NULL,
        acquisition_date    TEXT NOT NULL,
        acquisition_method  TEXT NOT NULL DEFAULT 'purchase',
        account_id          INTEGER,
        broker              TEXT,
        adjusted_basis_cents INTEGER,
        wash_sale_adj_cents INTEGER NOT NULL DEFAULT 0,
        status              TEXT NOT NULL DEFAULT 'open',
        created_at          INTEGER NOT NULL
    );
    CREATE INDEX IF NOT EXISTS idx_lots_user ON tax_lots(user_id, symbol);

    -- Lot dispositions (sales, exchanges)
    CREATE TABLE IF NOT EXISTS lot_dispositions (
        id                  INTEGER PRIMARY KEY AUTOINCREMENT,
        lot_id              INTEGER NOT NULL REFERENCES tax_lots(id),
        user_id             INTEGER NOT NULL,
        sell_date           TEXT NOT NULL,
        quantity_sold       REAL NOT NULL,
        proceeds_cents      INTEGER NOT NULL,
        cost_basis_cents    INTEGER NOT NULL,
        wash_sale_adj_cents INTEGER NOT NULL DEFAULT 0,
        gain_loss_cents     INTEGER NOT NULL,
        holding_period      TEXT NOT NULL,
        form_8949_code      TEXT NOT NULL DEFAULT 'A',
        created_at          INTEGER NOT NULL
    );
    CREATE INDEX IF NOT EXISTS idx_dispositions_user ON lot_dispositions(user_id);

    -- Wash sale matches (30-day rule)
    CREATE TABLE IF NOT EXISTS wash_sale_matches (
        id                  INTEGER PRIMARY KEY AUTOINCREMENT,
        user_id             INTEGER NOT NULL,
        sell_disposition_id INTEGER NOT NULL REFERENCES lot_dispositions(id),
        replacement_lot_id  INTEGER NOT NULL REFERENCES tax_lots(id),
        disallowed_cents    INTEGER NOT NULL,
        basis_adj_cents     INTEGER NOT NULL,
        created_at          INTEGER NOT NULL
    );

    -- K-1 income from partnerships/trusts/S-corps
    CREATE TABLE IF NOT EXISTS k1_income (
        id                  INTEGER PRIMARY KEY AUTOINCREMENT,
        user_id             INTEGER NOT NULL,
        tax_year            INTEGER NOT NULL,
        entity_name         TEXT NOT NULL,
        entity_type         TEXT NOT NULL DEFAULT 'partnership',
        ordinary_cents      INTEGER NOT NULL DEFAULT 0,
        rental_cents        INTEGER NOT NULL DEFAULT 0,
        interest_cents      INTEGER NOT NULL DEFAULT 0,
        dividend_cents      INTEGER NOT NULL DEFAULT 0,
        capital_gain_cents  INTEGER NOT NULL DEFAULT 0,
        section_179_cents   INTEGER NOT NULL DEFAULT 0,
        se_income_cents     INTEGER NOT NULL DEFAULT 0,
        other_json          TEXT DEFAULT '{}',
        created_at          INTEGER NOT NULL
    );

    -- Capital loss carryforward across years
    CREATE TABLE IF NOT EXISTS capital_loss_carryforward (
        id                  INTEGER PRIMARY KEY AUTOINCREMENT,
        user_id             INTEGER NOT NULL,
        tax_year            INTEGER NOT NULL,
        short_term_cents    INTEGER NOT NULL DEFAULT 0,
        long_term_cents     INTEGER NOT NULL DEFAULT 0,
        created_at          INTEGER NOT NULL,
        UNIQUE(user_id, tax_year)
    );
    "#,

    // Migration 34: AI tax advisor — insights, audit risk, scenarios.
    r#"
    CREATE TABLE IF NOT EXISTS tax_insights (
        id                  INTEGER PRIMARY KEY AUTOINCREMENT,
        user_id             INTEGER NOT NULL,
        tax_year            INTEGER NOT NULL,
        insight_type        TEXT NOT NULL,
        title               TEXT NOT NULL,
        body                TEXT NOT NULL,
        estimated_savings_cents INTEGER NOT NULL DEFAULT 0,
        priority            INTEGER NOT NULL DEFAULT 5,
        status              TEXT NOT NULL DEFAULT 'new',
        dismissed_at        INTEGER,
        created_at          INTEGER NOT NULL
    );
    CREATE INDEX IF NOT EXISTS idx_insights_user ON tax_insights(user_id, tax_year);

    CREATE TABLE IF NOT EXISTS audit_risk_factors (
        id                  INTEGER PRIMARY KEY AUTOINCREMENT,
        user_id             INTEGER NOT NULL,
        tax_year            INTEGER NOT NULL,
        factor_type         TEXT NOT NULL,
        description         TEXT NOT NULL,
        risk_level          TEXT NOT NULL DEFAULT 'low',
        details_json        TEXT DEFAULT '{}',
        created_at          INTEGER NOT NULL
    );

    CREATE TABLE IF NOT EXISTS tax_scenarios (
        id                  INTEGER PRIMARY KEY AUTOINCREMENT,
        user_id             INTEGER NOT NULL,
        tax_year            INTEGER NOT NULL,
        scenario_name       TEXT NOT NULL,
        parameters_json     TEXT NOT NULL DEFAULT '{}',
        baseline_json       TEXT NOT NULL DEFAULT '{}',
        result_json         TEXT NOT NULL DEFAULT '{}',
        difference_json     TEXT NOT NULL DEFAULT '{}',
        created_at          INTEGER NOT NULL
    );
    "#,

    // Migration 35: State income tax engine.
    r#"
    CREATE TABLE IF NOT EXISTS state_tax_profiles (
        id                  INTEGER PRIMARY KEY AUTOINCREMENT,
        user_id             INTEGER NOT NULL,
        tax_year            INTEGER NOT NULL,
        state               TEXT NOT NULL,
        residency_type      TEXT NOT NULL DEFAULT 'full_year',
        months_resident     INTEGER NOT NULL DEFAULT 12,
        state_wages_cents   INTEGER NOT NULL DEFAULT 0,
        state_withheld_cents INTEGER NOT NULL DEFAULT 0,
        created_at          INTEGER NOT NULL,
        updated_at          INTEGER NOT NULL,
        UNIQUE(user_id, tax_year, state)
    );

    CREATE TABLE IF NOT EXISTS state_tax_estimates (
        id                  INTEGER PRIMARY KEY AUTOINCREMENT,
        user_id             INTEGER NOT NULL,
        tax_year            INTEGER NOT NULL,
        state               TEXT NOT NULL,
        state_agi_cents     INTEGER NOT NULL DEFAULT 0,
        state_taxable_cents INTEGER NOT NULL DEFAULT 0,
        state_tax_cents     INTEGER NOT NULL DEFAULT 0,
        state_credits_cents INTEGER NOT NULL DEFAULT 0,
        state_withheld_cents INTEGER NOT NULL DEFAULT 0,
        state_owed_cents    INTEGER NOT NULL DEFAULT 0,
        effective_rate      REAL NOT NULL DEFAULT 0.0,
        created_at          INTEGER NOT NULL,
        UNIQUE(user_id, tax_year, state)
    );
    "#,

    // Migration 36: Business entities — S-Corp, Partnership, C-Corp.
    r#"
    CREATE TABLE IF NOT EXISTS business_entities (
        id                  INTEGER PRIMARY KEY AUTOINCREMENT,
        user_id             INTEGER NOT NULL,
        entity_name         TEXT NOT NULL,
        entity_type         TEXT NOT NULL,
        ein_encrypted       TEXT,
        formation_date      TEXT,
        state_of_formation  TEXT,
        fiscal_year_end     TEXT DEFAULT '12-31',
        ownership_pct       INTEGER NOT NULL DEFAULT 100,
        status              TEXT NOT NULL DEFAULT 'active',
        created_at          INTEGER NOT NULL,
        updated_at          INTEGER NOT NULL
    );
    CREATE INDEX IF NOT EXISTS idx_entities_user ON business_entities(user_id);

    CREATE TABLE IF NOT EXISTS entity_income (
        id                  INTEGER PRIMARY KEY AUTOINCREMENT,
        entity_id           INTEGER NOT NULL REFERENCES business_entities(id) ON DELETE CASCADE,
        tax_year            INTEGER NOT NULL,
        income_type         TEXT NOT NULL,
        amount_cents        INTEGER NOT NULL,
        description         TEXT,
        created_at          INTEGER NOT NULL
    );

    CREATE TABLE IF NOT EXISTS entity_expenses (
        id                  INTEGER PRIMARY KEY AUTOINCREMENT,
        entity_id           INTEGER NOT NULL REFERENCES business_entities(id) ON DELETE CASCADE,
        tax_year            INTEGER NOT NULL,
        category            TEXT NOT NULL,
        amount_cents        INTEGER NOT NULL,
        vendor              TEXT,
        expense_date        TEXT,
        description         TEXT,
        created_at          INTEGER NOT NULL
    );

    CREATE TABLE IF NOT EXISTS entity_shareholders (
        id                  INTEGER PRIMARY KEY AUTOINCREMENT,
        entity_id           INTEGER NOT NULL REFERENCES business_entities(id) ON DELETE CASCADE,
        name                TEXT NOT NULL,
        ssn_encrypted       TEXT,
        ownership_pct       INTEGER NOT NULL,
        distribution_cents  INTEGER NOT NULL DEFAULT 0,
        salary_cents        INTEGER NOT NULL DEFAULT 0,
        tax_year            INTEGER NOT NULL,
        created_at          INTEGER NOT NULL,
        UNIQUE(entity_id, name, tax_year)
    );

    CREATE TABLE IF NOT EXISTS entity_1099s (
        id                  INTEGER PRIMARY KEY AUTOINCREMENT,
        entity_id           INTEGER NOT NULL REFERENCES business_entities(id) ON DELETE CASCADE,
        tax_year            INTEGER NOT NULL,
        recipient_name      TEXT NOT NULL,
        recipient_tin       TEXT,
        recipient_address   TEXT,
        amount_cents        INTEGER NOT NULL,
        form_type           TEXT NOT NULL DEFAULT '1099-NEC',
        status              TEXT NOT NULL DEFAULT 'draft',
        created_at          INTEGER NOT NULL
    );
    "#,

    // Migration 37: Module agent defaults table — seed registry for the 8
    // canonical personas (Peter/Kyron/Positron/Cortex/Silvr/Thaddeus/Maurice/Mushi).
    // Rows are upserted on gateway startup from src/agents/defaults.rs.
    r#"
    CREATE TABLE IF NOT EXISTS module_agent_defaults (
        id                       INTEGER PRIMARY KEY AUTOINCREMENT,
        agent_key                TEXT NOT NULL UNIQUE,
        module_name              TEXT,
        default_display_name     TEXT NOT NULL,
        easter_egg_inspiration   TEXT NOT NULL,
        system_prompt_template   TEXT NOT NULL,
        tone_dials_json          TEXT NOT NULL,
        memory_scope_json        TEXT NOT NULL,
        public_role              TEXT NOT NULL,
        configurable_humor_dial  INTEGER NOT NULL DEFAULT 0,
        default_humor_value      INTEGER,
        created_at               INTEGER NOT NULL,
        updated_at               INTEGER NOT NULL
    );
    CREATE INDEX IF NOT EXISTS idx_mad_module ON module_agent_defaults(module_name);
    "#,

    // Migration 38: Voice identity — per-user wake words, voiceprint embeddings,
    // TTS voice clones, and house-level voice defaults for the multi-user
    // satellite architecture. See vault/projects/syntaur_personas.md for design.
    r#"
    CREATE TABLE IF NOT EXISTS user_voice_models (
        id                      INTEGER PRIMARY KEY AUTOINCREMENT,
        user_id                 INTEGER NOT NULL REFERENCES users(id) ON DELETE CASCADE,
        wake_word_name          TEXT NOT NULL,
        wake_model_path         TEXT,
        voiceprint_embedding    BLOB,
        voiceprint_confidence   REAL NOT NULL DEFAULT 0.0,
        tts_voice_sample_path   TEXT,
        tts_model_path          TEXT,
        satellite_id            TEXT,
        enabled                 INTEGER NOT NULL DEFAULT 1,
        created_at              INTEGER NOT NULL,
        updated_at              INTEGER NOT NULL,
        UNIQUE(user_id, wake_word_name)
    );
    CREATE INDEX IF NOT EXISTS idx_voice_user ON user_voice_models(user_id);
    CREATE INDEX IF NOT EXISTS idx_voice_satellite ON user_voice_models(satellite_id);

    CREATE TABLE IF NOT EXISTS voice_settings (
        key         TEXT PRIMARY KEY,
        value       TEXT NOT NULL,
        updated_at  INTEGER NOT NULL
    );

    INSERT OR IGNORE INTO voice_settings (key, value, updated_at) VALUES
        ('default_tts_voice', 'system', 0),
        ('default_wake_word', 'Hey Kyron', 0);
    "#,

    // Migration 39: Agent memories — persistent, typed, cross-session knowledge
    // that agents accumulate and recall. Topic-organized, FTS5-searchable,
    // per-user + per-agent scoped with controlled sharing.
    // See vault/research/agent_memory_architecture.md for full design.
    r#"
    CREATE TABLE IF NOT EXISTS agent_memories (
        id                      INTEGER PRIMARY KEY AUTOINCREMENT,
        user_id                 INTEGER NOT NULL REFERENCES users(id) ON DELETE CASCADE,
        agent_id                TEXT NOT NULL,
        memory_type             TEXT NOT NULL,
        key                     TEXT NOT NULL,
        title                   TEXT NOT NULL,
        description             TEXT,
        content                 TEXT NOT NULL,
        tags                    TEXT,
        source                  TEXT NOT NULL DEFAULT 'agent_learned',
        source_conversation_id  TEXT,
        confidence              REAL NOT NULL DEFAULT 1.0,
        importance              INTEGER NOT NULL DEFAULT 5,
        access_count            INTEGER NOT NULL DEFAULT 0,
        last_accessed_at        INTEGER,
        shared                  INTEGER NOT NULL DEFAULT 0,
        created_at              INTEGER NOT NULL,
        updated_at              INTEGER NOT NULL,
        expires_at              INTEGER,
        UNIQUE(user_id, agent_id, key)
    );
    CREATE INDEX IF NOT EXISTS idx_mem_user_agent ON agent_memories(user_id, agent_id);
    CREATE INDEX IF NOT EXISTS idx_mem_type ON agent_memories(memory_type);
    CREATE INDEX IF NOT EXISTS idx_mem_shared ON agent_memories(shared);

    CREATE VIRTUAL TABLE IF NOT EXISTS agent_memories_fts USING fts5(
        title, description, content, tags,
        content=agent_memories, content_rowid=id
    );
    CREATE TRIGGER IF NOT EXISTS agent_memories_ai AFTER INSERT ON agent_memories BEGIN
        INSERT INTO agent_memories_fts(rowid, title, description, content, tags)
        VALUES (new.id, new.title, COALESCE(new.description,''), new.content, COALESCE(new.tags,''));
    END;
    CREATE TRIGGER IF NOT EXISTS agent_memories_au AFTER UPDATE ON agent_memories BEGIN
        INSERT INTO agent_memories_fts(agent_memories_fts, rowid, title, description, content, tags)
        VALUES ('delete', old.id, old.title, COALESCE(old.description,''), old.content, COALESCE(old.tags,''));
        INSERT INTO agent_memories_fts(rowid, title, description, content, tags)
        VALUES (new.id, new.title, COALESCE(new.description,''), new.content, COALESCE(new.tags,''));
    END;
    CREATE TRIGGER IF NOT EXISTS agent_memories_ad AFTER DELETE ON agent_memories BEGIN
        INSERT INTO agent_memories_fts(agent_memories_fts, rowid, title, description, content, tags)
        VALUES ('delete', old.id, old.title, COALESCE(old.description,''), old.content, COALESCE(old.tags,''));
    END;
    "#,
    // ── v40 ──────────────────────────────────────────────────────────────
    // Multi main-agent support + descriptions / avatar color. Any agent with
    // is_main_thread = 1 is eligible for the dashboard's main-thread picker
    // and gets Peter/Kyron-tier privileges (cross-module reads, handoff
    // targets). Existing user_agents rows stay single-main by default;
    // users can promote / create additional main-thread agents via the
    // Settings → Agents page.
    r#"
    ALTER TABLE user_agents ADD COLUMN is_main_thread INTEGER NOT NULL DEFAULT 0;
    ALTER TABLE user_agents ADD COLUMN description TEXT;
    ALTER TABLE user_agents ADD COLUMN avatar_color TEXT;
    ALTER TABLE user_agents ADD COLUMN imported_from TEXT;

    -- Seed: any row whose base_agent is 'main' (or whose agent_id = 'main')
    -- gets main-thread privilege automatically so the existing Peter / Felix
    -- / Kyron continues to work without a manual settings migration.
    UPDATE user_agents SET is_main_thread = 1
     WHERE base_agent = 'main' OR agent_id = 'main';
    "#,
    // ── v41 ──────────────────────────────────────────────────────────────
    // Per-user preferences key/value store. Powers the Privacy toggles
    // (telemetry, LLM logging, memory retention, etc.) and any other
    // per-user UI flags. Small + simple: one row per (user, key).
    r#"
    CREATE TABLE IF NOT EXISTS user_preferences (
        user_id    INTEGER NOT NULL REFERENCES users(id) ON DELETE CASCADE,
        key        TEXT NOT NULL,
        value      TEXT,
        updated_at INTEGER NOT NULL,
        PRIMARY KEY (user_id, key)
    );
    CREATE INDEX IF NOT EXISTS idx_user_prefs_user ON user_preferences(user_id);
    "#,
    // ── v42 ──────────────────────────────────────────────────────────────
    // Journal moments — fragments the user has starred while reviewing a
    // day's entries. Lives fully inside journal isolation: no foreign keys
    // out to conversations, nothing that another module could join on.
    // Just text + date + optional metadata, owned by the user.
    r#"
    CREATE TABLE IF NOT EXISTS journal_moments (
        id          INTEGER PRIMARY KEY AUTOINCREMENT,
        user_id     INTEGER NOT NULL REFERENCES users(id) ON DELETE CASCADE,
        date        TEXT NOT NULL,
        text        TEXT NOT NULL,
        source      TEXT,
        time_of_day TEXT,
        note        TEXT,
        created_at  INTEGER NOT NULL
    );
    CREATE INDEX IF NOT EXISTS idx_journal_moments_user ON journal_moments(user_id, created_at DESC);
    CREATE INDEX IF NOT EXISTS idx_journal_moments_date ON journal_moments(user_id, date);
    "#,
    // ── v43 ──────────────────────────────────────────────────────────────
    // Local music library — folders the user has added as sources, and the
    // tracks indexed from inside them. `path` in local_music_tracks is an
    // absolute path on the gateway host; the user_id owner gates file
    // access so a user can't stream another user's folders.
    r#"
    CREATE TABLE IF NOT EXISTS local_music_folders (
        id         INTEGER PRIMARY KEY AUTOINCREMENT,
        user_id    INTEGER NOT NULL REFERENCES users(id) ON DELETE CASCADE,
        path       TEXT NOT NULL,
        label      TEXT,
        added_at   INTEGER NOT NULL,
        last_scan_at INTEGER,
        track_count INTEGER NOT NULL DEFAULT 0,
        UNIQUE (user_id, path)
    );
    CREATE INDEX IF NOT EXISTS idx_local_folders_user ON local_music_folders(user_id);

    CREATE TABLE IF NOT EXISTS local_music_tracks (
        id          INTEGER PRIMARY KEY AUTOINCREMENT,
        user_id     INTEGER NOT NULL REFERENCES users(id) ON DELETE CASCADE,
        folder_id   INTEGER NOT NULL REFERENCES local_music_folders(id) ON DELETE CASCADE,
        path        TEXT NOT NULL,
        title       TEXT,
        artist      TEXT,
        album       TEXT,
        duration_ms INTEGER,
        track_no    INTEGER,
        year        INTEGER,
        indexed_at  INTEGER NOT NULL
    );
    CREATE INDEX IF NOT EXISTS idx_local_tracks_user   ON local_music_tracks(user_id, artist, album, track_no);
    CREATE INDEX IF NOT EXISTS idx_local_tracks_folder ON local_music_tracks(folder_id);
    CREATE UNIQUE INDEX IF NOT EXISTS uniq_local_tracks_user_path ON local_music_tracks(user_id, path);
    "#,
    // ── v44 ──────────────────────────────────────────────────────────────
    // Social-module platform connections. Each row represents one connected
    // platform (Bluesky, Threads, YouTube, etc.) for one user. `credentials_json`
    // holds the platform-specific auth blob (app password, OAuth tokens, etc.);
    // plaintext for v1 — align with the rest of the SQLite storage posture.
    // `agent_id` is optional so a multi-agent user can have e.g. a Bluesky
    // connection for their artist persona and a different one for their
    // business persona, keyed independently.
    r#"
    CREATE TABLE IF NOT EXISTS social_connections (
        id                INTEGER PRIMARY KEY AUTOINCREMENT,
        user_id           INTEGER NOT NULL REFERENCES users(id) ON DELETE CASCADE,
        platform          TEXT    NOT NULL,
        display_name      TEXT,
        credentials_json  TEXT    NOT NULL,
        status            TEXT    NOT NULL DEFAULT 'connected',
        status_detail     TEXT,
        agent_id          TEXT,
        connected_at      INTEGER NOT NULL,
        last_verified_at  INTEGER,
        expires_at        INTEGER,
        created_at        INTEGER NOT NULL,
        updated_at        INTEGER NOT NULL
    );
    CREATE INDEX IF NOT EXISTS idx_social_conn_user     ON social_connections(user_id);
    CREATE INDEX IF NOT EXISTS idx_social_conn_platform ON social_connections(user_id, platform);
    "#,
    // ── v45 ──────────────────────────────────────────────────────────────
    // Social module runtime: drafts, replies, engagement log, stats
    // snapshots. Each per-user with cascade-on-delete. Source = 'auto'
    // (from cron/Nyota) or 'manual' (Compose pane). Status progresses
    // pending → posted (or rejected/failed). Telegram columns let us
    // mirror draft cards to the phone + update them on state change.
    r#"
    CREATE TABLE IF NOT EXISTS social_drafts (
        id               INTEGER PRIMARY KEY AUTOINCREMENT,
        user_id          INTEGER NOT NULL REFERENCES users(id) ON DELETE CASCADE,
        platform         TEXT    NOT NULL,
        agent_id         TEXT,
        connection_id    INTEGER REFERENCES social_connections(id) ON DELETE SET NULL,
        text             TEXT    NOT NULL,
        pillar           TEXT,
        source           TEXT    NOT NULL DEFAULT 'auto',
        status           TEXT    NOT NULL DEFAULT 'pending',
        scheduled_for    INTEGER,
        posted_uri       TEXT,
        posted_at        INTEGER,
        error_detail     TEXT,
        telegram_message_id INTEGER,
        telegram_chat_id    INTEGER,
        created_at       INTEGER NOT NULL,
        updated_at       INTEGER NOT NULL
    );
    CREATE INDEX IF NOT EXISTS idx_social_drafts_user_status ON social_drafts(user_id, status);
    CREATE INDEX IF NOT EXISTS idx_social_drafts_sched       ON social_drafts(status, scheduled_for);

    CREATE TABLE IF NOT EXISTS social_replies (
        id               INTEGER PRIMARY KEY AUTOINCREMENT,
        user_id          INTEGER NOT NULL REFERENCES users(id) ON DELETE CASCADE,
        platform         TEXT    NOT NULL,
        agent_id         TEXT,
        parent_uri       TEXT    NOT NULL,
        parent_author    TEXT,
        parent_text      TEXT,
        draft_text       TEXT,
        status           TEXT    NOT NULL DEFAULT 'pending',
        posted_uri       TEXT,
        posted_at        INTEGER,
        telegram_message_id INTEGER,
        telegram_chat_id    INTEGER,
        created_at       INTEGER NOT NULL,
        updated_at       INTEGER NOT NULL
    );
    CREATE UNIQUE INDEX IF NOT EXISTS uniq_social_replies_parent ON social_replies(user_id, platform, parent_uri);

    CREATE TABLE IF NOT EXISTS social_engagement_log (
        id          INTEGER PRIMARY KEY AUTOINCREMENT,
        user_id     INTEGER NOT NULL REFERENCES users(id) ON DELETE CASCADE,
        platform    TEXT    NOT NULL,
        action      TEXT    NOT NULL,
        target_uri  TEXT    NOT NULL,
        target_info TEXT,
        created_at  INTEGER NOT NULL
    );
    CREATE INDEX IF NOT EXISTS idx_social_engagement_user_plat ON social_engagement_log(user_id, platform);
    CREATE INDEX IF NOT EXISTS idx_social_engagement_target    ON social_engagement_log(user_id, platform, target_uri);

    CREATE TABLE IF NOT EXISTS social_stats_snapshots (
        id               INTEGER PRIMARY KEY AUTOINCREMENT,
        user_id          INTEGER NOT NULL REFERENCES users(id) ON DELETE CASCADE,
        platform         TEXT    NOT NULL,
        as_of            INTEGER NOT NULL,
        followers        INTEGER,
        following        INTEGER,
        posts_count      INTEGER,
        likes_received   INTEGER,
        reposts_received INTEGER,
        replies_received INTEGER,
        extra_json       TEXT
    );
    CREATE INDEX IF NOT EXISTS idx_social_stats ON social_stats_snapshots(user_id, platform, as_of);

    CREATE TABLE IF NOT EXISTS social_pillar_cursor (
        user_id    INTEGER NOT NULL,
        platform   TEXT    NOT NULL,
        agent_id   TEXT    NOT NULL DEFAULT '',
        last_idx   INTEGER NOT NULL DEFAULT 0,
        updated_at INTEGER NOT NULL,
        PRIMARY KEY (user_id, platform, agent_id)
    );

    CREATE TABLE IF NOT EXISTS social_alerts (
        id           INTEGER PRIMARY KEY AUTOINCREMENT,
        user_id      INTEGER NOT NULL REFERENCES users(id) ON DELETE CASCADE,
        platform     TEXT    NOT NULL,
        alert_type   TEXT    NOT NULL,
        target_uri   TEXT,
        detail       TEXT    NOT NULL,
        acknowledged INTEGER NOT NULL DEFAULT 0,
        created_at   INTEGER NOT NULL
    );
    CREATE INDEX IF NOT EXISTS idx_social_alerts_open ON social_alerts(user_id, acknowledged);
    "#,

    // MACE scoped tokens — narrow the blast radius of MAURICE_TOKEN so a
    // leaked env only grants the four endpoints MACE actually needs.
    // `scopes` is a comma-separated list; empty string = unscoped (legacy
    // web-session tokens keep working without change).
    r#"
    ALTER TABLE user_api_tokens ADD COLUMN scopes TEXT NOT NULL DEFAULT '';
    "#,

    // Local music library — tag-cleanup support. `mbid` stores a
    // MusicBrainz recording ID once the user confirms a canonical match;
    // `metadata_source` records where the current title/artist/album
    // came from ('file_tags' = lofty read from file, 'llm' = auto-tag,
    // 'musicbrainz' = user accepted an MB match, 'user_edit' = manual
    // edit). Lets us surface "this was AI-inferred" badges + revert to
    // file tags.
    r#"
    ALTER TABLE local_music_tracks ADD COLUMN mbid TEXT;
    ALTER TABLE local_music_tracks ADD COLUMN metadata_source TEXT NOT NULL DEFAULT 'file_tags';
    "#,

    // Music module Tier 0 + 1 + 2 schema in one bundle:
    // • original_* columns preserve the file-tag snapshot so retag /
    //   MB-match / user-edit are all reversible.
    // • art_cache_key points at /config/art/<key>.jpg once an embedded
    //   picture or folder.jpg has been extracted; NULL = try MB Cover
    //   Art Archive via mbid next.
    // • bit_depth + sample_rate feed the lossless/format badge.
    // • play_count + last_played_at + favorite feed Recently Played /
    //   Most Played / Favorites views.
    // • Playlists tables (manual + smart-via-rule_json).
    // • Album-liner-notes + lyrics caches land as empty tables so the
    //   T3/T4 features don't need their own migration later.
    r#"
    ALTER TABLE local_music_tracks ADD COLUMN original_title TEXT;
    ALTER TABLE local_music_tracks ADD COLUMN original_artist TEXT;
    ALTER TABLE local_music_tracks ADD COLUMN original_album TEXT;
    ALTER TABLE local_music_tracks ADD COLUMN art_cache_key TEXT;
    ALTER TABLE local_music_tracks ADD COLUMN bit_depth INTEGER;
    ALTER TABLE local_music_tracks ADD COLUMN sample_rate INTEGER;
    ALTER TABLE local_music_tracks ADD COLUMN play_count INTEGER NOT NULL DEFAULT 0;
    ALTER TABLE local_music_tracks ADD COLUMN last_played_at INTEGER;
    ALTER TABLE local_music_tracks ADD COLUMN favorite INTEGER NOT NULL DEFAULT 0;
    UPDATE local_music_tracks SET original_title = title WHERE original_title IS NULL;
    UPDATE local_music_tracks SET original_artist = artist WHERE original_artist IS NULL;
    UPDATE local_music_tracks SET original_album = album WHERE original_album IS NULL;

    CREATE TABLE IF NOT EXISTS local_playlists (
        id           INTEGER PRIMARY KEY AUTOINCREMENT,
        user_id      INTEGER NOT NULL REFERENCES users(id) ON DELETE CASCADE,
        name         TEXT NOT NULL,
        kind         TEXT NOT NULL DEFAULT 'manual',
        rule_json    TEXT,
        created_at   INTEGER NOT NULL,
        updated_at   INTEGER NOT NULL
    );
    CREATE INDEX IF NOT EXISTS idx_local_playlists_user ON local_playlists(user_id);

    CREATE TABLE IF NOT EXISTS local_playlist_tracks (
        playlist_id  INTEGER NOT NULL REFERENCES local_playlists(id) ON DELETE CASCADE,
        track_id     INTEGER NOT NULL REFERENCES local_music_tracks(id) ON DELETE CASCADE,
        position     INTEGER NOT NULL,
        added_at     INTEGER NOT NULL,
        PRIMARY KEY (playlist_id, track_id)
    );
    CREATE INDEX IF NOT EXISTS idx_local_playlist_tracks_pos ON local_playlist_tracks(playlist_id, position);

    CREATE TABLE IF NOT EXISTS local_album_notes (
        id           INTEGER PRIMARY KEY AUTOINCREMENT,
        user_id      INTEGER NOT NULL REFERENCES users(id) ON DELETE CASCADE,
        artist       TEXT NOT NULL,
        album        TEXT NOT NULL,
        body         TEXT NOT NULL,
        generated_at INTEGER NOT NULL,
        UNIQUE (user_id, artist, album)
    );

    CREATE TABLE IF NOT EXISTS local_lyrics_cache (
        track_id     INTEGER PRIMARY KEY REFERENCES local_music_tracks(id) ON DELETE CASCADE,
        plain_text   TEXT,
        synced_lrc   TEXT,
        fetched_at   INTEGER NOT NULL
    );
    "#,

    // Scheduler module — Thaddeus's backing store. Covers the consent gate,
    // the one-observation-per-pattern rule, per-calendar subscriptions with
    // enable + write-enable flags, custom lists + items, habit tracking,
    // sticker placements, and per-user scheduler prefs. Existing tables
    // (calendar_events / todos / calendar_reminders_sent) stay and are
    // extended in place.
    r#"
    CREATE TABLE IF NOT EXISTS pending_approvals (
        id            INTEGER PRIMARY KEY AUTOINCREMENT,
        user_id       INTEGER NOT NULL REFERENCES users(id) ON DELETE CASCADE,
        kind          TEXT NOT NULL,
        source        TEXT NOT NULL DEFAULT '',
        summary       TEXT NOT NULL,
        payload_json  TEXT NOT NULL,
        reply_draft   TEXT,
        reply_sent_at INTEGER,
        created_at    INTEGER NOT NULL,
        resolved_at   INTEGER,
        resolution    TEXT
    );
    CREATE INDEX IF NOT EXISTS idx_pending_approvals_user   ON pending_approvals(user_id, resolved_at);
    CREATE INDEX IF NOT EXISTS idx_pending_approvals_source ON pending_approvals(source);

    CREATE TABLE IF NOT EXISTS detected_patterns (
        id           INTEGER PRIMARY KEY AUTOINCREMENT,
        user_id      INTEGER NOT NULL REFERENCES users(id) ON DELETE CASCADE,
        signature    TEXT NOT NULL,
        description  TEXT NOT NULL,
        confidence   REAL NOT NULL,
        first_seen   INTEGER NOT NULL,
        last_seen    INTEGER NOT NULL,
        surfaced_at  INTEGER,
        dismissed    INTEGER NOT NULL DEFAULT 0,
        UNIQUE(user_id, signature)
    );

    CREATE TABLE IF NOT EXISTS custom_lists (
        id           INTEGER PRIMARY KEY AUTOINCREMENT,
        user_id      INTEGER NOT NULL REFERENCES users(id) ON DELETE CASCADE,
        name         TEXT NOT NULL,
        icon         TEXT NOT NULL DEFAULT '📋',
        color        TEXT NOT NULL DEFAULT '#94a3b8',
        sort_order   INTEGER NOT NULL DEFAULT 0,
        created_at   INTEGER NOT NULL,
        updated_at   INTEGER NOT NULL
    );
    CREATE INDEX IF NOT EXISTS idx_custom_lists_user ON custom_lists(user_id, sort_order);

    CREATE TABLE IF NOT EXISTS list_items (
        id           INTEGER PRIMARY KEY AUTOINCREMENT,
        list_id      INTEGER NOT NULL REFERENCES custom_lists(id) ON DELETE CASCADE,
        user_id      INTEGER NOT NULL REFERENCES users(id) ON DELETE CASCADE,
        text         TEXT NOT NULL,
        checked      INTEGER NOT NULL DEFAULT 0,
        sort_order   INTEGER NOT NULL DEFAULT 0,
        created_at   INTEGER NOT NULL,
        updated_at   INTEGER NOT NULL
    );
    CREATE INDEX IF NOT EXISTS idx_list_items_list ON list_items(list_id, sort_order);

    CREATE TABLE IF NOT EXISTS habits (
        id           INTEGER PRIMARY KEY AUTOINCREMENT,
        user_id      INTEGER NOT NULL REFERENCES users(id) ON DELETE CASCADE,
        name         TEXT NOT NULL,
        icon         TEXT NOT NULL DEFAULT '●',
        color        TEXT NOT NULL DEFAULT '#84cc16',
        target_days  TEXT NOT NULL DEFAULT '1,2,3,4,5,6,7',
        sort_order   INTEGER NOT NULL DEFAULT 0,
        archived     INTEGER NOT NULL DEFAULT 0,
        created_at   INTEGER NOT NULL
    );

    CREATE TABLE IF NOT EXISTS habit_entries (
        id           INTEGER PRIMARY KEY AUTOINCREMENT,
        habit_id     INTEGER NOT NULL REFERENCES habits(id) ON DELETE CASCADE,
        user_id      INTEGER NOT NULL REFERENCES users(id) ON DELETE CASCADE,
        date         TEXT NOT NULL,
        done         INTEGER NOT NULL DEFAULT 1,
        note         TEXT,
        created_at   INTEGER NOT NULL,
        UNIQUE(habit_id, date)
    );
    CREATE INDEX IF NOT EXISTS idx_habit_entries_lookup ON habit_entries(user_id, date);

    CREATE TABLE IF NOT EXISTS user_calendar_subscriptions (
        id              INTEGER PRIMARY KEY AUTOINCREMENT,
        user_id         INTEGER NOT NULL REFERENCES users(id) ON DELETE CASCADE,
        provider        TEXT NOT NULL,
        calendar_id     TEXT NOT NULL,
        calendar_name   TEXT NOT NULL,
        color           TEXT NOT NULL DEFAULT '#3b82f6',
        enabled         INTEGER NOT NULL DEFAULT 1,
        write_enabled   INTEGER NOT NULL DEFAULT 0,
        last_synced_at  INTEGER,
        created_at      INTEGER NOT NULL,
        UNIQUE(user_id, provider, calendar_id)
    );
    CREATE INDEX IF NOT EXISTS idx_ucs_user ON user_calendar_subscriptions(user_id, enabled);

    CREATE TABLE IF NOT EXISTS scheduler_stickers_placed (
        id           INTEGER PRIMARY KEY AUTOINCREMENT,
        user_id      INTEGER NOT NULL REFERENCES users(id) ON DELETE CASCADE,
        date         TEXT NOT NULL,
        sticker_key  TEXT NOT NULL,
        position     TEXT NOT NULL DEFAULT 'tr',
        created_at   INTEGER NOT NULL
    );
    CREATE INDEX IF NOT EXISTS idx_stickers_lookup ON scheduler_stickers_placed(user_id, date);

    ALTER TABLE calendar_events ADD COLUMN source_calendar_id TEXT NOT NULL DEFAULT '';
    ALTER TABLE calendar_events ADD COLUMN color              TEXT NOT NULL DEFAULT '';
    ALTER TABLE calendar_events ADD COLUMN external_id        TEXT NOT NULL DEFAULT '';

    CREATE TABLE IF NOT EXISTS scheduler_prefs (
        user_id            INTEGER PRIMARY KEY REFERENCES users(id) ON DELETE CASCADE,
        theme              TEXT NOT NULL DEFAULT 'garden',
        default_view       TEXT NOT NULL DEFAULT 'month',
        week_starts_on     INTEGER NOT NULL DEFAULT 1,
        show_weekends      INTEGER NOT NULL DEFAULT 1,
        work_hours_start   TEXT NOT NULL DEFAULT '08:00',
        work_hours_end     TEXT NOT NULL DEFAULT '18:00',
        updated_at         INTEGER NOT NULL DEFAULT 0
    );
    "#,

    // Scheduler Tier 2 #10 (meeting prep), Tier 3 #17 (meal->grocery),
    // Tier 3 #20 (school ICS auto-import).
    //
    // meal_grocery_links: per-user binding between a "meals" custom_list and
    // a "groceries" custom_list. When a meal item is added, the linked
    // grocery list is auto-populated with LLM-extracted ingredients.
    //
    // school_ics_feeds: per-user ICS feed URLs (school calendars etc.) that
    // auto-resync every 6h. Each fetched event is written to calendar_events
    // with source='ics:school' and external_id='<feed_id>:<hash>' so repeat
    // syncs dedup instead of duplicating.
    //
    // meeting_prep_cards: cached prep card (attendees, past gmail, journal
    // entries) per calendar_event. Precomputed 3-60 min before event start
    // by a background task so the 5-min-before surfacing is instant.
    r#"
    CREATE TABLE IF NOT EXISTS meal_grocery_links (
        user_id          INTEGER PRIMARY KEY REFERENCES users(id) ON DELETE CASCADE,
        meal_list_id     INTEGER NOT NULL REFERENCES custom_lists(id) ON DELETE CASCADE,
        grocery_list_id  INTEGER NOT NULL REFERENCES custom_lists(id) ON DELETE CASCADE,
        created_at       INTEGER NOT NULL
    );

    CREATE TABLE IF NOT EXISTS school_ics_feeds (
        id               INTEGER PRIMARY KEY AUTOINCREMENT,
        user_id          INTEGER NOT NULL REFERENCES users(id) ON DELETE CASCADE,
        label            TEXT NOT NULL,
        feed_url         TEXT NOT NULL,
        color            TEXT NOT NULL DEFAULT '#b4572e',
        last_synced_at   INTEGER,
        last_result      TEXT,
        created_at       INTEGER NOT NULL,
        UNIQUE(user_id, feed_url)
    );
    CREATE INDEX IF NOT EXISTS idx_school_ics_user ON school_ics_feeds(user_id);

    CREATE TABLE IF NOT EXISTS meeting_prep_cards (
        id              INTEGER PRIMARY KEY AUTOINCREMENT,
        user_id         INTEGER NOT NULL REFERENCES users(id) ON DELETE CASCADE,
        event_id        INTEGER NOT NULL REFERENCES calendar_events(id) ON DELETE CASCADE,
        card_json       TEXT NOT NULL,
        generated_at    INTEGER NOT NULL,
        UNIQUE(user_id, event_id)
    );
    CREATE INDEX IF NOT EXISTS idx_meeting_prep_user ON meeting_prep_cards(user_id);
    "#,

    // Scheduler Tier 4: notebook-style frame/border system. Sean's wife
    // called out the Artful Agenda notebook feel by name — matching it
    // (and then some) is a standing mandate. `border` is applied via
    // `body[data-sch-border="<key>"]` the same way `data-sch-theme` is,
    // so new styles are pure-CSS additions.
    r#"
    ALTER TABLE scheduler_prefs ADD COLUMN border TEXT NOT NULL DEFAULT 'notebook';
    "#,

    // Phase 3.3 of the security remediation plan. Append-only audit log
    // for sensitive actions. The file-based `audit::AuditLogger` in
    // ~/.syntaur/audit-YYYY-MM-DD.jsonl keeps its role for system-
    // diagnostic events; this table is the user-queryable surface
    // (/api/audit returns per-user log, admin role sees all).
    //
    // Instrumented actions (namespaced):
    //   auth.login.success, auth.login.fail, auth.register,
    //   token.mint, token.refresh, token.revoke,
    //   admin.user_delete, admin.user_disable, admin.role_change,
    //   oauth.authorize, oauth.grant, oauth.revoke,
    //   approval.approve, approval.reject,
    //   settings.secret_change
    //
    // `metadata` is free-form JSON; keep fields stable within an action
    // namespace so log consumers (future syslog shipping, SIEM) can parse.
    //
    // Retention: trim rows older than 90 days via a nightly background
    // task (tracked separately; table stays append-only for now).
    r#"
    CREATE TABLE IF NOT EXISTS audit_log (
        id          INTEGER PRIMARY KEY AUTOINCREMENT,
        ts          INTEGER NOT NULL,
        user_id     INTEGER,
        action      TEXT NOT NULL,
        target      TEXT,
        metadata    TEXT NOT NULL DEFAULT '{}',
        ip          TEXT,
        user_agent  TEXT
    );
    CREATE INDEX IF NOT EXISTS idx_audit_log_user_ts ON audit_log(user_id, ts DESC);
    CREATE INDEX IF NOT EXISTS idx_audit_log_action_ts ON audit_log(action, ts DESC);
    CREATE INDEX IF NOT EXISTS idx_audit_log_ts ON audit_log(ts DESC);
    "#,

    // Scheduler backdrop tuning. Users drag and scale the painted backdrop
    // into place so watercolor corners line up behind their content. Values
    // are fractions/multipliers (not pixel offsets) so they scale cleanly
    // with viewport size. Defaults center the image and don't upscale.
    r#"
    ALTER TABLE scheduler_prefs ADD COLUMN backdrop_x     REAL NOT NULL DEFAULT 0.5;
    ALTER TABLE scheduler_prefs ADD COLUMN backdrop_y     REAL NOT NULL DEFAULT 0.5;
    ALTER TABLE scheduler_prefs ADD COLUMN backdrop_scale REAL NOT NULL DEFAULT 1.0;
    "#,

    // Backdrop needs INDEPENDENT X and Y scaling so users can stretch the
    // painting along only one axis at a time (watercolor corners want
    // vertical breathing room without also widening). `backdrop_scale`
    // from the previous migration was a single uniform multiplier; this
    // migration splits it into per-axis `scale_x` and `scale_y`. A
    // `scale_y` value of 0 is the sentinel for "auto" (aspect-preserved)
    // so existing default behavior is preserved on first load.
    r#"
    ALTER TABLE scheduler_prefs ADD COLUMN backdrop_scale_x REAL NOT NULL DEFAULT 1.0;
    ALTER TABLE scheduler_prefs ADD COLUMN backdrop_scale_y REAL NOT NULL DEFAULT 0.0;
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
