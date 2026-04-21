//! `/smart-home` — Smart Home and Network module dashboard.
//!
//! Track A week 1 ships the shell: top bar, two-pane layout (rooms
//! sidebar left, device grid right), empty states, and the primary
//! "Scan for new devices" affordance. Track B (week 1) is wiring the
//! JS + expanding the empty states; subsequent weeks fill device
//! tiles, automation surfaces, network diagnostics, camera timeline,
//! and the energy dashboard per the milestone calendar.

use axum::response::Html;
use maud::{html, PreEscaped};

use super::shared::{shell, top_bar, Page};

pub async fn render() -> Html<String> {
    let page = Page {
        title: "Smart Home and Network",
        authed: true,
        extra_style: Some(EXTRA_STYLE),
    };
    let body = html! {
        (top_bar("Smart Home and Network", None))

        main class="sh-root" {
            // Left rail — rooms.
            aside class="sh-rooms" {
                header class="sh-rooms-head" {
                    h2 { "Rooms" }
                    button type="button" class="sh-btn-icon" title="Add room"
                           onclick="shAddRoom()" { "+" }
                }
                nav id="sh-rooms-list" class="sh-rooms-list" {
                    // Populated by JS on load. Empty state shows while list=[].
                    div class="sh-empty" id="sh-rooms-empty" {
                        p { "No rooms yet." }
                        p class="sh-empty-sub" {
                            "Add a room, then assign devices to it after the next scan."
                        }
                    }
                }
            }

            // Right canvas — devices.
            section class="sh-canvas" {
                header class="sh-canvas-head" {
                    div class="sh-canvas-title" {
                        h1 id="sh-canvas-title" { "All devices" }
                        span id="sh-canvas-subtitle" class="sh-muted" {}
                    }
                    div class="sh-canvas-actions" {
                        button type="button" id="sh-scan-btn" class="sh-btn-primary" onclick="shScan()" {
                            "Scan for new devices"
                        }
                    }
                }

                // Collapsible summary strip: diagnostics + energy + scenes.
                // Populated by shLoadSummary on page load and after
                // mutating actions (add room, confirm candidate, etc.).
                section class="sh-summary-strip" {
                    article class="sh-summary-card" id="sh-card-diag" {
                        header class="sh-summary-card-head" {
                            h3 { "System status" }
                            button type="button" class="sh-btn-ghost" onclick="shDiagSweep()" {
                                "Check now"
                            }
                        }
                        div id="sh-diag-body" class="sh-summary-body" {
                            span class="sh-muted" { "Loading…" }
                        }
                    }
                    article class="sh-summary-card" id="sh-card-energy" {
                        header class="sh-summary-card-head" {
                            h3 { "Energy today" }
                            button type="button" class="sh-btn-ghost" onclick="shEnergyIngest()" {
                                "Refresh"
                            }
                        }
                        div id="sh-energy-body" class="sh-summary-body" {
                            span class="sh-muted" { "Loading…" }
                        }
                    }
                    article class="sh-summary-card" id="sh-card-scenes" {
                        header class="sh-summary-card-head" {
                            h3 { "Scenes" }
                            button type="button" class="sh-btn-ghost" onclick="shLoadScenes()" {
                                "Reload"
                            }
                        }
                        div id="sh-scenes-body" class="sh-summary-body" {
                            span class="sh-muted" { "Loading…" }
                        }
                    }
                }

                // Scan-report banner (hidden until a scan finishes).
                div id="sh-scan-report" class="sh-banner hidden" {}

                // Scan candidates section — one card per ScanCandidate,
                // with Add (to chosen room) and Skip controls. Populated
                // by shScan() and drained by confirm/skip actions.
                div id="sh-scan-candidates" class="sh-scan-candidates hidden" {
                    header class="sh-scan-candidates-head" {
                        h2 { "New devices from scan" }
                        button type="button" class="sh-btn-ghost" onclick="shDismissAllCandidates()" { "Skip all" }
                    }
                    div id="sh-scan-candidates-list" class="sh-scan-candidates-list" {}
                }

                // Device grid.
                div id="sh-devices" class="sh-grid" {
                    div class="sh-empty sh-empty-wide" id="sh-devices-empty" {
                        h3 { "No devices yet" }
                        p {
                            "Click "
                            strong { "Scan for new devices" }
                            " to discover Wi-Fi / Matter / Zigbee / Z-Wave / BLE / MQTT devices on your network."
                        }
                        details class="sh-hw-help" {
                            summary { "Need radio hardware?" }
                            p {
                                "Wi-Fi works with no extra hardware. For Matter / Zigbee / Z-Wave "
                                "you'll want a USB coordinator plugged into this machine:"
                            }
                            ul {
                                li { "Matter + Thread + Zigbee — " a href="https://www.amazon.com/s?k=home+assistant+skyconnect" target="_blank" rel="noopener" { "Home Assistant SkyConnect" } " (~$39)" }
                                li { "Z-Wave 700/800 — " a href="https://www.amazon.com/s?k=aeotec+z-stick+7" target="_blank" rel="noopener" { "Aeotec Z-Stick 7" } " or " a href="https://www.amazon.com/s?k=zooz+zst10" target="_blank" rel="noopener" { "Zooz ZST10" } " (~$50-60)" }
                                li { "BLE presence (if your host lacks Bluetooth) — any USB BT 5.0 dongle (~$10)" }
                            }
                        }
                    }
                }
            }
        }

        script { (PreEscaped(SMART_HOME_JS)) }
    };
    Html(shell(page, body).into_string())
}

const EXTRA_STYLE: &str = r#"
:root {
    --sh-bg: #0a0f17;
    --sh-panel: #0e1622;
    --sh-panel-2: #111d2d;
    --sh-border: #1f2e44;
    --sh-text: #e7ecf3;
    --sh-muted: #8a94a6;
    --sh-accent: #2aa3ff;
    --sh-accent-2: #6ce7a7;
}
.sh-root {
    display: grid;
    grid-template-columns: 280px minmax(0, 1fr);
    gap: 16px;
    padding: 20px 24px 48px 24px;
    min-height: calc(100vh - 48px);
    color: var(--sh-text);
    background: var(--sh-bg);
}
.sh-rooms {
    background: var(--sh-panel);
    border: 1px solid var(--sh-border);
    border-radius: 14px;
    padding: 14px;
    height: fit-content;
    position: sticky;
    top: 60px;
}
.sh-rooms-head {
    display: flex; align-items: center; justify-content: space-between;
    margin-bottom: 10px;
}
.sh-rooms-head h2 {
    font-size: 14px; text-transform: uppercase;
    letter-spacing: 0.08em; color: var(--sh-muted); margin: 0;
}
.sh-btn-icon {
    width: 26px; height: 26px; border-radius: 8px;
    background: var(--sh-panel-2); color: var(--sh-text);
    border: 1px solid var(--sh-border);
    cursor: pointer; font-size: 16px; line-height: 1;
}
.sh-btn-icon:hover { background: var(--sh-border); }
.sh-rooms-list { display: flex; flex-direction: column; gap: 4px; }
.sh-room-item {
    display: flex; align-items: center; justify-content: space-between;
    padding: 8px 10px; border-radius: 10px; cursor: pointer;
    color: var(--sh-text); background: transparent;
    border: 1px solid transparent;
    gap: 6px;
}
.sh-room-item:hover { background: var(--sh-panel-2); }
.sh-room-item.active {
    background: var(--sh-panel-2); border-color: var(--sh-border);
}
.sh-room-item .sh-room-name {
    flex: 1 1 auto;
    overflow: hidden; text-overflow: ellipsis; white-space: nowrap;
}
.sh-room-item .sh-room-actions {
    display: none; gap: 4px;
}
.sh-room-item:hover .sh-room-actions { display: inline-flex; }
.sh-room-item .sh-room-action {
    width: 22px; height: 22px; border-radius: 6px;
    border: 1px solid var(--sh-border); background: transparent;
    color: var(--sh-muted); cursor: pointer;
    font-size: 12px; line-height: 1;
}
.sh-room-item .sh-room-action:hover {
    color: var(--sh-text); background: var(--sh-panel);
}
.sh-room-item.sh-drop-target {
    border-color: var(--sh-accent);
    background: rgba(42, 163, 255, 0.08);
}
.sh-room-rename-input {
    flex: 1 1 auto;
    background: var(--sh-bg); color: var(--sh-text);
    border: 1px solid var(--sh-accent); border-radius: 6px;
    padding: 2px 6px; font: inherit;
    outline: none;
}
.sh-device-tile {
    padding: 14px;
    border: 1px solid var(--sh-border);
    border-radius: 12px;
    background: var(--sh-panel-2);
    cursor: grab;
    user-select: none;
    transition: opacity 0.15s, transform 0.1s;
}
.sh-device-tile:active { cursor: grabbing; }
.sh-device-tile.sh-dragging {
    opacity: 0.5;
    transform: scale(0.97);
}
.sh-device-tile .sh-device-name { font-weight: 600; }
.sh-device-tile .sh-device-meta {
    color: var(--sh-muted); font-size: 12px; margin-top: 4px;
}
.sh-device-tile .sh-device-room-badge {
    color: var(--sh-muted); font-size: 11px; margin-top: 8px;
    padding-top: 8px; border-top: 1px dashed var(--sh-border);
}
.sh-device-tile .sh-device-header {
    display: flex; align-items: center; gap: 10px; margin-bottom: 6px;
}
.sh-device-icon {
    width: 36px; height: 36px; border-radius: 10px;
    display: grid; place-items: center;
    font-size: 18px; line-height: 1;
    background: var(--sh-bg); border: 1px solid var(--sh-border);
    flex: 0 0 auto;
}
.sh-device-tile[data-kind="light"] .sh-device-icon,
.sh-device-tile[data-kind="switch"] .sh-device-icon,
.sh-device-tile[data-kind="plug"] .sh-device-icon {
    background: linear-gradient(160deg, #1a2638, #0f1826);
}
.sh-device-tile[data-on="true"] .sh-device-icon {
    background: linear-gradient(160deg, rgba(255,200,100,0.35), rgba(42,163,255,0.25));
    border-color: var(--sh-accent);
}
.sh-device-tile[data-kind="lock"][data-locked="false"] .sh-device-icon {
    background: rgba(255, 140, 100, 0.15); border-color: #ff8c64;
}
.sh-device-controls {
    display: flex; flex-direction: column; gap: 8px; margin-top: 8px;
}
.sh-toggle {
    width: 44px; height: 24px; border-radius: 12px;
    background: var(--sh-border); border: none; cursor: pointer;
    position: relative; flex: 0 0 auto;
    transition: background 0.15s;
}
.sh-toggle::after {
    content: ''; position: absolute;
    width: 18px; height: 18px; border-radius: 50%;
    background: white; top: 3px; left: 3px;
    transition: left 0.15s;
}
.sh-toggle[aria-pressed="true"] { background: var(--sh-accent); }
.sh-toggle[aria-pressed="true"]::after { left: 23px; }
.sh-toggle:disabled { opacity: 0.5; cursor: wait; }
.sh-row {
    display: flex; align-items: center; justify-content: space-between; gap: 8px;
}
.sh-slider-row { display: flex; align-items: center; gap: 8px; }
.sh-slider {
    flex: 1 1 auto; -webkit-appearance: none; appearance: none;
    height: 4px; border-radius: 2px;
    background: var(--sh-border); outline: none;
}
.sh-slider::-webkit-slider-thumb {
    -webkit-appearance: none; appearance: none;
    width: 14px; height: 14px; border-radius: 50%;
    background: var(--sh-accent); cursor: pointer; border: none;
}
.sh-slider::-moz-range-thumb {
    width: 14px; height: 14px; border-radius: 50%;
    background: var(--sh-accent); cursor: pointer; border: none;
}
.sh-slider-value {
    color: var(--sh-muted); font-size: 11px; min-width: 36px; text-align: right;
}
.sh-big-value {
    font-size: 28px; font-weight: 600; color: var(--sh-text);
    line-height: 1; margin: 4px 0;
}
.sh-setpoint-controls {
    display: flex; align-items: center; gap: 6px;
}
.sh-step-btn {
    width: 26px; height: 26px; border-radius: 8px;
    background: var(--sh-bg); color: var(--sh-text);
    border: 1px solid var(--sh-border); cursor: pointer; font-size: 14px;
}
.sh-step-btn:hover { background: var(--sh-border); }
.sh-step-btn:disabled { opacity: 0.5; cursor: wait; }
.sh-action-btn {
    background: var(--sh-panel); color: var(--sh-text);
    border: 1px solid var(--sh-border); border-radius: 8px;
    padding: 6px 10px; font-size: 12px; cursor: pointer;
    flex: 1 1 auto;
}
.sh-action-btn.sh-primary {
    background: var(--sh-accent); color: #081220; border-color: var(--sh-accent);
    font-weight: 600;
}
.sh-action-btn.sh-primary:hover { filter: brightness(1.1); }
.sh-action-btn:disabled { opacity: 0.5; cursor: wait; }
.sh-state-chip {
    display: inline-block; padding: 2px 8px; border-radius: 999px;
    font-size: 11px; text-transform: uppercase; letter-spacing: 0.06em;
    border: 1px solid var(--sh-border); color: var(--sh-muted);
}
.sh-state-chip.sh-chip-active { color: var(--sh-accent-2); border-color: var(--sh-accent-2); }
.sh-state-chip.sh-chip-alert  { color: #ff8c64; border-color: #ff8c64; }
.sh-legacy-tag {
    display: inline-block; padding: 1px 6px; border-radius: 4px;
    font-size: 10px; color: var(--sh-muted);
    border: 1px solid var(--sh-border); margin-left: 6px;
}
.sh-summary-strip {
    display: grid;
    grid-template-columns: repeat(auto-fit, minmax(240px, 1fr));
    gap: 10px;
    margin: 0 0 16px 0;
}
.sh-summary-card {
    padding: 12px 14px;
    border: 1px solid var(--sh-border);
    border-radius: 12px;
    background: var(--sh-panel-2);
    display: flex; flex-direction: column; gap: 8px;
}
.sh-summary-card-head {
    display: flex; align-items: center; justify-content: space-between;
}
.sh-summary-card-head h3 {
    margin: 0; font-size: 12px; text-transform: uppercase;
    letter-spacing: 0.08em; color: var(--sh-muted);
}
.sh-summary-body {
    min-height: 44px; font-size: 13px;
    display: flex; flex-direction: column; gap: 4px;
}
.sh-summary-big {
    font-size: 22px; font-weight: 600; color: var(--sh-text);
    line-height: 1.1;
}
.sh-summary-sub { color: var(--sh-muted); font-size: 11px; }
.sh-issue {
    display: flex; align-items: flex-start; gap: 6px;
    padding: 6px 8px; border-radius: 8px;
    background: rgba(255, 140, 100, 0.08);
    border: 1px solid rgba(255, 140, 100, 0.25);
}
.sh-issue-kind {
    color: #ff8c64; font-size: 10px; text-transform: uppercase;
    letter-spacing: 0.06em; flex: 0 0 auto;
}
.sh-issue-body { flex: 1 1 auto; color: var(--sh-text); font-size: 12px; }
.sh-scene-chip {
    display: inline-flex; align-items: center; gap: 6px;
    padding: 5px 10px; border-radius: 999px;
    border: 1px solid var(--sh-border); background: var(--sh-bg);
    color: var(--sh-text); font-size: 12px; cursor: pointer;
    margin: 0 4px 4px 0;
}
.sh-scene-chip:hover { background: var(--sh-panel); border-color: var(--sh-accent); }
.sh-scene-chip:disabled { opacity: 0.6; cursor: wait; }
.sh-scan-candidates {
    margin: 0 0 18px 0; padding: 14px 16px;
    border: 1px solid var(--sh-accent); border-radius: 12px;
    background: rgba(42, 163, 255, 0.04);
}
.sh-scan-candidates.hidden { display: none; }
.sh-scan-candidates-head {
    display: flex; align-items: center; justify-content: space-between;
    margin-bottom: 12px;
}
.sh-scan-candidates-head h2 {
    font-size: 14px; text-transform: uppercase; letter-spacing: 0.08em;
    color: var(--sh-accent); margin: 0;
}
.sh-btn-ghost {
    background: transparent; color: var(--sh-muted);
    border: 1px solid var(--sh-border); border-radius: 8px;
    padding: 6px 10px; font-size: 12px; cursor: pointer;
}
.sh-btn-ghost:hover { color: var(--sh-text); }
.sh-scan-candidates-list {
    display: grid; grid-template-columns: repeat(auto-fill, minmax(260px, 1fr));
    gap: 10px;
}
.sh-candidate {
    padding: 12px; border: 1px solid var(--sh-border); border-radius: 10px;
    background: var(--sh-panel-2); display: flex; flex-direction: column; gap: 6px;
}
.sh-candidate-name {
    font-weight: 600; color: var(--sh-text);
    overflow: hidden; text-overflow: ellipsis; white-space: nowrap;
}
.sh-candidate-meta { color: var(--sh-muted); font-size: 12px; }
.sh-candidate-sub { color: var(--sh-muted); font-size: 11px; margin-top: 2px; }
.sh-candidate-actions {
    display: flex; gap: 6px; margin-top: 8px; align-items: center;
}
.sh-candidate-actions select {
    flex: 1 1 auto; min-width: 0;
    background: var(--sh-bg); color: var(--sh-text);
    border: 1px solid var(--sh-border); border-radius: 6px;
    padding: 4px 6px; font: inherit; font-size: 12px;
}
.sh-candidate-actions button { padding: 4px 10px; font-size: 12px; border-radius: 6px; cursor: pointer; }
.sh-candidate-add {
    background: var(--sh-accent); color: #081220; border: none; font-weight: 600;
}
.sh-candidate-add:hover { filter: brightness(1.1); }
.sh-candidate-add:disabled { opacity: 0.6; cursor: wait; }
.sh-candidate-skip {
    background: transparent; color: var(--sh-muted);
    border: 1px solid var(--sh-border);
}
.sh-candidate-skip:hover { color: var(--sh-text); }
.sh-canvas {
    background: var(--sh-panel);
    border: 1px solid var(--sh-border);
    border-radius: 14px;
    padding: 18px 20px 24px 20px;
    min-height: 100%;
}
.sh-canvas-head {
    display: flex; align-items: flex-end; justify-content: space-between;
    margin-bottom: 16px;
}
.sh-canvas-title h1 { font-size: 22px; margin: 0 0 2px 0; color: var(--sh-text); }
.sh-muted { color: var(--sh-muted); font-size: 13px; }
.sh-btn-primary {
    background: var(--sh-accent); color: #081220;
    border: none; border-radius: 10px;
    padding: 9px 14px; font-weight: 600; cursor: pointer;
}
.sh-btn-primary:hover { filter: brightness(1.1); }
.sh-btn-primary:disabled { opacity: 0.6; cursor: wait; }
.sh-banner {
    margin: 0 0 14px 0; padding: 10px 14px;
    border: 1px solid var(--sh-border); border-radius: 10px;
    background: var(--sh-panel-2); color: var(--sh-text); font-size: 14px;
}
.sh-banner.hidden { display: none; }
.sh-grid {
    display: grid;
    grid-template-columns: repeat(auto-fill, minmax(220px, 1fr));
    gap: 12px;
}
.sh-empty {
    text-align: center; color: var(--sh-muted);
    padding: 22px 14px; font-size: 14px;
}
.sh-empty p { margin: 4px 0; }
.sh-empty-sub { font-size: 12px; opacity: 0.8; }
.sh-empty-wide { grid-column: 1 / -1; padding: 36px 20px; }
.sh-empty-wide h3 { color: var(--sh-text); margin: 0 0 6px 0; font-size: 17px; }
.sh-hw-help { margin-top: 16px; text-align: left; max-width: 520px; margin-left: auto; margin-right: auto; }
.sh-hw-help summary { cursor: pointer; color: var(--sh-accent); font-weight: 500; }
.sh-hw-help ul { margin: 8px 0 0 22px; color: var(--sh-text); font-size: 13px; }
.sh-hw-help a { color: var(--sh-accent); }
"#;

const SMART_HOME_JS: &str = r#"
// Local state — avoid two async fetches of rooms when the device list
// needs room-name resolution, and keeps the currently-selected room so
// the grid can re-filter after a drag-assign without a reload.
const shState = { rooms: [], devices: [], selectedRoomId: null };

async function shFetch(path, opts) {
    const r = await fetch(path, opts || {});
    if (!r.ok) {
        let msg = 'HTTP ' + r.status;
        try { const j = await r.json(); if (j.error) msg += ': ' + j.error; } catch (_) {}
        throw new Error(msg);
    }
    return r.json();
}

async function shLoadRooms() {
    const list = document.getElementById('sh-rooms-list');
    const empty = document.getElementById('sh-rooms-empty');
    try {
        const { rooms } = await shFetch('/api/smart-home/rooms');
        shState.rooms = rooms || [];
        list.innerHTML = '';
        list.appendChild(buildRoomItem(null));
        shState.rooms.forEach(r => list.appendChild(buildRoomItem(r)));
        if (empty) empty.style.display = shState.rooms.length === 0 ? 'block' : 'none';
        // "All devices" is only visible when we actually have rooms.
        list.firstChild.style.display = shState.rooms.length === 0 ? 'none' : '';
        // Re-apply active selection.
        shHighlightSelectedRoom();
    } catch (e) {
        console.warn('[smart-home] rooms load failed', e);
    }
}

function buildRoomItem(room) {
    const el = document.createElement('div');
    el.className = 'sh-room-item';
    el.dataset.roomId = room ? String(room.id) : '';

    const name = document.createElement('span');
    name.className = 'sh-room-name';
    name.textContent = room ? room.name : 'All devices';
    el.appendChild(name);

    el.onclick = (e) => {
        if (e.target.closest('.sh-room-action') || e.target.tagName === 'INPUT') return;
        shSelectRoom(room);
    };

    if (room) {
        // Rename on double-click of the name.
        name.ondblclick = (e) => {
            e.stopPropagation();
            shBeginRoomRename(el, room);
        };

        // Per-room hover actions: rename + delete.
        const actions = document.createElement('span');
        actions.className = 'sh-room-actions';
        const rename = document.createElement('button');
        rename.className = 'sh-room-action';
        rename.title = 'Rename';
        rename.textContent = '✎';
        rename.onclick = (e) => { e.stopPropagation(); shBeginRoomRename(el, room); };
        const del = document.createElement('button');
        del.className = 'sh-room-action';
        del.title = 'Delete room';
        del.textContent = '×';
        del.onclick = (e) => { e.stopPropagation(); shDeleteRoom(room); };
        actions.appendChild(rename);
        actions.appendChild(del);
        el.appendChild(actions);

        // Drag + drop target for devices.
        el.addEventListener('dragover', (e) => {
            if (e.dataTransfer && e.dataTransfer.types.includes('application/x-sh-device')) {
                e.preventDefault();
                el.classList.add('sh-drop-target');
            }
        });
        el.addEventListener('dragleave', () => el.classList.remove('sh-drop-target'));
        el.addEventListener('drop', async (e) => {
            e.preventDefault();
            el.classList.remove('sh-drop-target');
            const deviceId = e.dataTransfer.getData('application/x-sh-device');
            if (deviceId) await shAssignDeviceToRoom(parseInt(deviceId, 10), room.id);
        });
    } else {
        // "All devices" can accept a drop to *unassign* (set room_id=null).
        el.addEventListener('dragover', (e) => {
            if (e.dataTransfer && e.dataTransfer.types.includes('application/x-sh-device')) {
                e.preventDefault();
                el.classList.add('sh-drop-target');
            }
        });
        el.addEventListener('dragleave', () => el.classList.remove('sh-drop-target'));
        el.addEventListener('drop', async (e) => {
            e.preventDefault();
            el.classList.remove('sh-drop-target');
            const deviceId = e.dataTransfer.getData('application/x-sh-device');
            if (deviceId) await shAssignDeviceToRoom(parseInt(deviceId, 10), null);
        });
    }

    return el;
}

function shBeginRoomRename(el, room) {
    if (el.querySelector('input.sh-room-rename-input')) return;
    const nameSpan = el.querySelector('.sh-room-name');
    const actions = el.querySelector('.sh-room-actions');
    if (actions) actions.style.display = 'none';
    const input = document.createElement('input');
    input.className = 'sh-room-rename-input';
    input.type = 'text';
    input.value = room.name;
    input.maxLength = 80;
    el.replaceChild(input, nameSpan);
    input.focus();
    input.select();
    const finish = async (commit) => {
        if (!commit) {
            // Revert DOM.
            el.replaceChild(nameSpan, input);
            if (actions) actions.style.display = '';
            return;
        }
        const next = input.value.trim();
        if (!next || next === room.name) {
            el.replaceChild(nameSpan, input);
            if (actions) actions.style.display = '';
            return;
        }
        try {
            await shFetch('/api/smart-home/rooms/' + room.id, {
                method: 'PATCH',
                headers: {'Content-Type': 'application/json'},
                body: JSON.stringify({ name: next })
            });
            room.name = next;
            nameSpan.textContent = next;
            await shLoadRooms();
        } catch (e) {
            alert('Rename failed: ' + e.message);
            el.replaceChild(nameSpan, input);
        }
        if (actions) actions.style.display = '';
    };
    input.onkeydown = (e) => {
        if (e.key === 'Enter') finish(true);
        else if (e.key === 'Escape') finish(false);
    };
    input.onblur = () => finish(true);
}

async function shDeleteRoom(room) {
    const assigned = shState.devices.filter(d => d.room_id === room.id).length;
    const msg = assigned > 0
        ? 'Delete "' + room.name + '"? ' + assigned + ' device(s) will become unassigned.'
        : 'Delete room "' + room.name + '"?';
    if (!confirm(msg)) return;
    try {
        await shFetch('/api/smart-home/rooms/' + room.id, { method: 'DELETE' });
        if (shState.selectedRoomId === room.id) shState.selectedRoomId = null;
        await shLoadRooms();
        await shLoadDevices();
    } catch (e) {
        alert('Delete failed: ' + e.message);
    }
}

function shSelectRoom(room) {
    shState.selectedRoomId = room ? room.id : null;
    shHighlightSelectedRoom();
    const title = document.getElementById('sh-canvas-title');
    title.textContent = room ? room.name : 'All devices';
    shRenderDevices();
}

function shHighlightSelectedRoom() {
    document.querySelectorAll('.sh-room-item').forEach(n => {
        const id = n.dataset.roomId === '' ? null : parseInt(n.dataset.roomId, 10);
        n.classList.toggle('active', id === shState.selectedRoomId);
    });
}

async function shLoadDevices() {
    try {
        const { devices } = await shFetch('/api/smart-home/devices');
        shState.devices = devices || [];
        shRenderDevices();
    } catch (e) {
        console.warn('[smart-home] devices load failed', e);
    }
}

function shRenderDevices() {
    const grid = document.getElementById('sh-devices');
    const empty = document.getElementById('sh-devices-empty');
    const rid = shState.selectedRoomId;
    const shown = rid == null ? shState.devices : shState.devices.filter(d => d.room_id === rid);
    grid.innerHTML = '';
    if (shown.length === 0) {
        grid.appendChild(empty);
        empty.style.display = '';
        return;
    }
    empty.style.display = 'none';
    const roomById = Object.fromEntries(shState.rooms.map(r => [r.id, r]));
    shown.forEach(d => grid.appendChild(buildDeviceTile(d, roomById)));
}

// Kind → icon + renderer. Unknown kinds fall back to generic tile body.
const SH_KIND_ICONS = {
    light: '💡', switch: '🔘', plug: '🔌', lock: '🔒', thermostat: '🌡',
    sensor_motion: '🚶', sensor_contact: '🚪', sensor_climate: '📊',
    camera: '📷', media_player: '🎵', cover: '🪟', fan: '🌀', vacuum: '🤖',
    hub: '🏠', speaker: '🔈', unknown: '❓',
};

function shParseState(device) {
    try { return JSON.parse(device.state_json || '{}'); } catch (_) { return {}; }
}

function shParseMeta(device) {
    try { return JSON.parse(device.metadata_json || '{}'); } catch (_) { return {}; }
}

function buildDeviceTile(device, roomById) {
    const tile = document.createElement('div');
    tile.className = 'sh-device-tile';
    tile.draggable = true;
    tile.dataset.deviceId = String(device.id);
    tile.dataset.kind = device.kind || 'unknown';

    const state = shParseState(device);
    const meta = shParseMeta(device);
    if (typeof state.on === 'boolean') tile.dataset.on = String(state.on);
    if (typeof state.locked === 'boolean') tile.dataset.locked = String(state.locked);

    // Header: icon + name + optional legacy-bridge tag.
    const header = document.createElement('div');
    header.className = 'sh-device-header';
    const icon = document.createElement('div');
    icon.className = 'sh-device-icon';
    icon.textContent = SH_KIND_ICONS[device.kind] || SH_KIND_ICONS.unknown;
    header.appendChild(icon);
    const nameWrap = document.createElement('div');
    nameWrap.style.flex = '1 1 auto';
    nameWrap.style.minWidth = '0';
    const name = document.createElement('div');
    name.className = 'sh-device-name';
    name.textContent = device.name || '(unnamed)';
    nameWrap.appendChild(name);
    const metaLine = document.createElement('div');
    metaLine.className = 'sh-device-meta';
    metaLine.textContent = device.kind + ' · ' + device.driver;
    if (meta && meta.scan_details && meta.scan_details.bridge) {
        const tag = document.createElement('span');
        tag.className = 'sh-legacy-tag';
        tag.textContent = 'legacy bridge';
        tag.title = 'Routed through python-matter-server. Pure-Rust Controller ships in v1.1.';
        metaLine.appendChild(tag);
    }
    nameWrap.appendChild(metaLine);
    header.appendChild(nameWrap);
    tile.appendChild(header);

    // Kind-specific controls.
    const controls = shRenderKindControls(device, state);
    if (controls) tile.appendChild(controls);

    const room = device.room_id == null ? null : roomById[device.room_id];
    const badge = document.createElement('div');
    badge.className = 'sh-device-room-badge';
    badge.textContent = room ? ('Room: ' + room.name) : 'Drag onto a room to assign';
    tile.appendChild(badge);

    tile.addEventListener('dragstart', (e) => {
        e.dataTransfer.effectAllowed = 'move';
        e.dataTransfer.setData('application/x-sh-device', String(device.id));
        tile.classList.add('sh-dragging');
    });
    tile.addEventListener('dragend', () => tile.classList.remove('sh-dragging'));
    return tile;
}

// Dispatch by device.kind to a control renderer. Returns a DOM node
// (or null if the kind has no interactive controls, e.g. unknown).
function shRenderKindControls(device, state) {
    const controls = document.createElement('div');
    controls.className = 'sh-device-controls';
    switch (device.kind) {
        case 'light':
            shRenderLightControls(controls, device, state);
            break;
        case 'switch':
        case 'plug':
        case 'fan':
            shRenderOnOffControls(controls, device, state);
            break;
        case 'lock':
            shRenderLockControls(controls, device, state);
            break;
        case 'thermostat':
            shRenderThermostatControls(controls, device, state);
            break;
        case 'sensor_motion':
            shRenderSensorStatus(controls, state, {
                active: typeof state.motion === 'boolean' ? state.motion : state.active,
                activeLabel: 'Motion', inactiveLabel: 'Clear',
            });
            break;
        case 'sensor_contact':
            shRenderSensorStatus(controls, state, {
                active: typeof state.contact === 'boolean'
                    ? !state.contact  // "contact=false" = open = alert
                    : state.open,
                activeLabel: 'Open', inactiveLabel: 'Closed',
                alertWhenActive: true,
            });
            break;
        case 'sensor_climate':
            shRenderClimateSensor(controls, state);
            break;
        case 'media_player':
            shRenderMediaControls(controls, device, state);
            break;
        case 'cover':
            shRenderCoverControls(controls, device, state);
            break;
        default:
            // Unknown: show a refresh button so users can at least poll
            // the bridge for whatever state the driver surfaces.
            shRenderRefreshOnly(controls, device);
    }
    return controls;
}

function shRenderLightControls(root, device, state) {
    const row = document.createElement('div');
    row.className = 'sh-row';
    const label = document.createElement('span');
    label.className = 'sh-device-meta';
    label.textContent = state.on ? 'On' : 'Off';
    row.appendChild(label);
    const toggle = shMakeToggle(!!state.on, async (want) => {
        const ok = await shControl(device, { on: want });
        if (ok) label.textContent = want ? 'On' : 'Off';
        return ok;
    });
    row.appendChild(toggle);
    root.appendChild(row);

    // Brightness slider (0-100%) if the device advertises a level.
    if (typeof state.level === 'number' || typeof state.brightness === 'number') {
        const cur = Math.round((state.level ?? state.brightness ?? 0) * 100) / 100;
        const wrap = document.createElement('div');
        wrap.className = 'sh-slider-row';
        const slider = document.createElement('input');
        slider.type = 'range';
        slider.min = '0'; slider.max = '100'; slider.value = String(Math.round(cur));
        slider.className = 'sh-slider';
        const readout = document.createElement('span');
        readout.className = 'sh-slider-value';
        readout.textContent = slider.value + '%';
        slider.addEventListener('input', () => { readout.textContent = slider.value + '%'; });
        slider.addEventListener('change', async () => {
            const v = parseInt(slider.value, 10);
            await shControl(device, { level: v / 100 });
        });
        wrap.appendChild(slider);
        wrap.appendChild(readout);
        root.appendChild(wrap);
    }
}

function shRenderOnOffControls(root, device, state) {
    const row = document.createElement('div');
    row.className = 'sh-row';
    const label = document.createElement('span');
    label.className = 'sh-device-meta';
    label.textContent = state.on ? 'On' : 'Off';
    row.appendChild(label);
    row.appendChild(shMakeToggle(!!state.on, async (want) => {
        const ok = await shControl(device, { on: want });
        if (ok) label.textContent = want ? 'On' : 'Off';
        return ok;
    }));
    root.appendChild(row);
}

function shRenderLockControls(root, device, state) {
    const row = document.createElement('div');
    row.className = 'sh-row';
    const chip = document.createElement('span');
    chip.className = 'sh-state-chip';
    const locked = !!state.locked;
    chip.textContent = locked ? 'Locked' : 'Unlocked';
    chip.classList.toggle('sh-chip-alert', !locked);
    row.appendChild(chip);
    root.appendChild(row);

    const actions = document.createElement('div');
    actions.className = 'sh-row';
    const unlockBtn = document.createElement('button');
    unlockBtn.className = 'sh-action-btn';
    unlockBtn.textContent = 'Unlock';
    unlockBtn.disabled = !locked;
    const lockBtn = document.createElement('button');
    lockBtn.className = 'sh-action-btn sh-primary';
    lockBtn.textContent = 'Lock';
    lockBtn.disabled = locked;
    unlockBtn.onclick = async () => {
        if (!confirm('Unlock ' + (device.name || 'this lock') + '?')) return;
        const ok = await shControl(device, { locked: false });
        if (ok) { chip.textContent = 'Unlocked'; chip.classList.add('sh-chip-alert');
                  unlockBtn.disabled = true; lockBtn.disabled = false; }
    };
    lockBtn.onclick = async () => {
        const ok = await shControl(device, { locked: true });
        if (ok) { chip.textContent = 'Locked'; chip.classList.remove('sh-chip-alert');
                  lockBtn.disabled = true; unlockBtn.disabled = false; }
    };
    actions.appendChild(unlockBtn);
    actions.appendChild(lockBtn);
    root.appendChild(actions);
}

function shRenderThermostatControls(root, device, state) {
    const cur = state.current_temp_f ?? state.current_temp ?? state.temperature;
    const setp = state.setpoint_f ?? state.setpoint ?? state.target_temp;
    if (cur != null) {
        const big = document.createElement('div');
        big.className = 'sh-big-value';
        big.textContent = Math.round(cur) + '°';
        root.appendChild(big);
        const subMeta = document.createElement('div');
        subMeta.className = 'sh-device-meta';
        subMeta.textContent = 'Current';
        root.appendChild(subMeta);
    }
    if (setp != null) {
        const row = document.createElement('div');
        row.className = 'sh-row';
        const label = document.createElement('span');
        label.className = 'sh-device-meta';
        label.textContent = 'Set to ' + Math.round(setp) + '°';
        row.appendChild(label);
        const ctrl = document.createElement('span');
        ctrl.className = 'sh-setpoint-controls';
        const dec = document.createElement('button');
        dec.className = 'sh-step-btn'; dec.textContent = '−';
        const inc = document.createElement('button');
        inc.className = 'sh-step-btn'; inc.textContent = '+';
        let target = Math.round(setp);
        const apply = async (delta) => {
            const next = target + delta;
            dec.disabled = true; inc.disabled = true;
            const ok = await shControl(device, { setpoint: next });
            dec.disabled = false; inc.disabled = false;
            if (ok) { target = next; label.textContent = 'Set to ' + target + '°'; }
        };
        dec.onclick = () => apply(-1);
        inc.onclick = () => apply(+1);
        ctrl.appendChild(dec); ctrl.appendChild(inc);
        row.appendChild(ctrl);
        root.appendChild(row);
    }
}

function shRenderSensorStatus(root, state, opts) {
    const row = document.createElement('div');
    row.className = 'sh-row';
    const chip = document.createElement('span');
    chip.className = 'sh-state-chip';
    chip.textContent = opts.active ? opts.activeLabel : opts.inactiveLabel;
    if (opts.active) {
        chip.classList.add(opts.alertWhenActive ? 'sh-chip-alert' : 'sh-chip-active');
    }
    row.appendChild(chip);
    if (state.battery != null) {
        const batt = document.createElement('span');
        batt.className = 'sh-device-meta';
        batt.textContent = Math.round(state.battery) + '% battery';
        row.appendChild(batt);
    }
    root.appendChild(row);
}

function shRenderClimateSensor(root, state) {
    const parts = [];
    if (state.temperature != null) parts.push(Math.round(state.temperature * 10) / 10 + '°');
    if (state.humidity != null) parts.push(Math.round(state.humidity) + '% RH');
    if (state.co2 != null) parts.push(state.co2 + ' ppm CO₂');
    const row = document.createElement('div');
    row.className = 'sh-row';
    const readout = document.createElement('span');
    readout.className = 'sh-big-value';
    readout.textContent = parts[0] || '—';
    row.appendChild(readout);
    if (parts.length > 1) {
        const rest = document.createElement('span');
        rest.className = 'sh-device-meta';
        rest.textContent = parts.slice(1).join(' · ');
        row.appendChild(rest);
    }
    root.appendChild(row);
}

function shRenderMediaControls(root, device, state) {
    if (state.now_playing) {
        const meta = document.createElement('div');
        meta.className = 'sh-device-meta';
        meta.textContent = state.now_playing;
        root.appendChild(meta);
    }
    const row = document.createElement('div');
    row.className = 'sh-row';
    ['⏮', state.playing ? '⏸' : '▶', '⏭'].forEach((glyph, i) => {
        const b = document.createElement('button');
        b.className = 'sh-action-btn' + (i === 1 ? ' sh-primary' : '');
        b.textContent = glyph;
        b.onclick = async () => {
            const cmd = ['prev', state.playing ? 'pause' : 'play', 'next'][i];
            await shControl(device, { media_command: cmd });
        };
        row.appendChild(b);
    });
    root.appendChild(row);
}

function shRenderCoverControls(root, device, state) {
    const pos = state.position ?? state.open_pct;
    if (pos != null) {
        const big = document.createElement('div');
        big.className = 'sh-big-value';
        big.textContent = Math.round(pos) + '%';
        root.appendChild(big);
        const sub = document.createElement('div');
        sub.className = 'sh-device-meta';
        sub.textContent = 'Open';
        root.appendChild(sub);
    }
    const row = document.createElement('div');
    row.className = 'sh-row';
    ['Open', 'Stop', 'Close'].forEach((label, i) => {
        const b = document.createElement('button');
        b.className = 'sh-action-btn';
        b.textContent = label;
        b.onclick = async () => {
            const cmd = ['open', 'stop', 'close'][i];
            await shControl(device, { cover_command: cmd });
        };
        row.appendChild(b);
    });
    root.appendChild(row);
}

function shRenderRefreshOnly(root, device) {
    const b = document.createElement('button');
    b.className = 'sh-action-btn';
    b.textContent = 'Refresh state';
    b.onclick = async () => {
        b.disabled = true; b.textContent = 'Refreshing…';
        await shRefreshDeviceState(device);
        b.disabled = false; b.textContent = 'Refresh state';
    };
    root.appendChild(b);
}

function shMakeToggle(initial, onChange) {
    const b = document.createElement('button');
    b.type = 'button';
    b.className = 'sh-toggle';
    b.setAttribute('aria-pressed', initial ? 'true' : 'false');
    b.onclick = async () => {
        const wasOn = b.getAttribute('aria-pressed') === 'true';
        const want = !wasOn;
        b.disabled = true;
        b.setAttribute('aria-pressed', want ? 'true' : 'false'); // optimistic
        const ok = await onChange(want);
        b.disabled = false;
        if (!ok) b.setAttribute('aria-pressed', wasOn ? 'true' : 'false'); // revert on fail
    };
    return b;
}

// Send a control request; return true on success so optimistic-UI can
// stick, false so the caller can revert its visual state.
async function shControl(device, args) {
    try {
        await shFetch('/api/smart-home/control', {
            method: 'POST',
            headers: {'Content-Type': 'application/json'},
            body: JSON.stringify({ device_id: device.id, state: args })
        });
        return true;
    } catch (e) {
        alert((device.name || 'device') + ': ' + e.message);
        return false;
    }
}

async function shRefreshDeviceState(device) {
    try {
        const { device: updated } = await shFetch('/api/smart-home/devices/' + device.id + '/refresh-state', {
            method: 'POST'
        });
        if (updated) {
            const idx = shState.devices.findIndex(d => d.id === device.id);
            if (idx >= 0) shState.devices[idx] = updated;
            shRenderDevices();
        }
    } catch (e) {
        alert('Refresh failed: ' + e.message);
    }
}

async function shAssignDeviceToRoom(deviceId, roomId) {
    try {
        await shFetch('/api/smart-home/devices/' + deviceId + '/room', {
            method: 'POST',
            headers: {'Content-Type': 'application/json'},
            body: JSON.stringify({ room_id: roomId })
        });
        await shLoadDevices();
    } catch (e) {
        alert('Assign failed: ' + e.message);
    }
}

async function shAddRoom() {
    const name = prompt('Room name');
    if (!name) return;
    try {
        await shFetch('/api/smart-home/rooms', {
            method: 'POST',
            headers: {'Content-Type': 'application/json'},
            body: JSON.stringify({ name })
        });
        await shLoadRooms();
    } catch (e) { alert('Failed to add room: ' + e.message); }
}

async function shScan() {
    const btn = document.getElementById('sh-scan-btn');
    const banner = document.getElementById('sh-scan-report');
    btn.disabled = true;
    btn.textContent = 'Scanning…';
    banner.classList.remove('hidden');
    banner.textContent = 'Scanning the network and radios…';
    try {
        const report = await shFetch('/api/smart-home/scan', { method: 'POST' });
        const candidates = report.candidates || [];
        // Drop candidates we've already committed — an external_id we
        // already store shouldn't make the user re-confirm on every scan.
        const knownKey = new Set(shState.devices.map(d => d.driver + '|' + d.external_id));
        const newCandidates = candidates.filter(c => !knownKey.has(c.driver + '|' + c.external_id));
        const dupCount = candidates.length - newCandidates.length;
        if (newCandidates.length === 0) {
            banner.textContent = dupCount > 0
                ? 'Scan complete — ' + dupCount + ' device(s) already in your dashboard, no new ones.'
                : 'Scan complete — no devices discovered. Drivers are wired for Wi-Fi/mDNS today; Matter/Zigbee/Z-Wave/BLE/MQTT come online across weeks 3-13.';
            document.getElementById('sh-scan-candidates').classList.add('hidden');
        } else {
            banner.textContent = dupCount === 0
                ? 'Scan complete — ' + newCandidates.length + ' new device(s) found. Pick a room and click Add.'
                : 'Scan complete — ' + newCandidates.length + ' new, ' + dupCount + ' already known.';
            shRenderScanCandidates(newCandidates);
        }
        await shLoadDevices();
    } catch (e) {
        banner.textContent = 'Scan failed: ' + e.message;
        document.getElementById('sh-scan-candidates').classList.add('hidden');
    } finally {
        btn.disabled = false;
        btn.textContent = 'Scan for new devices';
    }
}

function shRenderScanCandidates(candidates) {
    const wrap = document.getElementById('sh-scan-candidates');
    const list = document.getElementById('sh-scan-candidates-list');
    list.innerHTML = '';
    if (!candidates || candidates.length === 0) {
        wrap.classList.add('hidden');
        return;
    }
    wrap.classList.remove('hidden');
    candidates.forEach(c => list.appendChild(buildCandidateCard(c)));
}

function buildCandidateCard(candidate) {
    const card = document.createElement('div');
    card.className = 'sh-candidate';
    // stable id so removal works cleanly
    card.dataset.candidateKey = candidate.driver + '|' + candidate.external_id;

    const name = document.createElement('div');
    name.className = 'sh-candidate-name';
    name.textContent = candidate.name || '(unnamed)';
    card.appendChild(name);

    const meta = document.createElement('div');
    meta.className = 'sh-candidate-meta';
    const parts = [candidate.kind, candidate.driver];
    if (candidate.vendor) parts.push(candidate.vendor);
    meta.textContent = parts.join(' · ');
    card.appendChild(meta);

    if (candidate.ip) {
        const sub = document.createElement('div');
        sub.className = 'sh-candidate-sub';
        sub.textContent = candidate.ip;
        card.appendChild(sub);
    }

    const actions = document.createElement('div');
    actions.className = 'sh-candidate-actions';

    const roomSel = document.createElement('select');
    roomSel.innerHTML = '<option value="">(no room)</option>';
    shState.rooms.forEach(r => {
        const opt = document.createElement('option');
        opt.value = String(r.id);
        opt.textContent = r.name;
        roomSel.appendChild(opt);
    });
    actions.appendChild(roomSel);

    const add = document.createElement('button');
    add.className = 'sh-candidate-add';
    add.textContent = 'Add';
    add.onclick = async () => {
        add.disabled = true;
        add.textContent = '…';
        const rid = roomSel.value === '' ? null : parseInt(roomSel.value, 10);
        try {
            await shFetch('/api/smart-home/scan/confirm', {
                method: 'POST',
                headers: {'Content-Type': 'application/json'},
                body: JSON.stringify({ candidate, room_id: rid })
            });
            card.remove();
            await shLoadDevices();
            shMaybeHideCandidatesWrap();
        } catch (e) {
            add.disabled = false;
            add.textContent = 'Add';
            alert('Add failed: ' + e.message);
        }
    };
    actions.appendChild(add);

    const skip = document.createElement('button');
    skip.className = 'sh-candidate-skip';
    skip.textContent = 'Skip';
    skip.onclick = () => {
        card.remove();
        shMaybeHideCandidatesWrap();
    };
    actions.appendChild(skip);

    card.appendChild(actions);
    return card;
}

function shMaybeHideCandidatesWrap() {
    const list = document.getElementById('sh-scan-candidates-list');
    if (list.children.length === 0) {
        document.getElementById('sh-scan-candidates').classList.add('hidden');
    }
}

function shDismissAllCandidates() {
    const list = document.getElementById('sh-scan-candidates-list');
    list.innerHTML = '';
    document.getElementById('sh-scan-candidates').classList.add('hidden');
}

// ── Summary strip (diagnostics + energy + scenes) ──────────────────────

async function shLoadSummary() {
    // Fire all three in parallel; each section fails independently.
    const [diag, energy, scenes] = await Promise.allSettled([
        shFetch('/api/smart-home/diagnostics/summary'),
        shFetch('/api/smart-home/energy/summary'),
        shFetch('/api/smart-home/scenes'),
    ]);
    shRenderDiag(diag.status === 'fulfilled' ? diag.value : null);
    shRenderEnergy(energy.status === 'fulfilled' ? energy.value : null);
    shRenderScenes(scenes.status === 'fulfilled' ? scenes.value : null);
}

function shRenderDiag(summary) {
    const el = document.getElementById('sh-diag-body');
    if (!summary) {
        el.innerHTML = '<span class="sh-muted">Status unavailable.</span>';
        return;
    }
    el.innerHTML = '';
    const big = document.createElement('div');
    big.className = 'sh-summary-big';
    big.textContent = summary.online_count + '/' + summary.total_devices + ' online';
    el.appendChild(big);
    if (summary.offline_count > 0) {
        const sub = document.createElement('div');
        sub.className = 'sh-summary-sub';
        sub.textContent = summary.offline_count + ' device(s) offline';
        el.appendChild(sub);
    }
    const issues = summary.active_issues || [];
    if (issues.length === 0) {
        const ok = document.createElement('div');
        ok.className = 'sh-summary-sub';
        ok.textContent = summary.last_sweep_at
            ? 'No active issues — last check ' + shRelativeTime(summary.last_sweep_at)
            : 'No checks yet. Click “Check now” to sweep.';
        el.appendChild(ok);
    } else {
        issues.slice(0, 3).forEach(issue => {
            const row = document.createElement('div');
            row.className = 'sh-issue';
            const kind = document.createElement('span');
            kind.className = 'sh-issue-kind';
            kind.textContent = issue.kind.replace(/_/g, ' ');
            const body = document.createElement('div');
            body.className = 'sh-issue-body';
            body.textContent = (issue.subject || 'unknown') + ' — ' + issue.remediation;
            row.appendChild(kind);
            row.appendChild(body);
            el.appendChild(row);
        });
        if (issues.length > 3) {
            const more = document.createElement('div');
            more.className = 'sh-summary-sub';
            more.textContent = (issues.length - 3) + ' more issue(s) hidden.';
            el.appendChild(more);
        }
    }
}

function shRenderEnergy(summary) {
    const el = document.getElementById('sh-energy-body');
    if (!summary) {
        el.innerHTML = '<span class="sh-muted">Energy data unavailable.</span>';
        return;
    }
    el.innerHTML = '';
    const kwh = summary.today_kwh || 0;
    const big = document.createElement('div');
    big.className = 'sh-summary-big';
    big.textContent = kwh.toFixed(2) + ' kWh';
    el.appendChild(big);
    if (summary.today_cost != null) {
        const cost = document.createElement('div');
        cost.className = 'sh-summary-sub';
        cost.textContent = '$' + summary.today_cost.toFixed(2) + ' today';
        if (summary.today_carbon_grams != null) {
            cost.textContent += ' · ' + Math.round(summary.today_carbon_grams) + ' g CO₂';
        }
        el.appendChild(cost);
    }
    const meteredDevices = (summary.devices || []).filter(d => d.today_kwh != null || d.current_watts != null);
    if (meteredDevices.length === 0) {
        const none = document.createElement('div');
        none.className = 'sh-summary-sub';
        none.textContent = 'No metering devices yet. Shelly/Matter plugs report here automatically.';
        el.appendChild(none);
    } else {
        // Top 3 by current watts.
        meteredDevices.sort((a, b) => (b.current_watts || 0) - (a.current_watts || 0));
        meteredDevices.slice(0, 3).forEach(d => {
            const row = document.createElement('div');
            row.className = 'sh-summary-sub';
            const parts = [d.device_name];
            if (d.current_watts != null) parts.push(Math.round(d.current_watts) + ' W');
            if (d.today_kwh != null) parts.push(d.today_kwh.toFixed(2) + ' kWh today');
            row.textContent = parts.join(' · ');
            el.appendChild(row);
        });
    }
}

function shRenderScenes(payload) {
    const el = document.getElementById('sh-scenes-body');
    if (!payload) {
        el.innerHTML = '<span class="sh-muted">Scene list unavailable.</span>';
        return;
    }
    const scenes = payload.scenes || [];
    if (scenes.length === 0) {
        el.innerHTML = '<span class="sh-muted">No scenes yet. Create one from the automation builder.</span>';
        return;
    }
    el.innerHTML = '';
    scenes.forEach(scene => {
        const chip = document.createElement('button');
        chip.type = 'button';
        chip.className = 'sh-scene-chip';
        chip.textContent = (scene.icon ? scene.icon + ' ' : '') + scene.name;
        chip.onclick = async () => {
            chip.disabled = true;
            const prev = chip.textContent;
            chip.textContent = prev + ' …';
            try {
                const res = await shFetch('/api/smart-home/scenes/' + scene.id + '/activate', { method: 'POST' });
                const failures = (res.outcomes || []).filter(o => !o.ok).length;
                if (failures === 0) {
                    chip.textContent = prev + ' ✓';
                } else {
                    chip.textContent = prev + ' ⚠ ' + failures + ' failed';
                }
                await shLoadDevices();
                setTimeout(() => { chip.textContent = prev; chip.disabled = false; }, 2000);
            } catch (e) {
                chip.textContent = prev + ' ✗';
                setTimeout(() => { chip.textContent = prev; chip.disabled = false; }, 2000);
            }
        };
        el.appendChild(chip);
    });
}

async function shDiagSweep() {
    try {
        const r = await shFetch('/api/smart-home/diagnostics/sweep', { method: 'POST' });
        shRenderDiag(r.summary);
    } catch (e) {
        alert('Sweep failed: ' + e.message);
    }
}

async function shEnergyIngest() {
    try {
        await shFetch('/api/smart-home/energy/ingest', { method: 'POST' });
        const s = await shFetch('/api/smart-home/energy/summary');
        shRenderEnergy(s);
    } catch (e) {
        alert('Ingest failed: ' + e.message);
    }
}

async function shLoadScenes() {
    try {
        const payload = await shFetch('/api/smart-home/scenes');
        shRenderScenes(payload);
    } catch (e) {
        console.warn('[smart-home] scenes load failed', e);
    }
}

function shRelativeTime(ts) {
    if (!ts) return 'never';
    const nowSec = Math.floor(Date.now() / 1000);
    const delta = Math.max(0, nowSec - ts);
    if (delta < 60) return delta + 's ago';
    if (delta < 3600) return Math.floor(delta / 60) + 'm ago';
    if (delta < 86400) return Math.floor(delta / 3600) + 'h ago';
    return Math.floor(delta / 86400) + 'd ago';
}

// ── Live updates via SSE event bus ─────────────────────────────────────

// Debounce handles so a burst of events doesn't hammer the endpoints.
const shDebounce = { diag: null, energy: null, devices: null };

function shScheduleRefresh(kind) {
    if (shDebounce[kind]) clearTimeout(shDebounce[kind]);
    shDebounce[kind] = setTimeout(async () => {
        try {
            if (kind === 'diag') {
                const s = await shFetch('/api/smart-home/diagnostics/summary');
                shRenderDiag(s);
            } else if (kind === 'energy') {
                const s = await shFetch('/api/smart-home/energy/summary');
                shRenderEnergy(s);
            } else if (kind === 'devices') {
                await shLoadDevices();
            }
        } catch (_) {}
        shDebounce[kind] = null;
    }, 500);
}

function shStartEventStream() {
    if (!window.EventSource) {
        console.info('[smart-home] no EventSource support, reactive updates disabled');
        return;
    }
    const es = new EventSource('/api/smart-home/events/stream');
    es.addEventListener('ready', () => {
        console.debug('[smart-home] event stream live');
    });
    es.addEventListener('automation-fired', () => {
        // Automations may have flipped device state; refresh the grid.
        shScheduleRefresh('devices');
    });
    es.addEventListener('network-transition', () => shScheduleRefresh('diag'));
    es.addEventListener('energy-sample', () => shScheduleRefresh('energy'));
    es.addEventListener('scene-activated', () => shScheduleRefresh('devices'));
    es.addEventListener('device-state-changed', () => shScheduleRefresh('devices'));
    es.addEventListener('lagged', (e) => {
        console.warn('[smart-home] event stream lagged', e.data);
        // On lag, force a full refresh so we're not out of sync.
        shLoadSummary();
        shLoadDevices();
    });
    es.onerror = () => {
        // Browser auto-reconnects; log once per disconnect.
        console.debug('[smart-home] event stream disconnected, browser will retry');
    };
}

document.addEventListener('DOMContentLoaded', async () => {
    await shLoadRooms();
    await shLoadDevices();
    shLoadSummary();
    shStartEventStream();
});
"#;
