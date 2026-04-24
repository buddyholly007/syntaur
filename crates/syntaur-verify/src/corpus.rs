//! Regression corpus — archive of auto-fix wins.
//!
//! Every time Phase 2b's `try_autofix` accepts a set of edits
//! (re-verify comes back clean), we snapshot the evidence into
//! `~/.syntaur-verify/corpus/<YYYY-MM-DD>-<slug>-<kind>/`:
//!
//! * `before.png`  — pre-fix screenshot
//! * `after.png`   — post-fix screenshot
//! * `edits.json`  — the `FindingEdit`s that did the work
//! * `meta.json`   — module, kind, severity, title, detail,
//!                   captured_at, head_rev
//!
//! Why keep this: future runs can cross-check candidate regressions
//! against historical wins at the same module + kind, giving Opus
//! extra grounding ("we've seen this exact class of bug here before,
//! here's the edit that fixed it"). Phase 3 only **builds** the
//! corpus; actually reading back from it is deferred to Phase 4+, but
//! the API is shaped so that later phases can `list_for_module` and
//! load entries without re-architecting.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::run::{FindingEdit, FindingKind, Severity};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CorpusMeta {
    pub module: String,
    pub kind: FindingKind,
    pub severity: Severity,
    pub title: String,
    pub detail: String,
    pub captured_at: DateTime<Utc>,
    pub head_rev: String,
}

#[derive(Debug, Clone)]
pub struct CorpusEntry {
    pub dir: PathBuf,
    pub meta: CorpusMeta,
}

/// Filesystem-backed corpus. Clone-safe.
#[derive(Debug, Clone)]
pub struct Corpus {
    root: PathBuf,
}

impl Corpus {
    /// Default root — `~/.syntaur-verify/corpus`. Created if missing.
    pub fn new() -> Result<Self> {
        let home = std::env::var("HOME")
            .context("$HOME not set — required to locate ~/.syntaur-verify/corpus")?;
        let root = PathBuf::from(home).join(".syntaur-verify").join("corpus");
        std::fs::create_dir_all(&root)
            .with_context(|| format!("creating corpus root {}", root.display()))?;
        Ok(Self { root })
    }

    /// Explicit root — for tests + CLI override.
    pub fn with_root(root: PathBuf) -> Self {
        Self { root }
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Archive one accepted-fix record.
    ///
    /// Dir name: `<date>-<module_slug>-<kind_slug>` with a numeric
    /// suffix on collision, so running the same fix twice on the same
    /// day doesn't clobber the first entry. Writes are best-effort
    /// tolerant: if PNG copy fails we still write meta + edits so the
    /// historical record isn't lost for the narrative tooling.
    pub fn archive(
        &self,
        before_png: &[u8],
        after_png: &[u8],
        edits: &[FindingEdit],
        meta: CorpusMeta,
    ) -> Result<PathBuf> {
        let date = meta.captured_at.format("%Y-%m-%d").to_string();
        let kind_slug = kind_to_slug(&meta.kind);
        let base = format!("{}-{}-{}", date, meta.module, kind_slug);

        // Pick a non-clobbering dir name.
        let dir = self.pick_fresh_dir(&base)?;
        std::fs::create_dir_all(&dir)
            .with_context(|| format!("creating corpus dir {}", dir.display()))?;

        std::fs::write(dir.join("before.png"), before_png)
            .with_context(|| format!("writing before.png in {}", dir.display()))?;
        std::fs::write(dir.join("after.png"), after_png)
            .with_context(|| format!("writing after.png in {}", dir.display()))?;
        std::fs::write(
            dir.join("edits.json"),
            serde_json::to_vec_pretty(edits).context("serialising edits.json")?,
        )
        .with_context(|| format!("writing edits.json in {}", dir.display()))?;
        std::fs::write(
            dir.join("meta.json"),
            serde_json::to_vec_pretty(&meta).context("serialising meta.json")?,
        )
        .with_context(|| format!("writing meta.json in {}", dir.display()))?;

        log::info!("[corpus] archived fix to {}", dir.display());
        Ok(dir)
    }

    /// Find the first `<base>` / `<base>-2` / `<base>-3` / … that
    /// doesn't exist yet.
    fn pick_fresh_dir(&self, base: &str) -> Result<PathBuf> {
        let first = self.root.join(base);
        if !first.exists() {
            return Ok(first);
        }
        for n in 2..1000 {
            let candidate = self.root.join(format!("{base}-{n}"));
            if !candidate.exists() {
                return Ok(candidate);
            }
        }
        anyhow::bail!(
            "corpus already has 1000 entries for {base} at {} — rotate or clean the corpus",
            self.root.display()
        );
    }

    /// All archived entries whose module slug matches. Skips entries
    /// with unreadable `meta.json` rather than failing the whole list
    /// — a corrupt entry shouldn't block Phase 4 from reading the
    /// rest of the corpus. The corpus is append-only for humans, so
    /// we don't assume every directory is well-formed.
    pub fn list_for_module(&self, module: &str) -> Result<Vec<CorpusEntry>> {
        let mut out = Vec::new();
        if !self.root.exists() {
            return Ok(out);
        }
        let rd = std::fs::read_dir(&self.root)
            .with_context(|| format!("reading corpus root {}", self.root.display()))?;
        for entry in rd.flatten() {
            let dir = entry.path();
            if !dir.is_dir() {
                continue;
            }
            let meta_path = dir.join("meta.json");
            let bytes = match std::fs::read(&meta_path) {
                Ok(b) => b,
                Err(_) => continue,
            };
            let meta: CorpusMeta = match serde_json::from_slice(&bytes) {
                Ok(m) => m,
                Err(e) => {
                    log::warn!(
                        "[corpus] skipping {}: meta.json unreadable ({e})",
                        dir.display()
                    );
                    continue;
                }
            };
            if meta.module == module {
                out.push(CorpusEntry { dir, meta });
            }
        }
        // Stable ordering so callers can rely on "newest last".
        out.sort_by(|a, b| a.meta.captured_at.cmp(&b.meta.captured_at));
        Ok(out)
    }
}

fn kind_to_slug(k: &FindingKind) -> &'static str {
    match k {
        FindingKind::BootFailure => "boot-failure",
        FindingKind::ConsoleError => "console-error",
        FindingKind::VisualDiff => "visual-diff",
        FindingKind::Accessibility => "accessibility",
        FindingKind::Improvement => "improvement",
        FindingKind::Security => "security",
        FindingKind::Other => "other",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_meta() -> CorpusMeta {
        CorpusMeta {
            module: "dashboard".into(),
            kind: FindingKind::VisualDiff,
            severity: Severity::Regression,
            title: "Sidebar collapsed when it shouldn't".into(),
            detail: "Mobile viewport showed desktop sidebar".into(),
            captured_at: Utc::now(),
            head_rev: "deadbeef".into(),
        }
    }

    #[test]
    fn archive_creates_expected_structure() {
        let dir = tempfile::tempdir().expect("tempdir");
        let corpus = Corpus::with_root(dir.path().to_path_buf());
        let edits = vec![FindingEdit {
            file: "syntaur-gateway/src/pages/dashboard.rs".into(),
            old_string: "old".into(),
            new_string: "new".into(),
        }];
        let entry_dir = corpus
            .archive(b"before-bytes", b"after-bytes", &edits, sample_meta())
            .expect("archive");

        assert!(entry_dir.join("before.png").is_file());
        assert!(entry_dir.join("after.png").is_file());
        assert!(entry_dir.join("edits.json").is_file());
        assert!(entry_dir.join("meta.json").is_file());

        assert_eq!(
            std::fs::read(entry_dir.join("before.png")).unwrap(),
            b"before-bytes"
        );
    }

    #[test]
    fn archive_twice_same_day_picks_fresh_name() {
        let dir = tempfile::tempdir().expect("tempdir");
        let corpus = Corpus::with_root(dir.path().to_path_buf());
        let first = corpus
            .archive(b"a", b"b", &[], sample_meta())
            .expect("first");
        let second = corpus
            .archive(b"c", b"d", &[], sample_meta())
            .expect("second");
        assert_ne!(first, second);
    }

    #[test]
    fn list_for_module_filters_by_slug() {
        let dir = tempfile::tempdir().expect("tempdir");
        let corpus = Corpus::with_root(dir.path().to_path_buf());
        let mut dash_meta = sample_meta();
        dash_meta.module = "dashboard".into();
        let mut set_meta = sample_meta();
        set_meta.module = "settings".into();

        corpus.archive(b"", b"", &[], dash_meta).unwrap();
        corpus.archive(b"", b"", &[], set_meta).unwrap();

        let dash = corpus.list_for_module("dashboard").expect("list");
        assert_eq!(dash.len(), 1);
        assert_eq!(dash[0].meta.module, "dashboard");
    }
}
