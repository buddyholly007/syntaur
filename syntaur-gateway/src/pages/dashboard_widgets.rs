//! Dashboard widget trait + registry.
//!
//! Every widget implements `DashboardWidget` and exposes four renders —
//! one per size preset (S/M/L/XL). Content *and* affordances scale with
//! size, not just information density: S is a single glanceable fact, M
//! adds one action, L adds a scannable list, XL gives the full detail
//! view with in-place interaction.
//!
//! Widgets are kept deliberately dumb — they render from a `WidgetContext`
//! passed in at draw time. Data fetching happens on the client via
//! `fetch()` calls inside the widget's inline `<script>`, so the grid
//! host doesn't need to know what modules are connected.

use maud::{html, Markup, PreEscaped};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WidgetSize { S, M, L, Xl }

impl WidgetSize {
    pub fn from_cells(w: u8, h: u8) -> Self {
        // Size presets: S 2×2, M 4×2, L 4×4, XL 8×4. Match on area with
        // a bias toward the taller preset on ties (L > M) since extra
        // rows usually mean the user wants the scannable layout.
        let area = (w as u16) * (h as u16);
        if area >= 24 { WidgetSize::Xl }
        else if area >= 16 { WidgetSize::L }
        else if area >= 8  { WidgetSize::M }
        else { WidgetSize::S }
    }
}

pub struct WidgetContext {
    /// Unique instance id (stable across renders) so the browser can
    /// wire `document.getElementById(&id)` lookups without collisions.
    pub instance_id: String,
}

pub trait DashboardWidget: Send + Sync {
    /// Stable kind slug used in persisted layouts. NEVER change for a
    /// shipped widget — old layouts will point at this key.
    fn kind(&self) -> &'static str;
    fn title(&self) -> &'static str;
    /// Short human description shown in the widget drawer.
    fn description(&self) -> &'static str;
    /// (min_cols, min_rows). Grid refuses to shrink below.
    fn min_size(&self) -> (u8, u8) { (2, 2) }
    /// (max_cols, max_rows). Grid refuses to grow beyond.
    fn max_size(&self) -> (u8, u8) { (8, 4) }
    /// Default spawn size when added from the drawer.
    fn default_size(&self) -> (u8, u8) { (4, 2) }
    /// Render the widget at the given size. Data fetching lives inside
    /// the emitted `<script>` so re-renders don't round-trip the server.
    fn render(&self, size: WidgetSize, ctx: &WidgetContext) -> Markup;
}

// ─── Registry ──────────────────────────────────────────────────────────

pub fn registry() -> Vec<Box<dyn DashboardWidget>> {
    vec![
        // Top tier — default layout members. Order drives the "Add widget"
        // drawer too; most-useful-first so Peter-chat sits at the top.
        Box::new(ChatWidget),
        Box::new(TodoWidget),
        Box::new(CalendarWidget),
        Box::new(TodayWidget),
        Box::new(QuickActionsWidget),
        Box::new(NowPlayingWidget),
        // Secondary — useful additions but not in the first-run layout.
        Box::new(ApprovalsWidget),
        Box::new(LatestJournalWidget),
        Box::new(RecentResearchWidget),
        Box::new(SystemStatusWidget),
    ]
}

pub fn find(kind: &str) -> Option<Box<dyn DashboardWidget>> {
    registry().into_iter().find(|w| w.kind() == kind)
}

// ─── Shared card chrome ────────────────────────────────────────────────

fn card(title: &str, body: Markup) -> Markup {
    html! {
        div class="sd-card" {
            div class="sd-card-head" {
                span class="sd-card-title" { (title) }
            }
            div class="sd-card-body" { (body) }
        }
    }
}

// ─── Today (Scheduler) ─────────────────────────────────────────────────

pub struct TodayWidget;

impl DashboardWidget for TodayWidget {
    fn kind(&self) -> &'static str { "today" }
    fn title(&self) -> &'static str { "Today" }
    fn description(&self) -> &'static str { "Your next calendar events and the day's weather." }
    fn min_size(&self) -> (u8, u8) { (2, 2) }
    fn max_size(&self) -> (u8, u8) { (8, 4) }
    fn default_size(&self) -> (u8, u8) { (4, 2) }

    fn render(&self, size: WidgetSize, ctx: &WidgetContext) -> Markup {
        let id = &ctx.instance_id;
        let body = match size {
            WidgetSize::S => html! {
                div class="sd-s-stack" id=(format!("{id}-content")) {
                    div class="sd-big-num" data-slot="count" { "0" }
                    div class="sd-mute" { "events today" }
                }
            },
            WidgetSize::M => html! {
                div class="sd-m-row" id=(format!("{id}-content")) {
                    div class="sd-m-left" {
                        div class="sd-big-num" data-slot="count" { "0" }
                        div class="sd-mute" { "today" }
                    }
                    div class="sd-m-right" {
                        div class="sd-label" { "Next" }
                        div class="sd-next-title" data-slot="next-title" { "Nothing scheduled" }
                        div class="sd-next-time" data-slot="next-time" { "" }
                    }
                }
                div class="sd-card-foot" {
                    a href="/scheduler" class="sd-action" { "Open scheduler →" }
                }
            },
            WidgetSize::L => html! {
                ul class="sd-list" id=(format!("{id}-list")) {
                    li class="sd-list-empty" { "Nothing scheduled — enjoy the quiet." }
                }
                div class="sd-card-foot" {
                    a href="/scheduler" class="sd-action" { "Open scheduler →" }
                }
            },
            WidgetSize::Xl => html! {
                div class="sd-xl-grid" id=(format!("{id}-xl")) {
                    div class="sd-xl-left" {
                        div class="sd-label" { "Today" }
                        ul class="sd-list" data-slot="events" {
                            li class="sd-list-empty" { "Nothing scheduled — enjoy the quiet." }
                        }
                    }
                    div class="sd-xl-right" {
                        div class="sd-label" { "Week ahead" }
                        div class="sd-weekbar" data-slot="weekbar" {}
                        div class="sd-label" style="margin-top:16px" { "Weather" }
                        div class="sd-weather" data-slot="weather" { "—" }
                    }
                }
            },
        };
        let markup = card("Today", body);
        html! {
            (markup)
            script { (PreEscaped(&format!(r#"
(function() {{
  const root = document.getElementById('{id}');
  if (!root) return;
  window.sdFetch('/api/scheduler/today', {{ credentials:'same-origin' }})
    .then(r => r.ok ? r.json() : null)
    .then(d => {{
      if (!d) return;
      const events = Array.isArray(d.events) ? d.events : [];
      const countEl = root.querySelector('[data-slot=count]');
      if (countEl) countEl.textContent = String(events.length);
      const next = events.find(e => !e.past) || events[0];
      const nt = root.querySelector('[data-slot=next-title]');
      if (nt) nt.textContent = next ? next.title : 'Nothing scheduled';
      const ntime = root.querySelector('[data-slot=next-time]');
      if (ntime) ntime.textContent = next ? (next.time_label || '') : '';
      const list = root.querySelector('[data-slot=events]') || document.getElementById('{id}-list');
      if (list) {{
        list.innerHTML = events.length
          ? events.slice(0,6).map(e => `<li class="sd-list-item"><span class="sd-list-time">${{e.time_label||''}}</span><span class="sd-list-title">${{e.title}}</span></li>`).join('')
          : '<li class="sd-list-empty">Nothing scheduled — enjoy the quiet.</li>';
      }}
      const weather = root.querySelector('[data-slot=weather]');
      if (weather && d.weather) weather.textContent = d.weather.summary + ' · ' + (d.weather.temp_f||'') + '°';
      const wb = root.querySelector('[data-slot=weekbar]');
      if (wb && Array.isArray(d.week)) {{
        wb.innerHTML = d.week.map(day =>
          `<div class="sd-weekbar-day" style="height:${{Math.min(40, 6 + (day.count||0)*8)}}px" title="${{day.label}}: ${{day.count}}"></div>`
        ).join('');
      }}
    }})
    .catch(() => {{}});
}})();
"#))) }
        }
    }
}

// ─── Now Playing (Music) ───────────────────────────────────────────────

pub struct NowPlayingWidget;

impl DashboardWidget for NowPlayingWidget {
    fn kind(&self) -> &'static str { "now_playing" }
    fn title(&self) -> &'static str { "Now playing" }
    fn description(&self) -> &'static str { "Current track with playback controls." }
    fn min_size(&self) -> (u8, u8) { (2, 2) }
    fn max_size(&self) -> (u8, u8) { (8, 4) }
    fn default_size(&self) -> (u8, u8) { (4, 2) }

    fn render(&self, size: WidgetSize, ctx: &WidgetContext) -> Markup {
        let id = &ctx.instance_id;
        let body = match size {
            WidgetSize::S => html! {
                div class="sd-s-stack" {
                    div class="sd-np-title-s" data-slot="title" { "—" }
                    div class="sd-mute" data-slot="artist" { "" }
                }
            },
            WidgetSize::M => html! {
                div class="sd-np-row" {
                    div class="sd-np-art" data-slot="art" {}
                    div class="sd-np-text" {
                        div class="sd-np-title" data-slot="title" { "Nothing playing" }
                        div class="sd-mute" data-slot="artist" { "" }
                    }
                    div class="sd-np-ctrls" {
                        button class="sd-np-btn" data-act="prev" aria-label="Previous" { "‹" }
                        button class="sd-np-btn sd-np-play" data-act="toggle" aria-label="Play/pause" { "▶" }
                        button class="sd-np-btn" data-act="next" aria-label="Next" { "›" }
                    }
                }
                div class="sd-np-progress" {
                    div class="sd-np-progress-fill" data-slot="progress" style="width:0%" {}
                }
            },
            WidgetSize::L => html! {
                div class="sd-np-row sd-np-row-l" {
                    div class="sd-np-art sd-np-art-l" data-slot="art" {}
                    div class="sd-np-text" {
                        div class="sd-np-title" data-slot="title" { "Nothing playing" }
                        div class="sd-mute" data-slot="artist" { "" }
                        div class="sd-mute" data-slot="album" { "" }
                    }
                }
                div class="sd-np-progress" {
                    div class="sd-np-progress-fill" data-slot="progress" style="width:0%" {}
                }
                div class="sd-np-ctrls sd-np-ctrls-l" {
                    button class="sd-np-btn" data-act="prev" aria-label="Previous" { "‹" }
                    button class="sd-np-btn sd-np-play" data-act="toggle" aria-label="Play/pause" { "▶" }
                    button class="sd-np-btn" data-act="next" aria-label="Next" { "›" }
                    a href="/music" class="sd-action" { "Queue →" }
                }
            },
            WidgetSize::Xl => html! {
                div class="sd-xl-grid" {
                    div class="sd-np-art sd-np-art-xl" data-slot="art" {}
                    div {
                        div class="sd-np-title sd-np-title-xl" data-slot="title" { "Nothing playing" }
                        div class="sd-mute" data-slot="artist" { "" }
                        div class="sd-mute" data-slot="album" { "" }
                        div class="sd-np-progress" style="margin-top:12px" {
                            div class="sd-np-progress-fill" data-slot="progress" style="width:0%" {}
                        }
                        div class="sd-np-ctrls sd-np-ctrls-l" {
                            button class="sd-np-btn" data-act="prev" { "‹" }
                            button class="sd-np-btn sd-np-play" data-act="toggle" { "▶" }
                            button class="sd-np-btn" data-act="next" { "›" }
                            a href="/music" class="sd-action" { "Open music →" }
                        }
                        div class="sd-label" style="margin-top:16px" { "Up next" }
                        ul class="sd-list" data-slot="queue" {
                            li class="sd-list-empty" { "Queue is empty." }
                        }
                    }
                }
            },
        };
        let markup = card("Now playing", body);
        html! {
            (markup)
            script { (PreEscaped(&format!(r#"
(function() {{
  const root = document.getElementById('{id}');
  if (!root) return;

  // Dashboard widget mirrors the SAME state surfaced by the bottom-
  // right pill: localStorage.syntaurMusic + the global-audio element
  // in the top bar. When local playback is active the widget reflects
  // it in real time (no polling). When there's no local session, fall
  // back to /api/music/now_playing for cloud sources (HA / Apple /
  // Spotify) on a slower poll.

  const ga = document.getElementById('global-audio');
  const MUSIC_KEY = 'syntaurMusic';

  function readLocal() {{
    try {{ return JSON.parse(localStorage.getItem(MUSIC_KEY) || 'null'); }} catch (_) {{ return null; }}
  }}
  function fmtTime(sec) {{
    if (!isFinite(sec) || sec < 0) return '';
    const m = Math.floor(sec / 60);
    const s = Math.floor(sec % 60);
    return m + ':' + String(s).padStart(2, '0');
  }}
  function setText(slot, text) {{
    root.querySelectorAll('[data-slot=' + slot + ']').forEach(el => el.textContent = text);
  }}
  function setArt(url) {{
    const bg = url ? "url('" + url + "')" : '';
    root.querySelectorAll('[data-slot=art]').forEach(el => {{
      el.style.backgroundImage = bg;
      el.classList.toggle('sd-np-art-empty', !url);
    }});
  }}
  function setProgress(frac) {{
    root.querySelectorAll('[data-slot=progress]').forEach(el => {{
      el.style.width = Math.max(0, Math.min(1, frac)) * 100 + '%';
    }});
  }}
  function setPlayIcon(playing) {{
    const play = root.querySelector('[data-act=toggle]');
    if (play) play.textContent = playing ? '❚❚' : '▶';
  }}

  function paintLocal() {{
    const s = readLocal();
    if (!s || !s.trackId) return false;
    setText('title', s.title || ('Track ' + s.trackId));
    setText('artist', s.artist || '');
    setText('album', s.album || '');
    // Mint a 60s stream token before painting art so the URL doesn't
    // need auth-on-cookie (which the gateway doesn't have).
    const __artPath = '/api/music/local/art/' + s.trackId;
    window.sdStreamQuery(__artPath).then(qs => {{
      const cur = readLocal();
      if (cur && cur.trackId === s.trackId) setArt(__artPath + qs);
    }});
    const playing = !!(ga && !ga.paused && ga.currentTime > 0);
    setPlayIcon(playing);
    if (ga && ga.duration) {{
      setProgress((ga.currentTime || s.position || 0) / ga.duration);
    }} else if (s.position) {{
      // Pre-load: render the saved position against an unknown duration
      // as a thin sliver so the bar isn't stuck at 0%.
      setProgress(0.02);
    }} else {{
      setProgress(0);
    }}
    root.dataset.source = 'local';
    return true;
  }}

  let _lastCloudPoll = 0;
  async function paintCloud() {{
    // Throttle cloud poll to 5s; widget calls this from the rAF loop
    // too, so we'd otherwise flood the endpoint.
    const now = Date.now();
    if (now - _lastCloudPoll < 4500) return;
    _lastCloudPoll = now;
    try {{
      const r = await window.sdFetch('/api/music/now_playing', {{ credentials: 'same-origin' }});
      if (!r || !r.ok) return;
      const d = await r.json();
      if (!d) return;
      const has = !!d.song;
      setText('title', has ? d.song : 'Nothing playing');
      setText('artist', d.artist || '');
      setText('album', d.album || '');
      setArt(d.art_url || '');
      setPlayIcon(d.state === 'playing');
      if (d.duration_ms && d.position_ms) {{
        setProgress(d.position_ms / d.duration_ms);
      }} else {{
        setProgress(0);
      }}
      root.dataset.source = has ? 'cloud' : 'empty';
    }} catch (_) {{}}
  }}

  function paint() {{
    if (paintLocal()) return;
    paintCloud();
  }}

  // Real-time updates from global-audio (no polling for local).
  if (ga) {{
    ga.addEventListener('timeupdate', paint);
    ga.addEventListener('play',       paint);
    ga.addEventListener('pause',      paint);
    ga.addEventListener('ended',      paint);
    ga.addEventListener('loadedmetadata', paint);
  }}

  // Cross-tab + cross-page state changes (the pill writing to
  // localStorage on another page would surface here).
  window.addEventListener('storage', (ev) => {{
    if (ev.key === MUSIC_KEY) paint();
  }});

  // Click handlers: route through the same syntaurMpControl that the
  // pill uses, so local sessions go to global-audio directly and only
  // cloud sessions hit /api/music/control.
  root.querySelectorAll('[data-act]').forEach(btn => btn.addEventListener('click', ev => {{
    ev.stopPropagation();
    ev.preventDefault();
    const act = btn.getAttribute('data-act');
    const action = act === 'toggle' ? 'play_pause' : act;
    if (typeof window.syntaurMpControl === 'function') {{
      window.syntaurMpControl(action);
      // Local play/pause updates fire through audio events; cloud
      // actions don't, so kick a paint after a short delay.
      setTimeout(paint, 350);
    }}
  }}));

  paint();
  // Cloud-only fallback poll. paint() short-circuits to local when
  // a session exists, so this only refreshes when nothing local is
  // playing.
  const timer = setInterval(paint, 5000);
  root.__cleanup = () => clearInterval(timer);
}})();
"#))) }
        }
    }
}

// ─── Approvals (pending_approvals) ─────────────────────────────────────

pub struct ApprovalsWidget;

impl DashboardWidget for ApprovalsWidget {
    fn kind(&self) -> &'static str { "approvals" }
    fn title(&self) -> &'static str { "Approvals" }
    fn description(&self) -> &'static str { "Things waiting for your yes/no." }
    fn min_size(&self) -> (u8, u8) { (2, 2) }
    fn max_size(&self) -> (u8, u8) { (8, 4) }
    fn default_size(&self) -> (u8, u8) { (2, 2) }

    fn render(&self, size: WidgetSize, ctx: &WidgetContext) -> Markup {
        let id = &ctx.instance_id;
        let body = match size {
            WidgetSize::S => html! {
                div class="sd-s-stack" {
                    div class="sd-big-num" data-slot="count" { "—" }
                    div class="sd-mute" { "waiting" }
                }
            },
            WidgetSize::M => html! {
                div class="sd-m-row" {
                    div class="sd-m-left" {
                        div class="sd-big-num" data-slot="count" { "—" }
                        div class="sd-mute" { "waiting" }
                    }
                    div class="sd-m-right" {
                        div class="sd-label" { "Oldest" }
                        div class="sd-next-title" data-slot="oldest-title" { "—" }
                        div class="sd-next-time" data-slot="oldest-age" { "" }
                    }
                }
                div class="sd-card-foot" {
                    a href="/settings#helpers/approvals" class="sd-action" { "Review →" }
                }
            },
            WidgetSize::L | WidgetSize::Xl => html! {
                ul class="sd-list" data-slot="list" {
                    li class="sd-list-empty" { "Nothing waiting." }
                }
                div class="sd-card-foot" {
                    a href="/settings#helpers/approvals" class="sd-action" { "Review all →" }
                }
            },
        };
        let markup = card("Approvals", body);
        html! {
            (markup)
            script { (PreEscaped(&format!(r#"
(function() {{
  const root = document.getElementById('{id}');
  if (!root) return;
  function refresh() {{
    window.sdFetch('/api/approvals?status=pending', {{ credentials:'same-origin' }})
      .then(r => r.ok ? r.json() : null)
      .then(d => {{
        if (!d) return;
        const items = Array.isArray(d.approvals) ? d.approvals : [];
        root.querySelectorAll('[data-slot=count]').forEach(el => el.textContent = String(items.length));
        const oldest = items[items.length - 1];
        const ot = root.querySelector('[data-slot=oldest-title]');
        if (ot) ot.textContent = oldest ? (oldest.summary || oldest.kind || 'Approval') : 'Nothing waiting';
        const oa = root.querySelector('[data-slot=oldest-age]');
        if (oa && oldest && oldest.created_at) {{
          const age = Math.floor((Date.now()/1000 - oldest.created_at));
          oa.textContent = age < 60 ? `${{age}}s` : age < 3600 ? `${{Math.floor(age/60)}}m` : age < 86400 ? `${{Math.floor(age/3600)}}h` : `${{Math.floor(age/86400)}}d`;
        }} else if (oa) oa.textContent = '';
        const list = root.querySelector('[data-slot=list]');
        if (list) list.innerHTML = items.length
          ? items.slice(0,8).map(a => `<li class="sd-list-item"><span class="sd-list-time">${{a.source||a.kind||''}}</span><span class="sd-list-title">${{a.summary||'(no summary)'}}</span></li>`).join('')
          : '<li class="sd-list-empty">Nothing waiting.</li>';
      }}).catch(() => {{}});
  }}
  refresh();
  setInterval(refresh, 20000);
}})();
"#))) }
        }
    }
}

// ─── Latest Journal (journal_moments) ──────────────────────────────────

pub struct LatestJournalWidget;

impl DashboardWidget for LatestJournalWidget {
    fn kind(&self) -> &'static str { "latest_journal" }
    fn title(&self) -> &'static str { "Journal" }
    fn description(&self) -> &'static str { "Most recent moments you've captured." }
    fn min_size(&self) -> (u8, u8) { (2, 2) }
    fn max_size(&self) -> (u8, u8) { (8, 4) }
    fn default_size(&self) -> (u8, u8) { (4, 2) }

    fn render(&self, size: WidgetSize, ctx: &WidgetContext) -> Markup {
        let id = &ctx.instance_id;
        let body = match size {
            WidgetSize::S => html! {
                div class="sd-s-stack" {
                    div class="sd-big-num" data-slot="count" { "—" }
                    div class="sd-mute" { "moments this week" }
                }
            },
            WidgetSize::M => html! {
                div class="sd-journal-latest" {
                    div class="sd-label" { "Latest" }
                    div class="sd-journal-text" data-slot="latest-text" { "No entries yet." }
                    div class="sd-next-time" data-slot="latest-date" { "" }
                }
                div class="sd-card-foot" {
                    a href="/journal" class="sd-action" { "Open journal →" }
                }
            },
            WidgetSize::L | WidgetSize::Xl => html! {
                ul class="sd-list sd-journal-list" data-slot="list" {
                    li class="sd-list-empty" { "No entries yet." }
                }
                div class="sd-card-foot" {
                    a href="/journal" class="sd-action" { "Open journal →" }
                }
            },
        };
        let markup = card("Journal", body);
        html! {
            (markup)
            script { (PreEscaped(&format!(r#"
(function() {{
  const root = document.getElementById('{id}');
  if (!root) return;
  window.sdFetch('/api/journal/moments?limit=10', {{ credentials:'same-origin' }})
    .then(r => r.ok ? r.json() : null)
    .then(d => {{
      if (!d) return;
      const items = Array.isArray(d.moments) ? d.moments : (Array.isArray(d) ? d : []);
      const weekAgo = Math.floor(Date.now()/1000) - 7*86400;
      const thisWeek = items.filter(it => (it.created_at || 0) >= weekAgo).length;
      root.querySelectorAll('[data-slot=count]').forEach(el => el.textContent = String(thisWeek));
      const latest = items[0];
      const lt = root.querySelector('[data-slot=latest-text]');
      if (lt) lt.textContent = latest ? ((latest.text||'').slice(0, 140) + ((latest.text||'').length > 140 ? '…' : '')) : 'No entries yet.';
      const ld = root.querySelector('[data-slot=latest-date]');
      if (ld) ld.textContent = latest ? (latest.date || '') : '';
      const list = root.querySelector('[data-slot=list]');
      if (list) list.innerHTML = items.length
        ? items.slice(0,6).map(m => `<li class="sd-list-item"><span class="sd-list-time">${{m.date||''}}</span><span class="sd-list-title">${{(m.text||'').slice(0,90)}}</span></li>`).join('')
        : '<li class="sd-list-empty">No entries yet.</li>';
    }}).catch(() => {{}});
}})();
"#))) }
        }
    }
}

// ─── Recent Research (research_sessions) ───────────────────────────────

pub struct RecentResearchWidget;

impl DashboardWidget for RecentResearchWidget {
    fn kind(&self) -> &'static str { "recent_research" }
    fn title(&self) -> &'static str { "Research" }
    fn description(&self) -> &'static str { "Your latest research reports." }
    fn min_size(&self) -> (u8, u8) { (2, 2) }
    fn max_size(&self) -> (u8, u8) { (8, 4) }
    fn default_size(&self) -> (u8, u8) { (4, 2) }

    fn render(&self, size: WidgetSize, ctx: &WidgetContext) -> Markup {
        let id = &ctx.instance_id;
        let body = match size {
            WidgetSize::S => html! {
                div class="sd-s-stack" {
                    div class="sd-big-num" data-slot="count" { "—" }
                    div class="sd-mute" { "reports" }
                }
            },
            WidgetSize::M => html! {
                div class="sd-journal-latest" {
                    div class="sd-label" { "Latest" }
                    div class="sd-journal-text" data-slot="latest" { "No research yet." }
                    div class="sd-next-time" data-slot="status" { "" }
                }
                div class="sd-card-foot" {
                    a href="/knowledge?tab=research" class="sd-action" { "Open research →" }
                }
            },
            WidgetSize::L | WidgetSize::Xl => html! {
                ul class="sd-list" data-slot="list" {
                    li class="sd-list-empty" { "No research yet." }
                }
                div class="sd-card-foot" {
                    a href="/knowledge?tab=research" class="sd-action" { "Open research →" }
                }
            },
        };
        let markup = card("Research", body);
        html! {
            (markup)
            script { (PreEscaped(&format!(r#"
(function() {{
  const root = document.getElementById('{id}');
  if (!root) return;
  window.sdFetch('/api/research/recent', {{ credentials:'same-origin' }})
    .then(r => r.ok ? r.json() : null)
    .then(d => {{
      if (!d) return;
      const items = Array.isArray(d.sessions) ? d.sessions : (Array.isArray(d) ? d : []);
      root.querySelectorAll('[data-slot=count]').forEach(el => el.textContent = String(items.length));
      const latest = items[0];
      const lt = root.querySelector('[data-slot=latest]');
      if (lt) lt.textContent = latest ? latest.query : 'No research yet.';
      const ls = root.querySelector('[data-slot=status]');
      if (ls && latest) ls.textContent = latest.status || '';
      const list = root.querySelector('[data-slot=list]');
      if (list) list.innerHTML = items.length
        ? items.slice(0,6).map(r => `<li class="sd-list-item"><span class="sd-list-time">${{r.status||''}}</span><span class="sd-list-title">${{r.query||''}}</span></li>`).join('')
        : '<li class="sd-list-empty">No research yet.</li>';
    }}).catch(() => {{}});
}})();
"#))) }
        }
    }
}

// ─── System Status (gateway vitals) ────────────────────────────────────

pub struct SystemStatusWidget;

impl DashboardWidget for SystemStatusWidget {
    fn kind(&self) -> &'static str { "system_status" }
    fn title(&self) -> &'static str { "System" }
    fn description(&self) -> &'static str { "Gateway uptime, LLM provider, active modules." }
    fn min_size(&self) -> (u8, u8) { (2, 2) }
    fn max_size(&self) -> (u8, u8) { (4, 4) }
    fn default_size(&self) -> (u8, u8) { (2, 2) }

    fn render(&self, size: WidgetSize, ctx: &WidgetContext) -> Markup {
        let id = &ctx.instance_id;
        let body = match size {
            WidgetSize::S => html! {
                div class="sd-s-stack" {
                    div class="sd-big-num" data-slot="uptime" { "—" }
                    div class="sd-mute" data-slot="uptime-unit" { "uptime" }
                }
            },
            WidgetSize::M => html! {
                div class="sd-sys-grid" {
                    div {
                        div class="sd-label" { "Uptime" }
                        div class="sd-sys-value" data-slot="uptime-full" { "—" }
                    }
                    div {
                        div class="sd-label" { "LLM" }
                        div class="sd-sys-value" data-slot="llm" { "—" }
                    }
                    div {
                        div class="sd-label" { "Modules" }
                        div class="sd-sys-value" data-slot="modules" { "—" }
                    }
                    div {
                        div class="sd-label" { "Version" }
                        div class="sd-sys-value" data-slot="version" { "—" }
                    }
                }
            },
            WidgetSize::L | WidgetSize::Xl => html! {
                div class="sd-sys-grid sd-sys-grid-l" {
                    div {
                        div class="sd-label" { "Uptime" }
                        div class="sd-sys-value sd-sys-value-big" data-slot="uptime-full" { "—" }
                    }
                    div {
                        div class="sd-label" { "LLM provider" }
                        div class="sd-sys-value" data-slot="llm" { "—" }
                    }
                    div {
                        div class="sd-label" { "Modules" }
                        div class="sd-sys-value" data-slot="modules" { "—" }
                    }
                    div {
                        div class="sd-label" { "Version" }
                        div class="sd-sys-value" data-slot="version" { "—" }
                    }
                }
                div class="sd-card-foot" {
                    a href="/settings" class="sd-action" { "Open settings →" }
                }
            },
        };
        let markup = card("System", body);
        html! {
            (markup)
            script { (PreEscaped(&format!(r#"
(function() {{
  const root = document.getElementById('{id}');
  if (!root) return;
  function fmtUptime(s) {{
    if (s < 60) return s + 's';
    if (s < 3600) return Math.floor(s/60) + 'm';
    if (s < 86400) return Math.floor(s/3600) + 'h';
    return Math.floor(s/86400) + 'd';
  }}
  function refresh() {{
    window.sdFetch('/api/dashboard/system', {{ credentials:'same-origin' }})
      .then(r => r.ok ? r.json() : null)
      .then(d => {{
        if (!d) return;
        root.querySelectorAll('[data-slot=uptime]').forEach(el => el.textContent = fmtUptime(d.uptime_secs || 0));
        root.querySelectorAll('[data-slot=uptime-full]').forEach(el => el.textContent = fmtUptime(d.uptime_secs || 0));
        root.querySelectorAll('[data-slot=llm]').forEach(el => el.textContent = d.llm_provider || '—');
        root.querySelectorAll('[data-slot=modules]').forEach(el => el.textContent = `${{d.modules_on || 0}}/${{d.modules_total || 0}}`);
        root.querySelectorAll('[data-slot=version]').forEach(el => el.textContent = d.version || '—');
      }}).catch(() => {{}});
  }}
  refresh();
  setInterval(refresh, 30000);
}})();
"#))) }
        }
    }
}

// ─── Chat (Peter dashboard persona + handoff) ──────────────────────────

pub struct ChatWidget;

impl DashboardWidget for ChatWidget {
    fn kind(&self) -> &'static str { "chat" }
    fn title(&self) -> &'static str { "Chat" }
    fn description(&self) -> &'static str { "Talk to Peter on the dashboard. Switch to any other agent at any time." }
    fn min_size(&self) -> (u8, u8) { (4, 2) }
    fn max_size(&self) -> (u8, u8) { (8, 4) }
    fn default_size(&self) -> (u8, u8) { (4, 4) }

    fn render(&self, size: WidgetSize, ctx: &WidgetContext) -> Markup {
        let id = &ctx.instance_id;
        let show_chips = matches!(size, WidgetSize::L | WidgetSize::Xl);
        let show_history = matches!(size, WidgetSize::M | WidgetSize::L | WidgetSize::Xl);
        let body = html! {
            div class="sd-chat" data-slot="chat-root" {
                @if show_chips {
                    div class="sd-chat-chips" data-slot="chips" {
                        span class="sd-mute" { "Loading agents…" }
                    }
                }
                @if show_history {
                    div class="sd-chat-log" data-slot="log" {
                        div class="sd-chat-welcome" {
                            strong data-slot="agent-name" { "Peter" }
                            span class="sd-mute" { " — hi. What's on your mind?" }
                        }
                    }
                }
                form class="sd-chat-form" data-slot="form" autocomplete="off" {
                    input type="text" class="sd-chat-input" data-slot="input"
                        placeholder="Ask Peter anything…" maxlength="2000" {}
                    button type="submit" class="sd-chat-send" data-slot="send" aria-label="Send" {
                        svg viewBox="0 0 24 24" width="14" height="14" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" {
                            path d="M5 12h14" {}
                            path d="M12 5l7 7-7 7" {}
                        }
                    }
                }
                div class="sd-chat-foot" {
                    a href="/chat" class="sd-action" { "Open full chat →" }
                }
            }
        };
        let markup = card("Chat", body);
        html! {
            (markup)
            script { (PreEscaped(&format!(r#"
(function() {{
  const root = document.getElementById('{id}');
  if (!root) return;
  const chips = root.querySelector('[data-slot=chips]');
  const log = root.querySelector('[data-slot=log]');
  const form = root.querySelector('[data-slot=form]');
  const input = root.querySelector('[data-slot=input]');
  const agentNameEl = root.querySelector('[data-slot=agent-name]');
  // Peter is the default dashboard persona. Other main-thread agents can
  // take over by clicking a chip; handoff is conversational (the user
  // asks Peter to bring someone in, or flips directly).
  let currentAgent = 'main';
  let currentName = 'Peter';
  function setAgent(id, name) {{
    currentAgent = id; currentName = name;
    if (agentNameEl) agentNameEl.textContent = name;
    if (input) input.placeholder = `Ask ${{name}} anything…`;
    if (chips) chips.querySelectorAll('.sd-chat-chip').forEach(c =>
      c.classList.toggle('active', c.dataset.agent === id));
  }}
  function renderChips(agents) {{
    if (!chips) return;
    chips.innerHTML = '';
    // Always show Peter (main) first. Then any `is_main_thread`
    // user-agents (Felix, Crimson Lantern, Woodworks, Kyron …).
    const dash = [{{ agent_id:'main', display_name:'Peter' }}]
      .concat((agents || []).filter(a => a.is_main_thread && a.agent_id !== 'main'));
    dash.forEach(a => {{
      const b = document.createElement('button');
      b.type = 'button';
      b.className = 'sd-chat-chip' + (a.agent_id === currentAgent ? ' active' : '');
      b.dataset.agent = a.agent_id;
      b.textContent = a.display_name || a.agent_id;
      b.addEventListener('click', () => setAgent(a.agent_id, b.textContent));
      chips.appendChild(b);
    }});
  }}
  window.sdFetch('/api/me/agents', {{ credentials:'same-origin' }})
    .then(r => r.ok ? r.json() : null)
    .then(d => renderChips((d && d.agents) || []))
    .catch(() => renderChips([]));
  function appendMsg(kind, text) {{
    if (!log) return;
    const wel = log.querySelector('.sd-chat-welcome');
    if (wel) wel.remove();
    const row = document.createElement('div');
    row.className = 'sd-chat-msg sd-chat-msg-' + kind;
    row.innerHTML = `<span class="sd-chat-who">${{kind === 'user' ? 'You' : currentName}}</span>` +
      `<span class="sd-chat-body"></span>`;
    row.querySelector('.sd-chat-body').textContent = text;
    log.appendChild(row);
    log.scrollTop = log.scrollHeight;
    return row;
  }}
  form.addEventListener('submit', async ev => {{
    ev.preventDefault();
    const text = (input.value || '').trim();
    if (!text) return;
    appendMsg('user', text);
    input.value = ''; input.disabled = true;
    const typing = appendMsg('agent', '…');
    if (typing) typing.classList.add('sd-chat-typing');
    try {{
      // /api/message expects `token` in the JSON body (not just the
       // Authorization header). sdFetch adds the header regardless;
       // the body field is what the handler actually uses.
      const tk = (window.sdToken && window.sdToken()) || '';
      const r = await window.sdFetch('/api/message', {{
        method: 'POST', credentials: 'same-origin',
        headers: {{ 'Content-Type': 'application/json' }},
        body: JSON.stringify({{ token: tk, agent: currentAgent, message: text }})
      }});
      const d = r.ok ? await r.json() : null;
      // /api/message returns (response, rounds, conversation_id) on
      // success or (error) on failure. The older handler returned
      // (message) / (text); keep those as compat fallbacks.
      const reply = (d && (d.response || d.message || d.text)) ||
        (d && d.error ? `(${{d.error}})` : "(no response)");
      if (typing) {{
        typing.classList.remove('sd-chat-typing');
        typing.querySelector('.sd-chat-body').textContent = reply;
      }}
    }} catch (e) {{
      if (typing) typing.querySelector('.sd-chat-body').textContent = "(network error)";
    }} finally {{
      input.disabled = false; input.focus();
    }}
  }});
}})();
"#))) }
        }
    }
}

// ─── Todo (dashboard Thaddeus tasks) ───────────────────────────────────

pub struct TodoWidget;

impl DashboardWidget for TodoWidget {
    fn kind(&self) -> &'static str { "todo" }
    fn title(&self) -> &'static str { "To do" }
    fn description(&self) -> &'static str { "Quick personal checklist. Check off, add, reorder." }
    fn min_size(&self) -> (u8, u8) { (2, 2) }
    fn max_size(&self) -> (u8, u8) { (4, 4) }
    fn default_size(&self) -> (u8, u8) { (4, 2) }

    fn render(&self, size: WidgetSize, ctx: &WidgetContext) -> Markup {
        let id = &ctx.instance_id;
        let show_list = !matches!(size, WidgetSize::S);
        let show_input = !matches!(size, WidgetSize::S);
        let body = html! {
            @if let WidgetSize::S = size {
                div class="sd-s-stack" {
                    div class="sd-big-num" data-slot="open-count" { "—" }
                    div class="sd-mute" { "open" }
                }
            }
            @if show_list {
                ul class="sd-todo-list" data-slot="list" {
                    li class="sd-list-empty" data-slot="empty" { "Nothing on the list — add your first below." }
                }
            }
            @if show_input {
                form class="sd-todo-form" data-slot="form" autocomplete="off" {
                    input type="text" class="sd-todo-input" data-slot="input"
                        placeholder="Add a todo…" maxlength="500" {}
                    button type="submit" class="sd-todo-add" aria-label="Add" { "+" }
                }
            }
        };
        let markup = card("To do", body);
        html! {
            (markup)
            script { (PreEscaped(&format!(r#"
(function() {{
  const root = document.getElementById('{id}');
  if (!root) return;
  const listEl = root.querySelector('[data-slot=list]');
  const formEl = root.querySelector('[data-slot=form]');
  const inputEl = root.querySelector('[data-slot=input]');
  const countEl = root.querySelector('[data-slot=open-count]');
  function render(todos) {{
    const open = todos.filter(t => !t.done);
    if (countEl) countEl.textContent = String(open.length);
    if (!listEl) return;
    if (!todos.length) {{
      listEl.innerHTML = '<li class="sd-list-empty">Nothing on the list — add your first below.</li>';
      return;
    }}
    listEl.innerHTML = '';
    todos.forEach(t => {{
      const li = document.createElement('li');
      li.className = 'sd-todo-item' + (t.done ? ' done' : '');
      li.dataset.id = t.id;
      li.innerHTML = `
        <label class="sd-todo-check">
          <input type="checkbox" ${{t.done ? 'checked' : ''}}>
          <span class="sd-todo-text"></span>
        </label>
        <button class="sd-todo-del" aria-label="Delete" title="Delete">×</button>`;
      li.querySelector('.sd-todo-text').textContent = t.text;
      li.querySelector('input').addEventListener('change', ev => toggle(t.id, ev.target.checked));
      li.querySelector('.sd-todo-del').addEventListener('click', () => remove(t.id));
      listEl.appendChild(li);
    }});
  }}
  function refresh() {{
    window.sdFetch('/api/todos', {{ credentials:'same-origin' }})
      .then(r => r.ok ? r.json() : null)
      .then(d => render((d && d.todos) || []))
      .catch(() => {{}});
  }}
  // /api/todos uses a body-token auth convention (TodoCreateRequest /
   // TodoUpdateRequest / TodoDeleteRequest all expect `token` in the
   // JSON payload). sdFetch adds the Authorization header too, but the
   // handler reads from the body — so include both.
  function tk() {{ return (window.sdToken && window.sdToken()) || ''; }}
  function toggle(id, done) {{
    window.sdFetch('/api/todos/' + id, {{
      method:'PUT', credentials:'same-origin',
      headers: {{ 'Content-Type':'application/json' }},
      body: JSON.stringify({{ token: tk(), done }})
    }}).then(refresh).catch(() => {{}});
  }}
  function remove(id) {{
    window.sdFetch('/api/todos/' + id, {{
      method:'DELETE', credentials:'same-origin',
      headers: {{ 'Content-Type':'application/json' }},
      body: JSON.stringify({{ token: tk() }})
    }}).then(refresh).catch(() => {{}});
  }}
  if (formEl) formEl.addEventListener('submit', ev => {{
    ev.preventDefault();
    const text = (inputEl.value || '').trim();
    if (!text) return;
    inputEl.value = '';
    window.sdFetch('/api/todos', {{
      method:'POST', credentials:'same-origin',
      headers: {{ 'Content-Type':'application/json' }},
      body: JSON.stringify({{ token: tk(), text }})
    }}).then(refresh).catch(() => {{}});
  }});
  refresh();
}})();
"#))) }
        }
    }
}

// ─── Calendar (mini month) ─────────────────────────────────────────────

pub struct CalendarWidget;

impl DashboardWidget for CalendarWidget {
    fn kind(&self) -> &'static str { "calendar" }
    fn title(&self) -> &'static str { "Calendar" }
    fn description(&self) -> &'static str { "Mini month view. Click a date to open the Scheduler on that day." }
    fn min_size(&self) -> (u8, u8) { (2, 2) }
    fn max_size(&self) -> (u8, u8) { (4, 4) }
    fn default_size(&self) -> (u8, u8) { (4, 2) }

    fn render(&self, size: WidgetSize, ctx: &WidgetContext) -> Markup {
        let id = &ctx.instance_id;
        // Full month grid only fits at L (4×4 = ~320px) or XL. At M the
        // widget is ~144px tall — a month grid + foot would overflow and
        // the foot link rendered mid-widget (confirmed via screenshot).
        // M now shows a compact today + next event view instead.
        let full_grid = matches!(size, WidgetSize::L | WidgetSize::Xl);
        let body = html! {
            @if let WidgetSize::S = size {
                div class="sd-s-stack" {
                    div class="sd-cal-today-num" data-slot="today-num" { "—" }
                    div class="sd-mute" data-slot="today-month" { "" }
                    div class="sd-mute" data-slot="today-events" { "No events" }
                }
            }
            @if let WidgetSize::M = size {
                div class="sd-m-row" {
                    div class="sd-cal-today-cell" {
                        div class="sd-cal-today-dow" data-slot="today-dow" { "—" }
                        div class="sd-cal-today-num" data-slot="today-num" { "—" }
                        div class="sd-mute" data-slot="today-month" { "" }
                    }
                    div class="sd-m-right" {
                        div class="sd-label" { "Today" }
                        div class="sd-next-title" data-slot="today-events" { "No events" }
                        div class="sd-label" style="margin-top:10px" { "Next" }
                        div class="sd-next-title" data-slot="next-title" { "—" }
                        div class="sd-next-time" data-slot="next-time" { "" }
                    }
                }
                div class="sd-card-foot" {
                    a href="/scheduler" class="sd-action" { "Open scheduler →" }
                }
            }
            @if full_grid {
                div class="sd-cal-header" {
                    button class="sd-cal-nav" data-slot="prev" aria-label="Previous month" { "‹" }
                    div class="sd-cal-title" data-slot="month-title" { "—" }
                    button class="sd-cal-nav" data-slot="next" aria-label="Next month" { "›" }
                }
                div class="sd-cal-dow" {
                    @for d in &["S","M","T","W","T","F","S"] {
                        div class="sd-cal-dow-cell" { (*d) }
                    }
                }
                div class="sd-cal-grid" data-slot="grid" {}
                div class="sd-card-foot" {
                    a href="/scheduler" class="sd-action" { "Open scheduler →" }
                }
            }
        };
        let markup = card("Calendar", body);
        html! {
            (markup)
            script { (PreEscaped(&format!(r#"
(function() {{
  const root = document.getElementById('{id}');
  if (!root) return;
  let viewDate = new Date();
  viewDate.setDate(1);
  function fmtDate(d) {{ return d.getFullYear() + '-' + String(d.getMonth()+1).padStart(2,'0') + '-' + String(d.getDate()).padStart(2,'0'); }}
  let events = [];
  function firstOfMonth(d) {{ return new Date(d.getFullYear(), d.getMonth(), 1); }}
  function lastOfMonth(d) {{ return new Date(d.getFullYear(), d.getMonth()+1, 0); }}
  function renderSmall() {{
    const today = new Date();
    const tEl = root.querySelector('[data-slot=today-num]');
    const mEl = root.querySelector('[data-slot=today-month]');
    const eEl = root.querySelector('[data-slot=today-events]');
    const dowEl = root.querySelector('[data-slot=today-dow]');
    const ntEl = root.querySelector('[data-slot=next-title]');
    const nmEl = root.querySelector('[data-slot=next-time]');
    if (tEl) tEl.textContent = String(today.getDate());
    if (mEl) mEl.textContent = today.toLocaleString(undefined, {{ month:'long' }});
    if (dowEl) dowEl.textContent = today.toLocaleString(undefined, {{ weekday:'short' }}).toUpperCase();
    const todayStr = fmtDate(today);
    const todays = events.filter(e => (e.date || '') === todayStr);
    if (eEl) eEl.textContent = todays.length ? `${{todays.length}} event${{todays.length===1?'':'s'}} today` : 'Nothing scheduled today';
    // Find next upcoming event (today or later, first one by start_time).
    const upcoming = events
      .filter(e => (e.date || '') >= todayStr)
      .sort((a, b) => (a.start_time || '').localeCompare(b.start_time || ''))[0];
    if (ntEl) ntEl.textContent = upcoming ? (upcoming.title || 'Untitled') : 'Nothing coming up';
    if (nmEl) nmEl.textContent = upcoming ? (upcoming.start_time || '').slice(0, 10) : '';
  }}
  function renderGrid() {{
    const grid = root.querySelector('[data-slot=grid]');
    const title = root.querySelector('[data-slot=month-title]');
    if (!grid) return;  // S and M sizes: no grid — handled by renderSmall alone.
    if (title) title.textContent = viewDate.toLocaleString(undefined, {{ month:'long', year:'numeric' }});
    grid.innerHTML = '';
    const first = firstOfMonth(viewDate);
    const last = lastOfMonth(viewDate);
    const startDow = first.getDay();
    const daysInMonth = last.getDate();
    const today = new Date(); today.setHours(0,0,0,0);
    // Lead padding (previous month tail).
    for (let i = 0; i < startDow; i++) {{
      const cell = document.createElement('div');
      cell.className = 'sd-cal-cell sd-cal-cell-pad';
      grid.appendChild(cell);
    }}
    const eventDates = new Set(events.map(e => (e.date || '').slice(0,10)));
    for (let day = 1; day <= daysInMonth; day++) {{
      const d = new Date(viewDate.getFullYear(), viewDate.getMonth(), day);
      const cell = document.createElement('a');
      cell.href = '/scheduler?date=' + fmtDate(d);
      cell.className = 'sd-cal-cell';
      if (d.getTime() === today.getTime()) cell.classList.add('sd-cal-today');
      if (eventDates.has(fmtDate(d))) cell.classList.add('sd-cal-has-event');
      cell.innerHTML = `<span>${{day}}</span>`;
      grid.appendChild(cell);
    }}
  }}
  function loadEvents() {{
    const start = firstOfMonth(viewDate), end = lastOfMonth(viewDate);
    window.sdFetch(`/api/calendar?start=${{fmtDate(start)}}&end=${{fmtDate(end)}}`, {{ credentials:'same-origin' }})
      .then(r => r.ok ? r.json() : null)
      .then(d => {{
        const raw = Array.isArray(d && d.events) ? d.events : [];
        // Normalize: /api/calendar returns ISO-ish start_time strings
        // ("YYYY-MM-DDTHH:MM:SS"); we only need the date prefix for
        // the grid's event-dot lookup.
        events = raw.map(e => (Object.assign({{}}, e, {{
          date: (e.start_time || '').slice(0, 10)
        }})));
        renderSmall();
        renderGrid();
      }})
      .catch(() => {{ renderSmall(); renderGrid(); }});
  }}
  const prev = root.querySelector('[data-slot=prev]');
  const next = root.querySelector('[data-slot=next]');
  if (prev) prev.addEventListener('click', () => {{ viewDate = new Date(viewDate.getFullYear(), viewDate.getMonth()-1, 1); loadEvents(); }});
  if (next) next.addEventListener('click', () => {{ viewDate = new Date(viewDate.getFullYear(), viewDate.getMonth()+1, 1); loadEvents(); }});
  loadEvents();
}})();
"#))) }
        }
    }
}

// ─── Quick Actions (module launcher grid) ──────────────────────────────

pub struct QuickActionsWidget;

impl DashboardWidget for QuickActionsWidget {
    fn kind(&self) -> &'static str { "quick_actions" }
    fn title(&self) -> &'static str { "Quick actions" }
    fn description(&self) -> &'static str { "Jump straight into the modules you use most." }
    fn min_size(&self) -> (u8, u8) { (2, 2) }
    fn max_size(&self) -> (u8, u8) { (8, 4) }
    fn default_size(&self) -> (u8, u8) { (4, 2) }

    fn render(&self, size: WidgetSize, ctx: &WidgetContext) -> Markup {
        let _id = &ctx.instance_id;
        // Each action: (href, label, glyph). Keep the set small + high-signal.
        // Users can expand this via Customize → Widget config in a later pass.
        // Icons were previously decorative glyphs (✿ ❦ ▲ ⌂ ›_ ◑) which
        // Opus flagged as "unclear visual indicators" during a verify
        // run. Replaced with widely-recognized emoji that read as the
        // thing they are.
        // Smart Home moved into the first 4 (default M-size widget
        // shows count=4) so users with the default dashboard layout can
        // find it without typing the URL. The full ordering still matters
        // for L / Xl sizes which surface all 8.
        let actions: &[(&str, &str, &str)] = &[
            ("/scheduler",  "Scheduler",  "📅"),
            ("/smart-home", "Smart home", "🏠"),
            ("/journal",    "Journal",    "📔"),
            ("/music",      "Music",      "🎵"),
            ("/knowledge",  "Knowledge",  "📚"),
            ("/tax",        "Tax",        "💰"),
            ("/coders",     "Coders",     "💻"),
            ("/social",     "Social",     "💬"),
        ];
        let count = match size {
            WidgetSize::S  => 1,
            WidgetSize::M  => 4,
            WidgetSize::L  => 8,
            WidgetSize::Xl => 8,
        };
        let grid_class = match size {
            WidgetSize::S => "sd-qa-grid sd-qa-1",
            WidgetSize::M => "sd-qa-grid sd-qa-4",
            WidgetSize::L | WidgetSize::Xl => "sd-qa-grid sd-qa-8",
        };
        let body = html! {
            div class=(grid_class) {
                @for (href, label, glyph) in actions.iter().take(count) {
                    a class="sd-qa-tile" href=(*href) {
                        span class="sd-qa-glyph" { (*glyph) }
                        span class="sd-qa-label" { (*label) }
                    }
                }
            }
        };
        card("Quick actions", body)
    }
}

