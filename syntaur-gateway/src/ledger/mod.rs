//! Tax module — Ledger sub-feature.
//!
//! Absorbed from the standalone `rust-ledger` service that ran on
//! openclawprod:18790 (decommissioned 2026-04-22 as part of the VM
//! migration to the TrueNAS Syntaur container). Schema is the v1 shape
//! from rust-ledger; the existing `lcm.db` (renamed to `ledger.db` here)
//! mounts at `/data/ledger.db` inside the container.
//!
//! Lives under the existing Tax module (Positron persona) per the
//! migration plan at `~/.claude/plans/immutable-brewing-jellyfish.md` —
//! one persona owns the full money stack (receipts, expenses, ledger,
//! brokerage trading, e-file).
//!
//! Read-only at v1 — full CRUD + import surfaces will land in follow-on
//! commits as Sean exercises each path. Existing rust-ledger CLI
//! continues to write to the same DB out-of-band if needed.

use rusqlite::{Connection, Result, params};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

const SCHEMA_SQL: &str = r#"
PRAGMA foreign_keys = ON;
PRAGMA journal_mode = WAL;
PRAGMA synchronous = NORMAL;

CREATE TABLE IF NOT EXISTS schema_version (
    version INTEGER PRIMARY KEY,
    applied_at TEXT NOT NULL DEFAULT (datetime('now'))
);

CREATE TABLE IF NOT EXISTS entities (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    name TEXT NOT NULL UNIQUE,
    created_at TEXT NOT NULL DEFAULT (datetime('now'))
);

CREATE TABLE IF NOT EXISTS accounts (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    entity_id INTEGER NOT NULL REFERENCES entities(id),
    parent_id INTEGER REFERENCES accounts(id),
    name TEXT NOT NULL,
    account_type TEXT NOT NULL,
    subkind TEXT,
    gnucash_guid TEXT UNIQUE,
    institution TEXT,
    currency TEXT NOT NULL DEFAULT 'USD',
    archived INTEGER NOT NULL DEFAULT 0,
    created_at TEXT NOT NULL DEFAULT (datetime('now'))
);

CREATE TABLE IF NOT EXISTS transactions (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    entity_id INTEGER NOT NULL REFERENCES entities(id),
    txn_date TEXT NOT NULL,
    payee TEXT,
    memo TEXT,
    source TEXT NOT NULL DEFAULT 'manual',
    source_ref TEXT,
    gnucash_guid TEXT UNIQUE,
    created_at TEXT NOT NULL DEFAULT (datetime('now')),
    updated_at TEXT NOT NULL DEFAULT (datetime('now'))
);

CREATE TABLE IF NOT EXISTS splits (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    transaction_id INTEGER NOT NULL REFERENCES transactions(id) ON DELETE CASCADE,
    account_id INTEGER NOT NULL REFERENCES accounts(id),
    amount_cents INTEGER NOT NULL,
    memo TEXT,
    quantity_micro INTEGER,
    asset_symbol TEXT,
    gnucash_guid TEXT UNIQUE
);
"#;

#[derive(Clone)]
pub struct LedgerService {
    conn: Arc<Mutex<Connection>>,
    pub path: PathBuf,
}

#[derive(serde::Serialize)]
pub struct EntityRow {
    pub id: i64,
    pub name: String,
}

#[derive(serde::Serialize)]
pub struct AccountRow {
    pub id: i64,
    pub entity_id: i64,
    pub parent_id: Option<i64>,
    pub name: String,
    pub account_type: String,
    pub institution: Option<String>,
}

#[derive(serde::Serialize)]
pub struct TransactionRow {
    pub id: i64,
    pub entity_id: i64,
    pub txn_date: String,
    pub payee: Option<String>,
    pub memo: Option<String>,
}

#[derive(serde::Serialize)]
pub struct ExpenseSummaryRow {
    pub account_id: i64,
    pub account_name: String,
    pub total_cents: i64,
}

impl LedgerService {
    pub fn open<P: AsRef<Path>>(path: P) -> Result<Self> {
        let pb = path.as_ref().to_path_buf();
        let conn = Connection::open(&path)?;
        // Idempotent — only creates tables if missing. Existing migrated DB
        // already has them.
        conn.execute_batch(SCHEMA_SQL)?;
        // Idempotent entity seeds.
        conn.execute(
            "INSERT OR IGNORE INTO entities (name) VALUES ('Cherry Woodworks')",
            [],
        )?;
        conn.execute("INSERT OR IGNORE INTO entities (name) VALUES ('Personal')", [])?;
        Ok(Self { conn: Arc::new(Mutex::new(conn)), path: pb })
    }

    pub fn entities(&self) -> Result<Vec<EntityRow>> {
        let g = self.conn.lock().expect("ledger mutex");
        let mut stmt = g.prepare("SELECT id, name FROM entities ORDER BY id")?;
        let rows = stmt
            .query_map([], |r| Ok(EntityRow { id: r.get(0)?, name: r.get(1)? }))?
            .filter_map(|r| r.ok())
            .collect();
        Ok(rows)
    }

    pub fn accounts(&self, entity_id: Option<i64>, account_type: Option<&str>) -> Result<Vec<AccountRow>> {
        let g = self.conn.lock().expect("ledger mutex");
        let mut sql = String::from(
            "SELECT id, entity_id, parent_id, name, account_type, institution FROM accounts WHERE archived = 0",
        );
        let mut params_vec: Vec<Box<dyn rusqlite::ToSql>> = Vec::new();
        if let Some(eid) = entity_id {
            sql.push_str(" AND entity_id = ?");
            params_vec.push(Box::new(eid));
        }
        if let Some(at) = account_type {
            sql.push_str(" AND account_type = ?");
            params_vec.push(Box::new(at.to_string()));
        }
        sql.push_str(" ORDER BY entity_id, account_type, name");
        let mut stmt = g.prepare(&sql)?;
        let p: Vec<&dyn rusqlite::ToSql> = params_vec.iter().map(|b| b.as_ref()).collect();
        let rows = stmt
            .query_map(p.as_slice(), |r| {
                Ok(AccountRow {
                    id: r.get(0)?,
                    entity_id: r.get(1)?,
                    parent_id: r.get(2)?,
                    name: r.get(3)?,
                    account_type: r.get(4)?,
                    institution: r.get(5)?,
                })
            })?
            .filter_map(|r| r.ok())
            .collect();
        Ok(rows)
    }

    pub fn transactions(&self, entity_id: Option<i64>, from: Option<&str>, to: Option<&str>, limit: i64) -> Result<Vec<TransactionRow>> {
        let g = self.conn.lock().expect("ledger mutex");
        let mut sql = String::from("SELECT id, entity_id, txn_date, payee, memo FROM transactions WHERE 1=1");
        let mut params_vec: Vec<Box<dyn rusqlite::ToSql>> = Vec::new();
        if let Some(eid) = entity_id {
            sql.push_str(" AND entity_id = ?");
            params_vec.push(Box::new(eid));
        }
        if let Some(f) = from {
            sql.push_str(" AND txn_date >= ?");
            params_vec.push(Box::new(f.to_string()));
        }
        if let Some(t) = to {
            sql.push_str(" AND txn_date <= ?");
            params_vec.push(Box::new(t.to_string()));
        }
        sql.push_str(" ORDER BY txn_date DESC, id DESC LIMIT ?");
        params_vec.push(Box::new(limit));
        let mut stmt = g.prepare(&sql)?;
        let p: Vec<&dyn rusqlite::ToSql> = params_vec.iter().map(|b| b.as_ref()).collect();
        let rows = stmt
            .query_map(p.as_slice(), |r| {
                Ok(TransactionRow {
                    id: r.get(0)?,
                    entity_id: r.get(1)?,
                    txn_date: r.get(2)?,
                    payee: r.get(3)?,
                    memo: r.get(4)?,
                })
            })?
            .filter_map(|r| r.ok())
            .collect();
        Ok(rows)
    }

    pub fn account_balance(&self, account_id: i64, from: &str, to: &str) -> Result<i64> {
        let g = self.conn.lock().expect("ledger mutex");
        let total: Option<i64> = g.query_row(
            r#"SELECT COALESCE(SUM(s.amount_cents), 0)
               FROM splits s JOIN transactions t ON t.id = s.transaction_id
               WHERE s.account_id = ?1 AND t.txn_date >= ?2 AND t.txn_date <= ?3"#,
            params![account_id, from, to],
            |r| r.get(0),
        )?;
        Ok(total.unwrap_or(0))
    }

    pub fn expense_summary(&self, entity_id: i64, from: &str, to: &str) -> Result<Vec<ExpenseSummaryRow>> {
        let g = self.conn.lock().expect("ledger mutex");
        let mut stmt = g.prepare(
            r#"SELECT a.id, a.name, COALESCE(SUM(s.amount_cents), 0) as total
               FROM accounts a
               LEFT JOIN splits s ON s.account_id = a.id
               LEFT JOIN transactions t ON t.id = s.transaction_id AND t.txn_date >= ?2 AND t.txn_date <= ?3
               WHERE a.entity_id = ?1 AND a.account_type = 'expense' AND a.archived = 0
               GROUP BY a.id, a.name
               HAVING total != 0
               ORDER BY total DESC"#,
        )?;
        let rows = stmt
            .query_map(params![entity_id, from, to], |r| {
                Ok(ExpenseSummaryRow {
                    account_id: r.get(0)?,
                    account_name: r.get(1)?,
                    total_cents: r.get(2)?,
                })
            })?
            .filter_map(|r| r.ok())
            .collect();
        Ok(rows)
    }
}

/// HTTP handlers under `/api/ledger/*`. Wired in `main.rs` route table.
pub mod api {
    use super::*;
    use crate::AppState;
    use axum::{extract::{Query, State}, http::StatusCode, response::Json};
    use serde::Deserialize;
    use std::sync::Arc;

    fn unavailable() -> (StatusCode, String) {
        (
            StatusCode::SERVICE_UNAVAILABLE,
            "Ledger DB is not configured. Bind-mount ledger.db at /data/ledger.db.".to_string(),
        )
    }

    pub async fn handle_entities(State(state): State<Arc<AppState>>) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
        let svc = state.ledger.clone().ok_or_else(unavailable)?;
        let rows = tokio::task::spawn_blocking(move || svc.entities())
            .await
            .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, format!("join: {e}")))?
            .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, format!("query: {e}")))?;
        Ok(Json(serde_json::json!({ "entities": rows })))
    }

    #[derive(Deserialize)]
    pub struct AccountsQuery {
        pub entity_id: Option<i64>,
        pub account_type: Option<String>,
    }

    pub async fn handle_accounts(
        State(state): State<Arc<AppState>>,
        Query(q): Query<AccountsQuery>,
    ) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
        let svc = state.ledger.clone().ok_or_else(unavailable)?;
        let rows = tokio::task::spawn_blocking(move || svc.accounts(q.entity_id, q.account_type.as_deref()))
            .await
            .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, format!("join: {e}")))?
            .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, format!("query: {e}")))?;
        Ok(Json(serde_json::json!({ "accounts": rows })))
    }

    #[derive(Deserialize)]
    pub struct TransactionsQuery {
        pub entity_id: Option<i64>,
        pub from: Option<String>,
        pub to: Option<String>,
        pub limit: Option<i64>,
    }

    pub async fn handle_transactions(
        State(state): State<Arc<AppState>>,
        Query(q): Query<TransactionsQuery>,
    ) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
        let svc = state.ledger.clone().ok_or_else(unavailable)?;
        let limit = q.limit.unwrap_or(200).min(1000);
        let rows = tokio::task::spawn_blocking(move || {
            svc.transactions(q.entity_id, q.from.as_deref(), q.to.as_deref(), limit)
        })
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, format!("join: {e}")))?
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, format!("query: {e}")))?;
        Ok(Json(serde_json::json!({ "transactions": rows })))
    }

    #[derive(Deserialize)]
    pub struct ExpenseSummaryQuery {
        pub entity_id: i64,
        pub from: String,
        pub to: String,
    }

    pub async fn handle_expense_summary(
        State(state): State<Arc<AppState>>,
        Query(q): Query<ExpenseSummaryQuery>,
    ) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
        let svc = state.ledger.clone().ok_or_else(unavailable)?;
        let rows = tokio::task::spawn_blocking(move || svc.expense_summary(q.entity_id, &q.from, &q.to))
            .await
            .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, format!("join: {e}")))?
            .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, format!("query: {e}")))?;
        let total: i64 = rows.iter().map(|r| r.total_cents).sum();
        Ok(Json(serde_json::json!({ "rows": rows, "total_cents": total })))
    }

    #[derive(Deserialize)]
    pub struct BalanceQuery {
        pub account_id: i64,
        pub from: String,
        pub to: String,
    }

    pub async fn handle_account_balance(
        State(state): State<Arc<AppState>>,
        Query(q): Query<BalanceQuery>,
    ) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
        let svc = state.ledger.clone().ok_or_else(unavailable)?;
        let (acc, from, to) = (q.account_id, q.from.clone(), q.to.clone());
        let total = tokio::task::spawn_blocking(move || svc.account_balance(acc, &from, &to))
            .await
            .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, format!("join: {e}")))?
            .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, format!("query: {e}")))?;
        Ok(Json(serde_json::json!({ "account_id": q.account_id, "from": q.from, "to": q.to, "total_cents": total })))
    }
}
