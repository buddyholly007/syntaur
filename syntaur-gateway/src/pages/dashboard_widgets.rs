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
        Box::new(TodayWidget),
        Box::new(NowPlayingWidget),
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
                    div class="sd-big-num" data-slot="count" { "—" }
                    div class="sd-mute" { "events today" }
                }
            },
            WidgetSize::M => html! {
                div class="sd-m-row" id=(format!("{id}-content")) {
                    div class="sd-m-left" {
                        div class="sd-big-num" data-slot="count" { "—" }
                        div class="sd-mute" { "today" }
                    }
                    div class="sd-m-right" {
                        div class="sd-label" { "Next" }
                        div class="sd-next-title" data-slot="next-title" { "—" }
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
  fetch('/api/scheduler/today', {{ credentials:'same-origin' }})
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
  function refresh() {{
    fetch('/api/music/now_playing', {{ credentials:'same-origin' }})
      .then(r => r.ok ? r.json() : null)
      .then(d => {{
        if (!d) return;
        const has = !!(d.song);
        root.querySelectorAll('[data-slot=title]').forEach(el => el.textContent = has ? d.song : 'Nothing playing');
        root.querySelectorAll('[data-slot=artist]').forEach(el => el.textContent = d.artist || '');
        root.querySelectorAll('[data-slot=album]').forEach(el => el.textContent = d.album || '');
        const art = d.art_url ? `url('${{d.art_url}}')` : '';
        root.querySelectorAll('[data-slot=art]').forEach(el => {{
          el.style.backgroundImage = art;
          el.classList.toggle('sd-np-art-empty', has && !d.art_url);
        }});
        const play = root.querySelector('[data-act=toggle]');
        if (play) play.textContent = (d.state === 'playing') ? '❚❚' : '▶';
        const q = root.querySelector('[data-slot=queue]');
        if (q) q.innerHTML = '<li class="sd-list-empty">Queue shown in the Music module.</li>';
      }}).catch(() => {{}});
  }}
  refresh();
  root.querySelectorAll('[data-act]').forEach(btn => btn.addEventListener('click', ev => {{
    ev.stopPropagation();
    const act = btn.getAttribute('data-act');
    const action = act === 'toggle' ? 'play_pause' : act;
    fetch('/api/music/control', {{
      method:'POST', credentials:'same-origin',
      headers: {{ 'Content-Type': 'application/json' }},
      body: JSON.stringify({{ action, token: '' }})
    }}).then(refresh).catch(() => {{}});
  }}));
  const timer = setInterval(refresh, 5000);
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
    fetch('/api/approvals?status=pending', {{ credentials:'same-origin' }})
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
  fetch('/api/journal/moments?limit=10', {{ credentials:'same-origin' }})
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
  fetch('/api/research/recent', {{ credentials:'same-origin' }})
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
    fetch('/api/dashboard/system', {{ credentials:'same-origin' }})
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

