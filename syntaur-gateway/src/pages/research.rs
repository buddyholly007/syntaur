//! /research — legacy route.
//!
//! The research workflow was merged into the Knowledge module on 2026-04-17
//! so Cortex (the research/knowledge persona) has a single surface. This
//! handler redirects any inbound `/research` URL to `/knowledge?tab=research`
//! so bookmarks, dashboard links, and external references still land on the
//! right panel.

use axum::response::{IntoResponse, Redirect};

pub async fn render() -> impl IntoResponse {
    Redirect::permanent("/knowledge?tab=research")
}
