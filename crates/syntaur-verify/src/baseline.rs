//! Persistent baseline store for visual-diff.
//!
//! Phase 3 ships with a filesystem-backed store under
//! `~/.syntaur-verify/baselines/<module_slug>/<viewport>.png`. Layout
//! invariants we rely on:
//!
//! * Flat hierarchy. One dir per module slug, one PNG per viewport.
//!   No timestamps, no "last N" rotation — a baseline is by definition
//!   the canonical shape, replaced explicitly with `--update-baselines`.
//! * Path-safe slugs. Baselines are keyed on the module slug from
//!   `module-map.yaml`, which is already constrained to `[a-z0-9-]`.
//!   We don't re-sanitize here; callers pass trusted slugs.
//! * Atomic writes. Save writes to a sibling `.tmp` and renames so a
//!   half-written baseline can't poison the next diff.
//!
//! Phase 4+ may grow this into a keyed content-addressed store (SHA
//! of the PNG as the filename, with a symlink pointing at "current").
//! That would give us free historical baselines for bisect use —
//! punted for now; this phase only needs "does a baseline exist, load
//! it, save one".

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};

use crate::browser::Viewport;

/// Filesystem-backed baseline store. Clone-safe (owns a `PathBuf`).
#[derive(Debug, Clone)]
pub struct BaselineStore {
    root: PathBuf,
}

impl BaselineStore {
    /// Default root — `~/.syntaur-verify/baselines`. Created if missing.
    ///
    /// Errors if `$HOME` is unset — in which case nothing in this
    /// crate can resolve its own config dir anyway, so we surface
    /// that up-front with an actionable message rather than silently
    /// writing to `/baselines`.
    pub fn new() -> Result<Self> {
        let home = std::env::var("HOME")
            .context("$HOME not set — required to locate ~/.syntaur-verify/baselines")?;
        let root = PathBuf::from(home).join(".syntaur-verify").join("baselines");
        std::fs::create_dir_all(&root)
            .with_context(|| format!("creating baseline root {}", root.display()))?;
        Ok(Self { root })
    }

    /// Explicit root — for tests + for the CLI `--baseline-dir` flag.
    pub fn with_root(root: PathBuf) -> Self {
        Self { root }
    }

    /// Where the baseline for `(module, viewport)` lives. The file may
    /// or may not exist — use `exists` to check.
    ///
    /// Back-compat: persona-less overload. Equivalent to
    /// `path_for(module, None, vp)`.
    pub fn path(&self, module: &str, vp: Viewport) -> PathBuf {
        self.path_for(module, None, vp)
    }

    /// Phase 4b — full-keyed path including optional persona slug.
    ///
    /// Layout:
    ///   * `None`         → `<root>/<module>/<viewport>.png` (unchanged)
    ///   * `Some("peter")` → `<root>/<module>/<persona>/<viewport>.png`
    ///
    /// The persona dimension is an *extra directory level*, not a
    /// filename suffix, for two reasons:
    ///   1. It lets `rm -rf <root>/<module>/peter` drop one persona's
    ///      baselines without touching anonymous + other-persona shots.
    ///   2. Old layouts are preserved exactly — no rewrite of
    ///      already-locked baselines just to add a persona axis.
    pub fn path_for(&self, module: &str, persona: Option<&str>, vp: Viewport) -> PathBuf {
        match persona {
            None => self.root.join(module).join(format!("{}.png", vp.slug())),
            Some(p) => self
                .root
                .join(module)
                .join(p)
                .join(format!("{}.png", vp.slug())),
        }
    }

    /// True iff a baseline file is on disk for this key.
    pub fn exists(&self, module: &str, vp: Viewport) -> bool {
        self.exists_for(module, None, vp)
    }

    /// Persona-aware existence check. Mirrors `path_for`.
    pub fn exists_for(&self, module: &str, persona: Option<&str>, vp: Viewport) -> bool {
        self.path_for(module, persona, vp).is_file()
    }

    /// Persist `png` as the baseline for `(module, viewport)`,
    /// overwriting any prior baseline. Writes to `<target>.tmp` then
    /// renames, so a crash mid-write can't leave half a PNG behind.
    pub fn save(&self, module: &str, vp: Viewport, png: &[u8]) -> Result<()> {
        self.save_for(module, None, vp, png)
    }

    /// Persona-aware save. Mirrors `save`.
    pub fn save_for(
        &self,
        module: &str,
        persona: Option<&str>,
        vp: Viewport,
        png: &[u8],
    ) -> Result<()> {
        let final_path = self.path_for(module, persona, vp);
        let parent = final_path
            .parent()
            .ok_or_else(|| anyhow::anyhow!("baseline path has no parent: {}", final_path.display()))?;
        std::fs::create_dir_all(parent)
            .with_context(|| format!("creating baseline dir {}", parent.display()))?;
        let tmp = {
            let mut t = final_path.clone();
            let name = t
                .file_name()
                .map(|n| format!("{}.tmp", n.to_string_lossy()))
                .unwrap_or_else(|| "baseline.tmp".to_string());
            t.set_file_name(name);
            t
        };
        std::fs::write(&tmp, png)
            .with_context(|| format!("writing {} (tmp)", tmp.display()))?;
        std::fs::rename(&tmp, &final_path).with_context(|| {
            format!(
                "renaming {} -> {} (baseline store may be across filesystems)",
                tmp.display(),
                final_path.display()
            )
        })?;
        Ok(())
    }

    /// Load the baseline PNG bytes. Error messages follow the plain-
    /// language policy — tell the user what to run next.
    pub fn load(&self, module: &str, vp: Viewport) -> Result<Vec<u8>> {
        self.load_for(module, None, vp)
    }

    /// Persona-aware load. Mirrors `load`.
    pub fn load_for(
        &self,
        module: &str,
        persona: Option<&str>,
        vp: Viewport,
    ) -> Result<Vec<u8>> {
        let path = self.path_for(module, persona, vp);
        std::fs::read(&path).with_context(|| {
            format!(
                "Failed to load baseline {} — run with --update-baselines to regenerate",
                path.display()
            )
        })
    }

    /// Expose the root — for diagnostics + CLI `--baseline-dir` echo.
    pub fn root(&self) -> &Path {
        &self.root
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn save_load_round_trip() {
        let dir = tempfile::tempdir().expect("tempdir");
        let store = BaselineStore::with_root(dir.path().to_path_buf());
        let payload = b"fake-png-bytes-for-test".to_vec();

        assert!(!store.exists("dashboard", Viewport::Desktop));
        store
            .save("dashboard", Viewport::Desktop, &payload)
            .expect("save");
        assert!(store.exists("dashboard", Viewport::Desktop));

        let loaded = store.load("dashboard", Viewport::Desktop).expect("load");
        assert_eq!(loaded, payload);
    }

    #[test]
    fn save_overwrites_existing() {
        let dir = tempfile::tempdir().expect("tempdir");
        let store = BaselineStore::with_root(dir.path().to_path_buf());
        store.save("x", Viewport::Mobile, b"first").expect("save1");
        store.save("x", Viewport::Mobile, b"second").expect("save2");
        assert_eq!(
            store.load("x", Viewport::Mobile).expect("load"),
            b"second"
        );
    }

    #[test]
    fn path_separates_by_viewport() {
        let dir = tempfile::tempdir().expect("tempdir");
        let store = BaselineStore::with_root(dir.path().to_path_buf());
        assert_ne!(
            store.path("m", Viewport::Desktop),
            store.path("m", Viewport::Mobile)
        );
    }

    #[test]
    fn persona_slug_is_a_separate_path_segment() {
        // Phase 4b — baselines are keyed on (module, persona?, viewport).
        // The persona slug must be its own directory level, not a filename
        // suffix, so old-layout baselines don't collide with new ones.
        let dir = tempfile::tempdir().expect("tempdir");
        let store = BaselineStore::with_root(dir.path().to_path_buf());

        let none = store.path_for("dashboard", None, Viewport::Desktop);
        let peter = store.path_for("dashboard", Some("peter"), Viewport::Desktop);

        assert_ne!(none, peter, "persona must disambiguate the baseline path");
        // The persona slug must actually appear as a path component —
        // not just be concatenated into the filename.
        assert!(
            peter
                .components()
                .any(|c| c.as_os_str() == std::ffi::OsStr::new("peter")),
            "expected a `peter/` path segment in {}",
            peter.display()
        );
        // Old path layout is preserved verbatim for None.
        assert!(
            none.ends_with("dashboard/desktop.png"),
            "anonymous baseline path regressed: {}",
            none.display()
        );
        assert!(
            peter.ends_with("dashboard/peter/desktop.png"),
            "persona baseline path wrong: {}",
            peter.display()
        );
    }

    #[test]
    fn persona_save_load_round_trip_is_isolated_from_anonymous() {
        // Writing a persona-scoped baseline must NOT overwrite the
        // anonymous baseline at the same (module, viewport), and vice
        // versa — each lives in its own slot.
        let dir = tempfile::tempdir().expect("tempdir");
        let store = BaselineStore::with_root(dir.path().to_path_buf());

        store
            .save("dashboard", Viewport::Desktop, b"anon-bytes")
            .expect("save anon");
        store
            .save_for(
                "dashboard",
                Some("peter"),
                Viewport::Desktop,
                b"peter-bytes",
            )
            .expect("save peter");

        let anon = store.load("dashboard", Viewport::Desktop).expect("load anon");
        let peter = store
            .load_for("dashboard", Some("peter"), Viewport::Desktop)
            .expect("load peter");
        assert_eq!(anon, b"anon-bytes");
        assert_eq!(peter, b"peter-bytes");
    }
}
