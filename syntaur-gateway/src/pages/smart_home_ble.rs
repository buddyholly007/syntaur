//! `/smart-home/ble` — anchor configuration sub-page.
//!
//! BLE presence in the Syntaur smart-home stack works by attributing
//! RSSI observations from configured "anchor" devices (ESPHome BLE
//! proxies in opposite wings of the house, optionally augmented by the
//! gateway's local Bluetooth adapter) to the room they sit in.
//! `closest-anchor wins` then maps a phone/wearable MAC to a room.
//!
//! This page lets a user list their anchors, add/edit/remove rows, and
//! flag at most one anchor as the local "host scanner" (the gateway's
//! own BT adapter). Talks to:
//!   - GET  /api/smart-home/ble/anchors — current set
//!   - PUT  /api/smart-home/ble/anchors — full replacement set
//!   - GET  /api/smart-home/devices?kind=... — pool of devices the user
//!     can pick as anchors (sensor/proxy rows)
//!   - GET  /api/smart-home/rooms — pool of rooms to attribute to
//!
//! Validation is server-side; this page just batches form input into
//! a `BleAnchorsReplaceBody` and posts it. The driver hot-swaps anchors
//! on the next 15s tick — no gateway restart needed.

use axum::response::Html;
use maud::{html, PreEscaped};

use super::shared::{shell, top_bar, Page};

pub async fn render() -> Html<String> {
    let page = Page {
        title: "Smart Home — BLE Anchors",
        authed: true,
        extra_style: Some(EXTRA_STYLE),
        body_class: None,
        head_boot: None,
        crumb: Some("BLE Anchors"),
        topbar_status: None,
    };

    let body = html! {
        (top_bar("Smart Home — BLE Anchors", None))

        div class="sh-ble" {
            header class="sh-ble-hero" {
                h1 { "BLE presence anchors" }
                p class="sh-ble-sub" {
                    "Each anchor is a BLE-equipped device in a known room. "
                    "Phones and wearables seen by an anchor inherit that anchor's room. "
                    "You typically want one anchor per wing of the house."
                }
            }

            section class="sh-ble-toolbar" {
                button type="button" id="ble-add"   class="sh-btn sh-btn-primary" { "Add anchor" }
                button type="button" id="ble-save"  class="sh-btn"               { "Save changes" }
                button type="button" id="ble-reset" class="sh-btn sh-btn-ghost"  { "Discard" }
                span id="ble-status" class="sh-ble-status" {}
            }

            section class="sh-ble-list" {
                table class="sh-ble-table" {
                    thead {
                        tr {
                            th { "Anchor device" }
                            th { "Room" }
                            th title="Calibrated RSSI at 1 meter — typical −40 to −55 dBm" {
                                "RSSI @ 1m"
                            }
                            th title="Use the gateway's local Bluetooth adapter as this anchor" {
                                "Host scanner"
                            }
                            th { "" }
                        }
                    }
                    tbody id="ble-rows" {
                        // Rows injected by JS on first load.
                    }
                }
                p id="ble-empty" class="sh-ble-empty" hidden { "No anchors configured yet." }
            }

            details class="sh-ble-help" {
                summary { "How calibration works" }
                p {
                    "RSSI gets weaker (more negative) as you move farther from the anchor. "
                    "By telling Syntaur what RSSI to expect at 1 meter from each anchor, "
                    "the closest-anchor classifier can reason about distance even with "
                    "different transmit-power BLE devices in the same household."
                }
                ul {
                    li { "Default −50 dBm works for most consumer Bluetooth radios." }
                    li { "If your anchor sits in a wall pocket or metal cabinet, drop it 5–10 dB." }
                    li { "Calibration is monotonic — even a wrong-by-10 setting still picks the right anchor; it just affects the confidence score." }
                }
            }

            details class="sh-ble-help" {
                summary { "Host scanner" }
                p {
                    "Mark one anchor as the host scanner if your gateway hardware itself sits in a useful vantage room "
                    "(e.g. a small apartment with a single dedicated host). The gateway's local Bluetooth radio will "
                    "then contribute RSSI to that anchor alongside any ESPHome proxies. Most home installs leave "
                    "this off — proxies in opposite wings outperform a single host vantage."
                }
            }
        }

        script { (PreEscaped(PAGE_SCRIPT)) }
    };

    Html(shell(page, body).into_string())
}

const EXTRA_STYLE: &str = r#"
.sh-ble {
    max-width: 980px;
    margin: 0 auto;
    padding: 1.5rem 1.5rem 4rem;
    color: var(--sh-text, #e7eaf0);
}
.sh-ble-hero h1 { font-size: 1.5rem; margin: 0 0 0.4rem; }
.sh-ble-sub { color: var(--sh-text-muted, #98a2b3); margin: 0 0 1.2rem; max-width: 60ch; }
.sh-ble-toolbar {
    display: flex;
    gap: 0.5rem;
    align-items: center;
    margin-bottom: 1rem;
    flex-wrap: wrap;
}
.sh-btn {
    background: rgba(255,255,255,0.05);
    color: var(--sh-text, #e7eaf0);
    border: 1px solid rgba(255,255,255,0.13);
    border-radius: 10px;
    padding: 0.45rem 0.9rem;
    font: inherit;
    cursor: pointer;
    transition: background 0.12s ease;
}
.sh-btn:hover { background: rgba(255,255,255,0.10); }
.sh-btn-primary {
    background: rgba(94, 226, 255, 0.16);
    border-color: rgba(94, 226, 255, 0.45);
}
.sh-btn-ghost { background: transparent; }
.sh-ble-status { color: var(--sh-text-muted, #98a2b3); font-size: 0.85rem; }
.sh-ble-status.ok { color: #71e8a3; }
.sh-ble-status.err { color: #ff8a8a; }
.sh-ble-table {
    width: 100%;
    border-collapse: collapse;
    background: rgba(255,255,255,0.03);
    border: 1px solid rgba(255,255,255,0.08);
    border-radius: 12px;
    overflow: hidden;
}
.sh-ble-table th, .sh-ble-table td {
    padding: 0.6rem 0.8rem;
    text-align: left;
    border-bottom: 1px solid rgba(255,255,255,0.05);
    vertical-align: middle;
}
.sh-ble-table th {
    font-weight: 500;
    color: var(--sh-text-muted, #98a2b3);
    background: rgba(255,255,255,0.02);
}
.sh-ble-table tbody tr:last-child td { border-bottom: none; }
.sh-ble-table select, .sh-ble-table input[type="number"] {
    background: rgba(0,0,0,0.25);
    color: inherit;
    border: 1px solid rgba(255,255,255,0.13);
    border-radius: 8px;
    padding: 0.3rem 0.5rem;
    font: inherit;
    width: 100%;
}
.sh-ble-table input[type="number"] { width: 6em; }
.sh-ble-table input[type="checkbox"] { transform: scale(1.2); }
.sh-ble-empty {
    color: var(--sh-text-muted, #98a2b3);
    padding: 2rem;
    text-align: center;
}
.sh-ble-help {
    margin-top: 1.5rem;
    background: rgba(255,255,255,0.03);
    border: 1px solid rgba(255,255,255,0.08);
    border-radius: 10px;
    padding: 0.7rem 1rem;
}
.sh-ble-help summary { cursor: pointer; font-weight: 500; }
.sh-ble-help p { color: var(--sh-text-muted, #98a2b3); margin: 0.6rem 0; }
.sh-ble-help ul { color: var(--sh-text-muted, #98a2b3); margin: 0.4rem 0 0.2rem 1.2rem; }
.sh-row-remove {
    background: transparent;
    color: #ff8a8a;
    border: 1px solid rgba(255, 138, 138, 0.45);
    border-radius: 8px;
    padding: 0.25rem 0.7rem;
    cursor: pointer;
    font: inherit;
}
"#;

const PAGE_SCRIPT: &str = r#"
(function () {
    "use strict";
    // ── State ────────────────────────────────────────────────────
    // anchors: live form rows (what the user sees)
    // pristine: last-known server set (for "Discard" + dirty detection)
    // devicePool / roomPool: the dropdown source data
    var state = { anchors: [], pristine: [], devicePool: [], roomPool: [] };
    var $rows  = document.getElementById("ble-rows");
    var $empty = document.getElementById("ble-empty");
    var $stat  = document.getElementById("ble-status");

    function setStatus(msg, klass) {
        $stat.textContent = msg || "";
        $stat.className = "sh-ble-status" + (klass ? (" " + klass) : "");
    }

    function jsonGet(url) {
        return fetch(url, { credentials: "same-origin" })
            .then(function (r) {
                if (!r.ok) throw new Error(url + " → " + r.status);
                return r.json();
            });
    }

    function jsonPut(url, body) {
        return fetch(url, {
            method: "PUT",
            credentials: "same-origin",
            headers: { "Content-Type": "application/json" },
            body: JSON.stringify(body),
        }).then(function (r) {
            return r.json().then(function (j) {
                if (!r.ok) throw new Error(j.error || ("HTTP " + r.status));
                return j;
            });
        });
    }

    function deviceOptions(selected) {
        return state.devicePool.map(function (d) {
            var opt = "<option value=\"" + d.id + "\""
                + (d.id === selected ? " selected" : "")
                + ">" + escapeHtml(d.name || ("device-" + d.id)) + "</option>";
            return opt;
        }).join("");
    }

    function roomOptions(selected) {
        return state.roomPool.map(function (r) {
            return "<option value=\"" + r.id + "\""
                + (r.id === selected ? " selected" : "")
                + ">" + escapeHtml(r.name || ("room-" + r.id)) + "</option>";
        }).join("");
    }

    function escapeHtml(s) {
        return String(s)
            .replace(/&/g, "&amp;").replace(/</g, "&lt;").replace(/>/g, "&gt;");
    }

    function renderRows() {
        if (!state.anchors.length) {
            $rows.innerHTML = "";
            $empty.hidden = false;
            return;
        }
        $empty.hidden = true;
        $rows.innerHTML = state.anchors.map(function (a, idx) {
            return ""
                + "<tr data-row=\"" + idx + "\">"
                + "<td><select data-field=\"anchor_device_id\">"
                +   deviceOptions(a.anchor_device_id)
                + "</select></td>"
                + "<td><select data-field=\"room_id\">"
                +   roomOptions(a.room_id)
                + "</select></td>"
                + "<td><input data-field=\"rssi_at_1m\" type=\"number\" "
                +   "min=\"-127\" max=\"0\" step=\"1\" value=\"" + a.rssi_at_1m + "\"></td>"
                + "<td><input data-field=\"host_scanner\" type=\"checkbox\""
                +   (a.host_scanner ? " checked" : "") + "></td>"
                + "<td><button class=\"sh-row-remove\" data-action=\"remove\">Remove</button></td>"
                + "</tr>";
        }).join("");
    }

    $rows.addEventListener("input", function (e) {
        var tr = e.target.closest("tr");
        if (!tr) return;
        var idx = +tr.getAttribute("data-row");
        var field = e.target.getAttribute("data-field");
        if (idx == null || !field) return;
        if (field === "host_scanner") {
            // At most one host_scanner per tenant — checking this row
            // un-checks every other row to keep the contract loud.
            state.anchors.forEach(function (a, i) {
                a.host_scanner = (i === idx) ? e.target.checked : false;
            });
            renderRows();
        } else if (field === "rssi_at_1m") {
            state.anchors[idx][field] = parseInt(e.target.value, 10);
        } else {
            state.anchors[idx][field] = parseInt(e.target.value, 10);
        }
    });

    $rows.addEventListener("click", function (e) {
        if (e.target.getAttribute("data-action") !== "remove") return;
        var tr = e.target.closest("tr");
        var idx = +tr.getAttribute("data-row");
        state.anchors.splice(idx, 1);
        renderRows();
    });

    document.getElementById("ble-add").addEventListener("click", function () {
        var pickDev = state.devicePool[0];
        var pickRoom = state.roomPool[0];
        if (!pickDev || !pickRoom) {
            setStatus("Need at least one device and one room before adding an anchor.", "err");
            return;
        }
        state.anchors.push({
            anchor_device_id: pickDev.id,
            room_id: pickRoom.id,
            rssi_at_1m: -50,
            host_scanner: false,
        });
        renderRows();
    });

    document.getElementById("ble-reset").addEventListener("click", function () {
        state.anchors = JSON.parse(JSON.stringify(state.pristine));
        renderRows();
        setStatus("Reset to last saved.", "");
    });

    document.getElementById("ble-save").addEventListener("click", function () {
        setStatus("Saving…", "");
        jsonPut("/api/smart-home/ble/anchors", { anchors: state.anchors })
            .then(function (j) {
                state.pristine = JSON.parse(JSON.stringify(state.anchors));
                setStatus("Saved " + (j.written || 0) + " anchor(s).", "ok");
            })
            .catch(function (e) { setStatus(String(e.message || e), "err"); });
    });

    // ── Initial load: fan out three GETs in parallel ─────────────
    Promise.all([
        jsonGet("/api/smart-home/ble/anchors"),
        jsonGet("/api/smart-home/devices"),
        jsonGet("/api/smart-home/rooms"),
    ]).then(function (results) {
        var anchorsResp = results[0];
        var devicesResp = results[1];
        var roomsResp   = results[2];
        state.anchors = (anchorsResp.anchors || []).map(function (a) {
            return {
                anchor_device_id: a.anchor_device_id,
                room_id: a.room_id,
                rssi_at_1m: a.rssi_at_1m,
                host_scanner: !!a.host_scanner,
            };
        });
        state.pristine = JSON.parse(JSON.stringify(state.anchors));
        state.devicePool = (devicesResp.devices || []);
        state.roomPool   = (roomsResp.rooms     || []);
        if (anchorsResp.note) setStatus(anchorsResp.note, "");
        renderRows();
    }).catch(function (e) {
        setStatus("Load failed: " + (e.message || e), "err");
    });
})();
"#;
