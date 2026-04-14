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
