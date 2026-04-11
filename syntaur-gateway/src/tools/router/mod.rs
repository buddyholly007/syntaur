//! Embedding-based tool router for the voice pipeline.
//!
//! ## Why this exists
//!
//! The voice path's curated tool set was 5 typed HA tools (control_light,
//! set_thermostat, query_state, call_ha_service, web_search). To replace
//! Apple Home + Alexa, Peter needs ~30+ skills (timers, calendar, music,
//! email, weather, shopping list, …). Adding 30 raw tools to the LLM's
//! function-calling list at ~80 tokens each would balloon the prompt to
//! ~2400 tokens of just tool definitions, blowing past Qwen3.5-27B's
//! tool-selection accuracy ceiling (~12-15 tools) and tipping the model
//! into reasoning mode.
//!
//! The router solves this by exposing ONE meta-tool to the LLM
//! (`find_tool`) which takes a natural-language intent, looks up the
//! best-matching downstream tool by sentence embedding, extracts arguments
//! via a small inner LLM call, and executes the tool. The outer LLM sees a
//! single round-trip per dispatched skill instead of 2-3.
//!
//! ## Architecture
//!
//! ```text
//!   user voice → STT → "set a 5 minute timer for chicken"
//!                          ↓
//!                   outer LLM with 6 base tools (incl. find_tool)
//!                          ↓
//!                   find_tool(intent="set 5 min timer for chicken")
//!                          ↓
//!                   Embedder (BGE-small) → 384-dim vector
//!                          ↓
//!                   ToolRouter::find_best → cosine vs N entries
//!                          ↓
//!                   matched: start_timer (confidence 0.84)
//!                          ↓
//!                   inner LLM call with start_timer's parameter schema
//!                          ↓
//!                   {"duration_seconds": 300, "name": "chicken"}
//!                          ↓
//!                   start_timer.execute(args) → "Timer 'chicken' set for 5m"
//!                          ↓
//!                   outer LLM gets the tool result, speaks it
//! ```
//!
//! ## Why not just dump all tools into the LLM list?
//!
//! Tested empirically across LLM ecosystems: tool selection accuracy
//! degrades sharply past ~15 tools for any non-frontier model, and
//! Qwen3.5-27B-distilled-reasoning is more sensitive than most because
//! its reasoning bias makes it overthink ambiguous tool choices. The
//! router pattern keeps the LLM's tool list small while making the skill
//! count effectively unbounded.
//!
//! ## Confidence threshold
//!
//! BGE-small cosine similarity for related English text typically lands
//! in [0.45, 0.85]. We use 0.55 as the dispatch threshold by default —
//! low enough to catch genuine matches with rephrased intent, high enough
//! to reject completely off-topic queries. The threshold can be tuned per
//! environment via `ToolRouter::with_threshold`.

pub mod embedder;

use std::sync::Arc;

use log::{debug, info, warn};
use serde::Serialize;
use tokio::sync::RwLock;

use crate::tools::extension::Tool;

use embedder::{cosine, Embedder};

/// Logical category for a routed tool. Used for grouping in the registry,
/// optional filtering in `find_best_in_category`, and human-facing display
/// when describing the tool surface to the LLM. Categories are intentionally
/// broad — we'd rather have 10 categories with clean labels than 30 with
/// overlapping semantics.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ToolCategory {
    /// Lights, switches, climate, locks, scenes, covers, vacuums.
    SmartHome,
    /// Music playback, media transport, TV control, volume, AirPlay.
    Media,
    /// Read/write calendar events, free-time queries.
    Calendar,
    /// Read/send/draft email, search inbox.
    Email,
    /// Timers, alarms, reminders.
    Timers,
    /// Weather, news, web search, web fetch, Wikipedia, general info.
    Info,
    /// Shopping list, todo list, notes, personal preferences.
    Personal,
    /// Excel, Word, PPTX, document operations.
    Office,
    /// Status of trading bots, Crimson Lantern, tax agent, ledger.
    Household,
    /// Code execution, file ops, shell, dev tools.
    Dev,
    /// Doesn't fit anywhere else.
    Other,
}

impl ToolCategory {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::SmartHome => "smart_home",
            Self::Media => "media",
            Self::Calendar => "calendar",
            Self::Email => "email",
            Self::Timers => "timers",
            Self::Info => "info",
            Self::Personal => "personal",
            Self::Office => "office",
            Self::Household => "household",
            Self::Dev => "dev",
            Self::Other => "other",
        }
    }
}

/// One entry in the router. Wraps a `Tool` (the existing trait from
/// `tools/extension.rs`) with the metadata the dispatcher needs to route
/// natural-language intents to it.
pub struct RouterEntry {
    /// The actual Tool that gets executed when this entry is matched.
    /// Existing tools (web_search, code_execute, email_read, etc.) all
    /// implement this trait — adding them to the router is just one
    /// `RouterEntry` construction, no rewrite.
    pub tool: Arc<dyn Tool>,

    /// Logical category for grouping/filtering. Currently informational only;
    /// future versions may use this to pre-filter candidates before cosine.
    pub category: ToolCategory,

    /// Voice-flavored description (different from the LLM tool description
    /// in `Tool::description()`). The LLM tool description is written for
    /// function-calling and tends to be terse + technical. This one is
    /// written for human-style intent matching: "Start a countdown timer
    /// that fires a TTS announcement when it expires."
    pub voice_description: String,

    /// 2-5 example intents Sean might say for this tool. Joined into the
    /// embedded text so the router learns the phrasing variations Sean
    /// actually uses, not just the descriptive blurb. Examples:
    ///
    /// `vec!["set a 5 minute timer", "remind me in 10 minutes",
    ///       "wake me up at 7 am", "how long left on my timer"]`
    pub example_intents: Vec<String>,
}

impl RouterEntry {
    /// Build the text that gets embedded for cosine matching. Combines the
    /// voice description and example intents into one document. Including
    /// the example intents typically lifts dispatch accuracy by 10-15%
    /// over voice_description alone.
    fn embedding_text(&self) -> String {
        let mut s = self.voice_description.clone();
        for ex in &self.example_intents {
            s.push_str("\n");
            s.push_str(ex);
        }
        s
    }
}

/// Result of a `find` lookup. Either a confident single match, an ambiguous
/// set of candidates the caller may want to surface to the LLM for tie-breaking,
/// or no match at all.
#[derive(Debug)]
pub enum FindResult {
    /// One entry won decisively. `confidence` is the cosine similarity, in
    /// the range [-1, 1]. BGE-small produces values in roughly [0.4, 0.9]
    /// for in-distribution matches.
    Match {
        index: usize,
        confidence: f32,
    },
    /// No entry exceeded the confidence threshold. `best` is the top-1
    /// (with its low score) so the caller can show "best guess" if they
    /// want to be lenient.
    NoMatch {
        best: Option<(usize, f32)>,
    },
}

pub struct ToolRouter {
    entries: Vec<RouterEntry>,
    embeddings: Vec<Vec<f32>>,
    embedder: Arc<Embedder>,
    /// Cosine similarity threshold for `find` to return `Match` instead of
    /// `NoMatch`. Default 0.55. Tunable per deployment via `with_threshold`.
    confidence_threshold: f32,
}

impl ToolRouter {
    /// Construct a new router with no entries. The embedder is downloaded
    /// + initialized lazily here, so the first new() call after a fresh
    /// install does the ~30 MB BGE model download (cached for next runs).
    pub fn new() -> Result<Arc<RwLock<Self>>, String> {
        let embedder = Embedder::new()?;
        Ok(Arc::new(RwLock::new(Self {
            entries: Vec::new(),
            embeddings: Vec::new(),
            embedder,
            confidence_threshold: 0.55,
        })))
    }

    /// Override the default confidence threshold. Lower = more permissive
    /// (more dispatches happen), higher = more strict (more queries fall
    /// through to "no match" and the LLM has to pick a different tool).
    pub fn set_threshold(&mut self, t: f32) {
        self.confidence_threshold = t;
    }

    /// Add a single entry, computing its embedding immediately. For batch
    /// startup loading prefer `add_batch` — it's much faster because
    /// fastembed batches through ONNX in one inference call.
    pub async fn add(&mut self, entry: RouterEntry) -> Result<(), String> {
        let text = entry.embedding_text();
        let emb = self.embedder.embed(&text).await?;
        self.entries.push(entry);
        self.embeddings.push(emb);
        Ok(())
    }

    /// Add many entries in one batched embedding call. Use at startup.
    pub async fn add_batch(&mut self, entries: Vec<RouterEntry>) -> Result<(), String> {
        if entries.is_empty() {
            return Ok(());
        }
        let texts: Vec<String> = entries.iter().map(|e| e.embedding_text()).collect();
        let embeddings = self.embedder.embed_batch(texts).await?;
        if embeddings.len() != entries.len() {
            return Err(format!(
                "fastembed returned {} embeddings for {} entries",
                embeddings.len(),
                entries.len()
            ));
        }
        for (e, v) in entries.into_iter().zip(embeddings.into_iter()) {
            self.entries.push(e);
            self.embeddings.push(v);
        }
        info!("[router] now holding {} entries", self.entries.len());
        Ok(())
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    pub fn get(&self, index: usize) -> Option<&RouterEntry> {
        self.entries.get(index)
    }

    pub fn iter(&self) -> impl Iterator<Item = &RouterEntry> {
        self.entries.iter()
    }

    /// Find the best-matching entry for an intent. Returns `Match` if the
    /// top similarity exceeds `confidence_threshold`, otherwise `NoMatch`
    /// with the top-1 attached so the caller can fall through gracefully.
    pub async fn find(&self, intent: &str) -> Result<FindResult, String> {
        if self.entries.is_empty() {
            return Ok(FindResult::NoMatch { best: None });
        }
        let q = self.embedder.embed(intent).await?;
        let mut best_idx = 0usize;
        let mut best_score = f32::NEG_INFINITY;
        for (i, e) in self.embeddings.iter().enumerate() {
            let sim = cosine(&q, e);
            if sim > best_score {
                best_score = sim;
                best_idx = i;
            }
        }
        debug!(
            "[router] intent='{}' best={} score={:.3} threshold={}",
            intent.chars().take(60).collect::<String>(),
            self.entries[best_idx].tool.name(),
            best_score,
            self.confidence_threshold
        );
        if best_score >= self.confidence_threshold {
            Ok(FindResult::Match {
                index: best_idx,
                confidence: best_score,
            })
        } else {
            warn!(
                "[router] no match for '{}' (best={} score={:.3} < threshold {:.2})",
                intent.chars().take(80).collect::<String>(),
                self.entries[best_idx].tool.name(),
                best_score,
                self.confidence_threshold
            );
            Ok(FindResult::NoMatch {
                best: Some((best_idx, best_score)),
            })
        }
    }

    /// Find top-K candidates by cosine similarity. Used when the caller
    /// wants to surface multiple options to the LLM for tie-breaking
    /// instead of taking the single best. Returns sorted descending.
    pub async fn find_top_k(
        &self,
        intent: &str,
        k: usize,
    ) -> Result<Vec<(usize, f32)>, String> {
        if self.entries.is_empty() || k == 0 {
            return Ok(Vec::new());
        }
        let q = self.embedder.embed(intent).await?;
        let mut scored: Vec<(usize, f32)> = self
            .embeddings
            .iter()
            .enumerate()
            .map(|(i, e)| (i, cosine(&q, e)))
            .collect();
        scored.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
        scored.truncate(k);
        Ok(scored)
    }
}
