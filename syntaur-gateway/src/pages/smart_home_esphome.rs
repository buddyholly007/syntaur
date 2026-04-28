//! `/smart-home/esphome` — ESPHome quick-setup wizard.
//!
//! Discovers ESPHome devices on the network via mDNS
//! (`_esphomelib._tcp.local.`), parses each one's TXT record into a
//! capability snapshot, recommends a firmware role, and offers a
//! one-click adopt-as-anchor + (planned) one-click OTA flash.
//!
//! ## Why this lives in the smart-home section
//!
//! The Bluetooth Location page surfaces what *configured* scanners are
//! doing. This page is the on-ramp: a household with bare ESPHome
//! devices on the network shouldn't have to hand-edit YAML, copy MAC
//! addresses, or learn what `bluetooth_proxy_feature_flags` means.
//! The wizard does the discovery + recommendation, the user clicks
//! Adopt, and the device shows up in `/smart-home/ble` as an anchor
//! the next time the page loads.
//!
//! ## What's flashable today vs. follow-up
//!
//! v1: discovery + adopt. The "Recommended firmware" column tells the
//! user which role we'd flash if they let us, but the actual OTA push
//! is gated on a firmware-library / OTA-stream wiring (Phase 6b).
//! Until that lands, "Apply" only persists the recommendation as a
//! `metadata.recommended_role` hint — the user can still flash via
//! ESPHome dashboard, then re-discover.

use axum::response::Html;
use maud::{html, PreEscaped};

use super::shared::{shell, top_bar, Page};

pub async fn render() -> Html<String> {
    let page = Page {
        title: "Smart Home — ESPHome Setup",
        authed: true,
        extra_style: Some(EXTRA_STYLE),
        body_class: None,
        head_boot: None,
        crumb: Some("ESPHome"),
        topbar_status: None,
    };

    let body = html! {
        (top_bar("Smart Home — ESPHome", None))

        div class="sh-esp" {
            header class="sh-esp-hero" {
                h1 { "ESPHome quick setup" }
                p class="sh-esp-sub" {
                    "Find every ESPHome device on the network, see what it can do, "
                    "and adopt it as a Syntaur scanner with one click. Each device's "
                    "current firmware is read from its mDNS metadata; the recommended "
                    "role tells you what we'd flash to maximise data collection."
                }
                p class="sh-esp-cta" {
                    button type="button" id="esp-discover" class="sh-btn sh-btn-primary" {
                        "Discover devices"
                    }
                    span id="esp-status" class="sh-esp-status" {}
                    a href="/smart-home/ble" class="sh-btn sh-btn-ghost" { "↗ Bluetooth Location" }
                }
            }

            section class="sh-panel" {
                table class="sh-esp-table" {
                    thead {
                        tr {
                            th { "Device" }
                            th { "Address" }
                            th { "Firmware" }
                            th { "Board" }
                            th { "Current role hints" }
                            th title="What we recommend flashing for max data + capability" { "Recommended" }
                            th { "" }
                        }
                    }
                    tbody id="esp-rows" {}
                }
                p id="esp-empty" class="sh-esp-empty" hidden {
                    "No ESPHome devices found. Make sure they're on the same network "
                    "as the gateway and that mDNS isn't blocked."
                }
            }

            details class="sh-esp-help" {
                summary { "About the recommended role" }
                p {
                    "Every ESP32-class chip ships with a Bluetooth radio capable of "
                    "acting as an active BLE proxy. That's the role that maximises "
                    "what your household's other devices can see — passive scanning, "
                    "GATT relay for pairing, and BTHome battery readings all in one "
                    "firmware. We default to "
                    em { "BLE proxy (active)" }
                    " unless the device's mDNS metadata calls itself out as something "
                    "else (voice satellite, mmWave presence sensor, etc.)."
                }
            }
            details class="sh-esp-help" {
                summary { "What 'Adopt' does today" }
                p {
                    "Adopt registers the device as an "
                    code { "esphome_proxy" }
                    " in Syntaur's device list so the BLE ingest supervisor talks to it "
                    "for advertisement frames. The recommendation is persisted as a "
                    "hint on the device row. Pushing the actual firmware (OTA via the "
                    "ESPHome native API) is the next step — until that wiring lands, "
                    "you can flash through the ESPHome dashboard and click "
                    em { "Discover" }
                    " again to refresh the metadata."
                }
            }
        }

        script { (PreEscaped(PAGE_SCRIPT)) }
    };

    Html(shell(page, body).into_string())
}

const EXTRA_STYLE: &str = r#"
.sh-esp { max-width: 1100px; margin: 0 auto; padding: 1.5rem 1.5rem 4rem; color: var(--sh-text, #e7eaf0); }
.sh-esp-hero h1 { font-size: 1.5rem; margin: 0 0 0.4rem; }
.sh-esp-sub { color: var(--sh-text-muted, #98a2b3); margin: 0 0 0.8rem; max-width: 70ch; }
.sh-esp-cta { display: flex; gap: 0.6rem; align-items: center; flex-wrap: wrap; margin: 0 0 1.5rem; }
.sh-esp-status { color: var(--sh-text-muted, #98a2b3); font-size: 0.85rem; }
.sh-esp-status.ok  { color: #71e8a3; }
.sh-esp-status.err { color: #ff8a8a; }
.sh-panel {
    background: rgba(255,255,255,0.03);
    border: 1px solid rgba(255,255,255,0.08);
    border-radius: 14px;
    padding: 1rem 1.2rem 1.2rem;
    margin-bottom: 1.2rem;
}
.sh-esp-table {
    width: 100%; border-collapse: collapse;
    background: rgba(0,0,0,0.10);
    border: 1px solid rgba(255,255,255,0.08);
    border-radius: 10px;
    overflow: hidden;
    font-size: 0.93rem;
}
.sh-esp-table th, .sh-esp-table td {
    padding: 0.55rem 0.7rem;
    text-align: left;
    border-bottom: 1px solid rgba(255,255,255,0.05);
    vertical-align: middle;
}
.sh-esp-table th {
    font-weight: 500; color: var(--sh-text-muted, #98a2b3);
    background: rgba(255,255,255,0.02);
    font-size: 0.85rem;
}
.sh-esp-empty { color: var(--sh-text-muted, #98a2b3); padding: 1.4rem; text-align: center; font-size: 0.9rem; }
.sh-btn {
    background: rgba(255,255,255,0.05);
    color: var(--sh-text, #e7eaf0);
    border: 1px solid rgba(255,255,255,0.13);
    border-radius: 10px;
    padding: 0.45rem 0.9rem;
    font: inherit; cursor: pointer;
    transition: background 0.12s ease;
    text-decoration: none; display: inline-block;
}
.sh-btn:hover { background: rgba(255,255,255,0.10); }
.sh-btn-primary { background: rgba(94, 226, 255, 0.16); border-color: rgba(94, 226, 255, 0.45); }
.sh-btn-ghost { background: transparent; }
.sh-mac { font-family: ui-monospace, monospace; font-size: 0.85rem; color: var(--sh-text-muted, #98a2b3); }
.sh-pill {
    display: inline-block; padding: 0.15rem 0.55rem; border-radius: 999px;
    font-size: 0.78rem; font-weight: 500; border: 1px solid transparent;
    background: rgba(255,255,255,0.06); color: #98a2b3; border-color: rgba(255,255,255,0.13);
}
.sh-pill-rec { background: rgba(94, 226, 255, 0.13); color: #5ee2ff; border-color: rgba(94, 226, 255, 0.35); }
.sh-rec-cell { display: flex; flex-direction: column; gap: 0.2rem; }
.sh-rec-why { color: var(--sh-text-muted, #98a2b3); font-size: 0.78rem; max-width: 32ch; }
.sh-esp-help {
    margin-top: 1.5rem;
    background: rgba(255,255,255,0.03);
    border: 1px solid rgba(255,255,255,0.08);
    border-radius: 10px;
    padding: 0.7rem 1rem;
}
.sh-esp-help summary { cursor: pointer; font-weight: 500; }
.sh-esp-help p { color: var(--sh-text-muted, #98a2b3); margin: 0.6rem 0; }
.sh-esp-help code { background: rgba(0,0,0,0.30); padding: 0 0.3em; border-radius: 4px; }
"#;

const PAGE_SCRIPT: &str = r#"
(function () {
    "use strict";

    var $btn   = document.getElementById("esp-discover");
    var $stat  = document.getElementById("esp-status");
    var $rows  = document.getElementById("esp-rows");
    var $empty = document.getElementById("esp-empty");

    function setStatus(msg, klass) {
        $stat.textContent = msg || "";
        $stat.className = "sh-esp-status" + (klass ? (" " + klass) : "");
    }
    function escapeHtml(s) {
        return String(s == null ? "" : s)
            .replace(/&/g, "&amp;").replace(/</g, "&lt;").replace(/>/g, "&gt;")
            .replace(/"/g, "&quot;");
    }

    function roleLabel(r) {
        switch (r) {
            case "bt-proxy-active":  return "BLE proxy (active)";
            case "bt-proxy-passive": return "BLE proxy (passive)";
            case "voice-satellite":  return "Voice satellite";
            case "presence-mmwave":  return "Presence (mmWave + BLE)";
            default: return "Unknown";
        }
    }

    function renderRows(devices) {
        if (!devices.length) {
            $rows.innerHTML = "";
            $empty.hidden = false;
            return;
        }
        $empty.hidden = true;
        $rows.innerHTML = devices.map(function (d) {
            var hints = (d.current_role_hints || []).map(function (h) {
                return "<span class=\"sh-pill\">" + escapeHtml(h) + "</span>";
            }).join(" ");
            return ""
                + "<tr data-name=\"" + escapeHtml(d.name) + "\">"
                +   "<td>"
                +     "<div>" + escapeHtml(d.friendly_name || d.name) + "</div>"
                +     "<div class=\"sh-mac\">" + escapeHtml(d.name) + (d.mac ? " · " + escapeHtml(d.mac) : "") + "</div>"
                +   "</td>"
                +   "<td><span class=\"sh-mac\">" + escapeHtml(d.host) + ":" + d.port + "</span></td>"
                +   "<td>"
                +     escapeHtml(d.esphome_version || "—")
                +     (d.project_name ? "<div class=\"sh-mac\">" + escapeHtml(d.project_name)
                                      + (d.project_version ? "@" + escapeHtml(d.project_version) : "")
                                      + "</div>" : "")
                +   "</td>"
                +   "<td><span class=\"sh-mac\">" + escapeHtml(d.board || "—") + "</span></td>"
                +   "<td>" + (hints || "<span class=\"sh-mac\">—</span>") + "</td>"
                +   "<td><div class=\"sh-rec-cell\">"
                +     "<span class=\"sh-pill sh-pill-rec\">" + escapeHtml(roleLabel(d.recommended_role)) + "</span>"
                +     "<span class=\"sh-rec-why\">" + escapeHtml(d.recommendation_reason || "") + "</span>"
                +   "</div></td>"
                +   "<td><button type=\"button\" class=\"sh-btn\" data-action=\"adopt\">Adopt</button></td>"
                + "</tr>";
        }).join("");
    }

    var lastDiscovered = [];

    $btn.addEventListener("click", function () {
        $btn.disabled = true;
        setStatus("Scanning…", "");
        fetch("/api/smart-home/esphome/discover", {
            method: "POST",
            credentials: "same-origin",
            headers: { "Content-Type": "application/json" },
            body: JSON.stringify({ duration_secs: 4 }),
        })
        .then(function (r) {
            return r.json().then(function (j) {
                if (!r.ok) throw new Error(j.error || ("HTTP " + r.status));
                return j;
            });
        })
        .then(function (j) {
            lastDiscovered = j.devices || [];
            renderRows(lastDiscovered);
            setStatus("Found " + lastDiscovered.length + " device" + (lastDiscovered.length === 1 ? "" : "s") + ".", "ok");
        })
        .catch(function (e) { setStatus(String(e.message || e), "err"); })
        .finally(function () { $btn.disabled = false; });
    });

    $rows.addEventListener("click", function (e) {
        if (e.target.getAttribute("data-action") !== "adopt") return;
        var tr = e.target.closest("tr");
        var name = tr.getAttribute("data-name");
        var d = lastDiscovered.find(function (x) { return x.name === name; });
        if (!d) return;
        e.target.disabled = true;
        e.target.textContent = "Adopting…";
        fetch("/api/smart-home/esphome/adopt", {
            method: "POST", credentials: "same-origin",
            headers: { "Content-Type": "application/json" },
            body: JSON.stringify({
                name: d.name, host: d.host, port: d.port,
                friendly_name: d.friendly_name, mac: d.mac,
                mode: "tracking",
            }),
        })
        .then(function (r) {
            return r.json().then(function (j) {
                if (!r.ok) throw new Error(j.error || ("HTTP " + r.status));
                return j;
            });
        })
        .then(function (j) {
            e.target.textContent = "Adopted (id " + j.device_id + ")";
        })
        .catch(function (err) {
            e.target.disabled = false; e.target.textContent = "Adopt";
            setStatus("Adopt: " + (err.message || err), "err");
        });
    });
})();
"#;
