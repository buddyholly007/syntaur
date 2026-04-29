//! /tax — migrated from static/tax.html. Structural markup and
//! embedded scripts live as raw-string consts below so their bytes
//! count as Rust and the file compiles type-checked through maud.

use axum::response::Html;
use maud::{html, PreEscaped};

use super::shared::{shell, top_bar, Page};

pub async fn render() -> Html<String> {
    let page = Page {
        title: "Tax",
        authed: true,
        extra_style: Some(EXTRA_STYLE),
        body_class: None,
        head_boot: None,
        crumb: None,
        topbar_status: None,
    };
    let body = html! {
        // Module paywall overlay (hidden by default, shown if module is locked)
        div id="module-paywall" class="hidden fixed inset-0 z-50 bg-gray-950/95 backdrop-blur-sm flex items-center justify-center" {
            div class="max-w-md w-full mx-4" {
                div class="card text-center" {
                    div class="text-4xl mb-4" {
                        "💰"
                    }
                    h2 class="text-xl font-semibold text-white mb-2" {
                        "Tax & Expenses Module"
                    }
                    p class="text-sm text-gray-400 mb-6" {
                        "Receipt scanning, expense tracking, tax document management, deduction calculator, and year-end tax prep wizard."
                    }
                    div id="paywall-trial-available" {
                        button onclick="startFreeTrial()" class="w-full bg-oc-600 hover:bg-oc-700 text-white font-medium py-3 px-6 rounded-xl transition-colors text-base mb-3" {
                            " Start Free 3-Day Trial "
                        }
                        p class="text-xs text-gray-500 mb-4" {
                            "No credit card required. Full access for 3 days."
                        }
                    }
                    div id="paywall-trial-expired" class="hidden" {
                        p class="text-sm text-yellow-400 mb-4" {
                            "Your free trial has ended."
                        }
                    }
                    div class="border-t border-gray-700 pt-4" {
                        button onclick="upgradePro()" class="w-full bg-gradient-to-r from-purple-600 to-oc-600 hover:from-purple-700 hover:to-oc-700 text-white font-medium py-3 px-6 rounded-xl transition-all text-base" {
                            " Upgrade to Syntaur Pro — $49 "
                        }
                        p class="text-xs text-gray-500 mt-2" {
                            "One-time payment. Unlocks all modules forever."
                        }
                    }
                    a href="/" class="text-xs text-gray-500 hover:text-gray-300 mt-4 inline-block" {
                        "Back to Dashboard"
                    }
                }
            }
        }
        // Trial banner (shown when trial is active)
        div id="trial-banner" class="hidden bg-gradient-to-r from-oc-700/30 to-purple-700/30 border-b border-oc-800/30 text-center py-1.5 text-xs text-gray-300" {
            span id="trial-banner-text" {
                "Free trial — "
                strong id="trial-days-left" {
                    "3"
                }
                " days remaining"
            }
            button onclick="upgradePro()" class="ml-3 bg-oc-600 hover:bg-oc-700 text-white px-3 py-0.5 rounded-full text-xs transition-colors" {
                "Upgrade to Pro"
            }
        }
        // Shared global top bar
        // Tax-specific sub-bar — deadline pill + year selector. The
        // deadline-pill element ID is preserved so updateDeadlinePill JS
        // keeps updating it in place.
        div class="tax-subbar" {
            span id="deadline-pill" class="deadline-pill hidden" {
                span class="deadline-dot" {}
                span id="deadline-pill-text" { "—" }
            }
            div style="flex:1" {}
            select id="year-select" class="bg-gray-800 border border-gray-700 rounded-lg px-2 py-1 text-sm text-gray-300 outline-none" onchange="changeYear()" {}
        }
        div class="tax-section-wrap" {
            // Section nav (top-level)
            div class="border-t border-gray-800/50" {
                div class="max-w-6xl mx-auto px-4 flex items-center gap-1 overflow-x-auto whitespace-nowrap" {
                    button onclick="showSection('investments')" id="sec-btn-investments" class="sec-tab active" {
                        "Investments"
                    }
                    button onclick="showSection('documents')" id="sec-btn-documents" class="sec-tab" {
                        "Documents"
                    }
                    button onclick="showSection('deductions')" id="sec-btn-deductions" class="sec-tab" {
                        "Deductions"
                    }
                    button onclick="showSection('dashboard')" id="sec-btn-dashboard" class="sec-tab" {
                        "Dashboard"
                    }
                    button onclick="showSection('filing')" id="sec-btn-filing" class="sec-tab" {
                        "Filing"
                    }
                    button onclick="showSection('ledger')" id="sec-btn-ledger" class="sec-tab" {
                        "Ledger"
                    }
                }
            }
            // Sub-tab chip row (context-sensitive — populated by showSection())
            div class="border-t border-gray-800/50 bg-gray-900/30" {
                div class="max-w-6xl mx-auto px-4 py-2 flex items-center gap-2 overflow-x-auto whitespace-nowrap" id="sub-tab-bar" {
                    // filled in by JS
                }
            }
        }
        div class="flex h-[calc(100vh-126px)]" {
            // LEFT: Tax content
            div class="flex-1 overflow-y-auto px-4 py-4" {
                // KPI strip — always visible at top of main canvas
                div class="kpi-strip" id="kpi-strip" {
                    div class="kpi-tile" id="kpi-tile-portfolio" {
                        div class="kpi-label" {
                            "Portfolio value"
                        }
                        div class="kpi-value" id="kpi-portfolio-value" {
                            "—"
                        }
                        div class="kpi-sub" id="kpi-portfolio-sub" {
                        }
                    }
                    div class="kpi-tile" id="kpi-tile-income" {
                        div class="kpi-label" {
                            "Income YTD"
                        }
                        div class="kpi-value" id="kpi-income-value" {
                            "—"
                        }
                        div class="kpi-sub" id="kpi-income-sub" {
                        }
                    }
                    div class="kpi-tile" id="kpi-tile-deductions" {
                        div class="kpi-label" {
                            "Deductions YTD"
                        }
                        div class="kpi-value" id="kpi-deductions-value" {
                            "—"
                        }
                        div class="kpi-sub" id="kpi-deductions-sub" {
                        }
                    }
                    div class="kpi-tile" id="kpi-tile-tax" {
                        div class="kpi-label" {
                            "Est. refund / owe"
                        }
                        div class="kpi-value" id="kpi-tax-value" {
                            "—"
                        }
                        div class="kpi-sub" id="kpi-tax-sub" {
                        }
                    }
                }
                // Dashboard tab (the old "Overview") — hidden by default; Investments is the landing
                div id="tab-overview" class="hidden" {
                    // Taxpayer Profile
                    div class="card mb-4" id="profile-card" {
                        div class="flex items-center justify-between mb-3" {
                            h3 class="font-medium text-sm" {
                                "Tax Filing Profile"
                            }
                            div class="flex items-center gap-2" {
                                span id="profile-year-label" class="text-xs text-gray-500" {
                                }
                                button onclick="toggleProfileEdit()" class="text-xs text-oc-500 hover:text-oc-400" id="profile-edit-btn" {
                                    "Edit"
                                }
                            }
                        }
                        // Masked SSN banner — shown when stored SSN starts with X (i.e. user
                        // saved a redacted W-2 last-4 instead of their real digits).
                        div id="ssn-banner" class="hidden mb-3 p-3 rounded-lg border border-yellow-600/40 bg-yellow-500/10" {
                            div class="flex items-start gap-3" {
                                div class="text-yellow-300 text-lg leading-none mt-0.5" {
                                    "!"
                                }
                                div class="flex-1" {
                                    p class="text-xs font-medium text-yellow-200" id="ssn-banner-title" {
                                        "Your SSN is masked"
                                    }
                                    p class="text-xs text-gray-300 mt-1" id="ssn-banner-detail" {
                                        "The IRS needs your full 9-digit SSN to file Form 4868."
                                    }
                                    div class="mt-2 grid grid-cols-1 md:grid-cols-2 gap-2" {
                                        div id="ssn-banner-self-row" class="hidden" {
                                            label class="text-xs text-gray-400" {
                                                "Your SSN"
                                            }
                                            input id="ssn-banner-self" class="input text-sm" placeholder="###-##-####" autocomplete="off";
                                        }
                                        div id="ssn-banner-spouse-row" class="hidden" {
                                            label class="text-xs text-gray-400" {
                                                "Spouse SSN"
                                            }
                                            input id="ssn-banner-spouse" class="input text-sm" placeholder="###-##-####" autocomplete="off";
                                        }
                                    }
                                    div class="mt-2 flex items-center gap-2" {
                                        button onclick="saveSsnBanner()" class="btn-primary text-xs" {
                                            "Save SSN"
                                        }
                                        span id="ssn-banner-result" class="text-xs" {
                                        }
                                    }
                                }
                            }
                        }
                        // View mode
                        div id="profile-view" {
                            div class="grid grid-cols-2 md:grid-cols-4 gap-3 text-sm" id="profile-summary" {
                                p class="text-xs text-gray-600 col-span-4" {
                                    "Loading profile..."
                                }
                            }
                            // Dependents
                            div id="dependents-section" class="hidden mt-3 pt-3 border-t border-gray-800" {
                                div class="flex items-center justify-between mb-2" {
                                    p class="text-xs text-gray-500 font-medium" {
                                        "Dependents"
                                    }
                                    button onclick="showAddDependent()" class="text-[10px] text-oc-500 hover:text-oc-400" {
                                        "+ Add"
                                    }
                                }
                                div id="dependents-list" class="space-y-1" {
                                }
                            }
                        }
                        // Edit mode
                        div id="profile-edit" class="hidden" {
                            div class="grid grid-cols-2 gap-3 text-sm" {
                                div {
                                    label class="text-xs text-gray-500" {
                                        "First Name"
                                    }
                                    input class="input text-sm" id="pf-first";
                                }
                                div {
                                    label class="text-xs text-gray-500" {
                                        "Last Name"
                                    }
                                    input class="input text-sm" id="pf-last";
                                }
                                div {
                                    label class="text-xs text-gray-500" {
                                        "SSN"
                                    }
                                    input class="input text-sm" id="pf-ssn" type="password" placeholder="XXX-XX-XXXX";
                                }
                                div {
                                    label class="text-xs text-gray-500" {
                                        "Date of Birth"
                                    }
                                    input class="input text-sm" id="pf-dob" type="date";
                                }
                                div class="col-span-2" {
                                    label class="text-xs text-gray-500" {
                                        "Address"
                                    }
                                    input class="input text-sm" id="pf-addr" placeholder="123 Main St";
                                }
                                div {
                                    label class="text-xs text-gray-500" {
                                        "City"
                                    }
                                    input class="input text-sm" id="pf-city";
                                }
                                div class="grid grid-cols-2 gap-2" {
                                    div {
                                        label class="text-xs text-gray-500" {
                                            "State"
                                        }
                                        input class="input text-sm" id="pf-state" maxlength="2" placeholder="WA";
                                    }
                                    div {
                                        label class="text-xs text-gray-500" {
                                            "ZIP"
                                        }
                                        input class="input text-sm" id="pf-zip" maxlength="10";
                                    }
                                }
                                div {
                                    label class="text-xs text-gray-500" {
                                        "Filing Status"
                                    }
                                    select class="input text-sm" id="pf-filing" {
                                        option value="single" {
                                            "Single"
                                        }
                                        option value="married_jointly" {
                                            "Married Filing Jointly"
                                        }
                                        option value="married_separately" {
                                            "Married Filing Separately"
                                        }
                                        option value="head_of_household" {
                                            "Head of Household"
                                        }
                                    }
                                }
                                div {
                                    label class="text-xs text-gray-500" {
                                        "Occupation"
                                    }
                                    input class="input text-sm" id="pf-occupation";
                                }
                            }
                            details class="mt-3" id="spouse-section" {
                                summary class="text-xs text-gray-400 cursor-pointer hover:text-gray-300" {
                                    "Spouse Information"
                                }
                                div class="grid grid-cols-2 gap-3 text-sm mt-2" {
                                    div {
                                        label class="text-xs text-gray-500" {
                                            "Spouse First Name"
                                        }
                                        input class="input text-sm" id="pf-sp-first";
                                    }
                                    div {
                                        label class="text-xs text-gray-500" {
                                            "Spouse Last Name"
                                        }
                                        input class="input text-sm" id="pf-sp-last";
                                    }
                                    div {
                                        label class="text-xs text-gray-500" {
                                            "Spouse SSN"
                                        }
                                        input class="input text-sm" id="pf-sp-ssn" type="password";
                                    }
                                    div {
                                        label class="text-xs text-gray-500" {
                                            "Spouse DOB"
                                        }
                                        input class="input text-sm" id="pf-sp-dob" type="date";
                                    }
                                }
                            }
                            div class="flex gap-2 mt-3 items-center" {
                                button onclick="saveProfile()" class="btn-primary text-xs" {
                                    "Save Profile"
                                }
                                button onclick="autoFillProfileFromScans()" class="text-xs bg-gray-700 hover:bg-gray-600 text-gray-100 px-3 py-1.5 rounded-lg" title="Pull values from scanned W-2, 1095-C, mortgage statement" {
                                    "Auto-fill from scans"
                                }
                                button onclick="toggleProfileEdit()" class="text-xs text-gray-500 hover:text-gray-300" {
                                    "Cancel"
                                }
                                span id="profile-save-result" class="text-xs self-center" {
                                }
                            }
                            p id="profile-suggest-sources" class="text-xs text-gray-500 mt-2" {
                            }
                        }
                    }
                    // Summary cards
                    div class="grid grid-cols-2 md:grid-cols-4 gap-3 mb-6" {
                        div class="card" {
                            p class="text-xs text-gray-500 uppercase" {
                                "Total Expenses"
                            }
                            p class="text-2xl font-semibold mt-1" id="sum-total" {
                                "--"
                            }
                        }
                        div class="card" {
                            p class="text-xs text-gray-500 uppercase" {
                                "Business"
                            }
                            p class="text-2xl font-semibold mt-1 text-oc-500" id="sum-business" {
                                "--"
                            }
                        }
                        div class="card" {
                            p class="text-xs text-gray-500 uppercase" {
                                "Tax Deductible"
                            }
                            p class="text-2xl font-semibold mt-1 text-green-400" id="sum-deductible" {
                                "--"
                            }
                        }
                        div class="card" {
                            p class="text-xs text-gray-500 uppercase" {
                                "Receipts"
                            }
                            p class="text-2xl font-semibold mt-1" id="sum-receipts" {
                                "--"
                            }
                        }
                    }
                    // Potential Deductions
                    details class="card mb-6 cursor-pointer group" id="deductions-section" {
                        summary class="flex items-center justify-between" {
                            div class="flex items-center gap-2" {
                                span class="text-green-400" {
                                    "💡"
                                }
                                h3 class="font-medium" {
                                    "Potential Deductions You Might Be Missing"
                                }
                            }
                            span class="text-xs text-gray-500 group-open:hidden" {
                                "Click to expand"
                            }
                        }
                        div class="mt-4 space-y-3" id="deductions-list" {
                            p class="text-xs text-gray-500" {
                                "Analyzing your expenses..."
                            }
                        }
                    }
                    // Export to Tax Software
                    div class="card mb-4" {
                        div class="flex items-center justify-between mb-3" {
                            h3 class="font-medium text-sm" {
                                "Export to Tax Software"
                            }
                            span id="export-year-label" class="text-xs text-gray-500" {
                            }
                        }
                        div class="grid grid-cols-3 gap-3 mb-3" {
                            a href="#" id="export-txf" class="flex flex-col items-center gap-1.5 p-3 bg-gray-900 rounded-lg hover:bg-gray-800 border border-gray-700 transition-colors text-center" {
                                span class="text-lg" {
                                    "💾"
                                }
                                span class="text-xs text-white font-medium" {
                                    "TXF File"
                                }
                                span class="text-[10px] text-gray-500" {
                                    "TurboTax / H&R Block Desktop"
                                }
                            }
                            a href="#" id="export-csv-irs" class="flex flex-col items-center gap-1.5 p-3 bg-gray-900 rounded-lg hover:bg-gray-800 border border-gray-700 transition-colors text-center" {
                                span class="text-lg" {
                                    "📈"
                                }
                                span class="text-xs text-white font-medium" {
                                    "IRS Summary CSV"
                                }
                                span class="text-[10px] text-gray-500" {
                                    "CPA / Any software"
                                }
                            }
                            a href="#" id="export-csv-raw" class="flex flex-col items-center gap-1.5 p-3 bg-gray-900 rounded-lg hover:bg-gray-800 border border-gray-700 transition-colors text-center" {
                                span class="text-lg" {
                                    "📄"
                                }
                                span class="text-xs text-white font-medium" {
                                    "Expense CSV"
                                }
                                span class="text-[10px] text-gray-500" {
                                    "All transactions"
                                }
                            }
                        }
                        details class="text-xs text-gray-500" {
                            summary class="cursor-pointer hover:text-gray-300" {
                                "Import instructions"
                            }
                            div class="mt-2 space-y-1.5 pl-2 border-l border-gray-700" {
                                p {
                                    strong class="text-gray-300" {
                                        "TurboTax Desktop:"
                                    }
                                    " File → Import → From Accounting Software → select the .txf file"
                                }
                                p {
                                    strong class="text-gray-300" {
                                        "H&R Block Desktop:"
                                    }
                                    " File → Import Financial Information → browse for .txf file"
                                }
                                p {
                                    strong class="text-gray-300" {
                                        "TaxAct:"
                                    }
                                    " Use the IRS Summary CSV as reference for manual entry"
                                }
                                p {
                                    strong class="text-gray-300" {
                                        "CPA / Tax preparer:"
                                    }
                                    " Send the IRS Summary CSV — it maps every line to the correct IRS form"
                                }
                                p class="text-gray-600 mt-1" {
                                    "Note: TurboTax Online and H&R Block Online do not accept file imports. Desktop versions are required for TXF import."
                                }
                            }
                        }
                    }
                    // File Extension (Form 4868) — Multi-step workflow
                    div class="card mb-4" id="extension-card" {
                        div class="flex items-center justify-between mb-3" {
                            div class="flex items-center gap-2" {
                                span class="text-yellow-400" {
                                    "⏰"
                                }
                                h3 class="font-medium text-sm" {
                                    "File an Extension (Form 4868)"
                                }
                            }
                            div class="flex items-center gap-2" {
                                span id="ext-status-badge" class="badge badge-yellow text-[10px]" {
                                    "Not filed"
                                }
                                span class="badge badge-green text-[10px]" {
                                    "Free"
                                }
                            }
                        }
                        // Step 1: Review + Choose Method
                        div id="ext-step-review" {
                            p class="text-xs text-gray-400 mb-3" {
                                "Review your estimated tax data, adjust if needed, then choose how to file. Extends filing deadline to "
                                strong class="text-gray-300" {
                                    "October 15"
                                }
                                ". Payment is still due "
                                strong class="text-red-400" {
                                    "April 15"
                                }
                                "."
                            }
                            div class="bg-gray-800 rounded-lg p-3 mb-3 border border-gray-700" {
                                div class="grid grid-cols-2 gap-3 text-sm" {
                                    div {
                                        label class="text-xs text-gray-400" {
                                            "Estimated total tax"
                                        }
                                        input class="input" id="ext-total-tax" value="$0.00";
                                    }
                                    div {
                                        label class="text-xs text-gray-400" {
                                            "Payments made (withholding + estimated)"
                                        }
                                        input class="input" id="ext-payments" value="$0.00";
                                    }
                                    div {
                                        label class="text-xs text-gray-400" {
                                            "Balance due"
                                        }
                                        input class="input font-semibold text-green-600" id="ext-balance" value="$0.00" readonly;
                                    }
                                    div {
                                        label class="text-xs text-gray-400" {
                                            "Paying with this extension"
                                        }
                                        input class="input" id="ext-payment" value="0.00" oninput="updateExtBalance()";
                                    }
                                }
                            }
                            p class="text-xs text-gray-400 mb-2" {
                                "Choose how to file:"
                            }
                            div class="space-y-2" {
                                a href="javascript:void(0)" onclick="startExtFiling('direct_pay')" class="block p-3 rounded-lg bg-gray-900 border border-gray-700 hover:border-oc-600 cursor-pointer transition-colors no-underline" {
                                    div class="flex items-center justify-between" {
                                        div {
                                            p class="text-sm text-white font-medium" {
                                                "IRS Direct Pay"
                                            }
                                            p class="text-[11px] text-gray-500" {
                                                "Payment auto-files your extension. Instant confirmation number."
                                            }
                                        }
                                        span class="text-oc-500 text-xs font-medium" {
                                            "Recommended →"
                                        }
                                    }
                                }
                                a href="javascript:void(0)" onclick="startExtFiling('free_file')" class="block p-3 rounded-lg bg-gray-900 border border-gray-700 hover:border-gray-600 cursor-pointer transition-colors no-underline" {
                                    div class="flex items-center justify-between" {
                                        div {
                                            p class="text-sm text-white font-medium" {
                                                "IRS Free File"
                                            }
                                            p class="text-[11px] text-gray-500" {
                                                "E-file Form 4868 free. Confirmation via email within 24-48h."
                                            }
                                        }
                                        span class="text-gray-500 text-xs" {
                                            "Free →"
                                        }
                                    }
                                }
                                a href="javascript:void(0)" onclick="startExtFiling('mail')" class="block p-3 rounded-lg bg-gray-900 border border-gray-700 hover:border-gray-600 cursor-pointer transition-colors no-underline" {
                                    div class="flex items-center justify-between" {
                                        div {
                                            p class="text-sm text-white font-medium" {
                                                "Print & Mail"
                                            }
                                            p class="text-[11px] text-gray-500" {
                                                "Download form, print, mail by April 15. Use certified mail for proof."
                                            }
                                        }
                                        span class="text-gray-500 text-xs" {
                                            "Download →"
                                        }
                                    }
                                }
                            }
                            details class="mt-3 text-xs text-gray-400" {
                                summary class="cursor-pointer hover:text-gray-200" {
                                    "Form 4868 options & missing fields →"
                                }
                                div class="mt-3 p-3 bg-gray-900/40 rounded-lg space-y-3" {
                                    p class="text-xs text-gray-500" {
                                        "Override anything missing from your tax profile, or check the boxes for less-common situations. These apply when you generate the PDF."
                                    }
                                    div class="grid grid-cols-2 gap-2" {
                                        div {
                                            label class="label" {
                                                "Name(s) on return"
                                            }
                                            input id="ext-opt-name" class="input" placeholder="leave blank = use profile";
                                        }
                                        div {
                                            label class="label" {
                                                "SSN"
                                            }
                                            input id="ext-opt-ssn" class="input" placeholder="###-##-####";
                                        }
                                        div {
                                            label class="label" {
                                                "Spouse SSN (joint only)"
                                            }
                                            input id="ext-opt-spouse-ssn" class="input" placeholder="###-##-####";
                                        }
                                        div {
                                            label class="label" {
                                                "Street address"
                                            }
                                            input id="ext-opt-address" class="input" placeholder="leave blank = use profile";
                                        }
                                        div {
                                            label class="label" {
                                                "City"
                                            }
                                            input id="ext-opt-city" class="input";
                                        }
                                        div class="grid grid-cols-2 gap-2" {
                                            div {
                                                label class="label" {
                                                    "State"
                                                }
                                                input id="ext-opt-state" class="input" maxlength="2" placeholder="WA";
                                            }
                                            div {
                                                label class="label" {
                                                    "ZIP"
                                                }
                                                input id="ext-opt-zip" class="input" placeholder="98502";
                                            }
                                        }
                                    }
                                    div class="grid grid-cols-2 gap-2" {
                                        label class="flex items-center gap-2 text-gray-300" {
                                            input type="checkbox" id="ext-opt-ooc";
                                            " Line 8: I'm \"out of the country\" (US citizen/resident abroad)"
                                        }
                                        label class="flex items-center gap-2 text-gray-300" {
                                            input type="checkbox" id="ext-opt-1040nr";
                                            " Line 9: I file Form 1040-NR with no withheld wages"
                                        }
                                    }
                                    details class="text-xs text-gray-500" {
                                        summary class="cursor-pointer hover:text-gray-300" {
                                            "Fiscal-year filer (rare) →"
                                        }
                                        div class="mt-2 grid grid-cols-3 gap-2" {
                                            div {
                                                label class="label" {
                                                    "FY beginning (MM/DD)"
                                                }
                                                input id="ext-opt-fy-begin" class="input" placeholder="07/01";
                                            }
                                            div {
                                                label class="label" {
                                                    "FY ending (MM/DD)"
                                                }
                                                input id="ext-opt-fy-end" class="input" placeholder="06/30";
                                            }
                                            div {
                                                label class="label" {
                                                    "FY ending year (YY)"
                                                }
                                                input id="ext-opt-fy-end-year" class="input" placeholder="26";
                                            }
                                        }
                                    }
                                }
                            }
                            span id="ext-result" class="text-xs mt-2 block" {
                            }
                        }
                        // Step 2: Copy-Assist + File
                        div id="ext-step-file" class="hidden" {
                            div class="flex items-center gap-2 mb-3" {
                                button onclick="showExtStep('review')" class="text-xs text-gray-500 hover:text-gray-300" {
                                    "← Back"
                                }
                                span class="text-xs text-gray-400" id="ext-method-label" {
                                    "Filing via IRS Direct Pay"
                                }
                            }
                            div class="bg-gray-900 rounded-lg p-3 mb-3 space-y-2" id="ext-copy-fields" {
                                // Populated by JS: steps + identity + spouse + payment
                            }
                            div id="ext-form-link" class="mb-3 flex items-center gap-3 flex-wrap" {
                                a id="ext-form-anchor" href="#" onclick="openForm4868(); return false;" class="bg-oc-600 hover:bg-oc-700 text-white font-medium py-2 px-4 rounded-lg transition-colors text-sm inline-block no-underline cursor-pointer" {
                                    "Generate Form 4868 →"
                                }
                                a href="#" onclick="openEnvelope(); return false;" class="bg-gray-700 hover:bg-gray-600 text-white font-medium py-2 px-4 rounded-lg transition-colors text-sm inline-block no-underline cursor-pointer" title="9.5 x 4.125 inch PDF positioned for direct envelope printing" {
                                    "Print #10 envelope"
                                }
                                span class="text-xs text-gray-500" {
                                    "Pre-filled from your documents. Print or save as PDF."
                                }
                            }
                            div class="bg-gray-800 rounded-lg p-3 border border-gray-700" {
                                p class="text-xs text-gray-300 mb-2" {
                                    "After you submit on the IRS website, enter your confirmation below:"
                                }
                                div class="flex gap-2" {
                                    input class="input text-sm flex-1" id="ext-confirm-input" placeholder="Confirmation number or submission ID";
                                    button onclick="confirmExtension()" class="btn-primary text-xs" {
                                        "Save & Confirm"
                                    }
                                }
                                span id="ext-confirm-result" class="text-xs mt-1 block" {
                                }
                            }
                        }
                        // Step 3: Confirmed / Tracking
                        div id="ext-step-confirmed" class="hidden" {
                            div class="bg-green-900/20 border border-green-800/30 rounded-lg p-4 mb-3" {
                                div class="flex items-center gap-2 mb-1" {
                                    span class="text-green-400" {
                                        "✓"
                                    }
                                    p class="text-sm text-green-400 font-medium" {
                                        "Extension Confirmed"
                                    }
                                }
                                p class="text-xs text-gray-400" id="ext-confirmed-detail" {
                                    "Filed via IRS Direct Pay"
                                }
                            }
                            div class="grid grid-cols-2 gap-3 text-sm mb-3" {
                                div {
                                    label class="text-xs text-gray-500" {
                                        "Confirmation ID"
                                    }
                                    p class="text-sm text-white font-mono" id="ext-confirmed-id" {
                                        "—"
                                    }
                                }
                                div {
                                    label class="text-xs text-gray-500" {
                                        "Filed on"
                                    }
                                    p class="text-sm text-white" id="ext-confirmed-date" {
                                        "—"
                                    }
                                }
                                div {
                                    label class="text-xs text-gray-500" {
                                        "New filing deadline"
                                    }
                                    p class="text-sm text-yellow-400 font-medium" id="ext-deadline" {
                                        "October 15"
                                    }
                                }
                                div {
                                    label class="text-xs text-gray-500" {
                                        "Balance due"
                                    }
                                    p class="text-sm text-white" id="ext-confirmed-balance" {
                                        "—"
                                    }
                                }
                            }
                            div class="text-xs text-gray-500 space-y-1" {
                                p {
                                    a href="https://www.irs.gov/refunds" target="_blank" class="text-oc-500 hover:text-oc-400" {
                                        "Check status at irs.gov/refunds"
                                    }
                                    " or use the "
                                    a href="https://www.irs.gov/newsroom/irs2goapp" target="_blank" class="text-oc-500 hover:text-oc-400" {
                                        "IRS2Go app"
                                    }
                                }
                                p {
                                    "This confirmation is saved with your tax documents."
                                }
                            }
                        }
                    }
                    // Category breakdown
                    div class="card" {
                        div class="flex items-center justify-between mb-4" {
                            h3 class="font-medium" {
                                "By Category"
                            }
                        }
                        div id="category-list" class="space-y-2" {
                            p class="text-xs text-gray-600" {
                                "Loading..."
                            }
                        }
                    }
                }
                // Expenses tab
                div id="tab-expenses" class="hidden" {
                    // Add expense form
                    div class="card mb-6" {
                        h3 class="font-medium mb-4" {
                            "Log Expense"
                        }
                        div class="grid grid-cols-2 md:grid-cols-4 gap-3" {
                            div {
                                label class="label" {
                                    "Vendor"
                                }
                                input type="text" id="exp-vendor" class="input" placeholder="Home Depot";
                            }
                            div {
                                label class="label" {
                                    "Amount"
                                }
                                input type="text" id="exp-amount" class="input" placeholder="45.99";
                            }
                            div {
                                label class="label" {
                                    "Category"
                                }
                                select id="exp-category" class="input" {
                                }
                            }
                            div {
                                label class="label" {
                                    "Date"
                                }
                                input type="date" id="exp-date" class="input";
                            }
                        }
                        div class="grid grid-cols-2 gap-3 mt-3" {
                            div {
                                label class="label" {
                                    "Description (optional)"
                                }
                                input type="text" id="exp-desc" class="input" placeholder="2x4 lumber for shelving project";
                            }
                            div {
                                label class="label" {
                                    "Entity"
                                }
                                select id="exp-entity" class="input" {
                                    option value="business" {
                                        "Business"
                                    }
                                    option value="personal" {
                                        "Personal"
                                    }
                                }
                            }
                        }
                        div class="mt-3 flex items-center gap-3" {
                            button onclick="addExpense()" class="btn-primary" {
                                "Add Expense"
                            }
                            span id="exp-status" class="text-xs text-gray-500" {
                            }
                        }
                    }
                    // Expense list
                    div class="card" {
                        div class="flex items-center justify-between mb-4" {
                            h3 class="font-medium" {
                                "Recent Expenses"
                            }
                            div class="flex gap-2" {
                                select id="exp-filter-entity" class="input w-auto" onchange="loadExpenses()" {
                                    option value="" {
                                        "All"
                                    }
                                    option value="business" {
                                        "Business"
                                    }
                                    option value="personal" {
                                        "Personal"
                                    }
                                }
                            }
                        }
                        div id="expense-list" class="space-y-2" {
                            p class="text-xs text-gray-600" {
                                "Loading..."
                            }
                        }
                    }
                }
                // Receipts tab
                div id="tab-receipts" class="hidden" {
                    div class="p-3 rounded-lg bg-yellow-900/20 border border-yellow-800/50 mb-4" {
                        div class="flex items-start gap-2" {
                            span class="text-yellow-400 mt-0.5" {
                                "⚠"
                            }
                            p class="text-xs text-yellow-300/80" {
                                strong {
                                    "Always verify"
                                }
                                " AI-extracted amounts against your original receipts before using for tax filing."
                            }
                        }
                    }
                    // Upload
                    div class="card mb-6" {
                        h3 class="font-medium mb-3" {
                            "Upload Receipt"
                        }
                        p class="text-xs text-gray-500 mb-3" {
                            "Upload a photo or PDF of a receipt. AI will automatically extract the vendor, amount, date, and category."
                        }
                        div class="flex items-center gap-3" {
                            label class="btn-primary cursor-pointer" {
                                " Choose File "
                                input type="file" class="hidden" accept="image/*,.pdf" onchange="uploadReceipt(this)" id="receipt-input";
                            }
                            span id="receipt-status" class="text-xs text-gray-500" {
                            }
                        }
                    }
                    // Receipt gallery
                    div class="card" {
                        h3 class="font-medium mb-4" {
                            "Receipts"
                        }
                        div id="receipt-list" class="grid grid-cols-2 md:grid-cols-3 gap-3" {
                            p class="text-xs text-gray-600 col-span-full" {
                                "Loading..."
                            }
                        }
                    }
                }
                // Documents tab
                div id="tab-documents" class="hidden" {
                    div class="card mb-6" {
                        h3 class="font-medium mb-3" {
                            "Smart Upload"
                        }
                        p class="text-xs text-gray-500 mb-3" {
                            "Upload any tax document — receipts, W-2s, 1099s, bank statements, credit card statements, mortgage statements, or any other document. AI will automatically identify the type and route it to the correct handler."
                        }
                        details class="mb-3" {
                            summary class="text-xs text-gray-600 cursor-pointer hover:text-gray-400" {
                                "Accuracy tips"
                            }
                            div class="mt-2 p-2 rounded bg-gray-900 text-xs text-gray-500 space-y-1" {
                                p {
                                    "The scanner uses AI vision to read your documents. For best results:"
                                }
                                p {
                                    "• Upload clear, high-resolution scans (photos of paper documents work too)"
                                }
                                p {
                                    "• PDFs are automatically converted to high-res images for better reading"
                                }
                                p {
                                    "• "
                                    strong class="text-gray-400" {
                                        "If you see frequent errors"
                                    }
                                    ", switching to a more capable model in Settings can significantly improve accuracy."
                                }
                                p class="mt-1" {
                                    strong class="text-gray-400" {
                                        "Free models (OpenRouter):"
                                    }
                                }
                                p class="pl-3" {
                                    "• "
                                    strong class="text-green-400" {
                                        "NVIDIA Nemotron Nano VL"
                                    }
                                    " — #1 on OCR benchmarks, purpose-built for documents (current default)"
                                }
                                p class="pl-3" {
                                    "• "
                                    strong class="text-gray-300" {
                                        "Google Gemma 4 31B"
                                    }
                                    " — strong vision with 262K context for large documents"
                                }
                                p class="pl-3" {
                                    "• "
                                    strong class="text-gray-300" {
                                        "Google Gemma 4 26B"
                                    }
                                    " — lighter alternative, nearly as accurate"
                                }
                                p class="mt-1" {
                                    strong class="text-gray-400" {
                                        "Self-hosted (free, private, offline):"
                                    }
                                }
                                p class="pl-3" {
                                    "• "
                                    strong class="text-green-400" {
                                        "NVIDIA Nemotron Nano 12B VL"
                                    }
                                    " — same model as cloud default, runs locally on any GPU with 10+ GB VRAM. Q4_K_M quantization needs ~9 GB. Download GGUF from HuggingFace, run via llama.cpp. Cannot run alongside a large chat model — swap models when scanning."
                                }
                                p class="pl-3 text-gray-600" {
                                    "Setup: "
                                    code class="text-xs bg-gray-800 px-1 rounded" {
                                        "llama-server -m Nemotron-Nano-12B-v2-VL-Q4_K_M.gguf --mmproj mmproj-BF16.gguf --port 1237 -ngl 99"
                                    }
                                }
                                p class="mt-1" {
                                    strong class="text-gray-400" {
                                        "Paid cloud (highest accuracy):"
                                    }
                                }
                                p class="pl-3" {
                                    "• "
                                    strong class="text-gray-300" {
                                        "Anthropic Claude Sonnet"
                                    }
                                    " — best overall accuracy for complex tax documents"
                                }
                                p class="pl-3" {
                                    "• "
                                    strong class="text-gray-300" {
                                        "OpenAI GPT-4o"
                                    }
                                    " — excellent OCR, especially for handwritten notes"
                                }
                                p class="pl-3" {
                                    "• "
                                    strong class="text-gray-300" {
                                        "Qwen3 VL 235B"
                                    }
                                    " — top-tier vision, best value at scale"
                                }
                                p {
                                    "• Always verify extracted values against your original documents"
                                }
                            }
                        }
                        div class="flex items-center gap-3" {
                            label class="btn-primary cursor-pointer" {
                                " Upload Document "
                                input type="file" class="hidden" accept="image/*,.pdf" onchange="uploadTaxDoc(this)" id="doc-input";
                            }
                            span id="doc-upload-status" class="text-xs text-gray-500" {
                            }
                        }
                    }
                    div class="p-3 rounded-lg bg-yellow-900/20 border border-yellow-800/50 mb-4" {
                        div class="flex items-start gap-2" {
                            span class="text-yellow-400 mt-0.5" {
                                "⚠"
                            }
                            p class="text-xs text-yellow-300/80" {
                                "AI-extracted values may contain errors. "
                                strong {
                                    "Always verify amounts against your original documents"
                                }
                                " before filing. Click any value to correct it."
                            }
                        }
                    }
                    div class="card" {
                        h3 class="font-medium mb-4" {
                            "Tax Documents"
                        }
                        div id="doc-list" class="space-y-3" {
                            p class="text-xs text-gray-600" {
                                "Loading..."
                            }
                        }
                    }
                }
                // Property tab
                div id="tab-property" class="hidden" {
                    div class="card mb-6" {
                        h3 class="font-medium mb-4" {
                            "Property Profile"
                        }
                        p class="text-xs text-gray-500 mb-4" {
                            "Your property details for home office / workshop deduction calculations. Values auto-populate from scanned 1098s, settlement statements, and tax documents."
                        }
                        div class="grid grid-cols-2 gap-4" {
                            div {
                                label class="label" {
                                    "Address"
                                }
                                input type="text" id="prop-address" class="input" placeholder="1406 Summit Lake Shore";
                            }
                            div {
                                label class="label" {
                                    "Purchase Date"
                                }
                                input type="date" id="prop-purchase-date" class="input";
                            }
                            div {
                                label class="label" {
                                    "Total Sqft"
                                }
                                input type="number" id="prop-total-sqft" class="input" placeholder="5206";
                            }
                            div {
                                label class="label" {
                                    "Workshop Sqft"
                                }
                                input type="number" id="prop-workshop-sqft" class="input" placeholder="488";
                            }
                            div {
                                label class="label" {
                                    "Purchase Price"
                                }
                                input type="text" id="prop-purchase-price" class="input" placeholder="1060000";
                            }
                            div {
                                label class="label" {
                                    "Building Value (from assessor)"
                                }
                                input type="text" id="prop-building-value" class="input" placeholder="831762";
                            }
                            div {
                                label class="label" {
                                    "Land Value (from assessor)"
                                }
                                input type="text" id="prop-land-value" class="input" placeholder="228238";
                            }
                            div {
                                label class="label" {
                                    "Land Ratio"
                                }
                                input type="text" id="prop-land-ratio" class="input" placeholder="0.2153" readonly;
                            }
                        }
                        div class="grid grid-cols-2 gap-4 mt-4" {
                            div {
                                label class="label" {
                                    "Annual Property Tax"
                                }
                                input type="text" id="prop-property-tax" class="input" placeholder="571.60";
                            }
                            div {
                                label class="label" {
                                    "Annual Insurance (Homeowner's)"
                                }
                                input type="text" id="prop-insurance" class="input" placeholder="1680";
                            }
                            div {
                                label class="label" {
                                    "Mortgage Lender"
                                }
                                input type="text" id="prop-mortgage-lender" class="input" placeholder="Chase";
                            }
                            div {
                                label class="label" {
                                    "Annual Mortgage Interest (from 1098)"
                                }
                                input type="text" id="prop-mortgage-interest" class="input" placeholder="Will auto-fill from 1098";
                            }
                        }
                        div class="mt-4" {
                            label class="label" {
                                "Notes"
                            }
                            textarea id="prop-notes" class="input" rows="2" placeholder="e.g., Workshop used exclusively for woodworking business" {
                            }
                        }
                        div class="flex items-center gap-3 mt-4" {
                            button onclick="saveProperty()" class="btn-primary" {
                                "Save Property"
                            }
                            button onclick="autofillProperty()" class="text-xs text-oc-500 hover:text-oc-400" {
                                "Auto-fill from documents"
                            }
                            span id="prop-status" class="text-xs text-gray-500" {
                            }
                        }
                    }
                    // Depreciation Calculator
                    div class="card mb-6" id="depreciation-section" {
                        h3 class="font-medium mb-3" {
                            "Depreciation Calculator"
                        }
                        div id="depreciation-result" class="text-sm text-gray-400" {
                            p class="text-xs text-gray-600" {
                                "Save a property profile above to calculate depreciation."
                            }
                        }
                    }
                    // Statement Transactions
                    div class="card" {
                        div class="flex items-center justify-between mb-4" {
                            h3 class="font-medium" {
                                "Statement Transactions"
                            }
                            span class="text-xs text-gray-500" id="stmt-txn-count" {
                                "—"
                            }
                        }
                        p class="text-xs text-gray-500 mb-3" {
                            "Individual transactions extracted from uploaded bank/credit card statements."
                        }
                        div id="stmt-txn-list" class="space-y-1 max-h-96 overflow-y-auto" {
                            p class="text-xs text-gray-600" {
                                "No statement transactions yet. Upload a bank or credit card statement in the Documents tab."
                            }
                        }
                    }
                }
                // Deductions tab
                div id="tab-deductions" class="hidden" {
                    // Questionnaire section
                    div id="ded-questionnaire" class="card mb-4" {
                        div class="flex items-center justify-between mb-4" {
                            h3 class="font-medium text-sm" {
                                "Deduction Qualification"
                            }
                            span id="ded-quest-status" class="text-xs text-gray-500" {
                            }
                        }
                        div id="ded-quest-wizard" {
                            // Step 1
                            div class="ded-step" id="ded-step-1" {
                                p class="text-xs text-gray-500 mb-3" {
                                    "Step 1 of 3 — Filing & Employment"
                                }
                                div class="space-y-3" {
                                    div class="flex items-center justify-between py-2" {
                                        div {
                                            p class="text-sm text-gray-300" {
                                                "Filing status"
                                            }
                                        }
                                        select id="q-filing-status" onchange="saveQAnswer('filing_status', this.value)" class="bg-gray-900 border border-gray-700 rounded-lg px-3 py-1.5 text-sm text-white outline-none" {
                                            option value="single" {
                                                "Single"
                                            }
                                            option value="married_jointly" {
                                                "Married Filing Jointly"
                                            }
                                            option value="married_separately" {
                                                "Married Filing Separately"
                                            }
                                            option value="head_of_household" {
                                                "Head of Household"
                                            }
                                        }
                                    }
                                    div class="flex items-center justify-between py-2" {
                                        div {
                                            p class="text-sm text-gray-300" {
                                                "Self-employed / 1099 income?"
                                            }
                                            p class="text-xs text-gray-500" {
                                                "Enables SE tax, QBI deduction, business scanning"
                                            }
                                        }
                                        button onclick="toggleQAnswer('self_employed')" id="qtog-self_employed" class="w-10 h-5 rounded-full bg-gray-700 relative transition-colors flex-shrink-0" {
                                            span class="absolute left-0.5 top-0.5 w-4 h-4 rounded-full bg-gray-400 transition-transform" {
                                            }
                                        }
                                    }
                                    div class="flex items-center justify-between py-2" {
                                        div {
                                            p class="text-sm text-gray-300" {
                                                "Work from home?"
                                            }
                                            p class="text-xs text-gray-500" {
                                                "Enables home office utility scanning"
                                            }
                                        }
                                        button onclick="toggleQAnswer('home_office')" id="qtog-home_office" class="w-10 h-5 rounded-full bg-gray-700 relative transition-colors flex-shrink-0" {
                                            span class="absolute left-0.5 top-0.5 w-4 h-4 rounded-full bg-gray-400 transition-transform" {
                                            }
                                        }
                                    }
                                }
                                div class="flex justify-end mt-4" {
                                    button onclick="dedStep(2)" class="text-sm bg-oc-600 hover:bg-oc-700 text-white px-4 py-1.5 rounded-lg" {
                                        "Next"
                                    }
                                }
                            }
                            // Step 2
                            div class="ded-step hidden" id="ded-step-2" {
                                p class="text-xs text-gray-500 mb-3" {
                                    "Step 2 of 3 — Insurance & Health"
                                }
                                div class="space-y-3" {
                                    div class="flex items-center justify-between py-2" {
                                        div {
                                            p class="text-sm text-gray-300" {
                                                "Pay own health insurance?"
                                            }
                                            p class="text-xs text-gray-500" {
                                                "Not through an employer plan"
                                            }
                                        }
                                        button onclick="toggleQAnswer('health_insurance_self')" id="qtog-health_insurance_self" class="w-10 h-5 rounded-full bg-gray-700 relative transition-colors flex-shrink-0" {
                                            span class="absolute left-0.5 top-0.5 w-4 h-4 rounded-full bg-gray-400 transition-transform" {
                                            }
                                        }
                                    }
                                    div class="flex items-center justify-between py-2" {
                                        div {
                                            p class="text-sm text-gray-300" {
                                                "High-deductible health plan (HDHP)?"
                                            }
                                            p class="text-xs text-gray-500" {
                                                "Enables HSA contribution tracking"
                                            }
                                        }
                                        button onclick="toggleQAnswer('hdhp')" id="qtog-hdhp" class="w-10 h-5 rounded-full bg-gray-700 relative transition-colors flex-shrink-0" {
                                            span class="absolute left-0.5 top-0.5 w-4 h-4 rounded-full bg-gray-400 transition-transform" {
                                            }
                                        }
                                    }
                                }
                                div class="flex justify-between mt-4" {
                                    button onclick="dedStep(1)" class="text-sm text-gray-400 hover:text-white px-4 py-1.5 rounded-lg" {
                                        "Back"
                                    }
                                    button onclick="dedStep(3)" class="text-sm bg-oc-600 hover:bg-oc-700 text-white px-4 py-1.5 rounded-lg" {
                                        "Next"
                                    }
                                }
                            }
                            // Step 3
                            div class="ded-step hidden" id="ded-step-3" {
                                p class="text-xs text-gray-500 mb-3" {
                                    "Step 3 of 3 — Deductions"
                                }
                                div class="space-y-3" {
                                    div class="flex items-center justify-between py-2" {
                                        div {
                                            p class="text-sm text-gray-300" {
                                                "Use a vehicle for business?"
                                            }
                                        }
                                        button onclick="toggleQAnswer('vehicle_business')" id="qtog-vehicle_business" class="w-10 h-5 rounded-full bg-gray-700 relative transition-colors flex-shrink-0" {
                                            span class="absolute left-0.5 top-0.5 w-4 h-4 rounded-full bg-gray-400 transition-transform" {
                                            }
                                        }
                                    }
                                    div class="flex items-center justify-between py-2" {
                                        div {
                                            p class="text-sm text-gray-300" {
                                                "Contribute to retirement accounts?"
                                            }
                                            p class="text-xs text-gray-500" {
                                                "SEP-IRA, Solo 401(k), Traditional IRA"
                                            }
                                        }
                                        button onclick="toggleQAnswer('retirement_contributions')" id="qtog-retirement_contributions" class="w-10 h-5 rounded-full bg-gray-700 relative transition-colors flex-shrink-0" {
                                            span class="absolute left-0.5 top-0.5 w-4 h-4 rounded-full bg-gray-400 transition-transform" {
                                            }
                                        }
                                    }
                                    div class="flex items-center justify-between py-2" {
                                        div {
                                            p class="text-sm text-gray-300" {
                                                "Number of dependents"
                                            }
                                        }
                                        input type="number" min="0" max="10" value="0" id="q-dependents" onchange="saveQAnswer('dependents', parseInt(this.value)||0)" class="w-16 bg-gray-900 border border-gray-700 rounded-lg px-2 py-1 text-sm text-white text-center outline-none";
                                    }
                                    div class="flex items-center justify-between py-2" {
                                        div {
                                            p class="text-sm text-gray-300" {
                                                "Paid student loan interest?"
                                            }
                                        }
                                        button onclick="toggleQAnswer('student_loan_interest')" id="qtog-student_loan_interest" class="w-10 h-5 rounded-full bg-gray-700 relative transition-colors flex-shrink-0" {
                                            span class="absolute left-0.5 top-0.5 w-4 h-4 rounded-full bg-gray-400 transition-transform" {
                                            }
                                        }
                                    }
                                    div class="flex items-center justify-between py-2" {
                                        div {
                                            p class="text-sm text-gray-300" {
                                                "Made charitable donations?"
                                            }
                                        }
                                        button onclick="toggleQAnswer('charitable_donations')" id="qtog-charitable_donations" class="w-10 h-5 rounded-full bg-gray-700 relative transition-colors flex-shrink-0" {
                                            span class="absolute left-0.5 top-0.5 w-4 h-4 rounded-full bg-gray-400 transition-transform" {
                                            }
                                        }
                                    }
                                }
                                div class="flex justify-between mt-4" {
                                    button onclick="dedStep(2)" class="text-sm text-gray-400 hover:text-white px-4 py-1.5 rounded-lg" {
                                        "Back"
                                    }
                                    button onclick="completeQuestionnaire()" class="text-sm bg-green-600 hover:bg-green-700 text-white px-4 py-1.5 rounded-lg font-medium" {
                                        "Complete & Scan"
                                    }
                                }
                            }
                        }
                        // Collapsed summary (shown when complete)
                        div id="ded-quest-summary" class="hidden" {
                            div id="ded-quest-tags" class="flex flex-wrap gap-2" {
                            }
                            button onclick="editQuestionnaire()" class="text-xs text-oc-500 hover:text-oc-400 mt-2" {
                                "Edit answers"
                            }
                        }
                    }
                    // Scan status
                    div class="card mb-4" {
                        div class="flex items-center justify-between mb-3" {
                            h3 class="font-medium text-sm" {
                                "Deduction Scanner"
                            }
                            div class="flex gap-2" {
                                button onclick="triggerScan()" class="text-xs bg-gray-700 hover:bg-gray-600 text-gray-300 px-3 py-1 rounded-lg" id="scan-btn" {
                                    "Quick Scan"
                                }
                                button onclick="triggerDeepScan()" class="text-xs bg-oc-600 hover:bg-oc-700 text-white px-3 py-1 rounded-lg" id="deep-scan-btn" {
                                    "AI Deep Scan"
                                }
                            }
                        }
                        p id="scan-status-msg" class="text-xs text-gray-500 mb-2 hidden" {
                        }
                        div class="flex gap-4 text-sm" {
                            div {
                                span id="ded-pending" class="text-yellow-400 font-medium" {
                                    "0"
                                }
                                span class="text-gray-500" {
                                    "pending"
                                }
                            }
                            div {
                                span id="ded-approved" class="text-green-400 font-medium" {
                                    "0"
                                }
                                span class="text-gray-500" {
                                    "approved"
                                }
                            }
                            div {
                                span id="ded-denied" class="text-gray-500 font-medium" {
                                    "0"
                                }
                                span class="text-gray-500" {
                                    "denied"
                                }
                            }
                            div class="ml-auto" {
                                span id="ded-saved" class="text-green-400 font-medium" {
                                    "$0.00"
                                }
                                span class="text-gray-500" {
                                    "in deductions found"
                                }
                            }
                        }
                    }
                    // Review queue
                    div id="ded-review-wrapper" {
                        // Normal: full list. Review mode: split panel
                        div id="ded-list-mode" {
                            div class="flex items-center justify-between mb-3" {
                                div class="flex gap-2" {
                                    button onclick="filterDedStatus('pending')" class="text-xs px-2 py-1 rounded-lg bg-gray-700 text-yellow-400" id="ded-filter-pending" {
                                        "Pending"
                                    }
                                    button onclick="filterDedStatus('approved')" class="text-xs px-2 py-1 rounded-lg text-gray-500 hover:text-gray-300" id="ded-filter-approved" {
                                        "Approved"
                                    }
                                    button onclick="filterDedStatus('denied')" class="text-xs px-2 py-1 rounded-lg text-gray-500 hover:text-gray-300" id="ded-filter-denied" {
                                        "Denied"
                                    }
                                }
                                select onchange="filterDedType(this.value)" class="bg-gray-900 border border-gray-700 rounded-lg px-2 py-1 text-xs text-gray-400 outline-none" id="ded-type-filter" {
                                    option value="" {
                                        "All types"
                                    }
                                    option value="medical" {
                                        "Medical"
                                    }
                                    option value="health_insurance" {
                                        "Health Insurance"
                                    }
                                    option value="vehicle" {
                                        "Vehicle"
                                    }
                                    option value="home_office" {
                                        "Home Office"
                                    }
                                    option value="software" {
                                        "Software"
                                    }
                                    option value="education" {
                                        "Education"
                                    }
                                    option value="charitable" {
                                        "Charitable"
                                    }
                                    option value="professional" {
                                        "Professional"
                                    }
                                    option value="retirement" {
                                        "Retirement"
                                    }
                                    option value="student_loan" {
                                        "Student Loan"
                                    }
                                    option value="hsa" {
                                        "HSA"
                                    }
                                }
                            }
                            div id="ded-candidates-list" class="space-y-1.5" {
                                p class="text-xs text-gray-600 text-center py-8" {
                                    "Complete the questionnaire above to scan for deductions."
                                }
                            }
                        }
                        // Review split panel
                        div id="ded-review-mode" class="hidden flex gap-3" style="height:calc(100vh - 340px)" {
                            div class="w-[35%] overflow-y-auto border-r border-gray-800 pr-3 space-y-1" id="ded-review-list" {
                            }
                            div class="flex-1 flex flex-col" {
                                div class="flex-1 overflow-y-auto bg-gray-900 rounded-lg border border-gray-700 p-3 mb-3" id="ded-viewer" {
                                    p class="text-xs text-gray-500 text-center py-8" {
                                        "Select a candidate to review"
                                    }
                                }
                                div class="flex items-center gap-3 p-3 bg-gray-800 rounded-lg border border-gray-700" {
                                    select id="review-category" class="bg-gray-900 border border-gray-700 rounded-lg px-2 py-1.5 text-sm text-white outline-none" {
                                    }
                                    select id="review-entity" class="bg-gray-900 border border-gray-700 rounded-lg px-2 py-1.5 text-sm text-white outline-none" {
                                        option value="business" {
                                            "Business"
                                        }
                                        option value="personal" {
                                            "Personal"
                                        }
                                    }
                                    div class="flex-1" {
                                    }
                                    span class="text-xs text-gray-600" id="review-keys" {
                                        "j/k navigate · a approve · d deny"
                                    }
                                    button onclick="reviewAction('deny')" class="px-4 py-1.5 bg-gray-700 hover:bg-gray-600 text-gray-300 rounded-lg text-sm" {
                                        "Deny"
                                    }
                                    button onclick="reviewAction('approve')" class="px-4 py-1.5 bg-green-600 hover:bg-green-700 text-white rounded-lg text-sm font-medium" {
                                        "Approve"
                                    }
                                }
                            }
                        }
                    }
                }
                // Wizard tab
                div id="tab-wizard" class="hidden" {
                    div class="card mb-6" {
                        div class="flex items-center justify-between mb-4" {
                            h3 class="font-medium" {
                                "Tax Prep Wizard — "
                                span id="wizard-year" {
                                    "2025"
                                }
                            }
                            div class="flex items-center gap-2" {
                                span class="text-xs text-gray-500" {
                                    "Completeness:"
                                }
                                div class="w-32 h-2 rounded-full bg-gray-700 overflow-hidden" {
                                    div id="wizard-progress-bar" class="h-full rounded-full bg-green-500 transition-all" style="width:0%" {
                                    }
                                }
                                span id="wizard-pct" class="text-xs font-medium text-green-400" {
                                    "0%"
                                }
                            }
                        }
                        div id="wizard-steps" class="space-y-3" {
                            p class="text-xs text-gray-600" {
                                "Loading wizard..."
                            }
                        }
                    }
                    // Missing Items
                    div class="card mb-6" id="wizard-missing-section" style="display:none" {
                        h3 class="font-medium mb-3 text-yellow-400" {
                            "Missing Items"
                        }
                        div id="wizard-missing" class="space-y-2" {
                        }
                    }
                    // Tax Summary
                    div class="card" {
                        h3 class="font-medium mb-4" {
                            "Estimated Tax Summary"
                        }
                        div id="wizard-summary" class="space-y-2 text-sm" {
                            p class="text-xs text-gray-600" {
                                "Loading..."
                            }
                        }
                    }
                }
                // Connections tab
                div id="tab-connections" class="hidden" {
                    div class="card mb-6" {
                        h3 class="font-medium mb-3" {
                            "Connect Bank Account"
                        }
                        p class="text-xs text-gray-500 mb-4" {
                            "Link your bank or credit card via Plaid for automatic transaction imports. Your credentials are never stored — Plaid handles authentication directly with your bank."
                        }
                        div class="flex items-center gap-3" {
                            button onclick="launchPlaidLink()" class="btn-primary" id="plaid-link-btn" {
                                "Link Bank Account"
                            }
                            span id="plaid-status" class="text-xs text-gray-500" {
                            }
                        }
                    }
                    div class="card mb-6" {
                        h3 class="font-medium mb-3" {
                            "Connect via SimpleFIN"
                        }
                        p class="text-xs text-gray-500 mb-4" {
                            "Alternative to Plaid ($15/yr). Get a setup token from "
                            a href="https://beta-bridge.simplefin.org" target="_blank" class="text-oc-500 hover:text-oc-400" {
                                "SimpleFIN Bridge"
                            }
                            ", then paste it here."
                        }
                        div class="flex items-center gap-3" {
                            input type="text" id="simplefin-token" class="input flex-1" placeholder="Paste SimpleFIN setup token...";
                            button onclick="connectSimpleFIN()" class="btn-primary" {
                                "Connect"
                            }
                        }
                        span id="simplefin-status" class="text-xs text-gray-500 mt-2 block" {
                        }
                    }
                    div class="card mb-6" {
                        div class="flex items-center justify-between mb-4" {
                            h3 class="font-medium" {
                                "Connected Accounts"
                            }
                            button onclick="loadConnections()" class="text-xs text-oc-500 hover:text-oc-400" {
                                "Refresh"
                            }
                        }
                        div id="connections-list" class="space-y-3" {
                            p class="text-xs text-gray-600" {
                                "No accounts connected yet."
                            }
                        }
                    }
                    div class="card" {
                        h3 class="font-medium mb-3" {
                            "Email Receipt Scanning"
                        }
                        p class="text-xs text-gray-500 mb-4" {
                            "Connect Gmail to automatically find and import receipts from purchase confirmation emails."
                        }
                        div class="flex items-center gap-3" {
                            button onclick="connectGmail()" class="btn-primary" {
                                "Connect Gmail"
                            }
                            span id="gmail-status" class="text-xs text-gray-500" {
                            }
                        }
                        div id="email-connections-list" class="mt-4 space-y-2" {
                        }
                    }
                }
                // Investments tab — default landing (most-viewed section)
                div id="tab-investments" {
                    div class="card mb-6" {
                        h3 class="font-medium mb-3" {
                            "Connect Brokerage"
                        }
                        p class="text-xs text-gray-500 mb-4" {
                            "Connect your Alpaca account to import trades, dividends, and portfolio data."
                        }
                        div class="grid grid-cols-2 gap-3" {
                            div {
                                label class="label" {
                                    "API Key"
                                }
                                input type="text" id="alpaca-key" class="input" placeholder="PK...";
                            }
                            div {
                                label class="label" {
                                    "API Secret"
                                }
                                input type="password" id="alpaca-secret" class="input" placeholder="Secret key";
                            }
                            div {
                                label class="label" {
                                    "Nickname"
                                }
                                input type="text" id="alpaca-nickname" class="input" placeholder="e.g. Main Trading";
                            }
                            div {
                                label class="label" {
                                    "Environment"
                                }
                                select id="alpaca-env" class="input" {
                                    option value="https://api.alpaca.markets" {
                                        "Live"
                                    }
                                    option value="https://paper-api.alpaca.markets" {
                                        "Paper"
                                    }
                                }
                            }
                        }
                        div class="mt-3 flex items-center gap-3" {
                            button onclick="connectAlpaca()" class="btn-primary" {
                                "Connect Alpaca"
                            }
                            span id="alpaca-status" class="text-xs text-gray-500" {
                            }
                        }
                    }
                    div class="card mb-6" {
                        h3 class="font-medium mb-3" {
                            "Connect Crypto Exchange"
                        }
                        p class="text-xs text-gray-500 mb-4" {
                            "Import crypto trades from Coinbase for capital gains tracking."
                        }
                        div class="grid grid-cols-2 gap-3" {
                            div {
                                label class="label" {
                                    "API Key"
                                }
                                input type="text" id="coinbase-key" class="input" placeholder="API key";
                            }
                            div {
                                label class="label" {
                                    "API Secret"
                                }
                                input type="password" id="coinbase-secret" class="input" placeholder="API secret";
                            }
                        }
                        div class="mt-3 flex items-center gap-3" {
                            button onclick="connectCoinbase()" class="btn-primary" {
                                "Connect Coinbase"
                            }
                            span id="coinbase-status" class="text-xs text-gray-500" {
                            }
                        }
                    }
                    div class="grid grid-cols-2 md:grid-cols-4 gap-3 mb-6" {
                        div class="card" {
                            p class="text-xs text-gray-500 uppercase" {
                                "Short-Term Gains"
                            }
                            p class="text-2xl font-semibold mt-1" id="inv-short-term" {
                                "--"
                            }
                        }
                        div class="card" {
                            p class="text-xs text-gray-500 uppercase" {
                                "Long-Term Gains"
                            }
                            p class="text-2xl font-semibold mt-1 text-oc-500" id="inv-long-term" {
                                "--"
                            }
                        }
                        div class="card" {
                            p class="text-xs text-gray-500 uppercase" {
                                "Dividends"
                            }
                            p class="text-2xl font-semibold mt-1 text-green-400" id="inv-dividends" {
                                "--"
                            }
                        }
                        div class="card" {
                            p class="text-xs text-gray-500 uppercase" {
                                "Net P/L"
                            }
                            p class="text-2xl font-semibold mt-1" id="inv-net-pl" {
                                "--"
                            }
                        }
                    }
                    div class="card mb-6" {
                        div class="flex items-center justify-between mb-4" {
                            h3 class="font-medium" {
                                "Connected Accounts"
                            }
                            button onclick="loadInvestmentAccounts()" class="text-xs text-oc-500 hover:text-oc-400" {
                                "Refresh"
                            }
                        }
                        div id="inv-accounts-list" class="space-y-2" {
                            p class="text-xs text-gray-600" {
                                "No brokerage accounts connected yet."
                            }
                        }
                    }
                    div class="card mb-6" {
                        div class="flex items-center justify-between mb-4" {
                            h3 class="font-medium" {
                                "Holdings"
                            }
                            span class="text-xs text-gray-500" id="holdings-as-of" {
                                "--"
                            }
                        }
                        div id="holdings-list" class="space-y-1" {
                            p class="text-xs text-gray-600" {
                                "Connect a brokerage to see holdings."
                            }
                        }
                    }
                    div class="card mb-6" {
                        div class="flex items-center justify-between mb-4" {
                            h3 class="font-medium" {
                                "Investment Transactions"
                            }
                            div class="flex items-center gap-2" {
                                select id="inv-filter-type" class="input w-auto" onchange="loadInvestmentTransactions()" {
                                    option value="" {
                                        "All"
                                    }
                                    option value="fill" {
                                        "Trades"
                                    }
                                    option value="dividend" {
                                        "Dividends"
                                    }
                                }
                                select id="inv-filter-broker" class="input w-auto" onchange="loadInvestmentTransactions()" {
                                    option value="" {
                                        "All"
                                    }
                                    option value="alpaca" {
                                        "Alpaca"
                                    }
                                    option value="coinbase" {
                                        "Coinbase"
                                    }
                                }
                                span class="text-xs text-gray-500" id="inv-txn-count" {
                                    "--"
                                }
                            }
                        }
                        div id="inv-txn-list" class="space-y-1 max-h-[500px] overflow-y-auto" {
                            p class="text-xs text-gray-600" {
                                "No transactions yet."
                            }
                        }
                    }
                    div class="card mb-6" {
                        h3 class="font-medium mb-3" {
                            "Capital Gains Summary"
                        }
                        div class="grid grid-cols-2 md:grid-cols-4 gap-3" id="cap-gains-grid" {
                            p class="text-xs text-gray-600 col-span-full" {
                                "Loading..."
                            }
                        }
                        p class="text-xs text-gray-500 mt-3" id="cap-gains-note" {
                        }
                    }
                    div class="card mb-6" {
                        div class="flex items-center justify-between mb-3" {
                            h3 class="font-medium" {
                                "Tax Lots (Open Positions)"
                            }
                            div class="flex items-center gap-2" {
                                select id="lots-status" class="input w-auto" onchange="loadLots()" {
                                    option value="open" {
                                        "Open"
                                    }
                                    option value="closed" {
                                        "Closed"
                                    }
                                }
                                button onclick="showAddLotForm()" class="text-xs bg-oc-600 hover:bg-oc-700 text-white px-2.5 py-1 rounded-lg" {
                                    "+ Add Lot"
                                }
                            }
                        }
                        div id="add-lot-form" class="hidden mb-4 p-3 bg-gray-900/40 rounded-lg" {
                            div class="grid grid-cols-2 md:grid-cols-4 gap-2" {
                                div {
                                    label class="label" {
                                        "Symbol"
                                    }
                                    input id="lot-symbol" class="input" placeholder="AAPL";
                                }
                                div {
                                    label class="label" {
                                        "Type"
                                    }
                                    select id="lot-asset-type" class="input" {
                                        option value="stock" {
                                            "Stock"
                                        }
                                        option value="etf" {
                                            "ETF"
                                        }
                                        option value="crypto" {
                                            "Crypto"
                                        }
                                        option value="option" {
                                            "Option"
                                        }
                                    }
                                }
                                div {
                                    label class="label" {
                                        "Quantity"
                                    }
                                    input id="lot-qty" type="number" step="any" class="input" placeholder="100";
                                }
                                div {
                                    label class="label" {
                                        "Cost per Unit ($)"
                                    }
                                    input id="lot-cpu" type="number" step="0.01" class="input" placeholder="150.25";
                                }
                                div {
                                    label class="label" {
                                        "Acquisition Date"
                                    }
                                    input id="lot-date" type="date" class="input";
                                }
                                div {
                                    label class="label" {
                                        "Broker"
                                    }
                                    input id="lot-broker" class="input" placeholder="Alpaca";
                                }
                                div class="col-span-2 flex items-end gap-2" {
                                    button onclick="saveLot()" class="btn-primary" {
                                        "Save Lot"
                                    }
                                    button onclick="document.getElementById('add-lot-form').classList.add('hidden')" class="text-xs text-gray-400 hover:text-gray-200" {
                                        "Cancel"
                                    }
                                }
                            }
                        }
                        div id="lots-list" class="space-y-1 max-h-[400px] overflow-y-auto" {
                            p class="text-xs text-gray-600" {
                                "Loading lots..."
                            }
                        }
                    }
                    div class="card mb-6" {
                        div class="flex items-center justify-between mb-3" {
                            h3 class="font-medium" {
                                "Wash Sales"
                            }
                            span class="text-xs text-gray-500" id="wash-count" {
                                "--"
                            }
                        }
                        div id="wash-list" class="space-y-1" {
                            p class="text-xs text-gray-600" {
                                "Loading..."
                            }
                        }
                    }
                    div class="card mb-6" {
                        div class="flex items-center justify-between mb-3" {
                            h3 class="font-medium" {
                                "Form 8949 (Capital Gains Detail)"
                            }
                            span class="text-xs text-gray-500" id="form8949-count" {
                                "--"
                            }
                        }
                        div id="form8949-list" class="space-y-1 max-h-[300px] overflow-y-auto" {
                            p class="text-xs text-gray-600" {
                                "Loading dispositions..."
                            }
                        }
                    }
                    div class="card" {
                        div class="flex items-center justify-between mb-3" {
                            h3 class="font-medium" {
                                "K-1 Income (Partnerships, S-Corps)"
                            }
                            button onclick="showAddK1Form()" class="text-xs bg-oc-600 hover:bg-oc-700 text-white px-2.5 py-1 rounded-lg" {
                                "+ Add K-1"
                            }
                        }
                        div id="add-k1-form" class="hidden mb-4 p-3 bg-gray-900/40 rounded-lg" {
                            div class="grid grid-cols-2 md:grid-cols-3 gap-2" {
                                div class="col-span-2" {
                                    label class="label" {
                                        "Entity Name"
                                    }
                                    input id="k1-name" class="input" placeholder="Acme Partners LLC";
                                }
                                div {
                                    label class="label" {
                                        "Type"
                                    }
                                    select id="k1-type" class="input" {
                                        option value="partnership" {
                                            "Partnership"
                                        }
                                        option value="s_corp" {
                                            "S-Corp"
                                        }
                                    }
                                }
                                div {
                                    label class="label" {
                                        "Ordinary Income ($)"
                                    }
                                    input id="k1-ordinary" type="number" step="0.01" class="input" placeholder="0";
                                }
                                div {
                                    label class="label" {
                                        "Rental Income ($)"
                                    }
                                    input id="k1-rental" type="number" step="0.01" class="input" placeholder="0";
                                }
                                div {
                                    label class="label" {
                                        "Interest ($)"
                                    }
                                    input id="k1-interest" type="number" step="0.01" class="input" placeholder="0";
                                }
                                div {
                                    label class="label" {
                                        "Dividends ($)"
                                    }
                                    input id="k1-dividend" type="number" step="0.01" class="input" placeholder="0";
                                }
                                div {
                                    label class="label" {
                                        "Capital Gain ($)"
                                    }
                                    input id="k1-capgain" type="number" step="0.01" class="input" placeholder="0";
                                }
                                div {
                                    label class="label" {
                                        "SE Income ($)"
                                    }
                                    input id="k1-se" type="number" step="0.01" class="input" placeholder="0";
                                }
                                div class="col-span-full flex gap-2" {
                                    button onclick="saveK1()" class="btn-primary" {
                                        "Save K-1"
                                    }
                                    button onclick="document.getElementById('add-k1-form').classList.add('hidden')" class="text-xs text-gray-400 hover:text-gray-200" {
                                        "Cancel"
                                    }
                                }
                            }
                        }
                        div id="k1-list" class="space-y-1" {
                            p class="text-xs text-gray-600" {
                                "Loading K-1s..."
                            }
                        }
                    }
                }
                // Credits tab
                div id="tab-credits" class="hidden" {
                    div class="card mb-6" {
                        h3 class="font-medium mb-2" {
                            "Tax Credit Eligibility"
                        }
                        p class="text-xs text-gray-500 mb-4" {
                            "Credits directly reduce your tax owed, dollar for dollar. Based on your dependents, income, and expenses entered so far."
                        }
                        div class="grid grid-cols-1 md:grid-cols-2 gap-3" id="credits-eligibility-grid" {
                            p class="text-xs text-gray-600 col-span-full" {
                                "Loading credit eligibility..."
                            }
                        }
                        div class="mt-4 pt-4 border-t border-gray-700 flex items-center justify-between" {
                            span class="text-sm text-gray-400" {
                                "Estimated total credits:"
                            }
                            span class="text-xl font-semibold text-green-400" id="credits-total" {
                                "$0.00"
                            }
                        }
                    }
                    div class="card mb-6" {
                        div class="flex items-center justify-between mb-3" {
                            h3 class="font-medium" {
                                "Education Expenses "
                                span class="text-xs text-gray-500 font-normal" {
                                    "— AOTC / Lifetime Learning"
                                }
                            }
                            button onclick="toggleForm('edu-form')" class="text-xs bg-oc-600 hover:bg-oc-700 text-white px-2.5 py-1 rounded-lg" {
                                "+ Add"
                            }
                        }
                        div id="edu-form" class="hidden mb-3 p-3 bg-gray-900/40 rounded-lg" {
                            div class="grid grid-cols-2 gap-2" {
                                div {
                                    label class="label" {
                                        "Student Name"
                                    }
                                    input id="edu-student" class="input" placeholder="Jane Doe";
                                }
                                div {
                                    label class="label" {
                                        "Institution"
                                    }
                                    input id="edu-school" class="input" placeholder="Acme University";
                                }
                                div {
                                    label class="label" {
                                        "Tuition ($)"
                                    }
                                    input id="edu-tuition" type="number" step="0.01" class="input" placeholder="12000";
                                }
                                div {
                                    label class="label" {
                                        "Required Fees ($)"
                                    }
                                    input id="edu-fees" type="number" step="0.01" class="input" placeholder="0";
                                }
                                div {
                                    label class="label" {
                                        "Books & Supplies ($)"
                                    }
                                    input id="edu-books" type="number" step="0.01" class="input" placeholder="0";
                                }
                                div class="flex items-end gap-2" {
                                    button onclick="saveEducation()" class="btn-primary" {
                                        "Save"
                                    }
                                }
                            }
                        }
                        p class="text-xs text-gray-500" {
                            "Up to $2,500 credit per student for first 4 years of college (AOTC), or 20% of first $10,000 for graduate/continuing (LLC)."
                        }
                    }
                    div class="card mb-6" {
                        div class="flex items-center justify-between mb-3" {
                            h3 class="font-medium" {
                                "Dependent Care "
                                span class="text-xs text-gray-500 font-normal" {
                                    "— Child & Dependent Care Credit"
                                }
                            }
                            button onclick="toggleForm('care-form')" class="text-xs bg-oc-600 hover:bg-oc-700 text-white px-2.5 py-1 rounded-lg" {
                                "+ Add"
                            }
                        }
                        div id="care-form" class="hidden mb-3 p-3 bg-gray-900/40 rounded-lg" {
                            div class="grid grid-cols-2 gap-2" {
                                div {
                                    label class="label" {
                                        "Provider Name"
                                    }
                                    input id="care-provider" class="input" placeholder="Sunshine Daycare";
                                }
                                div {
                                    label class="label" {
                                        "Amount Paid ($)"
                                    }
                                    input id="care-amount" type="number" step="0.01" class="input" placeholder="5000";
                                }
                                div {
                                    label class="label" {
                                        "For Dependent (optional id)"
                                    }
                                    input id="care-dep" type="number" class="input" placeholder="";
                                }
                                div class="flex items-end gap-2" {
                                    button onclick="saveChildcare()" class="btn-primary" {
                                        "Save"
                                    }
                                }
                            }
                        }
                        p class="text-xs text-gray-500" {
                            "20-35% credit on up to $3,000 (one child) or $6,000 (two+) of care expenses for children under 13 or disabled dependents."
                        }
                    }
                    div class="card" {
                        div class="flex items-center justify-between mb-3" {
                            h3 class="font-medium" {
                                "Energy Improvements "
                                span class="text-xs text-gray-500 font-normal" {
                                    "— Residential Clean Energy / Efficient Home"
                                }
                            }
                            button onclick="toggleForm('energy-form')" class="text-xs bg-oc-600 hover:bg-oc-700 text-white px-2.5 py-1 rounded-lg" {
                                "+ Add"
                            }
                        }
                        div id="energy-form" class="hidden mb-3 p-3 bg-gray-900/40 rounded-lg" {
                            div class="grid grid-cols-2 gap-2" {
                                div {
                                    label class="label" {
                                        "Type"
                                    }
                                    select id="energy-type" class="input" {
                                        option value="solar" {
                                            "Solar panels / solar water heater"
                                        }
                                        option value="wind" {
                                            "Small wind"
                                        }
                                        option value="geothermal" {
                                            "Geothermal heat pump"
                                        }
                                        option value="battery" {
                                            "Battery storage"
                                        }
                                        option value="heat_pump" {
                                            "Heat pump"
                                        }
                                        option value="windows" {
                                            "Windows/doors/insulation"
                                        }
                                        option value="ev_charger" {
                                            "EV charger"
                                        }
                                    }
                                }
                                div {
                                    label class="label" {
                                        "Vendor"
                                    }
                                    input id="energy-vendor" class="input" placeholder="Installer name";
                                }
                                div {
                                    label class="label" {
                                        "Total Cost ($)"
                                    }
                                    input id="energy-cost" type="number" step="0.01" class="input" placeholder="15000";
                                }
                                div {
                                    label class="label" {
                                        "Qualifying Portion ($)"
                                    }
                                    input id="energy-qual" type="number" step="0.01" class="input" placeholder="leave blank = full";
                                }
                                div class="flex items-end gap-2" {
                                    button onclick="saveEnergy()" class="btn-primary" {
                                        "Save"
                                    }
                                }
                            }
                        }
                        p class="text-xs text-gray-500" {
                            "30% residential clean energy credit (solar, wind, geothermal). Energy efficient home improvement credit up to $1,200-$3,200/year depending on type."
                        }
                    }
                }
                // Quarterly tab
                div id="tab-quarterly" class="hidden" {
                    div class="grid grid-cols-1 md:grid-cols-3 gap-4 mb-6" {
                        div class="card" {
                            p class="text-xs text-gray-500 uppercase" {
                                "Projected Full-Year Tax"
                            }
                            p class="text-2xl font-semibold mt-1" id="qtr-projected-tax" {
                                "--"
                            }
                            p class="text-xs text-gray-500 mt-1" id="qtr-effective-rate" {
                            }
                        }
                        div class="card" {
                            p class="text-xs text-gray-500 uppercase" {
                                "Projected Owed at Year End"
                            }
                            p class="text-2xl font-semibold mt-1" id="qtr-owed" {
                                "--"
                            }
                            p class="text-xs text-gray-500 mt-1" {
                                "Before estimated payments"
                            }
                        }
                        div class="card" {
                            p class="text-xs text-gray-500 uppercase" {
                                "Per-Quarter Recommended"
                            }
                            p class="text-2xl font-semibold mt-1 text-oc-400" id="qtr-per-quarter" {
                                "--"
                            }
                            p class="text-xs text-gray-500 mt-1" id="qtr-safe-harbor" {
                            }
                        }
                    }
                    div class="card mb-6" {
                        h3 class="font-medium mb-3" {
                            "Quarterly Estimated Payments (Form 1040-ES)"
                        }
                        div id="qtr-list" class="space-y-2" {
                            p class="text-xs text-gray-600" {
                                "Loading..."
                            }
                        }
                        p class="text-xs text-gray-500 mt-3" {
                            "Deadlines: Q1 Apr 15, Q2 Jun 15, Q3 Sep 15, Q4 Jan 15. Safe harbor: pay 100% of prior year's tax (110% if AGI > $150K) OR 90% of current year."
                        }
                    }
                    div class="card" {
                        h3 class="font-medium mb-3" {
                            "Record a Payment"
                        }
                        div class="grid grid-cols-2 md:grid-cols-5 gap-2" {
                            div {
                                label class="label" {
                                    "Quarter"
                                }
                                select id="qtr-q" class="input" {
                                    option value="1" {
                                        "Q1"
                                    }
                                    option value="2" {
                                        "Q2"
                                    }
                                    option value="3" {
                                        "Q3"
                                    }
                                    option value="4" {
                                        "Q4"
                                    }
                                }
                            }
                            div {
                                label class="label" {
                                    "Amount ($)"
                                }
                                input id="qtr-amt" type="number" step="0.01" class="input";
                            }
                            div {
                                label class="label" {
                                    "Payment Date"
                                }
                                input id="qtr-date" type="date" class="input";
                            }
                            div {
                                label class="label" {
                                    "Method"
                                }
                                select id="qtr-method" class="input" {
                                    option value="IRS Direct Pay" {
                                        "IRS Direct Pay"
                                    }
                                    option value="EFTPS" {
                                        "EFTPS"
                                    }
                                    option value="Check" {
                                        "Check"
                                    }
                                    option value="Credit Card" {
                                        "Credit Card"
                                    }
                                }
                            }
                            div {
                                label class="label" {
                                    "Confirmation #"
                                }
                                input id="qtr-conf" class="input" placeholder="EFTPS#";
                            }
                            div class="col-span-full" {
                                button onclick="saveEstimatedPayment()" class="btn-primary" {
                                    "Record Payment"
                                }
                            }
                        }
                    }
                }
                // Depreciation tab
                div id="tab-depreciation" class="hidden" {
                    div class="card mb-6" {
                        div class="flex items-center justify-between mb-3" {
                            h3 class="font-medium" {
                                "Depreciable Assets"
                            }
                            button onclick="toggleForm('asset-form')" class="text-xs bg-oc-600 hover:bg-oc-700 text-white px-2.5 py-1 rounded-lg" {
                                "+ Add Asset"
                            }
                        }
                        div id="asset-form" class="hidden mb-4 p-3 bg-gray-900/40 rounded-lg" {
                            div class="grid grid-cols-2 md:grid-cols-3 gap-2" {
                                div class="col-span-2" {
                                    label class="label" {
                                        "Description"
                                    }
                                    input id="asset-desc" class="input" placeholder="Dell XPS laptop";
                                }
                                div {
                                    label class="label" {
                                        "Asset Class"
                                    }
                                    select id="asset-class" class="input" onchange="updateAssetLife()" {
                                        option value="computer" data-life="5" {
                                            "Computer / Software (5yr)"
                                        }
                                        option value="office_equipment" data-life="7" {
                                            "Office Equipment (7yr)"
                                        }
                                        option value="vehicle" data-life="5" {
                                            "Vehicle (5yr)"
                                        }
                                        option value="machinery" data-life="7" {
                                            "Machinery (7yr)"
                                        }
                                        option value="furniture" data-life="7" {
                                            "Furniture (7yr)"
                                        }
                                        option value="improvement" data-life="15" {
                                            "Leasehold Improvement (15yr)"
                                        }
                                        option value="building_residential" data-life="27" {
                                            "Residential Rental Bldg (27.5yr)"
                                        }
                                        option value="building_commercial" data-life="39" {
                                            "Commercial Building (39yr)"
                                        }
                                    }
                                }
                                div {
                                    label class="label" {
                                        "MACRS Life (yrs)"
                                    }
                                    input id="asset-life" type="number" class="input" value="5";
                                }
                                div {
                                    label class="label" {
                                        "Cost Basis ($)"
                                    }
                                    input id="asset-cost" type="number" step="0.01" class="input" placeholder="2500";
                                }
                                div {
                                    label class="label" {
                                        "Placed in Service"
                                    }
                                    input id="asset-date" type="date" class="input";
                                }
                                div {
                                    label class="label" {
                                        "Business Use %"
                                    }
                                    input id="asset-biz-pct" type="number" min="0" max="100" class="input" value="100";
                                }
                                div {
                                    label class="label" {
                                        "Section 179 ($)"
                                    }
                                    input id="asset-179" type="number" step="0.01" class="input" placeholder="0";
                                }
                                div class="flex items-end gap-2" {
                                    label class="flex items-center gap-2 text-xs text-gray-300" {
                                        input id="asset-bonus" type="checkbox";
                                        " Bonus Depreciation"
                                    }
                                    label class="flex items-center gap-2 text-xs text-gray-300" {
                                        input id="asset-is-vehicle" type="checkbox";
                                        " Vehicle"
                                    }
                                }
                                div class="col-span-full flex gap-2" {
                                    button onclick="saveAsset()" class="btn-primary" {
                                        "Save Asset"
                                    }
                                    button onclick="document.getElementById('asset-form').classList.add('hidden')" class="text-xs text-gray-400 hover:text-gray-200" {
                                        "Cancel"
                                    }
                                }
                            }
                        }
                        div id="assets-list" class="space-y-1" {
                            p class="text-xs text-gray-600" {
                                "Loading assets..."
                            }
                        }
                    }
                    div class="card mb-6" {
                        h3 class="font-medium mb-2" {
                            "This Year's Depreciation"
                        }
                        div class="grid grid-cols-2 md:grid-cols-4 gap-3" {
                            div {
                                p class="text-xs text-gray-500 uppercase" {
                                    "Section 179"
                                }
                                p class="text-xl font-semibold mt-1" id="depr-179" {
                                    "--"
                                }
                            }
                            div {
                                p class="text-xs text-gray-500 uppercase" {
                                    "Bonus Depreciation"
                                }
                                p class="text-xl font-semibold mt-1" id="depr-bonus" {
                                    "--"
                                }
                            }
                            div {
                                p class="text-xs text-gray-500 uppercase" {
                                    "MACRS Yr 1"
                                }
                                p class="text-xl font-semibold mt-1" id="depr-macrs" {
                                    "--"
                                }
                            }
                            div {
                                p class="text-xs text-gray-500 uppercase" {
                                    "Total Year 1"
                                }
                                p class="text-xl font-semibold mt-1 text-oc-400" id="depr-total" {
                                    "--"
                                }
                            }
                        }
                    }
                    div class="card" {
                        h3 class="font-medium mb-3" {
                            "Vehicle Mileage Log"
                        }
                        div class="grid grid-cols-2 md:grid-cols-4 gap-2 mb-3" {
                            div {
                                label class="label" {
                                    "Asset"
                                }
                                select id="veh-asset" class="input" {
                                    option value="" {
                                        "-- pick vehicle --"
                                    }
                                }
                            }
                            div {
                                label class="label" {
                                    "Tax Year"
                                }
                                input id="veh-year" type="number" class="input" value="2025";
                            }
                            div {
                                label class="label" {
                                    "Business Miles"
                                }
                                input id="veh-biz-miles" type="number" class="input";
                            }
                            div {
                                label class="label" {
                                    "Total Miles"
                                }
                                input id="veh-total-miles" type="number" class="input";
                            }
                            div class="col-span-full" {
                                button onclick="saveVehicleUsage()" class="btn-primary" {
                                    "Save Usage"
                                }
                            }
                        }
                        p class="text-xs text-gray-500" {
                            "IRS standard mileage rate 2025: $0.70/business mile. Use actual expense method if higher (gas, maintenance, depreciation, insurance)."
                        }
                    }
                }
                // State tab
                div id="tab-state" class="hidden" {
                    div class="card mb-6" {
                        h3 class="font-medium mb-3" {
                            "State Tax Estimates"
                        }
                        div class="grid grid-cols-1 md:grid-cols-3 gap-3" {
                            div class="p-3 bg-gray-900/40 rounded-lg" {
                                p class="text-xs text-gray-500 uppercase" {
                                    "Federal AGI"
                                }
                                p class="text-xl font-semibold mt-1" id="st-federal-agi" {
                                    "--"
                                }
                            }
                            div class="p-3 bg-gray-900/40 rounded-lg" {
                                p class="text-xs text-gray-500 uppercase" {
                                    "Total State Tax"
                                }
                                p class="text-xl font-semibold mt-1" id="st-total-state-tax" {
                                    "--"
                                }
                            }
                            div class="p-3 bg-gray-900/40 rounded-lg" {
                                p class="text-xs text-gray-500 uppercase" {
                                    "Combined Effective Rate"
                                }
                                p class="text-xl font-semibold mt-1" id="st-combined-rate" {
                                    "--"
                                }
                            }
                        }
                        div id="state-breakdown" class="mt-4 space-y-2" {
                            p class="text-xs text-gray-600" {
                                "Add a state residency below to estimate state taxes."
                            }
                        }
                    }
                    div class="card" {
                        h3 class="font-medium mb-3" {
                            "Add State Residency"
                        }
                        p class="text-xs text-gray-500 mb-3" {
                            "Full-year resident, part-year (moved), or non-resident (worked in another state). Multi-state supported."
                        }
                        div class="grid grid-cols-2 md:grid-cols-3 gap-2" {
                            div {
                                label class="label" {
                                    "State"
                                }
                                select id="st-state" class="input" {
                                }
                            }
                            div {
                                label class="label" {
                                    "Residency"
                                }
                                select id="st-residency" class="input" {
                                    option value="full_year" {
                                        "Full Year"
                                    }
                                    option value="part_year" {
                                        "Part Year"
                                    }
                                    option value="nonresident" {
                                        "Non-Resident"
                                    }
                                }
                            }
                            div {
                                label class="label" {
                                    "Months in State"
                                }
                                input id="st-months" type="number" min="0" max="12" class="input" value="12";
                            }
                            div {
                                label class="label" {
                                    "State Wages ($)"
                                }
                                input id="st-wages" type="number" step="0.01" class="input" placeholder="0";
                            }
                            div {
                                label class="label" {
                                    "State Tax Withheld ($)"
                                }
                                input id="st-withheld" type="number" step="0.01" class="input" placeholder="0";
                            }
                            div class="flex items-end gap-2" {
                                button onclick="saveStateProfile()" class="btn-primary" {
                                    "Save"
                                }
                            }
                        }
                    }
                }
                // Entities tab
                div id="tab-entities" class="hidden" {
                    div class="card mb-6" {
                        div class="flex items-center justify-between mb-3" {
                            h3 class="font-medium" {
                                "Business Entities"
                            }
                            button onclick="toggleForm('ent-form')" class="text-xs bg-oc-600 hover:bg-oc-700 text-white px-2.5 py-1 rounded-lg" {
                                "+ Add Entity"
                            }
                        }
                        div id="ent-form" class="hidden mb-4 p-3 bg-gray-900/40 rounded-lg" {
                            div class="grid grid-cols-2 md:grid-cols-3 gap-2" {
                                div class="col-span-2" {
                                    label class="label" {
                                        "Entity Name"
                                    }
                                    input id="ent-name" class="input" placeholder="Acme Woodworks LLC";
                                }
                                div {
                                    label class="label" {
                                        "Type"
                                    }
                                    select id="ent-type" class="input" {
                                        option value="sole_prop" {
                                            "Sole Proprietorship"
                                        }
                                        option value="s_corp" {
                                            "S-Corp"
                                        }
                                        option value="c_corp" {
                                            "C-Corp"
                                        }
                                        option value="partnership" {
                                            "Partnership"
                                        }
                                        option value="llc_single" {
                                            "Single-Member LLC"
                                        }
                                        option value="llc_multi" {
                                            "Multi-Member LLC"
                                        }
                                    }
                                }
                                div {
                                    label class="label" {
                                        "EIN (optional)"
                                    }
                                    input id="ent-ein" class="input" placeholder="XX-XXXXXXX";
                                }
                                div {
                                    label class="label" {
                                        "State of Formation"
                                    }
                                    input id="ent-state" class="input" placeholder="NC";
                                }
                                div {
                                    label class="label" {
                                        "Formation Date"
                                    }
                                    input id="ent-formed" type="date" class="input";
                                }
                                div {
                                    label class="label" {
                                        "Your Ownership %"
                                    }
                                    input id="ent-own" type="number" class="input" value="100";
                                }
                                div class="col-span-full flex gap-2" {
                                    button onclick="saveEntity()" class="btn-primary" {
                                        "Save Entity"
                                    }
                                    button onclick="document.getElementById('ent-form').classList.add('hidden')" class="text-xs text-gray-400 hover:text-gray-200" {
                                        "Cancel"
                                    }
                                }
                            }
                        }
                        div id="entities-list" class="space-y-1" {
                            p class="text-xs text-gray-600" {
                                "Loading entities..."
                            }
                        }
                    }
                    div id="entity-detail" class="hidden card mb-6" {
                        div class="flex items-center justify-between mb-3" {
                            h3 class="font-medium" id="ent-detail-name" {
                                "--"
                            }
                            button onclick="hideEntityDetail()" class="text-xs text-gray-400 hover:text-gray-200" {
                                "Close"
                            }
                        }
                        div class="grid grid-cols-2 md:grid-cols-4 gap-3 mb-4" {
                            div {
                                p class="text-xs text-gray-500 uppercase" {
                                    "Income"
                                }
                                p class="text-xl font-semibold mt-1 text-green-400" id="ent-d-income" {
                                    "--"
                                }
                            }
                            div {
                                p class="text-xs text-gray-500 uppercase" {
                                    "Expenses"
                                }
                                p class="text-xl font-semibold mt-1 text-red-400" id="ent-d-expenses" {
                                    "--"
                                }
                            }
                            div {
                                p class="text-xs text-gray-500 uppercase" {
                                    "Net Income"
                                }
                                p class="text-xl font-semibold mt-1" id="ent-d-net" {
                                    "--"
                                }
                            }
                            div {
                                p class="text-xs text-gray-500 uppercase" {
                                    "Entity Tax"
                                }
                                p class="text-xl font-semibold mt-1" id="ent-d-tax" {
                                    "--"
                                }
                                p class="text-xs text-gray-500" id="ent-d-passthrough" {
                                }
                            }
                        }
                        div class="grid grid-cols-1 md:grid-cols-2 gap-4" {
                            div {
                                h4 class="text-xs uppercase text-gray-500 mb-2" {
                                    "Shareholders / Partners"
                                }
                                div id="ent-d-shareholders" class="space-y-1" {
                                }
                                button onclick="showAddShareholder()" class="text-xs text-oc-400 hover:text-oc-300 mt-2" {
                                    "+ Add Shareholder"
                                }
                                div id="ent-sh-form" class="hidden mt-2 p-2 bg-gray-900/40 rounded-lg grid grid-cols-2 gap-2" {
                                    div {
                                        label class="label" {
                                            "Name"
                                        }
                                        input id="ent-sh-name" class="input";
                                    }
                                    div {
                                        label class="label" {
                                            "Ownership %"
                                        }
                                        input id="ent-sh-pct" type="number" class="input";
                                    }
                                    div {
                                        label class="label" {
                                            "Salary ($)"
                                        }
                                        input id="ent-sh-salary" type="number" step="0.01" class="input";
                                    }
                                    div {
                                        label class="label" {
                                            "Distributions ($)"
                                        }
                                        input id="ent-sh-dist" type="number" step="0.01" class="input";
                                    }
                                    div class="col-span-full" {
                                        button onclick="saveShareholder()" class="btn-primary" {
                                            "Save"
                                        }
                                    }
                                }
                            }
                            div {
                                h4 class="text-xs uppercase text-gray-500 mb-2" {
                                    "Expense Breakdown"
                                }
                                div id="ent-d-categories" class="space-y-1" {
                                    p class="text-xs text-gray-600" {
                                        "No expenses yet"
                                    }
                                }
                            }
                        }
                        div class="mt-4 pt-4 border-t border-gray-700 flex items-center gap-3" {
                            button onclick="generateK1s()" class="text-xs bg-oc-600 hover:bg-oc-700 text-white px-3 py-1.5 rounded-lg" {
                                "Generate K-1s"
                            }
                            button onclick="toggleForm('ent-1099-form')" class="text-xs bg-gray-700 hover:bg-gray-600 text-white px-3 py-1.5 rounded-lg" {
                                "Issue 1099-NEC"
                            }
                            button onclick="loadEntity1099List()" class="text-xs text-gray-400 hover:text-gray-200" {
                                "View issued 1099s"
                            }
                        }
                        div id="ent-1099-form" class="hidden mt-3 p-3 bg-gray-900/40 rounded-lg grid grid-cols-2 gap-2" {
                            div class="col-span-2" {
                                label class="label" {
                                    "Recipient Name"
                                }
                                input id="ent-1099-name" class="input";
                            }
                            div class="col-span-2" {
                                label class="label" {
                                    "Recipient Address"
                                }
                                input id="ent-1099-addr" class="input" placeholder="Street, City, ST ZIP";
                            }
                            div {
                                label class="label" {
                                    "Amount Paid ($)"
                                }
                                input id="ent-1099-amt" type="number" step="0.01" class="input" placeholder="min $600";
                            }
                            div class="flex items-end" {
                                button onclick="issue1099()" class="btn-primary" {
                                    "Issue"
                                }
                            }
                        }
                        div id="ent-k1-results" class="mt-3" {
                        }
                        div id="ent-1099-results" class="mt-3" {
                        }
                    }
                    div class="card" {
                        h3 class="font-medium mb-3" {
                            "Entity Structure Comparison"
                        }
                        p class="text-xs text-gray-500 mb-3" {
                            "Compare Sole Proprietorship vs S-Corp vs LLC based on projected SE income. Shows SE tax savings opportunities."
                        }
                        div class="flex items-end gap-2 mb-3" {
                            div class="flex-1" {
                                label class="label" {
                                    "SE Income ($, optional override)"
                                }
                                input id="ent-cmp-income" type="number" step="0.01" class="input" placeholder="Leave blank to use actual";
                            }
                            button onclick="loadEntityComparison()" class="btn-primary" {
                                "Compare"
                            }
                        }
                        div id="ent-comparison" class="space-y-2" {
                            p class="text-xs text-gray-600" {
                                "Click Compare to see structure analysis."
                            }
                        }
                    }
                }
                // Insights tab
                div id="tab-insights" class="hidden" {
                    div class="card mb-6" {
                        div class="flex items-center justify-between mb-3" {
                            h3 class="font-medium" {
                                "Audit Risk Score"
                            }
                            button onclick="loadAuditRisk()" class="text-xs text-oc-400 hover:text-oc-300" {
                                "Refresh"
                            }
                        }
                        div class="flex items-center gap-4 mb-3" {
                            div id="audit-score-ring" class="w-20 h-20 rounded-full border-4 border-gray-700 flex items-center justify-center text-2xl font-semibold" {
                                "--"
                            }
                            div {
                                p class="text-sm text-gray-300" id="audit-score-label" {
                                    "Loading..."
                                }
                                p class="text-xs text-gray-500 mt-1" id="audit-score-summary" {
                                }
                            }
                        }
                        div id="audit-factors" class="space-y-1 mt-3" {
                        }
                    }
                    div class="card mb-6" {
                        div class="flex items-center justify-between mb-3" {
                            h3 class="font-medium" {
                                "AI Tax Insights"
                            }
                            button onclick="loadInsightsList()" class="text-xs text-oc-400 hover:text-oc-300" {
                                "Refresh"
                            }
                        }
                        div id="insights-list" class="space-y-2" {
                            p class="text-xs text-gray-600" {
                                "Loading insights..."
                            }
                        }
                    }
                    div class="card mb-6" {
                        h3 class="font-medium mb-3" {
                            "What-If Scenario"
                        }
                        p class="text-xs text-gray-500 mb-3" {
                            "Model a tax change: raise, bonus, marriage, new dependent, IRA contribution."
                        }
                        div class="grid grid-cols-2 md:grid-cols-3 gap-2 mb-3" {
                            div class="col-span-2" {
                                label class="label" {
                                    "Scenario Name"
                                }
                                input id="wi-name" class="input" placeholder="10k raise + max IRA";
                            }
                            div {
                                label class="label" {
                                    "Additional Income ($)"
                                }
                                input id="wi-income" type="number" step="0.01" class="input" placeholder="0";
                            }
                            div {
                                label class="label" {
                                    "Additional Deductions ($)"
                                }
                                input id="wi-ded" type="number" step="0.01" class="input" placeholder="0";
                            }
                            div {
                                label class="label" {
                                    "Retirement Contribution ($)"
                                }
                                input id="wi-ret" type="number" step="0.01" class="input" placeholder="0";
                            }
                            div {
                                label class="label" {
                                    "Filing Status Override"
                                }
                                select id="wi-fs" class="input" {
                                    option value="" {
                                        "-- keep current --"
                                    }
                                    option value="single" {
                                        "Single"
                                    }
                                    option value="married_jointly" {
                                        "Married Jointly"
                                    }
                                    option value="married_separately" {
                                        "Married Separately"
                                    }
                                    option value="head_of_household" {
                                        "Head of Household"
                                    }
                                }
                            }
                            div class="col-span-full" {
                                button onclick="runWhatIf()" class="btn-primary" {
                                    "Run Scenario"
                                }
                            }
                        }
                        div id="wi-result" {
                        }
                    }
                    div class="card" {
                        div class="flex items-center justify-between mb-2" {
                            h3 class="font-medium" {
                                "Injected Tax Context (what the AI sees)"
                            }
                            button onclick="loadTaxContext()" class="text-xs text-oc-400 hover:text-oc-300" {
                                "Refresh"
                            }
                        }
                        pre id="tax-context-text" class="text-xs text-gray-400 whitespace-pre-wrap max-h-[300px] overflow-y-auto bg-gray-900/40 rounded p-3" {
                            "Loading..."
                        }
                    }
                }
                // Ledger tabs (migrated from rust-ledger 2026-04-22)
                div id="tab-ledger-overview" class="hidden" {
                    div id="ledger-overview-body" {
                        div class="text-sm text-gray-400" {
                            "Loading…"
                        }
                    }
                }
                div id="tab-ledger-accounts" class="hidden" {
                    div id="ledger-accounts-body" {
                        div class="text-sm text-gray-400" {
                            "Loading…"
                        }
                    }
                }
                div id="tab-ledger-transactions" class="hidden" {
                    div id="ledger-transactions-body" {
                        div class="text-sm text-gray-400" {
                            "Loading…"
                        }
                    }
                }
                // Trading snapshot — Alpaca live data + per-bot heartbeat
                div id="tab-trading-snapshot" class="hidden" {
                    div id="trading-snapshot-body" {
                        div class="text-sm text-gray-400" {
                            "Loading…"
                        }
                    }
                }
            }
            // end left panel
            // RIGHT: Positronic Matrix (AI chat, Positron persona)
            div class="w-[380px] positron-panel flex flex-col flex-shrink-0" {
                // Animated positronic brain canvas behind messages
                canvas id="positron-brain" {
                }
                // LCARS header
                div class="positron-header" {
                    div class="p-frame" {
                        "P"
                    }
                    div class="p-title" {
                        "Positron Channel"
                    }
                    div class="p-status" {
                        span class="p-led" {
                        }
                        span {
                            "ONLINE"
                        }
                    }
                }
                // Log counter / action strip
                div class="positron-logbar" {
                    span class="logbar-label" {
                        "LOG"
                    }
                    span class="logbar-value" id="positron-log-index" {
                        "0000.0"
                    }
                    span class="logbar-label" style="margin-left:auto" {
                        "CYCLES"
                    }
                    span class="logbar-value" id="positron-msg-count" {
                        "0"
                    }
                    button onclick="clearTaxChat()" class="logbar-label" style="background:none;border:none;color:inherit;cursor:pointer;margin-left:0.5rem;opacity:0.7" title="Clear conversation" {
                        "CLR"
                    }
                }
                div class="flex-1 overflow-y-auto space-y-3" id="tax-chat-messages" {
                    div class="flex gap-2" {
                        img src="/agent-avatar/positron" class="w-12 h-12 rounded-full flex-shrink-0 mt-0.5" alt="";
                        div class="text-sm text-gray-400" {
                            p {
                                "Positron here. I have access to your receipts, expenses, investments, and taxpayer profile. I can compute deductions, flag inconsistencies, and cite IRC when appropriate. Queries:"
                            }
                            div class="flex flex-wrap gap-1 mt-2" {
                                button onclick="taxChat('Show my expense summary for 2025')" {
                                    "2025 Summary"
                                }
                                button onclick="taxChat('What are my deductible expenses?')" {
                                    "Deductibility"
                                }
                                button onclick="taxChat('Log a $50 lunch at Chipotle today as business meal')" {
                                    "Log Expense"
                                }
                            }
                        }
                    }
                }
                // Input
                div class="positron-input-row" {
                    div class="flex gap-2 items-end" {
                        textarea id="tax-chat-input" rows="1" class="flex-1 px-3 py-2 outline-none resize-none" placeholder="Query Positron..." onkeydown="if(event.key==='Enter'&&!event.shiftKey){event.preventDefault();sendTaxChat()}" oninput="this.style.height='auto';this.style.height=Math.min(this.scrollHeight,100)+'px'" style="max-height:100px" {
                        }
                        button onclick="sendTaxChat()" class="transmit-btn flex-shrink-0" id="tax-send-btn" {
                            "TRANSMIT"
                        }
                    }
                }
            }
        }
        // end split layout
        script { (PreEscaped(PAGE_JS)) }
    };
    Html(shell(page, body).into_string())
}

const EXTRA_STYLE: &str = r##"@import url('/fonts.css');
  @import url('https://fonts.googleapis.com/css2?family=Antonio:wght@400;500;700&family=IBM+Plex+Mono:wght@400;500;600&family=Source+Serif+4:wght@400;600;700&display=swap');

  /* =========================================================================
     Tax module — ledger theme
     LCARS chrome (orange/gold/peach) + parchment accountant's desk main canvas
     + positronic-brain sidebar for Positron. Dark leather backing.
     ========================================================================= */
  :root {
    /* Warm-amber block palette */
    --lcars-orange: #ff9c3d;
    --lcars-peach:  #ffb889;
    --lcars-salmon: #e88b7a;
    --lcars-gold:   #d4a574;
    --lcars-purple: #b797c7;
    --lcars-blue:   #8aa7d9;
    --lcars-red:    #cc6666;
    /* Accountant / ledger palette */
    --paper:        #ece2c6;        /* warm parchment */
    --paper-dark:   #d6c8a4;        /* aged paper rule line */
    --ledger:       #3a6650;        /* forest-green ledger */
    --ledger-pale:  rgba(58,102,80,0.12);
    --ink-sepia:    #2b1d10;        /* serious ink on paper */
    --ink-navy:     #1a2540;        /* section heading ink */
    --ink-red:      #8b2f1f;        /* red-ink deductions */
    /* Dark leather desk */
    --desk-deep:    #0a0806;
    --desk-mid:     #15110c;
    --desk-trim:    #2a2014;
  }

  body { font-family: 'Inter', sans-serif; -webkit-font-smoothing: antialiased; -moz-osx-font-smoothing: grayscale; text-rendering: optimizeLegibility; }
  body.bg-gray-950 { background: radial-gradient(ellipse at 50% -20%, #1c160e 0%, #0a0806 70%) !important; }

  /* ─── Top bar — dark leather trim with LCARS gold underline ─── */
  .border-b.border-gray-800.bg-gray-900\/50 {
    background: rgba(14,11,7,0.88) !important;
    border-color: var(--desk-trim) !important;
    box-shadow: 0 1px 0 0 var(--lcars-gold);
  }

  /* ─── Section-level nav — LCARS block buttons ─── */
  .sec-tab {
    position: relative;
    padding: 0.55rem 1.1rem;
    font-family: 'Antonio', 'Inter', sans-serif;
    font-size: 0.8rem;
    font-weight: 500;
    letter-spacing: 0.14em;
    text-transform: uppercase;
    color: var(--lcars-peach);
    background: transparent;
    border: none;
    border-bottom: 2px solid transparent;
    cursor: pointer;
    white-space: nowrap;
    transition: all 0.12s;
  }
  .sec-tab:hover { color: var(--lcars-orange); }
  .sec-tab.active {
    color: #0a0806;
    background: var(--lcars-orange);
    border-bottom-color: var(--lcars-orange);
    box-shadow: 0 0 14px rgba(255,156,61,0.4);
  }

  /* ─── Sub-tab chips — LCARS pill row ─── */
  .sub-tab {
    padding: 0.3rem 0.85rem;
    font-family: 'Antonio', 'Inter', sans-serif;
    font-size: 0.7rem;
    letter-spacing: 0.1em;
    text-transform: uppercase;
    color: #0a0806;
    background: var(--lcars-gold);
    border: none;
    border-radius: 0 999px 999px 0;
    cursor: pointer;
    white-space: nowrap;
    transition: all 0.15s;
  }
  .sub-tab:first-child { border-radius: 999px 0 0 999px; }
  .sub-tab:hover { background: var(--lcars-peach); }
  .sub-tab.active {
    background: var(--lcars-orange);
    box-shadow: 0 0 8px rgba(255,156,61,0.5);
  }
  .sub-tab .sub-tab-badge {
    display: inline-block; margin-left: 0.35rem; padding: 0 0.35rem;
    min-width: 1.1rem; height: 1.1rem; line-height: 1.1rem; text-align: center;
    font-size: 0.625rem; border-radius: 999px; background: var(--lcars-red); color: #0a0806; font-weight: 600;
  }

  /* ─── KPI strip — LCARS block-headers + IBM Plex Mono numerals ─── */
  .kpi-strip {
    display: grid; grid-template-columns: repeat(4, minmax(0, 1fr)); gap: 0;
    margin-bottom: 1rem;
    border: 1px solid var(--desk-trim); border-radius: 0.35rem; overflow: hidden;
    background: var(--desk-deep);
  }
  @media (max-width: 900px) { .kpi-strip { grid-template-columns: repeat(2, 1fr); } }
  .kpi-tile { background: #110e09; padding: 0; border-left: 1px solid var(--desk-trim); border-radius: 0; }
  .kpi-tile:first-child { border-left: none; }
  .kpi-tile .kpi-label {
    background: var(--lcars-gold); color: #0a0806;
    padding: 0.3rem 0.7rem;
    font-family: 'Antonio', sans-serif; font-size: 0.65rem;
    letter-spacing: 0.14em; text-transform: uppercase;
  }
  .kpi-tile .kpi-value {
    font-family: 'IBM Plex Mono', monospace; font-size: 1.5rem; font-weight: 500;
    color: var(--lcars-peach); padding: 0.55rem 0.75rem 0;
  }
  .kpi-tile .kpi-sub {
    font-family: 'IBM Plex Mono', monospace; font-size: 0.68rem;
    color: #8a7a5c; padding: 0.1rem 0.75rem 0.6rem;
  }
  .kpi-tile.ok  .kpi-value { color: #7aea9e; }
  .kpi-tile.warn .kpi-value { color: var(--lcars-orange); }
  .kpi-tile.bad  .kpi-value { color: var(--lcars-red); }

  /* ─── Deadline pill — LCARS alert ─── */
  .tax-subbar {
    display: flex; align-items: center; gap: 10px;
    padding: 6px 18px;
    border-bottom: 1px solid rgb(31,41,55);
    background: rgba(17,24,39,0.4);
    font-size: 12px;
  }
  .deadline-pill {
    display: inline-flex; align-items: center; gap: 0.4rem;
    padding: 0.25rem 0.7rem 0.25rem 0.5rem;
    font-family: 'Antonio', sans-serif; font-size: 0.7rem;
    letter-spacing: 0.12em; text-transform: uppercase; font-weight: 500;
    background: var(--lcars-red) !important;
    color: #0a0806 !important;
    border: none !important;
    border-radius: 999px 0 0 999px;
    white-space: nowrap;
  }
  .deadline-pill .deadline-dot {
    width: 6px; height: 6px; border-radius: 50%;
    background: #0a0806; animation: deadline-pulse 2s infinite;
  }
  @keyframes deadline-pulse { 0%,100% { opacity: 1; } 50% { opacity: 0.35; } }

  /* ─── Cards — parchment on dark leather ─── */
  .card {
    background: linear-gradient(180deg, #f0e7cc 0%, #e3d6b4 100%) !important;
    color: var(--ink-sepia) !important;
    border: 1px solid var(--desk-trim) !important;
    border-radius: 0.3rem !important;
    padding: 1.1rem 1.25rem !important;
    box-shadow: 0 3px 14px rgba(0,0,0,0.55), inset 0 0 0 1px rgba(255,240,210,0.25);
    position: relative;
  }
  /* Faint ledger stripe on the left edge */
  .card::before {
    content: ''; position: absolute; left: 0; top: 0; bottom: 0; width: 4px;
    background: linear-gradient(180deg, var(--lcars-gold), var(--ledger));
  }
  .card h1, .card h2, .card h3, .card h4 {
    font-family: 'Source Serif 4', Georgia, serif;
    color: var(--ink-navy) !important;
    font-weight: 600 !important;
    letter-spacing: 0.01em;
  }
  .card p, .card span, .card div, .card label, .card td, .card th, .card li {
    color: var(--ink-sepia);
  }
  /* Amount / dollar text — always monospaced for a ledger feel */
  .card .amount, .card [class*="font-medium"][class*="gray-200"],
  .card .text-2xl, .card .text-xl, .card [id^="sum-"], .card [id^="ext-"] {
    font-family: 'IBM Plex Mono', monospace;
  }
  /* Muted text on cards — use softer sepia */
  .card .text-gray-300, .card .text-gray-400, .card .text-gray-500, .card .text-gray-600, .card .text-gray-700 {
    color: #6a5a3e !important;
  }
  .card .text-xs, .card .text-sm { color: #4a3a26 !important; }
  .card .text-yellow-500\/60 { color: var(--ink-red) !important; }
  .card .text-green-400 { color: var(--ledger) !important; font-family: 'IBM Plex Mono', monospace; }
  .card .text-red-400, .card .text-red-500 { color: var(--ink-red) !important; }
  .card .text-oc-500, .card .text-oc-400, .card .text-oc-300 { color: var(--ink-navy) !important; font-weight: 500; }
  /* Inner "sub-cards" (gray-900 boxes inside cards) — faded parchment wells */
  .card .bg-gray-900, .card .bg-gray-900\/40, .card .bg-gray-900\/50,
  .card .bg-gray-800, .card .bg-gray-800\/50 {
    background: rgba(58,40,20,0.08) !important;
    border-color: rgba(43,29,16,0.18) !important;
  }
  .card .border-gray-700, .card .border-gray-800, .card .border-gray-700\/50, .card .border-gray-800\/30, .card .border-gray-800\/50 {
    border-color: rgba(43,29,16,0.22) !important;
  }
  /* Ledger-green progress bars inside overview */
  .card .bg-oc-500 { background: var(--ledger) !important; }
  .card .bg-purple-500 { background: var(--lcars-purple) !important; }
  .card .bg-gray-700 { background: rgba(43,29,16,0.15) !important; }

  /* Badges on cards */
  .card .badge-green { background: rgba(58,102,80,0.25) !important; color: var(--ledger) !important; }
  .card .badge-yellow { background: rgba(212,165,116,0.3) !important; color: #6b4d1a !important; }
  .card .badge-red { background: rgba(139,47,31,0.18) !important; color: var(--ink-red) !important; }

  /* ─── Buttons ─── */
  .btn-primary {
    background: var(--lcars-orange) !important;
    color: #0a0806 !important;
    font-family: 'Antonio', sans-serif !important;
    font-weight: 500 !important;
    letter-spacing: 0.1em !important;
    text-transform: uppercase;
    border-radius: 0.25rem !important;
    padding: 0.5rem 1.1rem !important;
  }
  .btn-primary:hover { background: var(--lcars-peach) !important; box-shadow: 0 0 12px rgba(255,156,61,0.45); }

  /* ─── Inputs on parchment cards — strong override (beats inline styles) ─── */
  .card .input,
  .card input:not([type="checkbox"]):not([type="radio"]):not([type="file"]),
  .card select,
  .card textarea {
    background: rgba(252,245,226,0.7) !important;
    color: var(--ink-sepia) !important;
    border: 1px solid rgba(43,29,16,0.35) !important;
    border-radius: 0.3rem !important;
    padding: 0.4rem 0.65rem !important;
    font-family: 'IBM Plex Mono', monospace !important;
    font-size: 0.85rem !important;
    outline: none !important;
    width: 100% !important;
    box-sizing: border-box !important;
    min-height: 2.15rem;
  }
  .card .input:focus, .card input:focus, .card select:focus, .card textarea:focus {
    border-color: var(--ink-navy) !important;
    box-shadow: 0 0 0 1px var(--ink-navy) !important;
    outline: none !important;
  }
  .card .label {
    color: var(--ink-navy) !important;
    font-family: 'Antonio', sans-serif;
    text-transform: uppercase;
    letter-spacing: 0.1em;
    font-size: 0.7rem !important;
    margin-bottom: 0.2rem !important;
    display: block;
  }

  /* Native <select> — custom LCARS-ish chevron on the parchment */
  .card select {
    appearance: none !important;
    -webkit-appearance: none !important;
    background-image: url("data:image/svg+xml,%3Csvg xmlns='http://www.w3.org/2000/svg' viewBox='0 0 12 12' fill='%232b1d10'%3E%3Cpath d='M2 4h8L6 9z'/%3E%3C/svg%3E") !important;
    background-repeat: no-repeat !important;
    background-position: right 0.55rem center !important;
    background-size: 10px 10px !important;
    padding-right: 1.8rem !important;
    cursor: pointer;
  }
  .card select option { background: #f0e7cc; color: var(--ink-sepia); }

  /* Native date / datetime pickers — darker calendar indicator on cream */
  .card input[type="date"]::-webkit-calendar-picker-indicator,
  .card input[type="datetime-local"]::-webkit-calendar-picker-indicator,
  .card input[type="time"]::-webkit-calendar-picker-indicator {
    filter: saturate(0) brightness(0.4);
    opacity: 0.75;
    cursor: pointer;
  }

  /* Checkboxes / radios on parchment — dark accent */
  .card input[type="checkbox"], .card input[type="radio"] {
    accent-color: var(--ledger);
    width: 1rem; height: 1rem;
  }

  /* Narrow helper inputs (small quarter/quantity fields) get compact size */
  .card input[size], .card select[size] { width: auto !important; }

  /* Year-select in top bar keeps dark style */
  #year-select { background: var(--desk-mid); color: var(--lcars-peach); border-color: var(--desk-trim); font-family: 'IBM Plex Mono', monospace; }

  /* ─── Tables — alternating ledger rows ─── */
  .card table tr:nth-child(even), .card .ledger-table tr:nth-child(even) { background: var(--ledger-pale); }

  /* ═════════════════════════════════════════════════════════════════════════
     POSITRON CHANNEL — the right-rail AI chat
     ═════════════════════════════════════════════════════════════════════════ */
  .positron-panel {
    position: relative;
    background: radial-gradient(ellipse at top, #0d0d14, #05050a 80%);
    border-left: 2px solid var(--lcars-gold);
  }
  #positron-brain {
    position: absolute; inset: 0; pointer-events: none; z-index: 0;
    width: 100%; height: 100%;
    display: block;
    opacity: 0.8;
  }
  .positron-panel > * { position: relative; z-index: 1; }

  /* Header + logbar are absolutely positioned on top of the scroll area so
     chat content scrolls BEHIND them (the blur masks the overlap). */
  .positron-header {
    position: absolute; top: 0; left: 0; right: 0; z-index: 3;
    display: flex; align-items: center; gap: 0.6rem;
    padding: 0.55rem 0.85rem;
    background: linear-gradient(90deg, var(--lcars-orange) 0%, var(--lcars-orange) 60%, transparent 60.5%);
    color: #0a0806;
  }
  .positron-header .p-frame {
    width: 28px; height: 28px; border-radius: 4px;
    background: #0a0806;
    display: flex; align-items: center; justify-content: center;
    font-family: 'Antonio', sans-serif; font-weight: 700;
    font-size: 1rem; color: var(--lcars-peach);
    border: 1px solid rgba(255,156,61,0.6);
    box-shadow: inset 0 0 8px rgba(255,184,137,0.4);
  }
  .positron-header .p-title {
    flex: 1; font-family: 'Antonio', sans-serif;
    font-size: 0.82rem; font-weight: 500;
    letter-spacing: 0.18em; text-transform: uppercase;
  }
  .positron-header .p-status {
    display: flex; align-items: center; gap: 0.3rem;
    font-family: 'IBM Plex Mono', monospace; font-size: 0.65rem;
  }
  .positron-header .p-led {
    width: 7px; height: 7px; border-radius: 50%;
    background: #7aea9e; box-shadow: 0 0 6px rgba(122,234,158,0.9);
    animation: positron-pulse 2.2s ease-in-out infinite;
  }
  @keyframes positron-pulse { 0%,100% { opacity: 1; } 50% { opacity: 0.5; } }

  .positron-logbar {
    position: absolute; top: 46px; left: 0; right: 0; z-index: 3;
    padding: 0.35rem 0.85rem;
    /* Semi-transparent + blur so the matrix stays visible but scrolled
       chat content passing UNDER the bar gets frosted out. */
    background: rgba(14,10,5,0.72);
    backdrop-filter: blur(10px);
    -webkit-backdrop-filter: blur(10px);
    border-bottom: 1px solid rgba(255,156,61,0.25);
    font-family: 'IBM Plex Mono', monospace; font-size: 0.62rem;
    color: var(--lcars-peach);
    display: flex; justify-content: space-between; gap: 0.5rem;
  }
  .positron-logbar .logbar-label { letter-spacing: 0.12em; text-transform: uppercase; opacity: 0.7; }
  .positron-logbar .logbar-value { letter-spacing: 0.05em; }

  /* Chat messages — full-panel scroll region that sits BEHIND the bars.
     Padding-top/bottom = height of overlapping bars, so first + last
     messages don't get visually covered when scroll is at the extremes.
     Header (46px) + logbar (27px) = 73px of top overlap.
     Input row = 55px of bottom overlap. */
  #tax-chat-messages {
    padding: 82px 0.75rem 66px 0.75rem !important;
    flex: 1 1 auto;
    min-height: 0;
    position: relative;
    z-index: 2;
  }
  #tax-chat-messages > div {
    background: transparent;
    border-left: 2px solid var(--lcars-gold);
    padding: 0.6rem 0.75rem !important;
    border-radius: 0;
    backdrop-filter: blur(1.5px);
  }
  #tax-chat-messages > div > div, #tax-chat-messages > div > p,
  #tax-chat-messages > div p, #tax-chat-messages .text-sm, #tax-chat-messages .text-gray-400 {
    color: var(--lcars-peach) !important;
    font-family: 'IBM Plex Mono', monospace;
    font-size: 0.78rem !important;
    line-height: 1.55;
  }
  /* Quick-action chips inside chat greeting */
  #tax-chat-messages button {
    background: transparent !important;
    color: var(--lcars-gold) !important;
    border: 1px solid var(--lcars-gold) !important;
    border-radius: 0 999px 999px 0 !important;
    font-family: 'Antonio', sans-serif;
    letter-spacing: 0.08em; text-transform: uppercase;
    font-size: 0.65rem !important;
    padding: 0.2rem 0.6rem !important;
  }
  #tax-chat-messages button:hover { background: rgba(212,165,116,0.15) !important; color: var(--lcars-orange) !important; }

  /* Input row */
  .positron-input-row {
    position: absolute; bottom: 0; left: 0; right: 0; z-index: 3;
    padding: 0.55rem 0.75rem;
    border-top: 1px solid rgba(255,156,61,0.25);
    background: rgba(14,10,5,0.72);
    backdrop-filter: blur(10px);
    -webkit-backdrop-filter: blur(10px);
  }
  #tax-chat-input {
    background: rgba(255,184,137,0.05) !important;
    border: 1px solid rgba(255,156,61,0.3) !important;
    color: var(--lcars-peach) !important;
    font-family: 'IBM Plex Mono', monospace !important;
    font-size: 0.78rem !important;
    border-radius: 0.25rem !important;
  }
  #tax-chat-input:focus { border-color: var(--lcars-orange) !important; box-shadow: 0 0 8px rgba(255,156,61,0.3) !important; }
  #tax-chat-input::placeholder { color: rgba(212,165,116,0.55) !important; }

  .transmit-btn {
    background: var(--lcars-orange); color: #0a0806;
    font-family: 'Antonio', sans-serif;
    letter-spacing: 0.12em; text-transform: uppercase;
    font-size: 0.68rem; font-weight: 500;
    padding: 0.4rem 0.85rem;
    border: none;
    border-radius: 999px 0 0 999px;
    cursor: pointer;
    white-space: nowrap;
    transition: all 0.12s;
  }
  .transmit-btn:hover { background: var(--lcars-peach); box-shadow: 0 0 10px rgba(255,156,61,0.45); }"##;

const PAGE_JS: &str = r##"
const token = sessionStorage.getItem('syntaur_token') || localStorage.getItem('syntaur_token') || '';
// Client-side token-gate removed 2026-04-25 (module-reset bug fix).

// ── Module License Check ──
let moduleUnlocked = false;
// LCARS positron-log counter — must initialize early. sendTaxChat()
// and updatePositronLog() both read this; init order can call
// updatePositronLog (transitively via initPositronicBrain) before a
// late declaration site is reached, throwing TDZ ReferenceError.
let positronMsgCount = 0;

async function checkModuleAccess() {
  try {
    const resp = await authFetch(`/api/modules/status?module=tax`);
    const data = await resp.json();
    const access = data.access || {};

    if (access.granted) {
      moduleUnlocked = true;
      document.getElementById('module-paywall').classList.add('hidden');

      // Show trial banner if on trial
      if (access.reason === 'trial') {
        const banner = document.getElementById('trial-banner');
        banner.classList.remove('hidden');
        document.getElementById('trial-days-left').textContent = access.trial_days_left || '?';
        if (access.trial_days_left <= 1) {
          document.getElementById('trial-banner-text').innerHTML = 'Trial expires <strong>today</strong>!';
        }
      }
    } else {
      moduleUnlocked = false;
      const paywall = document.getElementById('module-paywall');
      paywall.classList.remove('hidden');

      if (access.reason === 'trial_expired') {
        document.getElementById('paywall-trial-available').classList.add('hidden');
        document.getElementById('paywall-trial-expired').classList.remove('hidden');
      }
    }
  } catch(e) {
    // If license check fails (e.g., table doesn't exist yet), allow access
    moduleUnlocked = true;
    console.log('License check skipped:', e);
  }
}

async function startFreeTrial() {
  try {
    const resp = await authFetch('/api/modules/trial', {
      method: 'POST', headers: {'Content-Type': 'application/json'},
      body: JSON.stringify({ module: 'tax' })
    });
    const data = await resp.json();
    if (data.granted) {
      moduleUnlocked = true;
      document.getElementById('module-paywall').classList.add('hidden');
      const banner = document.getElementById('trial-banner');
      banner.classList.remove('hidden');
      document.getElementById('trial-days-left').textContent = data.trial_days_left || '3';
      loadOverview();
    }
  } catch(e) { console.log('Trial start failed:', e); }
}

function upgradePro() {
  // TODO: integrate payment processor (Stripe, Paddle, etc.)
  // For now, show a message
  const paywall = document.getElementById('module-paywall');
  if (paywall && !paywall.classList.contains('hidden')) {
    paywall.querySelector('.card').insertAdjacentHTML('beforeend',
      '<p class="text-xs text-oc-400 mt-3">Payment integration coming soon. Contact support to purchase.</p>');
  } else {
    alert('Syntaur Pro upgrade — payment integration coming soon.');
  }
}

// Check license on page load
checkModuleAccess();

function authFetch(url, opts = {}) {
  opts.headers = opts.headers || {};
  opts.headers['Authorization'] = 'Bearer ' + token;
  return fetch(url, opts);
}

function showTab(name) {
  ['overview', 'expenses', 'receipts', 'documents', 'property', 'connections', 'credits', 'quarterly', 'investments', 'depreciation', 'state', 'entities', 'insights', 'deductions', 'wizard', 'ledger-overview', 'ledger-accounts', 'ledger-transactions', 'trading-snapshot'].forEach(t => {
    const el = document.getElementById('tab-' + t);
    if (el) el.classList.toggle('hidden', t !== name);
    const btn = document.getElementById('tab-btn-' + t);
    if (btn) btn.className = 'tab ' + (t === name ? 'text-white' : 'text-gray-400 hover:text-gray-300');
  });
  // Reflect active state on the sub-tab chips
  document.querySelectorAll('#sub-tab-bar .sub-tab').forEach(btn => {
    btn.classList.toggle('active', btn.dataset.tab === name);
  });
  if (name === 'expenses') loadExpenses();
  if (name === 'receipts') loadReceipts();
  if (name === 'documents') loadDocuments();
  if (name === 'property') loadPropertyProfile();
  if (name === 'connections') loadConnections();
  if (name === 'credits') loadCreditsTab();
  if (name === 'quarterly') loadQuarterlyTab();
  if (name === 'investments') { loadInvestmentAccounts(); loadInvestmentSummary(); loadInvestmentTransactions(); loadHoldings(); loadLots(); loadWashSales(); loadForm8949(); loadK1s(); loadCapitalGains(); }
  if (name === 'depreciation') loadDepreciationTab();
  if (name === 'state') loadStateTab();
  if (name === 'entities') loadEntitiesTab();
  if (name === 'insights') loadInsightsTab();
  if (name === 'deductions') loadDeductionsTab();
  if (name === 'wizard') loadWizard();
  if (name === 'ledger-overview') loadLedgerOverview();
  if (name === 'ledger-accounts') loadLedgerAccounts();
  if (name === 'ledger-transactions') loadLedgerTransactions();
  if (name === 'trading-snapshot') loadTradingSnapshot();
}

// ===== Ledger sub-feature (migrated from rust-ledger 2026-04-22) =====
let __ledgerEntityId = null;

async function loadLedgerOverview() {
  const root = document.getElementById('ledger-overview-body');
  if (!root) return;
  root.innerHTML = '<div class="text-sm text-gray-400">Loading…</div>';
  try {
    const r = await authFetch('/api/ledger/entities');
    if (r.status === 503) {
      root.innerHTML = '<div class="card text-sm text-gray-300">Ledger DB not yet bind-mounted. Migration pending.</div>';
      return;
    }
    const d = await r.json();
    const ents = d.entities || [];
    if (ents.length === 0) {
      root.innerHTML = '<div class="card text-sm">No entities. Use the standalone rust-ledger CLI to bootstrap.</div>';
      return;
    }
    if (__ledgerEntityId == null && ents.length > 0) __ledgerEntityId = ents[0].id;
    let html = '<div class="card mb-3"><div class="text-xs text-gray-400 mb-1">Active entity</div><div class="flex gap-2">';
    for (const e of ents) {
      const cls = (e.id === __ledgerEntityId) ? 'btn-primary' : 'btn-secondary';
      html += `<button class="${cls}" onclick="__ledgerEntityId=${e.id};loadLedgerOverview();loadLedgerAccounts();loadLedgerTransactions()">${e.name}</button>`;
    }
    html += '</div></div>';
    // YTD expense summary for this entity
    const yr = new Date().getUTCFullYear();
    const sumR = await authFetch(`/api/ledger/reports/expense_summary?entity_id=${__ledgerEntityId}&from=${yr}-01-01&to=${yr}-12-31`);
    const sumD = await sumR.json();
    const totalDollars = ((sumD.total_cents || 0) / 100).toLocaleString('en-US', { style: 'currency', currency: 'USD' });
    html += `<div class="card"><div class="text-xs text-gray-400 mb-1">${yr} YTD expenses</div><div class="text-2xl font-semibold mb-3">${totalDollars}</div>`;
    if (sumD.rows && sumD.rows.length) {
      html += '<table class="text-sm w-full"><thead><tr><th class="text-left py-1">Account</th><th class="text-right py-1">Total</th></tr></thead><tbody>';
      for (const row of sumD.rows.slice(0, 20)) {
        const dollars = (row.total_cents / 100).toLocaleString('en-US', { style: 'currency', currency: 'USD' });
        html += `<tr><td class="py-1">${row.account_name}</td><td class="text-right py-1">${dollars}</td></tr>`;
      }
      html += '</tbody></table>';
    }
    html += '</div>';
    root.innerHTML = html;
  } catch (e) {
    root.innerHTML = `<div class="card text-sm text-red-400">Failed to load ledger: ${e.message || e}</div>`;
  }
}

async function loadLedgerAccounts() {
  const root = document.getElementById('ledger-accounts-body');
  if (!root) return;
  root.innerHTML = '<div class="text-sm text-gray-400">Loading…</div>';
  try {
    const url = '/api/ledger/accounts' + (__ledgerEntityId ? `?entity_id=${__ledgerEntityId}` : '');
    const r = await authFetch(url);
    if (r.status === 503) { root.innerHTML = '<div class="card text-sm">Ledger DB not bind-mounted yet.</div>'; return; }
    const d = await r.json();
    const accs = d.accounts || [];
    if (accs.length === 0) { root.innerHTML = '<div class="card text-sm text-gray-400">No accounts.</div>'; return; }
    let html = `<div class="card"><div class="text-xs text-gray-400 mb-2">${accs.length} accounts</div>`;
    html += '<table class="text-sm w-full"><thead><tr><th class="text-left py-1">Name</th><th class="text-left py-1">Type</th><th class="text-left py-1">Institution</th></tr></thead><tbody>';
    for (const a of accs) {
      html += `<tr><td class="py-1">${a.name}</td><td class="py-1 text-gray-400">${a.account_type}</td><td class="py-1 text-gray-500">${a.institution || ''}</td></tr>`;
    }
    html += '</tbody></table></div>';
    root.innerHTML = html;
  } catch (e) {
    root.innerHTML = `<div class="card text-sm text-red-400">Failed: ${e.message || e}</div>`;
  }
}

async function loadLedgerTransactions() {
  const root = document.getElementById('ledger-transactions-body');
  if (!root) return;
  root.innerHTML = '<div class="text-sm text-gray-400">Loading…</div>';
  try {
    const url = '/api/ledger/transactions?limit=200' + (__ledgerEntityId ? `&entity_id=${__ledgerEntityId}` : '');
    const r = await authFetch(url);
    if (r.status === 503) { root.innerHTML = '<div class="card text-sm">Ledger DB not bind-mounted yet.</div>'; return; }
    const d = await r.json();
    const txs = d.transactions || [];
    if (txs.length === 0) { root.innerHTML = '<div class="card text-sm text-gray-400">No transactions.</div>'; return; }
    let html = `<div class="card"><div class="text-xs text-gray-400 mb-2">${txs.length} most-recent transactions</div>`;
    html += '<table class="text-sm w-full"><thead><tr><th class="text-left py-1">Date</th><th class="text-left py-1">Payee</th><th class="text-left py-1">Memo</th></tr></thead><tbody>';
    for (const t of txs) {
      html += `<tr><td class="py-1 text-gray-400">${t.txn_date}</td><td class="py-1">${t.payee || ''}</td><td class="py-1 text-gray-500">${(t.memo || '').slice(0, 60)}</td></tr>`;
    }
    html += '</tbody></table></div>';
    root.innerHTML = html;
  } catch (e) {
    root.innerHTML = `<div class="card text-sm text-red-400">Failed: ${e.message || e}</div>`;
  }
}

// ===== Trading snapshot — sibling syntaur-trading container =====
function fmtMoney(n) {
  if (n == null || isNaN(n)) return '—';
  const s = Math.abs(n).toLocaleString(undefined, {minimumFractionDigits: 2, maximumFractionDigits: 2});
  return (n < 0 ? '-' : '') + '$' + s;
}
function fmtSignedMoney(n) {
  if (n == null || isNaN(n)) return '—';
  const s = Math.abs(n).toLocaleString(undefined, {minimumFractionDigits: 2, maximumFractionDigits: 2});
  return (n >= 0 ? '+$' : '-$') + s;
}
function fmtAge(secs) {
  if (secs == null) return 'no data';
  if (secs < 90) return secs + 's ago';
  if (secs < 5400) return Math.round(secs/60) + 'm ago';
  if (secs < 86400) return (secs/3600).toFixed(1) + 'h ago';
  return Math.round(secs/86400) + 'd ago';
}
async function loadTradingSnapshot() {
  const root = document.getElementById('trading-snapshot-body');
  if (!root) return;
  root.innerHTML = '<div class="text-sm text-gray-400">Loading…</div>';
  try {
    const [accR, posR, actR, botsR, eqR] = await Promise.all([
      authFetch('/api/trading/account'),
      authFetch('/api/trading/positions'),
      authFetch('/api/trading/activity'),
      authFetch('/api/trading/bots'),
      authFetch('/api/trading/equity'),
    ]);
    if (accR.status === 503) {
      root.innerHTML = '<div class="card text-sm">Trading data dir not bind-mounted yet.</div>';
      return;
    }
    const acc = accR.ok ? await accR.json() : null;
    const pos = posR.ok ? await posR.json() : [];
    const act = actR.ok ? await actR.json() : [];
    const bots = botsR.ok ? await botsR.json() : {bots:[],kill_switch:{halted:false}};
    const eq = eqR.ok ? await eqR.json() : null;

    let html = `<div class="flex items-center justify-between mb-3"><div class="text-sm text-gray-400">Live snapshot of the syntaur-trading container</div><button class="btn-ghost text-xs" onclick="loadTradingSnapshot()">Refresh</button></div>`;

    // ----- Account card with KPIs -----
    if (acc) {
      const dayPlClass = acc.day_pnl >= 0 ? 'text-green-400' : 'text-red-400';
      const blocked = acc.trading_blocked || acc.account_blocked;
      const halted = bots.kill_switch && bots.kill_switch.halted;
      const statusBadge = halted
        ? '<span class="text-amber-300">⚠ KILL SWITCH ON</span>'
        : blocked
          ? '<span class="text-red-400">✕ blocked</span>'
          : `<span class="text-green-400">● ${acc.status}</span>`;
      html += `<div class="card mb-4"><div class="grid grid-cols-2 md:grid-cols-4 gap-4">
        <div><div class="text-xs text-gray-500">Equity</div><div class="text-2xl font-semibold">${fmtMoney(acc.equity)}</div><div class="text-xs ${dayPlClass}">${fmtSignedMoney(acc.day_pnl)} today (${acc.day_pnl_pct.toFixed(2)}%)</div></div>
        <div><div class="text-xs text-gray-500">Cash</div><div class="text-lg">${fmtMoney(acc.cash)}</div><div class="text-xs text-gray-500">${fmtMoney(acc.long_market_value)} in positions</div></div>
        <div><div class="text-xs text-gray-500">Buying power</div><div class="text-lg">${fmtMoney(acc.buying_power)}</div><div class="text-xs text-gray-500">DT count: ${acc.daytrade_count ?? 0}</div></div>
        <div><div class="text-xs text-gray-500">Status</div><div class="text-lg">${statusBadge}</div><div class="text-xs text-gray-500">Acct since ${(acc.created_at || '').slice(0,10)}</div></div>
      </div></div>`;
    }

    // ----- Bot heartbeat card -----
    html += `<div class="card mb-4"><div class="text-sm font-semibold mb-2">Bot health</div><table class="text-sm w-full"><thead><tr><th class="text-left py-1">Bot</th><th class="text-left py-1">State file</th><th class="text-left py-1">Heartbeat</th><th class="text-left py-1">Status</th></tr></thead><tbody>`;
    for (const b of (bots.bots || [])) {
      const cls = b.stale ? 'text-red-400' : (b.exists ? 'text-green-400' : 'text-gray-500');
      const label = b.stale ? 'STALE' : (b.exists ? 'OK' : 'MISSING');
      html += `<tr><td class="py-1">${b.name}</td><td class="py-1 text-gray-500 text-xs">${b.state_file}</td><td class="py-1">${fmtAge(b.age_secs)}</td><td class="py-1 ${cls}">${label}</td></tr>`;
    }
    html += '</tbody></table>';
    if (bots.monitor_state && bots.monitor_state.last_daily_summary) {
      html += `<div class="text-xs text-gray-500 mt-2">Last bot-monitor daily summary: ${bots.monitor_state.last_daily_summary}</div>`;
    } else if (bots.monitor_state && bots.monitor_state.extra && bots.monitor_state.extra.last_daily_summary) {
      html += `<div class="text-xs text-amber-300 mt-2">bot-monitor stale (last summary: ${bots.monitor_state.extra.last_daily_summary})</div>`;
    }
    html += '</div>';

    // ----- Positions card -----
    if (Array.isArray(pos) && pos.length) {
      html += `<div class="card mb-4"><div class="text-sm font-semibold mb-2">Open positions (${pos.length})</div><table class="text-sm w-full"><thead><tr><th class="text-left py-1">Symbol</th><th class="text-right py-1">Qty</th><th class="text-right py-1">Avg entry</th><th class="text-right py-1">Market value</th><th class="text-right py-1">Unrealized P&L</th></tr></thead><tbody>`;
      for (const p of pos) {
        const upl = parseFloat(p.unrealized_pl);
        const upct = parseFloat(p.unrealized_plpc) * 100;
        const cls = upl >= 0 ? 'text-green-400' : 'text-red-400';
        const qty = parseFloat(p.qty);
        if (Math.abs(qty) < 0.000001) continue;  // hide dust
        html += `<tr><td class="py-1 font-mono">${p.symbol}</td><td class="py-1 text-right">${qty.toLocaleString(undefined,{maximumFractionDigits:6})}</td><td class="py-1 text-right">${fmtMoney(parseFloat(p.avg_entry_price))}</td><td class="py-1 text-right">${fmtMoney(parseFloat(p.market_value))}</td><td class="py-1 text-right ${cls}">${fmtSignedMoney(upl)} (${upct.toFixed(2)}%)</td></tr>`;
      }
      html += '</tbody></table></div>';
    } else {
      html += '<div class="card mb-4 text-sm text-gray-400">No open positions.</div>';
    }

    // ----- Equity curve (last 30 daily samples) -----
    if (eq && Array.isArray(eq.timestamp) && eq.timestamp.length) {
      const ts = eq.timestamp, eqArr = eq.equity, plArr = eq.profit_loss;
      const start = parseFloat(eqArr[0]);
      const end = parseFloat(eqArr[eqArr.length-1]);
      const peak = Math.max(...eqArr.map(parseFloat));
      const trough = Math.min(...eqArr.slice(eqArr.indexOf(peak)).map(parseFloat));
      const ddPct = peak > 0 ? ((trough - peak)/peak*100) : 0;
      html += `<div class="card mb-4"><div class="text-sm font-semibold mb-2">Equity curve — last ${eqArr.length} samples</div>`;
      html += `<div class="grid grid-cols-3 gap-3 text-sm mb-2"><div><div class="text-xs text-gray-500">Window P&L</div><div class="${end-start >= 0 ? 'text-green-400' : 'text-red-400'}">${fmtSignedMoney(end-start)} (${start>0?((end-start)/start*100).toFixed(2):'0.00'}%)</div></div><div><div class="text-xs text-gray-500">Peak</div><div>${fmtMoney(peak)}</div></div><div><div class="text-xs text-gray-500">Max DD from peak</div><div class="${ddPct < -1 ? 'text-amber-300' : 'text-green-400'}">${ddPct.toFixed(2)}%</div></div></div>`;
      html += '<div class="text-xs"><table class="w-full"><thead><tr><th class="text-left py-1 text-gray-500">Date</th><th class="text-right py-1 text-gray-500">Equity</th><th class="text-right py-1 text-gray-500">Day P&L</th></tr></thead><tbody>';
      for (let i = Math.max(0, ts.length-10); i < ts.length; i++) {
        const d = new Date(ts[i]*1000).toISOString().slice(0,10);
        const dpl = parseFloat(plArr[i] || 0);
        const dplCls = dpl >= 0 ? 'text-green-400' : 'text-red-400';
        html += `<tr><td class="py-0.5 text-gray-400">${d}</td><td class="py-0.5 text-right">${fmtMoney(parseFloat(eqArr[i]))}</td><td class="py-0.5 text-right ${dplCls}">${fmtSignedMoney(dpl)}</td></tr>`;
      }
      html += '</tbody></table></div></div>';
    }

    // ----- Recent fills -----
    if (Array.isArray(act) && act.length) {
      html += `<div class="card mb-4"><div class="text-sm font-semibold mb-2">Recent fills (${act.length})</div><table class="text-sm w-full"><thead><tr><th class="text-left py-1">Time</th><th class="text-left py-1">Symbol</th><th class="text-left py-1">Side</th><th class="text-right py-1">Qty</th><th class="text-right py-1">Price</th></tr></thead><tbody>`;
      for (const f of act.slice(0, 30)) {
        if (!f.symbol) continue;
        const sideCls = f.side === 'buy' ? 'text-blue-300' : 'text-amber-300';
        const t = (f.transaction_time || '').slice(5,19).replace('T', ' ');
        html += `<tr><td class="py-0.5 text-gray-400 font-mono text-xs">${t}</td><td class="py-0.5 font-mono">${f.symbol}</td><td class="py-0.5 ${sideCls}">${f.side}</td><td class="py-0.5 text-right">${parseFloat(f.qty).toLocaleString(undefined,{maximumFractionDigits:6})}</td><td class="py-0.5 text-right">${fmtMoney(parseFloat(f.price))}</td></tr>`;
      }
      html += '</tbody></table></div>';
    }

    root.innerHTML = html;
  } catch (e) {
    root.innerHTML = `<div class="card text-sm text-red-400">Failed: ${e.message || e}</div>`;
  }
}

// ===== Section / sub-tab model =====
// Top-level sections, in display order (Investments first — most-frequent use).
// Each section lists its sub-tabs as [tab-id, display-label] pairs.
// The first sub-tab in each list is the default for that section.
const TAX_SECTIONS = {
  investments: { label: 'Investments', tabs: [['investments', 'Holdings & activity']] },
  documents:   { label: 'Documents',   tabs: [['documents', 'Tax documents'], ['receipts', 'Receipts']] },
  deductions:  { label: 'Deductions',  tabs: [['deductions', 'Summary'], ['expenses', 'Expenses'], ['property', 'Property'], ['depreciation', 'Depreciation'], ['credits', 'Credits']] },
  dashboard:   { label: 'Dashboard',   tabs: [['overview', 'Year overview']] },
  filing:      { label: 'Filing',      tabs: [['wizard', 'Wizard'], ['quarterly', 'Quarterly'], ['state', 'State'], ['entities', 'Entities'], ['insights', 'Insights'], ['connections', 'Bank & brokerage']] },
  ledger:      { label: 'Ledger',      tabs: [['ledger-overview', 'Overview'], ['ledger-accounts', 'Accounts'], ['ledger-transactions', 'Transactions']] },
  trading:     { label: 'Trading',     tabs: [['trading-snapshot', 'Snapshot']] },
};

let currentSection = 'investments';

function showSection(name) {
  const section = TAX_SECTIONS[name];
  if (!section) return;
  currentSection = name;
  // Highlight the top-level section button
  Object.keys(TAX_SECTIONS).forEach(k => {
    const btn = document.getElementById('sec-btn-' + k);
    if (btn) btn.classList.toggle('active', k === name);
  });
  // Repaint the sub-tab chip row
  const bar = document.getElementById('sub-tab-bar');
  if (bar) {
    bar.innerHTML = section.tabs.map(([id, label]) => {
      const badge = (id === 'deductions') ? '<span class="sub-tab-badge hidden" id="ded-badge"></span>' : '';
      return `<button class="sub-tab" data-tab="${id}" onclick="showTab('${id}')">${label}${badge}</button>`;
    }).join('');
  }
  // Show the section's default sub-tab (also triggers the panel's load fn)
  showTab(section.tabs[0][0]);
}

// ===== KPI strip — aggregates data from summary + investments endpoints =====
async function loadKpiStrip() {
  const portEl = document.getElementById('kpi-portfolio-value');
  const portSub = document.getElementById('kpi-portfolio-sub');
  const incEl = document.getElementById('kpi-income-value');
  const incSub = document.getElementById('kpi-income-sub');
  const dedEl = document.getElementById('kpi-deductions-value');
  const dedSub = document.getElementById('kpi-deductions-sub');
  const taxEl = document.getElementById('kpi-tax-value');
  const taxSub = document.getElementById('kpi-tax-sub');
  const taxTile = document.getElementById('kpi-tile-tax');
  if (!portEl) return;

  // Portfolio — live investment summary
  try {
    const r = await authFetch(`/api/financial/investments/summary`);
    if (r.ok) {
      const d = await r.json();
      if (d.total_value_cents != null) {
        portEl.textContent = fmtSignedDollars(d.total_value_cents).replace(/^-/, '');
        if (d.unrealized_pl_cents != null) {
          portSub.textContent = `${d.unrealized_pl_cents >= 0 ? '+' : ''}${fmtSignedDollars(d.unrealized_pl_cents)} unrealized`;
          portSub.style.color = d.unrealized_pl_cents >= 0 ? '#4ade80' : '#f87171';
        }
      } else {
        portEl.textContent = '—';
        portSub.textContent = 'No brokerage connected';
      }
    }
  } catch(e) { portEl.textContent = '—'; }

  // Income YTD, deductions, est refund — tax summary endpoint
  try {
    const r = await authFetch(`/api/tax/summary?start=${yearStart()}&end=${yearEnd()}`);
    if (r.ok) {
      const d = await r.json();
      if (d.income_ytd_display) {
        incEl.textContent = d.income_ytd_display;
      } else {
        incEl.textContent = '—';
        incSub.textContent = 'No income records yet';
      }
      dedEl.textContent = d.deductible_display || '$0.00';
      dedSub.textContent = `${d.receipt_count || 0} receipts`;
      if (d.est_refund_cents != null) {
        const refund = d.est_refund_cents;
        taxEl.textContent = (refund >= 0 ? '+' : '') + fmtSignedDollars(refund);
        taxSub.textContent = refund >= 0 ? 'Estimated refund' : 'Estimated owed';
        taxTile.classList.remove('ok','warn','bad');
        taxTile.classList.add(refund >= 0 ? 'ok' : 'warn');
      } else {
        taxEl.textContent = '—';
        taxSub.textContent = 'Need more data';
      }
    }
  } catch(e) {}
}

// ===== Deadline pill — next tax deadline within 60 days =====
function updateDeadlinePill() {
  const pill = document.getElementById('deadline-pill');
  const txt = document.getElementById('deadline-pill-text');
  if (!pill || !txt) return;
  const now = new Date();
  const yr = now.getFullYear();
  // Hardcoded IRS-calendar deadlines; dynamic year so this survives Jan 1 rollover
  const deadlines = [
    { d: new Date(yr, 3, 15),  label: 'April 15 filing' },
    { d: new Date(yr, 5, 15),  label: 'Q2 estimated' },
    { d: new Date(yr, 8, 15),  label: 'Q3 estimated' },
    { d: new Date(yr, 0, 15),  label: 'Q4 estimated (prev yr)' },
    { d: new Date(yr, 9, 15),  label: 'Extension final' },
  ].filter(x => x.d > now).sort((a,b) => a.d - b.d);
  if (!deadlines.length) { pill.classList.add('hidden'); return; }
  const next = deadlines[0];
  const days = Math.ceil((next.d - now) / 86400000);
  if (days > 60) { pill.classList.add('hidden'); return; }
  txt.textContent = `${next.label} — ${days}d`;
  pill.classList.remove('hidden');
}

// Overview
async function loadOverview() {
  try {
    const resp = await authFetch(`/api/tax/summary?start=${yearStart()}&end=${yearEnd()}`);
    const data = await resp.json();
    document.getElementById('sum-total').textContent = data.total_display || '$0.00';
    document.getElementById('sum-business').textContent = data.business_display || '$0.00';
    document.getElementById('sum-deductible').textContent = data.deductible_display || '$0.00';
    document.getElementById('sum-receipts').textContent = data.receipt_count || '0';
    const yr = selectedYear || yearStart().slice(0,4);
    document.getElementById('export-txf').href = `/api/tax/export/txf?year=${yr}`;
    document.getElementById('export-csv-irs').href = `/api/tax/export/csv-irs?year=${yr}`;
    document.getElementById('export-csv-raw').href = `/api/tax/export?start=${yearStart()}&end=${yearEnd()}`;
    document.getElementById('export-year-label').textContent = `Tax Year ${yr}`;
    // Pre-fill extension form from tax estimate
    loadExtensionData();

    const cats = data.categories || [];
    const list = document.getElementById('category-list');
    if (cats.length === 0) {
      list.innerHTML = '<p class="text-sm text-gray-600">No expenses yet. Start by logging an expense or uploading a receipt.</p>';
    } else {
      const biz = cats.filter(c => c.entity === 'business');
      const pers = cats.filter(c => c.entity === 'personal');
      const bizTotal = biz.reduce((s,c) => s + c.total_cents, 0);
      const persTotal = pers.reduce((s,c) => s + c.total_cents, 0);
      const maxCents = Math.max(...cats.map(c => c.total_cents), 1);

      function renderGroup(title, items, subtotal, color) {
        if (items.length === 0) return '';
        const subtotalDisplay = '$' + (Math.abs(subtotal)/100).toFixed(2).replace(/\B(?=(\d{3})+(?!\d))/g, ',');
        return `
          <div class="mb-4">
            <div class="flex items-center justify-between mb-2">
              <h4 class="text-xs font-semibold uppercase tracking-wider ${color}">${title}</h4>
              <span class="text-sm font-medium text-gray-300">${subtotalDisplay}</span>
            </div>
            ${items.map(c => {
              const pct = Math.max((c.total_cents / maxCents) * 100, 2);
              const notDed = !c.tax_deductible ? '<span class="text-xs text-yellow-500/60 ml-1">non-deductible</span>' : '';
              return `
              <button onclick="filterByCategory('${c.category}')" class="w-full text-left py-1.5 hover:bg-gray-800/50 rounded px-1 -mx-1 transition-colors">
                <div class="flex items-center justify-between">
                  <span class="text-sm text-gray-300">${c.category}${notDed}</span>
                  <div class="text-right">
                    <span class="text-sm font-medium text-gray-200">${c.total_display}</span>
                    <span class="text-xs text-gray-600 ml-1">${c.count}</span>
                  </div>
                </div>
                <div class="mt-1 h-1 rounded-full bg-gray-700 overflow-hidden">
                  <div class="h-full rounded-full ${c.entity === 'business' ? 'bg-oc-500' : 'bg-purple-500'}" style="width:${pct}%"></div>
                </div>
              </button>`;
            }).join('')}
          </div>`;
      }

      list.innerHTML = renderGroup('Business Expenses', biz, bizTotal, 'text-oc-400') +
                        renderGroup('Personal', pers, persTotal, 'text-purple-400');
    }

    analyzeDeductions(data);
  } catch(e) {
    console.warn('[tax] loadOverview:', e);
    for (const id of ['sum-total','sum-business','sum-deductible']) {
      const el = document.getElementById(id);
      if (el) el.textContent = '$0.00';
    }
    const rc = document.getElementById('sum-receipts');
    if (rc) rc.textContent = '0';
    const list = document.getElementById('category-list');
    if (list) list.innerHTML = '<p class="text-sm text-gray-600">Could not load summary. Try refreshing.</p>';
  }
}

function filterByCategory(cat) {
  showTab('expenses');
  // Set a global filter and reload
  window._catFilter = cat;
  loadExpenses();
}

// Expenses
async function loadExpenses() {
  try {
    const entity = document.getElementById('exp-filter-entity').value;
    let url = `/api/tax/expenses?start=${yearStart()}&end=${yearEnd()}`;
    if (entity) url += `&entity=${entity}`;
    const resp = await authFetch(url);
    const data = await resp.json();
    const list = document.getElementById('expense-list');
    let expenses = data.expenses || [];
    // Apply category filter if set
    if (window._catFilter) {
      expenses = expenses.filter(e => e.category === window._catFilter);
    }
    const filterLabel = window._catFilter
      ? `<div class="flex items-center gap-2 mb-3"><span class="text-sm text-gray-400">Filtered: <strong class="text-gray-200">${esc(window._catFilter)}</strong></span><button onclick="window._catFilter=null;loadExpenses()" class="text-xs text-oc-500 hover:text-oc-400">Clear</button></div>`
      : '';

    if (expenses.length === 0) {
      list.innerHTML = filterLabel + '<p class="text-sm text-gray-600">No expenses found.</p>';
    } else {
      list.innerHTML = filterLabel + expenses.map(e => `
        <div class="flex items-center justify-between py-2 border-b border-gray-700/50 last:border-0">
          <div class="flex-1 min-w-0">
            <div class="flex items-center gap-2">
              <p class="text-sm text-gray-300 truncate">${esc(e.vendor || '(no vendor)')}</p>
              ${e.receipt_id ? `<a href="/api/tax/receipts/${e.receipt_id}/image" target="_blank" class="text-xs text-oc-500 hover:text-oc-400 flex-shrink-0" title="View receipt">&#128206;</a>` : ''}
            </div>
            <p class="text-xs text-gray-500">${esc(e.expense_date || '—')} &middot; ${esc(e.category || 'Uncategorized')} &middot; <span class="${e.entity === 'business' ? 'text-oc-400' : 'text-purple-400'}">${esc(e.entity || 'personal')}</span></p>
            ${e.description ? '<p class="text-xs text-gray-600 truncate">' + esc(e.description) + '</p>' : ''}
          </div>
          <span class="text-sm font-medium text-gray-200 flex-shrink-0 ml-3">${esc(e.amount_display || '$0.00')}</span>
        </div>
      `).join('');
    }
  } catch(e) { console.log('expenses:', e); }
}

async function addExpense() {
  const vendor = document.getElementById('exp-vendor').value.trim();
  const amount = document.getElementById('exp-amount').value.trim();
  const date = document.getElementById('exp-date').value;
  if (!vendor || !amount || !date) { document.getElementById('exp-status').textContent = 'Fill in vendor, amount, and date'; return; }

  const cat = document.getElementById('exp-category');
  const body = {
    token, vendor, amount, date,
    category: cat.value || null,
    description: document.getElementById('exp-desc').value || null,
    entity: document.getElementById('exp-entity').value,
  };

  try {
    const resp = await authFetch('/api/tax/expenses', {
      method: 'POST', headers: {'Content-Type': 'application/json'},
      body: JSON.stringify(body)
    });
    const data = await resp.json();
    document.getElementById('exp-status').textContent = `Added: ${data.amount_display} at ${vendor}`;
    document.getElementById('exp-vendor').value = '';
    document.getElementById('exp-amount').value = '';
    document.getElementById('exp-desc').value = '';
    loadExpenses();
    loadOverview();
  } catch(e) { document.getElementById('exp-status').textContent = 'Error adding expense'; }
}

// Receipts
async function loadReceipts() {
  try {
    const resp = await authFetch(`/api/tax/receipts`);
    const data = await resp.json();
    const list = document.getElementById('receipt-list');
    const receipts = data.receipts || [];
    if (receipts.length === 0) {
      list.innerHTML = '<p class="text-sm text-gray-600 col-span-full">No receipts uploaded yet. Upload a photo or PDF above.</p>';
    } else {
      list.innerHTML = receipts.map(r => `
        <div class="bg-gray-900 rounded-lg border border-gray-700 overflow-hidden flex flex-col">
          <a href="${r.image_url}" target="_blank" rel="noopener" class="block group relative bg-gray-800" title="Open full scan in new tab">
            <img src="${r.image_url}" class="w-full h-32 object-cover" alt="Scan of ${esc(r.vendor || 'receipt')}" onerror="this.parentElement.classList.add('rcpt-noimg');this.style.display='none'">
            <div class="rcpt-noimg-fallback absolute inset-0 hidden items-center justify-center text-xs text-gray-500">
              <span>📄 PDF / no preview — click to open</span>
            </div>
          </a>
          <div class="p-3 flex-1 flex flex-col">
            <p class="text-sm font-medium text-gray-300">${esc(r.vendor || 'Scanning...')}</p>
            <p class="text-xs text-gray-500">${r.receipt_date || ''} ${r.amount_display ? '&middot; ' + r.amount_display : ''}</p>
            <p class="text-xs text-gray-600">${r.category || ''}</p>
            <div class="flex items-center justify-between mt-2">
              <span class="inline-block text-xs px-1.5 py-0.5 rounded ${r.status === 'scanned' ? 'bg-green-900/50 text-green-400' : 'bg-yellow-900/50 text-yellow-400'}">${r.status}</span>
              <a href="${r.image_url}" target="_blank" rel="noopener" class="text-xs text-oc-500 hover:text-oc-400 underline" title="Open full scan in new tab">View scan ↗</a>
            </div>
          </div>
        </div>
      `).join('');
      // Show fallback caption when img onerror fired (parent gets .rcpt-noimg).
      list.querySelectorAll('.rcpt-noimg .rcpt-noimg-fallback').forEach(el => {
        el.classList.remove('hidden');
        el.classList.add('flex');
      });
    }
  } catch(e) { console.log('receipts:', e); }
}

async function uploadReceipt(input) {
  const file = input.files[0];
  if (!file) return;
  const status = document.getElementById('receipt-status');
  status.textContent = 'Uploading...';

  try {
    const resp = await fetch(`/api/tax/receipts`, {
      method: 'POST',
      headers: { 'Content-Type': file.type },
      body: file
    });
    const data = await resp.json();
    status.textContent = data.message || 'Uploaded!';
    setTimeout(() => loadReceipts(), 3000); // Wait for vision scan
    loadReceipts();
  } catch(e) { status.textContent = 'Upload failed'; }
  input.value = '';
}

// Load categories for the form
async function loadCategories() {
  try {
    const resp = await authFetch(`/api/tax/categories`);
    const data = await resp.json();
    const sel = document.getElementById('exp-category');
    sel.innerHTML = '<option value="">Select category</option>' +
      (data.categories || []).map(c =>
        `<option value="${c.name}">${c.name} (${c.entity})</option>`
      ).join('');
  } catch(e) {}
}

function esc(s) { return (s||'').replace(/&/g,'&amp;').replace(/</g,'&lt;').replace(/>/g,'&gt;'); }

// Set today's date as default
document.getElementById('exp-date').value = new Date().toISOString().split('T')[0];

// ── Tax Chat (persistent) ──
let taxChatSending = false;
let taxConvId = null;

function taxChat(msg) {
  document.getElementById('tax-chat-input').value = msg;
  sendTaxChat();
}

function clearTaxChat() {
  taxConvId = null;
  document.getElementById('tax-chat-messages').innerHTML = `
    <div class="flex gap-2">
      <img src="/agent-avatar/positron" class="w-12 h-12 rounded-full flex-shrink-0 mt-0.5" alt="">
      <div class="text-sm text-gray-400">
        <p>New tax conversation started. How can I help?</p>
        <div class="flex flex-wrap gap-1 mt-2">
          <button onclick="taxChat('Show my expense summary for ${selectedYear}')" class="text-xs bg-gray-800 hover:bg-gray-700 border border-gray-700 rounded-full px-2.5 py-1 transition-colors">${selectedYear} summary</button>
          <button onclick="taxChat('What are my deductible expenses?')" class="text-xs bg-gray-800 hover:bg-gray-700 border border-gray-700 rounded-full px-2.5 py-1 transition-colors">Deductible?</button>
        </div>
      </div>
    </div>`;
}

// Load existing tax conversation on page load
async function loadTaxConversation() {
  try {
    const resp = await authFetch(`/api/conversations?agent=main&limit=5`);
    const data = await resp.json();
    const convs = (data.conversations || []).filter(c =>
      c.title && c.title.toLowerCase().includes('tax')
    );
    if (convs.length > 0) {
      taxConvId = convs[0].id;
      await loadTaxMessages(taxConvId);
    }
  } catch(e) {}
}

async function loadTaxMessages(convId) {
  try {
    const resp = await authFetch(`/api/conversations/${convId}`);
    const data = await resp.json();
    const messages = data.messages || [];
    if (messages.length === 0) return;

    const container = document.getElementById('tax-chat-messages');
    container.innerHTML = '';

    for (const msg of messages) {
      if (msg.role === 'user') {
        // Strip the tax context injection from display
        let text = msg.content || '';
        const ctxIdx = text.indexOf('\n\n[TAX MODULE CONTEXT');
        if (ctxIdx > 0) text = text.substring(0, ctxIdx);

        const el = document.createElement('div');
        el.className = 'flex gap-2 justify-end';
        el.innerHTML = `<div class="max-w-[85%] bg-oc-900/40 border border-oc-800/40 rounded-xl rounded-br-sm px-3 py-2 text-sm text-gray-200">${esc(text)}</div>`;
        container.appendChild(el);
      } else if (msg.role === 'assistant' && msg.content) {
        const el = document.createElement('div');
        el.className = 'flex gap-2';
        el.innerHTML = `<img src="/agent-avatar/positron" class="w-12 h-12 rounded-full flex-shrink-0 mt-0.5" alt="">
          <div class="flex-1 text-sm text-gray-300 leading-relaxed">${esc(msg.content).replace(/\n/g,'<br>')}</div>`;
        container.appendChild(el);
      }
    }
    container.scrollTop = container.scrollHeight;
  } catch(e) {}
}

async function sendTaxChat() {
  if (taxChatSending) return;
  const input = document.getElementById('tax-chat-input');
  const msg = input.value.trim();
  if (!msg) return;
  input.value = '';
  input.style.height = 'auto';
  taxChatSending = true;

  const messages = document.getElementById('tax-chat-messages');
  const btn = document.getElementById('tax-send-btn');
  btn.disabled = true;

  // User message
  const userEl = document.createElement('div');
  userEl.className = 'flex gap-2 justify-end';
  userEl.innerHTML = `<div class="max-w-[85%] bg-oc-900/40 border border-oc-800/40 rounded-xl rounded-br-sm px-3 py-2 text-sm text-gray-200">${esc(msg)}</div>`;
  messages.appendChild(userEl);
  positronMsgCount++;

  // Thinking
  const aiEl = document.createElement('div');
  aiEl.className = 'flex gap-2';
  aiEl.innerHTML = `<img src="/agent-avatar/positron" class="w-12 h-12 rounded-full flex-shrink-0 mt-0.5" alt="">
    <div class="flex-1 text-sm" id="tax-ai-resp">
      <div class="flex gap-1 py-1"><span class="w-1.5 h-1.5 rounded-full bg-oc-500 animate-bounce"></span><span class="w-1.5 h-1.5 rounded-full bg-oc-500 animate-bounce" style="animation-delay:150ms"></span><span class="w-1.5 h-1.5 rounded-full bg-oc-500 animate-bounce" style="animation-delay:300ms"></span></div>
    </div>`;
  messages.appendChild(aiEl);
  messages.scrollTop = messages.scrollHeight;
  updatePositronLog();

  const respEl = aiEl.querySelector('#tax-ai-resp');
  respEl.removeAttribute('id');

  try {
    // Create tax conversation if we don't have one
    if (!taxConvId) {
      try {
        const cr = await authFetch('/api/conversations', {
          method: 'POST', headers: {'Content-Type': 'application/json'},
          body: JSON.stringify({ agent: 'main', title: `Tax Assistant ${selectedYear}` })
        });
        const cd = await cr.json();
        if (cd.id) taxConvId = cd.id;
      } catch(e) {}
    }

    // Fetch tax context
    let taxContext = '';
    try {
      const [sumResp, incResp] = await Promise.all([
        authFetch(`/api/tax/summary?start=${yearStart()}&end=${yearEnd()}`),
        authFetch(`/api/tax/income?year=${selectedYear}`).catch(() => null),
      ]);
      const sum = await sumResp.json();
      const inc = incResp ? await incResp.json().catch(() => ({})) : {};
      const cats = (sum.categories || []).map(c => `${c.category} (${c.entity}): ${c.total_display} (${c.count} items)`).join('\n');
      const incLines = (inc.income || []).map(i => `${i.source}: $${(i.amount_cents/100).toFixed(2)} (${i.description})`).join('\n');
      taxContext = `\n\n[TAX MODULE CONTEXT - You have FULL access to the user's ${selectedYear} financial data. DO NOT ask for information. Answer directly.\n\n` +
        `INCOME:\n${incLines || 'No income data'}\nTotal gross income: ${inc.total_display || 'unknown'}\n\n` +
        `EXPENSES:\nTotal expenses: ${sum.total_display}\nBusiness expenses: ${sum.business_display}\nTax deductible: ${sum.deductible_display}\nReceipts on file: ${sum.receipt_count}\n\nBy category:\n${cats}\n\n` +
        `Filing status: Married filing jointly (2 W-2 earners)\n` +
        `INSTRUCTIONS: Use tools (estimate_tax, expense_summary, get_income) to get data. Show your math. FICA is NOT a credit against income tax.]`;
    } catch(e) {}

    // Use streaming endpoint for tool visibility
    const startResp = await authFetch('/api/message/start', {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({ message: msg + taxContext, agent: 'main', token, conversation_id: taxConvId })
    });
    const startData = await startResp.json();
    const turnId = startData.turn_id;

    if (!turnId) {
      // Fallback to non-streaming
      const resp = await authFetch('/api/message', {
        method: 'POST',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify({ message: msg + taxContext, agent: 'main', token, conversation_id: taxConvId })
      });
      const data = await resp.json();
      respEl.innerHTML = data.response
        ? `<div class="text-gray-300 leading-relaxed text-sm">${esc(data.response).replace(/\n/g,'<br>')}</div>`
        : `<p class="text-red-400 text-sm">${esc(data.error || 'No response')}</p>`;
    } else {
      // Stream with tool visibility
      let toolsUsed = [];
      const evtSource = new EventSource(`/api/message/${turnId}/stream`);
      evtSource.onmessage = (event) => {
        const ev = JSON.parse(event.data);
        switch (ev.event) {
          case 'started':
            respEl.innerHTML = '<p class="text-xs text-gray-500">Analyzing your tax data...</p>';
            break;
          case 'llm_call_started':
            respEl.innerHTML = `<p class="text-xs text-gray-500">${ev.round > 0 ? 'Processing results...' : 'Thinking about your question...'}</p>`;
            break;
          case 'tool_call_started':
            toolsUsed.push(ev.tool_name);
            const toolLabels = {
              'estimate_tax': 'Calculating tax liability...',
              'expense_summary': 'Reviewing expenses...',
              'get_income': 'Looking up income records...',
              'log_expense': 'Logging expense...',
              'list_todos': 'Checking tasks...',
              'web_search': 'Searching...',
            };
            const label = toolLabels[ev.tool_name] || `Using ${ev.tool_name}...`;
            respEl.innerHTML = `<div class="space-y-1">${toolsUsed.map(t => {
              const l = toolLabels[t] || `Using ${t}`;
              return `<div class="flex items-center gap-2 text-xs text-gray-500"><span class="w-1 h-1 rounded-full bg-oc-500 ${t === ev.tool_name ? 'animate-pulse' : ''}"></span>${l}</div>`;
            }).join('')}</div>`;
            break;
          case 'tool_call_completed':
            // Update dot color
            break;
          case 'complete':
            evtSource.close();
            respEl.innerHTML = `<div class="text-gray-300 leading-relaxed text-sm">${esc(ev.response).replace(/\n/g,'<br>')}</div>`;
            messages.scrollTop = messages.scrollHeight;
            taxChatSending = false; btn.disabled = false; input.focus();
            positronMsgCount++; updatePositronLog();
            if (msg.toLowerCase().match(/log|add|expense/)) { loadOverview(); loadExpenses(); loadKpiStrip(); }
            return;
          case 'error':
            evtSource.close();
            respEl.innerHTML = `<p class="text-red-400 text-sm">${esc(ev.message)}</p>`;
            taxChatSending = false; btn.disabled = false;
            return;
        }
        messages.scrollTop = messages.scrollHeight;
      };
      evtSource.onerror = () => {
        evtSource.close();
        if (respEl.querySelector('.animate-pulse')) respEl.innerHTML = '<p class="text-red-400 text-sm">Connection lost</p>';
        taxChatSending = false; btn.disabled = false;
      };
      return;
    }
  } catch(e) {
    respEl.innerHTML = `<p class="text-red-400 text-sm">Error: ${e.message}</p>`;
  }

  messages.scrollTop = messages.scrollHeight;
  btn.disabled = false;
  taxChatSending = false;
  input.focus();
}

// ── Year selector ──
let selectedYear = new Date().getFullYear();

function initYearSelector() {
  const sel = document.getElementById('year-select');
  const current = new Date().getFullYear();
  // Show current year and a few previous
  for (let y = current; y >= current - 5; y--) {
    const opt = document.createElement('option');
    opt.value = y;
    opt.textContent = y;
    sel.appendChild(opt);
  }
  // Default to previous year during tax season (Jan-Oct)
  // Most users are working on the prior year's taxes
  const month = new Date().getMonth(); // 0-indexed
  if (month < 10) { // Before November, default to prior year
    sel.value = current - 1;
    selectedYear = current - 1;
  } else {
    sel.value = current;
  }
}

function changeYear() {
  selectedYear = parseInt(document.getElementById('year-select').value);
  loadOverview();
  loadExpenses();
}

function yearStart() { return `${selectedYear}-01-01`; }
function yearEnd() { return `${selectedYear}-12-31`; }

// ── Tax Documents ──
const docTypeLabels = {
  'w2': 'W-2', '1099_int': '1099-INT', '1099_div': '1099-DIV', '1099_b': '1099-B',
  '1099_misc': '1099-MISC', '1099_nec': '1099-NEC', '1095_c': '1095-C',
  'property_tax_statement': 'Property Tax', 'mortgage_statement': 'Mortgage Statement',
  'receipt': 'Receipt', 'bank_statement': 'Bank Statement', 'credit_card_statement': 'Credit Card Statement',
  'insurance_policy': 'Insurance', 'invoice': 'Invoice', 'other': 'Other', 'unknown': 'Scanning...'
};

async function uploadTaxDoc(input) {
  const file = input.files[0];
  if (!file) return;
  const status = document.getElementById('doc-upload-status');
  status.textContent = 'Uploading & classifying...';
  try {
    // Use smart upload endpoint — auto-routes to receipt, document, or statement handler
    const resp = await fetch(`/api/tax/upload`, {
      method: 'POST', headers: { 'Content-Type': file.type }, body: file
    });
    const data = await resp.json();
    const routedTo = data.routed_to || 'document';
    status.textContent = data.message || 'Uploaded!';
    status.innerHTML += ` <span class="text-oc-400">(${routedTo})</span>`;
    // Refresh appropriate tab after processing
    setTimeout(() => {
      loadDocuments();
      if (routedTo === 'receipt') loadReceipts();
      if (routedTo === 'statement') loadStatementTransactions();
    }, 5000);
    loadDocuments();
  } catch(e) {
    // Fallback to legacy endpoint
    try {
      const resp2 = await fetch(`/api/tax/documents`, {
        method: 'POST', headers: { 'Content-Type': file.type }, body: file
      });
      const data2 = await resp2.json();
      status.textContent = data2.message || 'Uploaded!';
      setTimeout(() => loadDocuments(), 5000);
      loadDocuments();
    } catch(e2) { status.textContent = 'Upload failed'; }
  }
  input.value = '';
}

async function loadDocuments() {
  try {
    const resp = await authFetch(`/api/tax/documents?year=${selectedYear}`);
    const data = await resp.json();
    const docs = data.documents || [];
    const list = document.getElementById('doc-list');
    if (docs.length === 0) {
      list.innerHTML = '<p class="text-sm text-gray-600">No tax documents uploaded yet for ' + selectedYear + '.</p>';
      return;
    }

    // Group by type
    const groups = {};
    for (const d of docs) {
      const type = d.doc_type || 'other';
      if (!groups[type]) groups[type] = [];
      groups[type].push(d);
    }

    list.innerHTML = Object.entries(groups).map(([type, items]) => `
      <div class="mb-4">
        <h4 class="text-sm font-semibold text-gray-300 mb-2">${docTypeLabels[type] || type} (${items.length})</h4>
        <div class="space-y-2">
          ${items.map(d => {
            const fields = d.fields || {};
            let fieldsHtml = '';
            if (type === 'w2') {
              fieldsHtml = `
                <div class="grid grid-cols-2 gap-x-4 gap-y-1 mt-2 text-xs">
                  <div class="flex justify-between"><span class="text-gray-500">Box 1 Wages:</span>
                    <input type="text" value="${fields.box1_wages || ''}" class="bg-transparent text-right text-gray-300 w-24 border-b border-transparent hover:border-gray-600 focus:border-oc-500 outline-none" onchange="updateDocField(${d.id},'box1_wages',this.value)"></div>
                  <div class="flex justify-between"><span class="text-gray-500">Box 2 Fed Withheld:</span>
                    <input type="text" value="${fields.box2_fed_withheld || ''}" class="bg-transparent text-right text-gray-300 w-24 border-b border-transparent hover:border-gray-600 focus:border-oc-500 outline-none" onchange="updateDocField(${d.id},'box2_fed_withheld',this.value)"></div>
                  <div class="flex justify-between"><span class="text-gray-500">Box 3 SS Wages:</span>
                    <input type="text" value="${fields.box3_ss_wages || ''}" class="bg-transparent text-right text-gray-300 w-24 border-b border-transparent hover:border-gray-600 focus:border-oc-500 outline-none" onchange="updateDocField(${d.id},'box3_ss_wages',this.value)"></div>
                  <div class="flex justify-between"><span class="text-gray-500">Box 4 SS Withheld:</span>
                    <input type="text" value="${fields.box4_ss_withheld || ''}" class="bg-transparent text-right text-gray-300 w-24 border-b border-transparent hover:border-gray-600 focus:border-oc-500 outline-none" onchange="updateDocField(${d.id},'box4_ss_withheld',this.value)"></div>
                  <div class="flex justify-between"><span class="text-gray-500">Box 5 Medicare Wages:</span>
                    <input type="text" value="${fields.box5_medicare_wages || ''}" class="bg-transparent text-right text-gray-300 w-24 border-b border-transparent hover:border-gray-600 focus:border-oc-500 outline-none" onchange="updateDocField(${d.id},'box5_medicare_wages',this.value)"></div>
                  <div class="flex justify-between"><span class="text-gray-500">Box 6 Medicare Withheld:</span>
                    <input type="text" value="${fields.box6_medicare_withheld || ''}" class="bg-transparent text-right text-gray-300 w-24 border-b border-transparent hover:border-gray-600 focus:border-oc-500 outline-none" onchange="updateDocField(${d.id},'box6_medicare_withheld',this.value)"></div>
                  <div class="flex justify-between"><span class="text-gray-500">State:</span>
                    <input type="text" value="${fields.state || ''}" class="bg-transparent text-right text-gray-300 w-24 border-b border-transparent hover:border-gray-600 focus:border-oc-500 outline-none" onchange="updateDocField(${d.id},'state',this.value)"></div>
                  <div class="flex justify-between"><span class="text-gray-500">Box 17 State Withheld:</span>
                    <input type="text" value="${fields.box17_state_withheld || ''}" class="bg-transparent text-right text-gray-300 w-24 border-b border-transparent hover:border-gray-600 focus:border-oc-500 outline-none" onchange="updateDocField(${d.id},'box17_state_withheld',this.value)"></div>
                </div>
                <p class="text-xs text-gray-600 mt-1">Employee: ${esc(fields.employee_name || '?')}</p>`;
            } else if (type.startsWith('1099')) {
              const amt = fields.box1_interest || fields.box1a_ordinary || fields.box1_nonemployee_comp || fields.total_proceeds || 0;
              fieldsHtml = `<p class="text-xs text-gray-400 mt-1">Amount: <input type="text" value="${amt}" class="bg-transparent text-gray-300 w-20 border-b border-transparent hover:border-gray-600 focus:border-oc-500 outline-none" onchange="updateDocField(${d.id},'amount',this.value)"></p>`;
            } else if (type === 'mortgage_statement') {
              fieldsHtml = `<p class="text-xs text-gray-400 mt-1">Interest Paid: <input type="text" value="${fields.box1_interest_paid || ''}" class="bg-transparent text-gray-300 w-20 border-b border-transparent hover:border-gray-600 focus:border-oc-500 outline-none" onchange="updateDocField(${d.id},'box1_interest_paid',this.value)"></p>`;
            } else if (type === 'property_tax_statement') {
              fieldsHtml = `<p class="text-xs text-gray-400 mt-1">Amount: <input type="text" value="${fields.amount || ''}" class="bg-transparent text-gray-300 w-20 border-b border-transparent hover:border-gray-600 focus:border-oc-500 outline-none" onchange="updateDocField(${d.id},'amount',this.value)"></p>`;
            }
            return `
            <div class="p-3 rounded-lg bg-gray-900 border border-gray-700/50">
              <div class="flex items-center justify-between">
                <div class="flex-1">
                  <p class="text-sm text-gray-300">${esc(d.issuer || 'Unknown')}</p>
                  <p class="text-xs text-gray-600">Year: ${d.tax_year || '?'}</p>
                </div>
                <div class="flex items-center gap-2">
                  <span class="text-xs px-2 py-0.5 rounded ${
                    d.status === 'scanned' ? 'bg-green-900/50 text-green-400' :
                    d.status === 'duplicate' ? 'bg-red-900/50 text-red-400' :
                    'bg-yellow-900/50 text-yellow-400'
                  }">${d.status === 'duplicate' ? 'Possible duplicate' : d.status}</span>
                  <a href="${d.image_url}" target="_blank" class="text-xs text-oc-500 hover:text-oc-400">View Original</a>
                </div>
              </div>
              ${fieldsHtml}
              ${d.status === 'duplicate' ? `
                <div class="mt-2 p-2 rounded bg-red-900/20 border border-red-800/30 flex items-center justify-between">
                  <p class="text-xs text-red-300">This looks like a duplicate of an existing document.</p>
                  <div class="flex gap-2 flex-shrink-0">
                    <button onclick="keepDoc(${d.id})" class="text-xs bg-gray-700 hover:bg-gray-600 px-2 py-1 rounded transition-colors">Keep</button>
                    <button onclick="discardDoc(${d.id})" class="text-xs bg-red-800 hover:bg-red-700 px-2 py-1 rounded transition-colors">Discard</button>
                  </div>
                </div>
              ` : ''}
            </div>`;
          }).join('')}
        </div>
      </div>
    `).join('');
  } catch(e) { console.log('docs:', e); }
}

async function keepDoc(docId) {
  try {
    await authFetch(`/api/tax/documents/${docId}/status`, {
      method: 'PUT',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({ status: 'scanned' })
    });
    loadDocuments();
  } catch(e) { console.log('keep:', e); }
}

async function discardDoc(docId) {
  try {
    await authFetch(`/api/tax/documents/${docId}/status`, {
      method: 'PUT',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({ status: 'discarded' })
    });
    loadDocuments();
  } catch(e) { console.log('discard:', e); }
}

async function updateDocField(docId, field, value) {
  try {
    await authFetch(`/api/tax/documents/${docId}/field`, {
      method: 'PUT',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({ field, value })
    });
  } catch(e) { console.log('update field:', e); }
}

// ── Deduction Calculators ──

function recalcHomeOffice() {
  const sqft = parseFloat(document.getElementById('ho-sqft')?.value || 0);
  const total = parseFloat(document.getElementById('ho-total-sqft')?.value || 1);
  const pct = sqft / total;
  const pctEl = document.getElementById('ho-pct');
  if (pctEl) pctEl.textContent = (pct * 100).toFixed(2) + '%';

  const mortgage = parseFloat(document.getElementById('ho-mortgage')?.value || 0);
  const proptax = parseFloat(document.getElementById('ho-proptax')?.value || 0);
  const insurance = parseFloat(document.getElementById('ho-insurance')?.value || 0);
  const utilities = parseFloat(document.getElementById('ho-utilities')?.value || 0);
  const total_expenses = mortgage + proptax + insurance + utilities;
  const deduction = total_expenses * pct;

  const el = document.getElementById('ho-actual');
  if (el) el.textContent = '$' + deduction.toFixed(2).replace(/\B(?=(\d{3})+(?!\d))/g, ',');
}

async function saveHomeOfficeDeduction() {
  const sqft = parseFloat(document.getElementById('ho-sqft')?.value || 0);
  const total = parseFloat(document.getElementById('ho-total-sqft')?.value || 1);
  const pct = sqft / total;
  const mortgage = parseFloat(document.getElementById('ho-mortgage')?.value || 0);
  const proptax = parseFloat(document.getElementById('ho-proptax')?.value || 0);
  const insurance = parseFloat(document.getElementById('ho-insurance')?.value || 0);
  const utilities = parseFloat(document.getElementById('ho-utilities')?.value || 0);
  const deduction = (mortgage + proptax + insurance + utilities) * pct;

  if (deduction <= 0) { return; }

  try {
    await authFetch('/api/tax/expenses', {
      method: 'POST', headers: {'Content-Type': 'application/json'},
      body: JSON.stringify({
        token, vendor: 'Home Office / Workshop Deduction',
        amount: deduction.toFixed(2), category: 'Home Office',
        date: `${selectedYear}-12-31`,
        description: `${sqft} sqft workshop in ${total} sqft home (${(pct*100).toFixed(1)}% business use). Actual method.`,
        entity: 'business'
      })
    });
    document.getElementById('ho-result').classList.remove('hidden');
    document.getElementById('ho-result').className = 'text-xs text-green-400';
    document.getElementById('ho-result').textContent = `Saved $${deduction.toFixed(2)} home office deduction!`;
    loadOverview();
  } catch(e) {
    document.getElementById('ho-result').className = 'text-xs text-red-400';
    document.getElementById('ho-result').textContent = 'Error saving';
  }
}

function recalcMileage() {
  const miles = parseFloat(document.getElementById('mi-miles')?.value || 0);
  const deduction = miles * 0.67;
  const el = document.getElementById('mi-result');
  if (el) el.textContent = '$' + deduction.toFixed(2).replace(/\B(?=(\d{3})+(?!\d))/g, ',');
}

async function saveMileageDeduction() {
  const miles = parseFloat(document.getElementById('mi-miles')?.value || 0);
  if (miles <= 0) return;
  const deduction = miles * 0.67;

  try {
    await authFetch('/api/tax/expenses', {
      method: 'POST', headers: {'Content-Type': 'application/json'},
      body: JSON.stringify({
        token, vendor: 'Business Mileage Deduction',
        amount: deduction.toFixed(2), category: 'Vehicle & Mileage',
        date: `${selectedYear}-12-31`,
        description: `${miles} business miles @ $0.67/mile (IRS standard rate ${selectedYear})`,
        entity: 'business'
      })
    });
    loadOverview();
  } catch(e) {}
}

// Auto-calculate home office on load
setTimeout(recalcHomeOffice, 500);

// ── Property Profile ──
async function loadPropertyProfile() {
  try {
    const resp = await authFetch(`/api/tax/property`);
    const data = await resp.json();
    const profiles = data.profiles || [];
    if (profiles.length > 0) {
      const p = profiles[0];
      document.getElementById('prop-address').value = p.address || '';
      document.getElementById('prop-total-sqft').value = p.total_sqft || '';
      document.getElementById('prop-workshop-sqft').value = p.workshop_sqft || '';
      document.getElementById('prop-purchase-price').value = p.purchase_price_cents ? (p.purchase_price_cents / 100).toFixed(0) : '';
      document.getElementById('prop-purchase-date').value = p.purchase_date || '';
      document.getElementById('prop-building-value').value = p.building_value_cents ? (p.building_value_cents / 100).toFixed(0) : '';
      document.getElementById('prop-land-value').value = p.land_value_cents ? (p.land_value_cents / 100).toFixed(0) : '';
      document.getElementById('prop-land-ratio').value = p.land_ratio ? p.land_ratio.toFixed(4) : '';
      document.getElementById('prop-property-tax').value = p.annual_property_tax_cents ? (p.annual_property_tax_cents / 100).toFixed(2) : '';
      document.getElementById('prop-insurance').value = p.annual_insurance_cents ? (p.annual_insurance_cents / 100).toFixed(2) : '';
      document.getElementById('prop-mortgage-lender').value = p.mortgage_lender || '';
      document.getElementById('prop-mortgage-interest').value = p.mortgage_interest_cents ? (p.mortgage_interest_cents / 100).toFixed(2) : '';
      document.getElementById('prop-notes').value = p.notes || '';

      // Show depreciation
      if (p.building_value_cents) {
        const basis = p.building_value_cents;
        const annual = Math.round(basis / 2750);
        const biz = p.workshop_sqft && p.total_sqft ? (p.workshop_sqft / p.total_sqft) : 0;
        const bizAnnual = Math.round(annual * biz);
        document.getElementById('depreciation-result').innerHTML = `
          <div class="grid grid-cols-2 gap-x-4 gap-y-1 text-xs">
            <div class="flex justify-between"><span class="text-gray-500">Building basis:</span><span class="text-gray-300">${fmtDollars(basis/100)}</span></div>
            <div class="flex justify-between"><span class="text-gray-500">Depreciation period:</span><span class="text-gray-300">27.5 years (residential)</span></div>
            <div class="flex justify-between"><span class="text-gray-500">Annual depreciation:</span><span class="text-gray-300">${fmtDollars(annual/100)}/year</span></div>
            <div class="flex justify-between"><span class="text-gray-500">Business portion (${(biz*100).toFixed(2)}%):</span><span class="text-green-400 font-medium">${fmtDollars(bizAnnual/100)}/year</span></div>
          </div>`;
      }
    }
    loadStatementTransactions();
  } catch(e) { console.log('property:', e); }
}

async function saveProperty() {
  const addr = document.getElementById('prop-address').value.trim();
  if (!addr) { document.getElementById('prop-status').textContent = 'Address is required'; return; }

  const buildingVal = parseFloat(document.getElementById('prop-building-value').value || 0);
  const landVal = parseFloat(document.getElementById('prop-land-value').value || 0);
  const totalVal = buildingVal + landVal;
  const landRatio = totalVal > 0 ? landVal / totalVal : null;
  if (landRatio !== null) document.getElementById('prop-land-ratio').value = landRatio.toFixed(4);

  const body = {
    token, address: addr,
    total_sqft: parseInt(document.getElementById('prop-total-sqft').value) || null,
    workshop_sqft: parseInt(document.getElementById('prop-workshop-sqft').value) || null,
    purchase_price: document.getElementById('prop-purchase-price').value || null,
    purchase_date: document.getElementById('prop-purchase-date').value || null,
    building_value: document.getElementById('prop-building-value').value || null,
    land_value: document.getElementById('prop-land-value').value || null,
    land_ratio: landRatio,
    annual_property_tax: document.getElementById('prop-property-tax').value || null,
    annual_insurance: document.getElementById('prop-insurance').value || null,
    mortgage_lender: document.getElementById('prop-mortgage-lender').value || null,
    mortgage_interest: document.getElementById('prop-mortgage-interest').value || null,
    notes: document.getElementById('prop-notes').value || null,
  };

  try {
    const resp = await authFetch('/api/tax/property', {
      method: 'POST', headers: {'Content-Type': 'application/json'}, body: JSON.stringify(body)
    });
    const data = await resp.json();
    document.getElementById('prop-status').textContent = data.success ? 'Saved!' : 'Error saving';
    document.getElementById('prop-status').className = data.success ? 'text-xs text-green-400' : 'text-xs text-red-400';
    loadPropertyProfile();
  } catch(e) { document.getElementById('prop-status').textContent = 'Error: ' + e.message; }
}

async function autofillProperty() {
  document.getElementById('prop-status').textContent = 'Auto-filling from documents...';
  try {
    const resp = await authFetch(`/api/tax/deduction/autofill?year=${selectedYear}`);
    const data = await resp.json();
    if (data.mortgage_interest_cents > 0) {
      document.getElementById('prop-mortgage-interest').value = (data.mortgage_interest_cents / 100).toFixed(2);
    }
    if (data.property_tax_cents > 0) {
      document.getElementById('prop-property-tax').value = (data.property_tax_cents / 100).toFixed(2);
    }
    if (data.insurance_cents > 0) {
      document.getElementById('prop-insurance').value = (data.insurance_cents / 100).toFixed(2);
    }
    if (data.property) {
      if (data.property.address) document.getElementById('prop-address').value = data.property.address;
      if (data.property.total_sqft) document.getElementById('prop-total-sqft').value = data.property.total_sqft;
      if (data.property.workshop_sqft) document.getElementById('prop-workshop-sqft').value = data.property.workshop_sqft;
    }
    const sources = data.sources || {};
    document.getElementById('prop-status').innerHTML = `Auto-filled! Sources: ${Object.entries(sources).filter(([k,v]) => v !== 'none').map(([k,v]) => `<span class="text-oc-400">${k}</span>: ${v}`).join(', ') || 'no data found'}`;
    document.getElementById('prop-status').className = 'text-xs text-green-400';
  } catch(e) { document.getElementById('prop-status').textContent = 'Auto-fill failed'; }
}

// ── Statement Transactions ──
async function loadStatementTransactions() {
  try {
    const resp = await authFetch(`/api/tax/statements/transactions?start=${yearStart()}&end=${yearEnd()}`);
    const data = await resp.json();
    const txns = data.transactions || [];
    const countEl = document.getElementById('stmt-txn-count');
    if (countEl) countEl.textContent = `${txns.length} transactions | ${data.total_display || '$0.00'}`;

    const list = document.getElementById('stmt-txn-list');
    if (!list) return;
    if (txns.length === 0) {
      list.innerHTML = '<p class="text-xs text-gray-600">No statement transactions yet. Upload a bank or credit card statement in the Documents tab.</p>';
      return;
    }

    list.innerHTML = txns.map(t => `
      <div class="flex items-center justify-between py-1.5 border-b border-gray-700/30 last:border-0 text-xs">
        <div class="flex-1 min-w-0">
          <div class="flex items-center gap-2">
            <span class="text-gray-400">${t.transaction_date}</span>
            <span class="text-gray-300 truncate">${esc(t.vendor || t.description)}</span>
            ${t.insurance_type ? `<span class="px-1.5 py-0.5 rounded bg-purple-900/30 text-purple-400 text-[10px]">${t.insurance_type} ins</span>` : ''}
            ${t.is_deductible ? '<span class="text-green-500 text-[10px]">deductible</span>' : ''}
          </div>
          <span class="text-gray-600">${t.category || ''}</span>
        </div>
        <span class="font-medium ${t.amount_cents < 0 ? 'text-green-400' : 'text-gray-200'} flex-shrink-0 ml-2">${t.amount_display}</span>
      </div>
    `).join('');
  } catch(e) { console.log('stmt-txn:', e); }
}

// ── Tax Prep Wizard ──
async function loadWizard() {
  document.getElementById('wizard-year').textContent = selectedYear;
  try {
    const resp = await authFetch(`/api/tax/wizard?year=${selectedYear}`);
    const data = await resp.json();

    // Progress
    document.getElementById('wizard-pct').textContent = data.completeness + '%';
    document.getElementById('wizard-progress-bar').style.width = data.completeness + '%';
    document.getElementById('wizard-progress-bar').className = `h-full rounded-full transition-all ${
      data.completeness >= 80 ? 'bg-green-500' : data.completeness >= 50 ? 'bg-yellow-500' : 'bg-red-500'}`;

    // Steps
    const stepsEl = document.getElementById('wizard-steps');
    stepsEl.innerHTML = (data.steps || []).map(s => {
      const icon = s.status === 'complete' ? '<span class="text-green-400 text-sm">&#10004;</span>' :
                    s.status === 'needs_attention' ? '<span class="text-yellow-400 text-sm">&#9888;</span>' :
                    s.status === 'partial' ? '<span class="text-yellow-400 text-sm">&#8230;</span>' :
                    '<span class="text-gray-500 text-sm">&#9675;</span>';
      const borderColor = s.status === 'complete' ? 'border-green-800/30' :
                           s.status === 'needs_attention' ? 'border-yellow-800/30' : 'border-gray-700/50';
      const bgColor = s.status === 'complete' ? 'bg-green-900/10' :
                       s.status === 'needs_attention' ? 'bg-yellow-900/10' : 'bg-gray-900';
      return `
        <div class="p-3 rounded-lg ${bgColor} border ${borderColor}">
          <div class="flex items-center gap-2">
            ${icon}
            <span class="text-sm font-medium text-gray-300">Step ${s.step}: ${s.title}</span>
          </div>
          <p class="text-xs text-gray-500 mt-1 ml-6">${s.detail}</p>
          ${s.data && s.data.w2s ? s.data.w2s.map(w => `
            <p class="text-xs text-gray-600 ml-6 mt-0.5">&bull; ${esc(w.employer || '?')} — ${esc(w.employee || '?')}: wages ${w.wages || '?'}, withheld ${w.withheld || '?'}</p>
          `).join('') : ''}
        </div>`;
    }).join('');

    // Missing
    const missing = data.missing || [];
    const missingSection = document.getElementById('wizard-missing-section');
    if (missing.length > 0) {
      missingSection.style.display = '';
      document.getElementById('wizard-missing').innerHTML = missing.map(m => `
        <div class="flex items-center gap-2 p-2 rounded bg-yellow-900/20 border border-yellow-800/30">
          <span class="text-yellow-400">&#9888;</span>
          <span class="text-sm text-yellow-300">${esc(m)}</span>
          <button onclick="showTab('documents')" class="text-xs text-oc-500 hover:text-oc-400 ml-auto">Upload &rarr;</button>
        </div>
      `).join('');
    } else {
      missingSection.style.display = 'none';
    }

    // Summary
    const sum = data.summary || {};
    document.getElementById('wizard-summary').innerHTML = `
      <div class="grid grid-cols-2 gap-x-6 gap-y-2 text-sm">
        <div class="flex justify-between"><span class="text-gray-500">Gross Income:</span><span class="text-gray-300">${sum.gross_income || '--'}</span></div>
        <div class="flex justify-between"><span class="text-gray-500">Business Deductions:</span><span class="text-gray-300">-${sum.business_deductions || '--'}</span></div>
        <div class="flex justify-between"><span class="text-gray-500">AGI:</span><span class="text-gray-300">${sum.agi || '--'}</span></div>
        <div class="flex justify-between"><span class="text-gray-500">${(sum.deduction_type||'standard').charAt(0).toUpperCase()+(sum.deduction_type||'standard').slice(1)} Deduction:</span><span class="text-gray-300">-${sum.deduction || '--'}</span></div>
        <div class="flex justify-between"><span class="text-gray-500">Taxable Income:</span><span class="text-white font-medium">${sum.taxable_income || '--'}</span></div>
        <div class="flex justify-between"><span class="text-gray-500">Estimated Tax:</span><span class="text-gray-300">${sum.estimated_tax || '--'}</span></div>
        <div class="flex justify-between"><span class="text-gray-500">Withheld:</span><span class="text-gray-300">-${sum.withheld || '--'}</span></div>
        <div class="flex justify-between border-t border-gray-700 pt-2">
          <span class="text-gray-400 font-medium">Result:</span>
          <span class="font-semibold ${sum.estimated_owed && !sum.estimated_owed.startsWith('-') && !sum.refund_or_owe?.startsWith('Refund') ? 'text-red-400' : 'text-green-400'}">${sum.refund_or_owe || '--'}</span>
        </div>
      </div>
      ${data.bracket_warning ? `<p class="text-xs text-yellow-400 mt-3">&#9888; ${esc(data.bracket_warning)}</p>` : ''}`;
  } catch(e) { console.log('wizard:', e); }
}

function fmtDollars(n) { return '$' + Math.abs(n).toFixed(2).replace(/\B(?=(\d{3})+(?!\d))/g, ','); }
function fmtSignedDollars(cents) { const abs = Math.abs(cents / 100); const f = '$' + abs.toFixed(2).replace(/\B(?=(\d{3})+(?!\d))/g, ','); return cents < 0 ? '-' + f : f; }

// ── Financial Connections ──
async function launchPlaidLink() {
  const status = document.getElementById('plaid-status');
  status.textContent = 'Creating link session...';
  try {
    const resp = await authFetch('/api/financial/plaid/link-token', { method: 'POST', headers: {'Content-Type':'application/json'}, body: JSON.stringify({token}) });
    const data = await resp.json();
    if (data.error) { status.textContent = data.error; return; }
    if (!window.Plaid) { const s=document.createElement('script'); s.src='https://cdn.plaid.com/link/v2/stable/link-initialize.js'; document.head.appendChild(s); await new Promise(r=>s.onload=r); }
    const handler = window.Plaid.create({
      token: data.link_token,
      onSuccess: async (pub_tok, meta) => {
        status.textContent = 'Linking...';
        const exResp = await authFetch('/api/financial/plaid/exchange', { method:'POST', headers:{'Content-Type':'application/json'}, body:JSON.stringify({token,public_token:pub_tok,institution:meta.institution}) });
        const exData = await exResp.json();
        status.textContent = exData.success ? 'Linked!' : (exData.error||'Failed');
        if (exData.success) loadConnections();
      },
      onExit: (err) => { status.textContent = err ? 'Cancelled' : ''; }
    });
    handler.open();
  } catch(e) { status.textContent = 'Error: '+e.message; }
}

async function connectSimpleFIN() {
  const tok=document.getElementById('simplefin-token').value.trim(), status=document.getElementById('simplefin-status');
  if (!tok) { status.textContent='Paste setup token'; return; }
  status.textContent='Connecting...';
  try {
    const resp=await authFetch('/api/financial/connections',{method:'POST',headers:{'Content-Type':'application/json'},body:JSON.stringify({token,provider:'simplefin',setup_token:tok})});
    const data=await resp.json();
    status.textContent=data.success?'Connected!':data.error||'Failed'; status.className='text-xs mt-2 block '+(data.success?'text-green-400':'text-red-400');
    if(data.success){document.getElementById('simplefin-token').value='';loadConnections();}
  } catch(e){status.textContent='Error: '+e.message;}
}

async function loadConnections() {
  try {
    const [cr,er]=await Promise.all([authFetch(`/api/financial/connections`),authFetch(`/api/financial/email`).catch(()=>null)]);
    const cd=await cr.json(), conns=cd.connections||[], list=document.getElementById('connections-list');
    if(!conns.length){list.innerHTML='<p class="text-xs text-gray-600">No accounts connected yet.</p>';}
    else{list.innerHTML=conns.map(c=>`<div class="p-3 rounded-lg bg-gray-900 border border-gray-700/50 flex items-center justify-between"><div class="flex-1"><div class="flex items-center gap-2"><span class="text-sm font-medium text-gray-300">${esc(c.institution_name||'Unknown')}</span><span class="text-xs px-1.5 py-0.5 rounded ${c.provider==='plaid'?'bg-indigo-900/50 text-indigo-400':'bg-emerald-900/50 text-emerald-400'}">${c.provider}</span><span class="text-xs px-1.5 py-0.5 rounded ${c.status==='active'?'bg-green-900/50 text-green-400':'bg-red-900/50 text-red-400'}">${c.status}</span></div><p class="text-xs text-gray-500 mt-1">${c.last_sync_at?'Synced: '+new Date(c.last_sync_at*1000).toLocaleDateString():'Never synced'}${c.error?' <span class="text-red-400">'+esc(c.error)+'</span>':''}</p></div><div class="flex gap-2 flex-shrink-0 ml-3"><button onclick="syncConn(${c.id})" class="text-xs bg-gray-700 hover:bg-gray-600 text-gray-300 px-2.5 py-1.5 rounded-lg">Sync</button><button onclick="disconnConn(${c.id},'${esc(c.institution_name||'')}')" class="text-xs bg-red-900/30 hover:bg-red-900/50 text-red-400 px-2.5 py-1.5 rounded-lg">Disconnect</button></div></div>`).join('');}
    if(er){const ed=await er.json(),emails=ed.connections||[],el=document.getElementById('email-connections-list');
      if(emails.length){el.innerHTML=emails.map(e=>`<div class="flex items-center justify-between p-2 rounded-lg bg-gray-900 border border-gray-700/50"><span class="text-sm text-gray-300">${esc(e.email_address||'Gmail')}</span><div class="flex gap-2"><button onclick="scanEmail(${e.id})" class="text-xs bg-gray-700 hover:bg-gray-600 text-gray-300 px-2 py-1 rounded">Scan</button><button onclick="disconnEmail(${e.id})" class="text-xs text-red-400">Remove</button></div></div>`).join('');}else{el.innerHTML='';}}
  } catch(e){console.log('connections:',e);}
}
async function syncConn(id){try{await authFetch(`/api/financial/connections/${id}/sync`,{method:'POST',headers:{'Content-Type':'application/json'},body:JSON.stringify({token})});loadConnections();}catch(e){alert('Sync failed');}}
async function disconnConn(id,name){if(!confirm(`Disconnect ${name}?`))return;try{await authFetch(`/api/financial/connections/${id}`,{method:'DELETE'});loadConnections();}catch(e){}}
async function connectGmail(){const s=document.getElementById('gmail-status');s.textContent='Starting OAuth...';try{const r=await authFetch('/api/financial/email',{method:'POST',headers:{'Content-Type':'application/json'},body:JSON.stringify({token,provider:'gmail'})});const d=await r.json();if(d.oauth_url){window.open(d.oauth_url,'_blank','width=600,height=700');s.textContent='Complete in popup...';}else{s.textContent=d.error||'Failed';}}catch(e){s.textContent='Error';}}
async function scanEmail(id){try{await authFetch(`/api/financial/email/${id}/scan`,{method:'POST',headers:{'Content-Type':'application/json'},body:JSON.stringify({token})});loadConnections();}catch(e){}}
async function disconnEmail(id){if(!confirm('Remove?'))return;try{await authFetch(`/api/financial/email/${id}`,{method:'DELETE'});loadConnections();}catch(e){}}

// ── Investments ──
async function connectAlpaca(){const k=document.getElementById('alpaca-key').value.trim(),s=document.getElementById('alpaca-secret').value.trim(),st=document.getElementById('alpaca-status');if(!k||!s){st.textContent='Key+secret required';return;}st.textContent='Connecting...';try{const r=await authFetch('/api/financial/investments/accounts',{method:'POST',headers:{'Content-Type':'application/json'},body:JSON.stringify({token,broker:'alpaca',api_key:k,api_secret:s,base_url:document.getElementById('alpaca-env').value,nickname:document.getElementById('alpaca-nickname').value.trim()||null})});const d=await r.json();st.textContent=d.success?'Connected!':d.error||'Failed';st.className='text-xs '+(d.success?'text-green-400':'text-red-400');if(d.success){document.getElementById('alpaca-key').value='';document.getElementById('alpaca-secret').value='';loadInvestmentAccounts();loadInvestmentSummary();}}catch(e){st.textContent='Error';}}
async function connectCoinbase(){const k=document.getElementById('coinbase-key').value.trim(),s=document.getElementById('coinbase-secret').value.trim(),st=document.getElementById('coinbase-status');if(!k||!s){st.textContent='Key+secret required';return;}st.textContent='Connecting...';try{const r=await authFetch('/api/financial/investments/accounts',{method:'POST',headers:{'Content-Type':'application/json'},body:JSON.stringify({token,broker:'coinbase',api_key:k,api_secret:s})});const d=await r.json();st.textContent=d.success?'Connected!':d.error||'Failed';st.className='text-xs '+(d.success?'text-green-400':'text-red-400');if(d.success){document.getElementById('coinbase-key').value='';document.getElementById('coinbase-secret').value='';loadInvestmentAccounts();}}catch(e){st.textContent='Error';}}
async function loadInvestmentAccounts(){try{const r=await authFetch(`/api/financial/investments/accounts`);const d=await r.json();const a=d.accounts||[],l=document.getElementById('inv-accounts-list');if(!a.length){l.innerHTML='<p class="text-xs text-gray-600">No brokerage accounts connected.</p>';return;}l.innerHTML=a.map(a=>`<div class="flex items-center justify-between p-3 rounded-lg bg-gray-900 border border-gray-700/50"><div class="flex-1"><div class="flex items-center gap-2"><span class="text-sm font-medium text-gray-300">${esc(a.nickname||a.broker)}</span><span class="text-xs px-1.5 py-0.5 rounded ${a.broker==='alpaca'?'bg-yellow-900/50 text-yellow-400':'bg-blue-900/50 text-blue-400'}">${a.broker}</span></div><p class="text-xs text-gray-500 mt-0.5">${a.last_sync_at?'Synced: '+new Date(a.last_sync_at*1000).toLocaleDateString():'Never synced'}</p></div><div class="flex gap-2 ml-3"><button onclick="syncInv(${a.id})" class="text-xs bg-gray-700 hover:bg-gray-600 text-gray-300 px-2.5 py-1.5 rounded-lg">Sync</button><button onclick="disconnInv(${a.id},'${esc(a.nickname||a.broker)}')" class="text-xs bg-red-900/30 hover:bg-red-900/50 text-red-400 px-2.5 py-1.5 rounded-lg">Remove</button></div></div>`).join('');}catch(e){console.log('inv:',e);}}
async function syncInv(id){try{const r=await authFetch(`/api/financial/investments/accounts/${id}/sync`,{method:'POST',headers:{'Content-Type':'application/json'},body:JSON.stringify({token})});const d=await r.json();if(d.success){loadInvestmentAccounts();loadInvestmentSummary();loadInvestmentTransactions();loadHoldings();}else{alert(d.error||'Failed');}}catch(e){alert('Error');}}
async function disconnInv(id,name){if(!confirm(`Remove ${name}?`))return;try{await authFetch(`/api/financial/investments/accounts/${id}`,{method:'DELETE'});loadInvestmentAccounts();loadInvestmentSummary();}catch(e){}}
async function loadInvestmentSummary(){try{const r=await authFetch(`/api/financial/investments?year=${selectedYear}`);const d=await r.json();document.getElementById('inv-short-term').textContent=fmtSignedDollars(d.short_term_cents||0);document.getElementById('inv-long-term').textContent=fmtSignedDollars(d.long_term_cents||0);document.getElementById('inv-dividends').textContent=fmtDollars((d.dividends_cents||0)/100);document.getElementById('inv-net-pl').textContent=fmtSignedDollars(d.net_realized_cents||0);}catch(e){console.log('inv sum:',e);}}
async function loadInvestmentTransactions(){try{let url=`/api/financial/investments/transactions?start=${yearStart()}&end=${yearEnd()}`;const tf=document.getElementById('inv-filter-type')?.value;const bf=document.getElementById('inv-filter-broker')?.value;if(tf)url+=`&activity_type=${tf}`;if(bf)url+=`&broker=${bf}`;const r=await authFetch(url);const d=await r.json();const txns=d.transactions||[];document.getElementById('inv-txn-count').textContent=txns.length+' txns';const l=document.getElementById('inv-txn-list');if(!txns.length){l.innerHTML='<p class="text-xs text-gray-600">No transactions found.</p>';return;}l.innerHTML=txns.map(t=>{const sc=t.side==='buy'?'text-green-400':t.side==='sell'?'text-red-400':'text-gray-400';const pl=t.realized_pl_cents!=null?`<span class="${t.realized_pl_cents>=0?'text-green-400':'text-red-400'}">${fmtSignedDollars(t.realized_pl_cents)}</span>`:'';return`<div class="flex items-center justify-between py-1.5 border-b border-gray-700/30 text-xs"><div class="flex-1"><div class="flex items-center gap-2"><span class="text-gray-400">${t.transaction_date}</span><span class="px-1.5 py-0.5 rounded bg-gray-800 text-gray-400">${t.activity_type}</span>${t.symbol?`<span class="font-medium text-gray-200">${esc(t.symbol)}</span>`:''}${t.side?`<span class="${sc}">${t.side}</span>`:''}</div></div><div class="flex items-center gap-3 ml-3">${pl}<span class="font-medium text-gray-200">${fmtSignedDollars(t.amount_cents)}</span><span class="text-gray-600">${t.broker}</span></div></div>`;}).join('');}catch(e){console.log('inv txn:',e);}}
async function loadHoldings(){try{const r=await authFetch(`/api/financial/investments/holdings`);const d=await r.json();const h=d.holdings||[];document.getElementById('holdings-as-of').textContent=d.as_of?'as of '+d.as_of:'';const l=document.getElementById('holdings-list');if(!h.length){l.innerHTML='<p class="text-xs text-gray-600">No holdings. Connect and sync a brokerage.</p>';return;}l.innerHTML=h.map(p=>`<div class="flex items-center justify-between py-1.5 border-b border-gray-700/30 text-xs"><span class="w-16 font-medium text-gray-200">${esc(p.symbol)}</span><span class="w-16 text-right text-gray-400">${p.qty}</span><span class="w-20 text-right text-gray-300">${p.market_value_cents?fmtDollars(p.market_value_cents/100):'--'}</span><span class="w-20 text-right ${(p.market_value_cents||0)-(p.cost_basis_cents||0)>=0?'text-green-400':'text-red-400'}">${fmtSignedDollars((p.market_value_cents||0)-(p.cost_basis_cents||0))}</span><span class="w-12 text-right text-gray-600">${p.broker}</span></div>`).join('');}catch(e){console.log('holdings:',e);}}

initYearSelector();

// ── Deduction Suggestions ──
function analyzeDeductions(summaryData) {
  const cats = summaryData.categories || [];
  const catNames = new Set(cats.map(c => c.category));
  const suggestions = [];

  // Common deductions people miss
  const checks = [
    { cat: 'Vehicle & Mileage', msg: 'Track business mileage', detail: 'If you drive for business (supply runs, client visits, deliveries), you can deduct $0.67/mile (2025).', condition: () => !catNames.has('Vehicle & Mileage'),
      form: `
        <div class="mt-3 p-3 rounded-lg bg-gray-800 space-y-2" id="mileage-form">
          <div class="grid grid-cols-2 gap-2 text-xs">
            <div><span class="text-gray-500">Business miles in ${selectedYear}:</span> <input type="number" id="mi-miles" class="w-20 bg-gray-900 border border-gray-700 rounded px-1 py-0.5 text-white outline-none focus:border-oc-500" placeholder="e.g. 3000" onchange="recalcMileage()"></div>
            <div><span class="text-gray-500">Deduction:</span> <strong class="text-green-400" id="mi-result">$0.00</strong> <span class="text-gray-600">@ $0.67/mile</span></div>
          </div>
          <button onclick="saveMileageDeduction()" class="text-xs bg-oc-600 hover:bg-oc-700 text-white px-3 py-1.5 rounded transition-colors">Save deduction</button>
        </div>` },
    { cat: 'Home Office', msg: 'Home office / workshop deduction', detail: 'Your 488 sq ft workshop in a 5,206 sq ft home = 9.37% business use. Two calculation methods available.', condition: () => !catNames.has('Home Office') && !catNames.has('Workshop Space') && cats.some(c => c.entity === 'business'),
      form: `
        <div class="mt-3 p-3 rounded-lg bg-gray-800 space-y-3" id="home-office-form">
          <p class="text-xs font-medium text-gray-300">Calculate your workshop deduction:</p>
          <div class="grid grid-cols-2 gap-2 text-xs">
            <div><span class="text-gray-500">Workshop:</span> <input type="number" id="ho-sqft" value="488" class="w-16 bg-gray-900 border border-gray-700 rounded px-1 py-0.5 text-white outline-none focus:border-oc-500"> sq ft</div>
            <div><span class="text-gray-500">Total home:</span> <input type="number" id="ho-total-sqft" value="5206" class="w-16 bg-gray-900 border border-gray-700 rounded px-1 py-0.5 text-white outline-none focus:border-oc-500"> sq ft</div>
          </div>
          <div class="p-2 rounded bg-gray-900 text-xs space-y-1">
            <p class="text-gray-400">Business use: <strong class="text-white" id="ho-pct">9.37%</strong></p>
            <p class="text-gray-400">Simplified method: <strong class="text-white">$1,500</strong> <span class="text-gray-600">(capped at 300 sqft × $5)</span></p>
            <p class="text-gray-400">Actual method: <strong class="text-green-400" id="ho-actual">calculating...</strong></p>
          </div>
          <div class="grid grid-cols-2 gap-2 text-xs">
            <div><span class="text-gray-500">Mortgage interest:</span> <input type="text" id="ho-mortgage" value="72696.17" class="w-20 bg-gray-900 border border-gray-700 rounded px-1 py-0.5 text-white outline-none focus:border-oc-500" onchange="recalcHomeOffice()"></div>
            <div><span class="text-gray-500">Property tax:</span> <input type="text" id="ho-proptax" value="571.60" class="w-20 bg-gray-900 border border-gray-700 rounded px-1 py-0.5 text-white outline-none focus:border-oc-500" onchange="recalcHomeOffice()"></div>
            <div><span class="text-gray-500">Insurance:</span> <input type="text" id="ho-insurance" value="" class="w-20 bg-gray-900 border border-gray-700 rounded px-1 py-0.5 text-white outline-none focus:border-oc-500" placeholder="annual" onchange="recalcHomeOffice()"></div>
            <div><span class="text-gray-500">Utilities:</span> <input type="text" id="ho-utilities" value="" class="w-20 bg-gray-900 border border-gray-700 rounded px-1 py-0.5 text-white outline-none focus:border-oc-500" placeholder="annual" onchange="recalcHomeOffice()"></div>
          </div>
          <div class="flex gap-2">
            <button onclick="saveHomeOfficeDeduction()" class="text-xs bg-oc-600 hover:bg-oc-700 text-white px-3 py-1.5 rounded transition-colors">Save deduction</button>
            <button onclick="taxChat('Help me calculate my workshop deduction for 488 sqft in a 5206 sqft home')" class="text-xs bg-gray-700 hover:bg-gray-600 text-gray-300 px-3 py-1.5 rounded transition-colors">Ask assistant</button>
          </div>
          <div id="ho-result" class="hidden text-xs"></div>
        </div>` },
    { cat: 'Internet & Phone', msg: 'Internet and phone bills', detail: 'The business-use percentage of your internet and phone bills is deductible. Estimate the % used for work.', condition: () => !catNames.has('Internet & Phone') && !catNames.has('Utilities') && cats.some(c => c.entity === 'business') },
    { cat: 'Health Insurance', msg: 'Self-employed health insurance', detail: 'If self-employed, you can deduct 100% of health insurance premiums for yourself, spouse, and dependents.', condition: () => !catNames.has('Health Insurance') && cats.some(c => c.entity === 'business') },
    { cat: 'Education & Training', msg: 'Professional development', detail: 'Courses, books, conferences, and certifications related to your business are deductible.', condition: () => !catNames.has('Education & Training') },
    { cat: 'Advertising & Marketing', msg: 'Marketing expenses', detail: 'Website hosting, domain names, business cards, social media ads, and promotional materials.', condition: () => !catNames.has('Advertising & Marketing') && cats.some(c => c.entity === 'business') },
    { cat: 'Professional Services', msg: 'Professional services', detail: 'Accountant, lawyer, bookkeeper, business consulting fees. Also tax preparation fees for business returns.', condition: () => !catNames.has('Professional Services') && cats.some(c => c.entity === 'business') },
    { cat: 'Retirement', msg: 'Retirement contributions', detail: 'SEP-IRA (up to 25% of net self-employment income), Solo 401(k), or traditional IRA contributions reduce taxable income.', condition: () => !catNames.has('Retirement') },
    { cat: 'Charitable', msg: 'Charitable donations', detail: 'Cash donations, donated goods (at fair market value), and mileage driven for charity ($0.14/mile).', condition: () => !catNames.has('Donations') && !catNames.has('Charitable') },
    { cat: 'State Taxes', msg: 'State and local tax deduction (SALT)', detail: 'State income tax or sales tax, plus property tax — deductible up to $10,000 combined on Schedule A.', condition: () => true },
    { cat: 'Student Loans', msg: 'Student loan interest', detail: 'Deduct up to $2,500 of student loan interest paid, even if you don\'t itemize. Income limits apply.', condition: () => !catNames.has('Student Loan Interest') },
  ];

  for (const c of checks) {
    if (c.condition()) {
      suggestions.push(c);
    }
  }

  const el = document.getElementById('deductions-list');
  if (suggestions.length === 0) {
    el.innerHTML = '<p class="text-sm text-green-400">Looking good! You seem to be tracking all common deduction categories.</p>';
    return;
  }

  el.innerHTML = suggestions.map(s => `
    <div class="p-3 rounded-lg bg-gray-900 border border-gray-700/50">
      <div class="flex items-start gap-2">
        <span class="text-green-400 mt-0.5 flex-shrink-0">&#10004;</span>
        <div>
          <p class="text-sm font-medium text-gray-300">${s.msg}</p>
          <p class="text-xs text-gray-500 mt-1">${s.detail}</p>
          ${s.form ? '' : '<button onclick="taxChat(\'Help me figure out if I can deduct ' + s.cat.toLowerCase() + ' expenses\')" class="text-xs text-oc-500 hover:text-oc-400 mt-2">Ask the tax assistant about this &rarr;</button>'}
        </div>
      </div>
    ${s.form || ''}
    </div>
  `).join('');
}

// Reload tax chat when returning to page
document.addEventListener('visibilitychange', async () => {
  if (document.visibilityState === 'visible' && taxConvId && !taxChatSending) {
    await loadTaxMessages(taxConvId);
  }
});

// ── Taxpayer Profile ────────────────────────────────────────────────────

async function loadProfile() {
  try {
    const yr = selectedYear || yearStart().slice(0,4);
    document.getElementById('profile-year-label').textContent = `Tax Year ${yr}`;
    const resp = await authFetch(`/api/tax/profile?year=${yr}`);
    const data = await resp.json();
    renderProfileView(data.profile, data.dependents, data.income_sources);
  } catch(e) {}
}

// True when the stored SSN is missing or contains masking characters
// (anything other than 9 digits with optional dashes).
function _ssnIsMasked(s) {
  if (!s) return true;
  const digits = (s.match(/\d/g) || []).join('');
  return /[xX]/.test(s) || digits.length !== 9;
}

function renderSsnBanner(profile) {
  const banner = document.getElementById('ssn-banner');
  if (!profile) { banner.classList.add('hidden'); return; }
  const selfMasked = _ssnIsMasked(profile.ssn);
  const isJoint = profile.filing_status === 'married_jointly';
  const spouseMasked = isJoint && _ssnIsMasked(profile.spouse_ssn);
  if (!selfMasked && !spouseMasked) { banner.classList.add('hidden'); return; }
  // Build a clear title that names exactly what's missing.
  let parts = [];
  if (selfMasked) parts.push('your SSN');
  if (spouseMasked) parts.push("spouse's SSN");
  document.getElementById('ssn-banner-title').textContent =
    'Form 4868 needs ' + parts.join(' and ') + ' — your saved value is masked.';
  document.getElementById('ssn-banner-detail').textContent =
    'Most W-2s now show only the last 4 digits, so we can\'t pull this from a scan. Type the full 9 digits once and we\'ll keep them with the rest of your profile.';
  document.getElementById('ssn-banner-self-row').classList.toggle('hidden', !selfMasked);
  document.getElementById('ssn-banner-spouse-row').classList.toggle('hidden', !spouseMasked);
  document.getElementById('ssn-banner-result').textContent = '';
  banner.classList.remove('hidden');
}

async function saveSsnBanner() {
  const result = document.getElementById('ssn-banner-result');
  const yr = selectedYear || yearStart().slice(0,4);
  const ssn = (document.getElementById('ssn-banner-self').value || '').trim();
  const sp = (document.getElementById('ssn-banner-spouse').value || '').trim();
  // Validate: 9 digits expected (dashes optional).
  const checkDigits = s => (s.match(/\d/g) || []).length;
  if (ssn && checkDigits(ssn) !== 9) { result.textContent = 'SSN needs 9 digits.'; result.className='text-xs text-red-400'; return; }
  if (sp && checkDigits(sp) !== 9) { result.textContent = "Spouse's SSN needs 9 digits."; result.className='text-xs text-red-400'; return; }
  if (!ssn && !sp) { result.textContent = 'Type at least one SSN.'; result.className='text-xs text-yellow-400'; return; }
  result.textContent = 'Saving...'; result.className='text-xs text-gray-400';
  try {
    const body = { token, year: parseInt(yr) };
    if (ssn) body.ssn = ssn;
    if (sp) body.spouse_ssn = sp;
    await authFetch('/api/tax/profile', { method:'POST', headers:{'Content-Type':'application/json'}, body:JSON.stringify(body) });
    result.textContent = 'Saved.'; result.className='text-xs text-green-400';
    document.getElementById('ssn-banner-self').value = '';
    document.getElementById('ssn-banner-spouse').value = '';
    setTimeout(() => loadProfile(), 400); // re-render with the new full value
  } catch(e) { result.textContent = 'Error: '+e.message; result.className='text-xs text-red-400'; }
}

function renderProfileView(profile, deps, income) {
  renderSsnBanner(profile);
  const summary = document.getElementById('profile-summary');
  if (!profile) {
    summary.innerHTML = `<div class="col-span-4 text-center py-3">
      <p class="text-xs text-gray-500 mb-2">No filing profile yet. Set up your basic information to get started.</p>
      <button onclick="toggleProfileEdit()" class="btn-primary text-xs">Set Up Profile</button>
    </div>`;
    return;
  }
  const p = profile;
  const statuses = {single:'Single', married_jointly:'Married Filing Jointly', married_separately:'Married Filing Separately', head_of_household:'Head of Household'};
  let html = `
    <div><p class="text-xs text-gray-500">Name</p><p class="text-white">${p.first_name||''} ${p.last_name||''}</p></div>
    <div><p class="text-xs text-gray-500">Filing Status</p><p class="text-white">${statuses[p.filing_status]||p.filing_status}</p></div>
    <div><p class="text-xs text-gray-500">SSN</p><p class="text-white font-mono">${p.ssn_last4||'Not set'}</p></div>
    <div><p class="text-xs text-gray-500">Occupation</p><p class="text-white">${p.occupation||'—'}</p></div>`;
  if (p.address_line1) html += `<div class="col-span-2"><p class="text-xs text-gray-500">Address</p><p class="text-white">${p.address_line1}${p.city ? ', '+p.city : ''}${p.state ? ' '+p.state : ''} ${p.zip||''}</p></div>`;
  if (p.filing_status === 'married_jointly' && p.spouse_first) {
    html += `<div><p class="text-xs text-gray-500">Spouse</p><p class="text-white">${p.spouse_first} ${p.spouse_last||''}</p></div>`;
    html += `<div><p class="text-xs text-gray-500">Spouse SSN</p><p class="text-white font-mono">${p.spouse_ssn_last4||'Not set'}</p></div>`;
  }
  summary.innerHTML = html;

  // Fill edit form for later
  if (p.first_name) document.getElementById('pf-first').value = p.first_name;
  if (p.last_name) document.getElementById('pf-last').value = p.last_name;
  if (p.date_of_birth) document.getElementById('pf-dob').value = p.date_of_birth;
  if (p.address_line1) document.getElementById('pf-addr').value = p.address_line1;
  if (p.city) document.getElementById('pf-city').value = p.city;
  if (p.state) document.getElementById('pf-state').value = p.state;
  if (p.zip) document.getElementById('pf-zip').value = p.zip;
  if (p.filing_status) document.getElementById('pf-filing').value = p.filing_status;
  if (p.occupation) document.getElementById('pf-occupation').value = p.occupation;
  if (p.spouse_first) document.getElementById('pf-sp-first').value = p.spouse_first;
  if (p.spouse_last) document.getElementById('pf-sp-last').value = p.spouse_last;
  if (p.spouse_dob) document.getElementById('pf-sp-dob').value = p.spouse_dob;

  // Dependents
  const depSection = document.getElementById('dependents-section');
  const depList = document.getElementById('dependents-list');
  if (deps && deps.length > 0) {
    depSection.classList.remove('hidden');
    depList.innerHTML = deps.map(d => `
      <div class="flex items-center justify-between p-2 rounded-lg bg-gray-900 text-xs">
        <div class="flex items-center gap-3">
          <span class="text-white font-medium">${d.first_name} ${d.last_name}</span>
          <span class="badge badge-blue">${d.relationship}</span>
          ${d.date_of_birth ? `<span class="text-gray-500">DOB: ${d.date_of_birth}</span>` : ''}
          ${d.qualifies_ctc ? '<span class="badge badge-green">CTC eligible</span>' : ''}
        </div>
        <div class="flex items-center gap-2">
          <span class="text-gray-600">${d.ssn_last4 || 'No SSN'}</span>
          <button onclick="deleteDependent(${d.id})" class="text-red-500 hover:text-red-400 text-xs">Remove</button>
        </div>
      </div>`).join('');
  } else {
    depSection.classList.remove('hidden');
    depList.innerHTML = '<p class="text-xs text-gray-600">No dependents. Click "+ Add" to add one.</p>';
  }
}

function toggleProfileEdit() {
  const view = document.getElementById('profile-view');
  const edit = document.getElementById('profile-edit');
  const btn = document.getElementById('profile-edit-btn');
  const showing = !edit.classList.contains('hidden');
  view.classList.toggle('hidden', !showing);
  edit.classList.toggle('hidden', showing);
  btn.textContent = showing ? 'Edit' : 'View';
}

async function saveProfile() {
  const yr = selectedYear || yearStart().slice(0,4);
  const result = document.getElementById('profile-save-result');
  result.textContent = 'Saving...'; result.className = 'text-xs self-center text-gray-400';
  try {
    const body = {
      token, year: parseInt(yr),
      first_name: document.getElementById('pf-first').value || null,
      last_name: document.getElementById('pf-last').value || null,
      ssn: document.getElementById('pf-ssn').value || null,
      date_of_birth: document.getElementById('pf-dob').value || null,
      address_line1: document.getElementById('pf-addr').value || null,
      city: document.getElementById('pf-city').value || null,
      state: document.getElementById('pf-state').value || null,
      zip: document.getElementById('pf-zip').value || null,
      filing_status: document.getElementById('pf-filing').value,
      occupation: document.getElementById('pf-occupation').value || null,
      spouse_first: document.getElementById('pf-sp-first').value || null,
      spouse_last: document.getElementById('pf-sp-last').value || null,
      spouse_ssn: document.getElementById('pf-sp-ssn').value || null,
      spouse_dob: document.getElementById('pf-sp-dob').value || null,
    };
    await authFetch('/api/tax/profile', { method:'POST', headers:{'Content-Type':'application/json'}, body:JSON.stringify(body) });
    result.textContent = 'Auto-saved'; result.className = 'text-xs self-center text-green-400';
    setTimeout(() => { result.textContent = ''; }, 2000);
  } catch(e) { result.textContent = 'Error'; result.className = 'text-xs self-center text-red-400'; }
}

async function autoFillProfileFromScans() {
  const yr = selectedYear || yearStart().slice(0,4);
  const result = document.getElementById('profile-save-result');
  const sources = document.getElementById('profile-suggest-sources');
  result.textContent = 'Reading scans...'; result.className = 'text-xs self-center text-gray-400';
  sources.textContent = '';
  try {
    const resp = await authFetch(`/api/tax/profile/suggest?year=${yr}`);
    if (!resp.ok) throw new Error('HTTP ' + resp.status);
    const data = await resp.json();
    // Map suggestion field → input id. Order matters for the spouse section
    // (we open it if any spouse field has a value).
    const map = [
      ['first_name', 'pf-first'], ['last_name', 'pf-last'], ['ssn', 'pf-ssn'],
      ['address_line1', 'pf-addr'], ['city', 'pf-city'], ['state', 'pf-state'], ['zip', 'pf-zip'],
      ['spouse_first', 'pf-sp-first'], ['spouse_last', 'pf-sp-last'], ['spouse_ssn', 'pf-sp-ssn'],
    ];
    let filled = 0, skipped = 0;
    for (const [key, id] of map) {
      const el = document.getElementById(id);
      if (!el) continue;
      const val = data[key];
      if (val == null || val === '') continue;
      // Don't overwrite anything the user already typed.
      if (el.value && el.value.trim() !== '') { skipped++; continue; }
      el.value = val;
      filled++;
    }
    // Open the spouse disclosure if we filled anything in it.
    if (data.spouse_first || data.spouse_last || data.spouse_ssn) {
      const det = document.getElementById('spouse-section');
      if (det) det.open = true;
    }
    if (filled === 0 && skipped === 0) {
      result.textContent = 'No scan data available — upload a W-2 or mortgage statement first.';
      result.className = 'text-xs self-center text-gray-400';
    } else {
      result.textContent = `Filled ${filled} field${filled===1?'':'s'}` + (skipped > 0 ? ` (${skipped} kept)` : '') + ' — review and click Save Profile';
      result.className = 'text-xs self-center text-oc-400';
    }
    if (Array.isArray(data.sources) && data.sources.length > 0) {
      sources.textContent = 'Sources: ' + data.sources.join('; ');
    }
  } catch(e) {
    result.textContent = 'Error: ' + e.message;
    result.className = 'text-xs self-center text-red-400';
  }
}

// Auto-save profile on field blur (so closing the app doesn't lose data)
let _profileAutoSaveTimer = null;
function autoSaveProfile() {
  clearTimeout(_profileAutoSaveTimer);
  _profileAutoSaveTimer = setTimeout(() => {
    const result = document.getElementById('profile-save-result');
    if (result) { result.textContent = 'Saving...'; result.className = 'text-xs self-center text-gray-400'; }
    saveProfile();
  }, 1000);
}
document.addEventListener('DOMContentLoaded', () => {
  const fields = ['pf-first','pf-last','pf-ssn','pf-dob','pf-addr','pf-city','pf-state','pf-zip','pf-filing','pf-occupation','pf-sp-first','pf-sp-last','pf-sp-ssn','pf-sp-dob'];
  fields.forEach(id => {
    const el = document.getElementById(id);
    if (el) {
      el.addEventListener('blur', autoSaveProfile);
      if (el.tagName === 'SELECT') el.addEventListener('change', autoSaveProfile);
    }
  });
});

function showAddDependent() {
  const name = prompt('Dependent full name (First Last):');
  if (!name) return;
  const parts = name.trim().split(' ');
  const first = parts[0] || '';
  const last = parts.slice(1).join(' ') || '';
  const rel = prompt('Relationship (child, stepchild, parent, sibling, other):', 'child') || 'child';
  const dob = prompt('Date of birth (YYYY-MM-DD):', '') || null;
  const yr = selectedYear || yearStart().slice(0,4);
  authFetch('/api/tax/dependents', {
    method:'POST', headers:{'Content-Type':'application/json'},
    body: JSON.stringify({ year:parseInt(yr), first_name:first, last_name:last, relationship:rel, date_of_birth:dob })
  }).then(() => loadProfile());
}

async function deleteDependent(id) {
  if (!confirm('Remove this dependent?')) return;
  await authFetch(`/api/tax/dependents/${id}`, { method:'DELETE' });
  loadProfile();
}

// ── Extension Filing Workflow ────────────────────────────────────────────

let extId = null;
let extData = null;

function formatDollar(n) { return '$' + n.toFixed(2).replace(/\B(?=(\d{3})+(?!\d))/g, ','); }
function parseCents(s) { return Math.round(parseFloat(String(s).replace(/[$,]/g, '')) * 100) || 0; }

async function loadExtensionData() {
  try {
    const yr = selectedYear || yearStart().slice(0,4);
    const resp = await authFetch(`/api/tax/extension/status?year=${yr}`);
    const data = await resp.json();
    extData = data;
    // Debug: show what we got
    const dbg = document.getElementById('ext-result');
    if (dbg && data.estimate && data.estimate.total_tax_cents === 0 && data.estimate.total_paid_cents === 0 && !data.extension) {
      dbg.textContent = 'No tax data found for ' + yr + '. Upload W-2s and expenses first.';
      dbg.className = 'text-xs mt-2 block text-yellow-400';
    }
    if (data.extension) {
      extId = data.extension.id;
      const ext = data.extension;
      if (ext.status === 'confirmed') { showExtStep('confirmed'); renderExtConfirmed(ext); }
      else if (ext.status === 'filed') { fillExtReview(ext.total_tax_cents, ext.total_paid_cents, ext.payment_cents); showExtStep('file'); await renderExtFile(ext.filing_method); }
      else { showExtStep('review'); fillExtReview(ext.total_tax_cents, ext.total_paid_cents, ext.payment_cents); }
      updateExtBadge(ext.status);
    } else if (data.estimate) {
      showExtStep('review');
      fillExtReview(data.estimate.total_tax_cents, data.estimate.total_paid_cents, 0);
      updateExtBadge('none');
    }
  } catch(e) {}
}

function fillExtReview(taxCents, paidCents, payCents) {
  document.getElementById('ext-total-tax').value = formatDollar(taxCents / 100);
  document.getElementById('ext-payments').value = formatDollar(paidCents / 100);
  document.getElementById('ext-payment').value = (payCents / 100).toFixed(2);
  updateExtBalance();
}

function updateExtBalance() {
  const tax = parseCents(document.getElementById('ext-total-tax').value);
  const paid = parseCents(document.getElementById('ext-payments').value);
  const bal = Math.max(tax - paid, 0);
  const el = document.getElementById('ext-balance');
  el.value = formatDollar(bal / 100);
  el.className = 'input text-sm font-medium ' + (bal > 0 ? 'text-red-400' : 'text-green-400');
}

function updateExtBadge(status) {
  const b = document.getElementById('ext-status-badge');
  if (status === 'confirmed') { b.className = 'badge badge-green text-[10px]'; b.textContent = 'Confirmed'; }
  else if (status === 'filed') { b.className = 'badge badge-blue text-[10px]'; b.textContent = 'Filed'; }
  else if (status === 'draft') { b.className = 'badge badge-yellow text-[10px]'; b.textContent = 'Draft'; }
  else { b.className = 'badge badge-yellow text-[10px]'; b.textContent = 'Not filed'; }
}

function showExtStep(step) {
  document.getElementById('ext-step-review').classList.toggle('hidden', step !== 'review');
  document.getElementById('ext-step-file').classList.toggle('hidden', step !== 'file');
  document.getElementById('ext-step-confirmed').classList.toggle('hidden', step !== 'confirmed');
}

async function startExtFiling(method) {
  const yr = selectedYear || yearStart().slice(0,4);
  // Create or update draft
  try {
    const resp = await authFetch('/api/tax/extension/create', {
      method: 'POST', headers: {'Content-Type':'application/json'},
      body: JSON.stringify({
        token, year: parseInt(yr),
        total_tax_cents: parseCents(document.getElementById('ext-total-tax').value),
        total_paid_cents: parseCents(document.getElementById('ext-payments').value),
        payment_cents: parseCents(document.getElementById('ext-payment').value),
      })
    });
    const data = await resp.json();
    extId = data.extension?.id;
    if (!extId) { document.getElementById('ext-result').textContent = 'Error creating extension: ' + JSON.stringify(data); document.getElementById('ext-result').className = 'text-xs mt-2 block text-red-400'; return; }
  } catch(e) { document.getElementById('ext-result').textContent = 'Error: ' + e.message; document.getElementById('ext-result').className = 'text-xs mt-2 block text-red-400'; return; }

  if (method === 'mail') {
    // Download form text and mark as filed
    const payment = parseCents(document.getElementById('ext-payment').value);
    const resp = await authFetch(`/api/tax/extension?year=${yr}&payment=${payment}`);
    if (resp.ok) {
      const blob = await resp.blob();
      const a = document.createElement('a'); a.href = URL.createObjectURL(blob);
      a.download = `form-4868-${yr}.pdf`; a.click(); URL.revokeObjectURL(a.href);
    }
    if (extId) await authFetch(`/api/tax/extension/${extId}/file`, {
      method:'PUT', headers:{'Content-Type':'application/json'},
      body: JSON.stringify({token, method: 'mail'})
    });
    renderExtFile('mail');
    showExtStep('file');
    updateExtBadge('filed');
    return;
  }

  // Mark as filed
  if (extId) await authFetch(`/api/tax/extension/${extId}/file`, {
    method:'PUT', headers:{'Content-Type':'application/json'},
    body: JSON.stringify({token, method})
  });
  renderExtFile(method);
  showExtStep('file');
  updateExtBadge('filed');
}

async function renderExtFile(method) {
  const labels = {direct_pay:'IRS Direct Pay', free_file:'IRS Free File', mail:'Print & Mail'};
  const urls = {direct_pay:'https://www.irs.gov/payments/direct-pay-with-bank-account', free_file:'https://www.irs.gov/filing/free-file-do-your-federal-taxes-for-free', mail:null};
  document.getElementById('ext-method-label').textContent = 'Filing via ' + (labels[method]||method);
  const irsUrl = urls[method] || '';

  // Build copy-assist fields
  const yr = selectedYear || yearStart().slice(0,4);
  const tax = document.getElementById('ext-total-tax').value;
  const paid = document.getElementById('ext-payments').value;
  const bal = document.getElementById('ext-balance').value;
  const pay = document.getElementById('ext-payment').value;
  const copyFieldsEl = document.getElementById('ext-copy-fields');
  copyFieldsEl.innerHTML = '';

  // Step-by-step instructions that match the actual IRS flow
  if (method === 'direct_pay') {
    const steps = document.createElement('div');
    steps.className = 'mb-3 text-xs text-gray-400 space-y-1';
    steps.innerHTML = '<p class="text-gray-300 font-medium mb-1">How to file your extension:</p>' +
      '<p>1. Review your information below — pre-filled from your uploaded documents</p>' +
      '<p>2. Click <strong class="text-white">Generate Form 4868</strong> to create your completed form</p>' +
      '<p>3. Print or save as PDF &rarr; mail to the IRS address shown on the form</p>' +
      '<p>4. Enter your <strong class="text-green-400">confirmation details</strong> below for your records</p>';
    copyFieldsEl.appendChild(steps);
  } else if (method === 'free_file') {
    const steps = document.createElement('div');
    steps.className = 'mb-3 text-xs text-gray-400 space-y-1';
    steps.innerHTML = '<p class="text-gray-300 font-medium mb-1">IRS Free File steps:</p>' +
      '<p>1. Click "Open IRS Website" below &rarr; choose any Free File partner</p>' +
      '<p>2. Select <strong class="text-white">File an Extension (Form 4868)</strong></p>' +
      '<p>3. Enter the values below when prompted</p>' +
      '<p>4. Submit &rarr; save your <strong class="text-green-400">confirmation email</strong></p>';
    copyFieldsEl.appendChild(steps);
  }

  // Fetch taxpayer profile for identity fields
  try {
    const profResp = await authFetch('/api/tax/profile?year=' + yr);
    const profData = await profResp.json();
    const p = profData.profile || {};
    const filingLabels = {single:'Single', married_jointly:'Married Filing Jointly', married_separately:'Married Filing Separately', head_of_household:'Head of Household'};

    // Identity section
    addSection(copyFieldsEl, 'Identity verification');
    addCopyRow(copyFieldsEl, 'SSN', p.ssn || '(set in Tax Profile above)');
    if (p.date_of_birth) {
      addCopyRow(copyFieldsEl, 'Date of Birth', p.date_of_birth);
    } else {
      // DOB not on any uploaded doc — let user enter it inline
      const dobRow = document.createElement('div');
      dobRow.className = 'flex items-center justify-between py-0.5';
      dobRow.innerHTML = '<span class="text-xs text-gray-400">Date of Birth</span>' +
        '<div class="flex items-center gap-2">' +
        '<input type="date" id="ext-dob-input" class="bg-gray-800 border border-gray-600 rounded px-2 py-0.5 text-sm text-white" style="color-scheme:dark">' +
        '<button onclick="saveExtDob()" class="text-[10px] text-oc-500 hover:text-oc-400 px-1.5 py-0.5 bg-gray-800 rounded">Save</button>' +
        '</div>';
      copyFieldsEl.appendChild(dobRow);
    }
    addCopyRow(copyFieldsEl, 'Filing Status', filingLabels[p.filing_status] || p.filing_status || 'Single');
    if (p.first_name) addCopyRow(copyFieldsEl, 'Name', p.first_name + ' ' + (p.last_name || ''));
    if (p.address_line1) addCopyRow(copyFieldsEl, 'Street Address', p.address_line1);
    if (p.city) addCopyRow(copyFieldsEl, 'City, State, ZIP', p.city + ', ' + (p.state || '') + ' ' + (p.zip || ''));

    // Spouse section (if filing jointly)
    if (p.filing_status === 'married_jointly') {
      addSection(copyFieldsEl, 'Spouse information (jointly filed)');
      addCopyRow(copyFieldsEl, 'Spouse Name', (p.spouse_first || '') + ' ' + (p.spouse_last || '') || '(set in Tax Profile)');
      addCopyRow(copyFieldsEl, 'Spouse SSN', p.spouse_ssn || '(set in Tax Profile)');
      if (p.spouse_dob) addCopyRow(copyFieldsEl, 'Spouse DOB', p.spouse_dob);
    }

    // Payment section
    addSection(copyFieldsEl, 'Payment details');
  } catch(e) {
    addSection(copyFieldsEl, 'Payment details (set up Tax Profile above for identity fields)');
  }

  addCopyRow(copyFieldsEl, 'Tax Year', yr);
  addCopyRow(copyFieldsEl, 'Estimated Total Tax', tax);
  addCopyRow(copyFieldsEl, 'Total Payments Made', paid);
  addCopyRow(copyFieldsEl, 'Balance Due', bal);
  addCopyRow(copyFieldsEl, 'Payment Amount', '$' + parseFloat(pay).toFixed(2));

  // Show the form generation button
  const formLink = document.getElementById('ext-form-link');
  if (formLink) formLink.style.display = '';
}

async function saveExtDob() {
  const dob = document.getElementById('ext-dob-input')?.value;
  if (!dob) return;
  const yr = selectedYear || yearStart().slice(0,4);
  try {
    await authFetch('/api/tax/profile', {
      method: 'POST', headers: {'Content-Type':'application/json'},
      body: JSON.stringify({token, year: parseInt(yr), date_of_birth: dob})
    });
    // Replace the input with a copy row
    const row = document.getElementById('ext-dob-input').closest('.flex');
    row.innerHTML = '<span class="text-xs text-gray-400">Date of Birth</span><div class="flex items-center gap-2"><span class="text-sm text-white font-mono">' + dob + '</span></div>';
    const btn = document.createElement('button');
    btn.className = 'text-[10px] text-oc-500 hover:text-oc-400 px-1.5 py-0.5 bg-gray-800 rounded';
    btn.textContent = 'Copy';
    btn.onclick = function() { navigator.clipboard.writeText(dob).then(() => { btn.textContent = 'Copied!'; setTimeout(() => btn.textContent = 'Copy', 1500); }); };
    row.querySelector('div').appendChild(btn);
  } catch(e) { console.error('saveExtDob:', e); }
}

function addSection(parent, title) {
  const s = document.createElement('div');
  s.className = 'border-t border-gray-700 pt-2 mt-2';
  s.innerHTML = '<p class="text-[10px] text-gray-500 font-medium mb-1">' + title + '</p>';
  parent.appendChild(s);
}

function addCopyRow(parent, label, value) {
  const row = document.createElement('div');
  row.className = 'flex items-center justify-between py-0.5';
  row.innerHTML = '<span class="text-xs text-gray-400">' + label + '</span><div class="flex items-center gap-2"><span class="text-sm text-white font-mono">' + (value || '—') + '</span></div>';
  if (value && !value.startsWith('(')) {
    const btn = document.createElement('button');
    btn.className = 'text-[10px] text-oc-500 hover:text-oc-400 px-1.5 py-0.5 bg-gray-800 rounded';
    btn.textContent = 'Copy';
    btn.onclick = function() { navigator.clipboard.writeText(value).then(() => { btn.textContent = 'Copied!'; setTimeout(() => btn.textContent = 'Copy', 1500); }); };
    row.querySelector('div').appendChild(btn);
  }
  parent.appendChild(row);
}

function openEnvelope() {
  const yr = selectedYear || yearStart().slice(0,4);
  const payment = parseCents(document.getElementById('ext-payment').value);
  const params = new URLSearchParams({ token, year: yr, payment });
  const opt = id => (document.getElementById(id) || {}).value || '';
  [['name','ext-opt-name'],['address','ext-opt-address'],['city','ext-opt-city'],
   ['state','ext-opt-state'],['zip','ext-opt-zip']
  ].forEach(([k, id]) => { const v = opt(id).trim(); if (v) params.set(k, v); });
  window.location.href = '/api/tax/extension/envelope?' + params.toString();
}

function openForm4868() {
  const yr = selectedYear || yearStart().slice(0,4);
  const payment = parseCents(document.getElementById('ext-payment').value);
  const params = new URLSearchParams({ token, year: yr, payment });
  // Optional overrides + checkboxes from the "Form 4868 options" section.
  const opt = id => (document.getElementById(id) || {}).value || '';
  [['name','ext-opt-name'],['address','ext-opt-address'],['city','ext-opt-city'],
   ['state','ext-opt-state'],['zip','ext-opt-zip'],['ssn','ext-opt-ssn'],
   ['spouse_ssn','ext-opt-spouse-ssn'],
   ['fy_begin','ext-opt-fy-begin'],['fy_end','ext-opt-fy-end'],['fy_end_year','ext-opt-fy-end-year']
  ].forEach(([k, id]) => { const v = opt(id).trim(); if (v) params.set(k, v); });
  if (document.getElementById('ext-opt-ooc') && document.getElementById('ext-opt-ooc').checked) params.set('out_of_country', '1');
  if (document.getElementById('ext-opt-1040nr') && document.getElementById('ext-opt-1040nr').checked) params.set('is_1040nr', '1');
  window.location.href = '/api/tax/extension?' + params.toString();
}

async function confirmExtension() {
  const confirmId = document.getElementById('ext-confirm-input').value.trim();
  if (!confirmId) { document.getElementById('ext-confirm-result').textContent = 'Please enter your confirmation number.'; return; }
  try {
    const resp = await authFetch(`/api/tax/extension/${extId}/confirm`, {
      method:'PUT', headers:{'Content-Type':'application/json'},
      body: JSON.stringify({token, confirmation_id: confirmId})
    });
    const data = await resp.json();
    if (data.success) {
      renderExtConfirmed({confirmation_id: confirmId, filing_method: extData?.extension?.filing_method || 'direct_pay',
        filed_at: Date.now()/1000, balance_due: document.getElementById('ext-balance').value,
        ...data.extension});
      showExtStep('confirmed');
      updateExtBadge('confirmed');
    } else {
      document.getElementById('ext-confirm-result').className = 'text-xs mt-1 block text-red-400';
      document.getElementById('ext-confirm-result').textContent = data.error || 'Failed to confirm.';
    }
  } catch(e) {}
}

function renderExtConfirmed(ext) {
  const labels = {direct_pay:'IRS Direct Pay', free_file:'IRS Free File', mail:'Print & Mail'};
  document.getElementById('ext-confirmed-id').textContent = ext.confirmation_id || '—';
  document.getElementById('ext-confirmed-detail').textContent = 'Filed via ' + (labels[ext.filing_method]||ext.filing_method||'unknown');
  document.getElementById('ext-confirmed-date').textContent = ext.filed_at ? new Date(ext.filed_at * 1000).toLocaleDateString() : new Date().toLocaleDateString();
  document.getElementById('ext-confirmed-balance').textContent = ext.balance_due || document.getElementById('ext-balance')?.value || '—';
  const yr = selectedYear || yearStart().slice(0,4);
  document.getElementById('ext-deadline').textContent = `October 15, ${parseInt(yr)+1}`;
}

// ── Deductions Tab ──────────────────────────────────────────────────────
let dedQAnswers = {};
let dedQComplete = false;
let dedCurrentStatus = 'pending';
let dedCurrentType = '';
let dedCandidates = [];
let dedSelectedIdx = -1;
let dedReviewMode = false;

async function loadDeductionsTab() {
  await loadQuestionnaire();
  await loadDeductionSummary();
  await loadCandidates();
}

async function loadQuestionnaire() {
  try {
    const resp = await authFetch(`/api/tax/questionnaire?year=${selectedYear || yearStart().slice(0,4)}`);
    const data = await resp.json();
    const q = data.questionnaire || {};
    dedQAnswers = q.answers || {};
    dedQComplete = q.completed || false;
    renderQuestionnaire();
  } catch(e) {}
}

function renderQuestionnaire() {
  if (dedQComplete) {
    document.getElementById('ded-quest-wizard').classList.add('hidden');
    document.getElementById('ded-quest-summary').classList.remove('hidden');
    document.getElementById('ded-quest-status').textContent = 'Completed';
    const tags = document.getElementById('ded-quest-tags');
    tags.innerHTML = '';
    const labels = {self_employed:'Self-Employed', home_office:'Home Office', health_insurance_self:'Own Health Insurance',
      hdhp:'HDHP/HSA', vehicle_business:'Business Vehicle', retirement_contributions:'Retirement',
      student_loan_interest:'Student Loans', charitable_donations:'Charitable'};
    for (const [k,v] of Object.entries(dedQAnswers)) {
      if (v === true) tags.innerHTML += `<span class="badge badge-green">${labels[k]||k}</span>`;
    }
    if (dedQAnswers.filing_status) tags.innerHTML += `<span class="badge badge-blue">${dedQAnswers.filing_status.replace('_',' ')}</span>`;
    if (dedQAnswers.dependents > 0) tags.innerHTML += `<span class="badge badge-blue">${dedQAnswers.dependents} dependent${dedQAnswers.dependents>1?'s':''}</span>`;
  } else {
    document.getElementById('ded-quest-wizard').classList.remove('hidden');
    document.getElementById('ded-quest-summary').classList.add('hidden');
    document.getElementById('ded-quest-status').textContent = 'Not completed';
    // Restore toggle states
    for (const key of ['self_employed','home_office','health_insurance_self','hdhp','vehicle_business','retirement_contributions','student_loan_interest','charitable_donations']) {
      updateToggleUI(key, !!dedQAnswers[key]);
    }
    if (dedQAnswers.filing_status) document.getElementById('q-filing-status').value = dedQAnswers.filing_status;
    if (dedQAnswers.dependents) document.getElementById('q-dependents').value = dedQAnswers.dependents;
  }
}

function updateToggleUI(key, on) {
  const btn = document.getElementById('qtog-' + key);
  if (!btn) return;
  btn.className = `w-10 h-5 rounded-full ${on ? 'bg-oc-600' : 'bg-gray-700'} relative transition-colors flex-shrink-0`;
  btn.firstElementChild.style.transform = on ? 'translateX(20px)' : 'translateX(0)';
}

function toggleQAnswer(key) {
  dedQAnswers[key] = !dedQAnswers[key];
  updateToggleUI(key, dedQAnswers[key]);
  saveQAnswer(key, dedQAnswers[key]);
}

async function saveQAnswer(key, value) {
  dedQAnswers[key] = value;
  try {
    await authFetch('/api/tax/questionnaire', {
      method: 'POST', headers: {'Content-Type':'application/json'},
      body: JSON.stringify({ year: parseInt(selectedYear || yearStart().slice(0,4)), answers: dedQAnswers, completed: false })
    });
  } catch(e) {}
}

function dedStep(n) {
  document.querySelectorAll('.ded-step').forEach(el => el.classList.add('hidden'));
  const next = document.getElementById('ded-step-' + n);
  if (next) next.classList.remove('hidden');
}

function editQuestionnaire() {
  dedQComplete = false;
  renderQuestionnaire();
  dedStep(1);
}

async function completeQuestionnaire() {
  try {
    await authFetch('/api/tax/questionnaire', {
      method: 'POST', headers: {'Content-Type':'application/json'},
      body: JSON.stringify({ year: parseInt(selectedYear || yearStart().slice(0,4)), answers: dedQAnswers, completed: true })
    });
    dedQComplete = true;
    renderQuestionnaire();
    await triggerScan();
  } catch(e) {}
}

async function triggerScan() {
  const btn = document.getElementById('scan-btn');
  const msg = document.getElementById('scan-status-msg');
  btn.textContent = 'Scanning...'; btn.disabled = true;
  msg.textContent = 'Quick scan: checking vendor names and descriptions...'; msg.classList.remove('hidden');
  try {
    const resp = await authFetch('/api/tax/deductions/scan', {
      method: 'POST', headers: {'Content-Type':'application/json'},
      body: JSON.stringify({ year: parseInt(selectedYear || yearStart().slice(0,4)) })
    });
    const data = await resp.json();
    const found = data.scan?.candidates_found || 0;
    msg.textContent = found > 0 ? `Quick scan found ${found} potential deduction${found>1?'s':''}` : 'Quick scan complete — no new deductions found';
    msg.className = 'text-xs mb-2 ' + (found > 0 ? 'text-green-400' : 'text-gray-500');
    await loadDeductionSummary();
    await loadCandidates();
  } catch(e) { msg.textContent = 'Scan error'; msg.className = 'text-xs mb-2 text-red-400'; }
  btn.textContent = 'Quick Scan'; btn.disabled = false;
}

async function triggerDeepScan() {
  const btn = document.getElementById('deep-scan-btn');
  const msg = document.getElementById('scan-status-msg');
  btn.textContent = 'Analyzing...'; btn.disabled = true;
  document.getElementById('scan-btn').disabled = true;
  msg.classList.remove('hidden');
  msg.className = 'text-xs mb-2 text-oc-400';
  msg.textContent = 'AI Deep Scan: analyzing receipts and documents with AI vision... This may take a minute.';
  try {
    const resp = await authFetch('/api/tax/deductions/deep-scan', {
      method: 'POST', headers: {'Content-Type':'application/json'},
      body: JSON.stringify({ year: parseInt(selectedYear || yearStart().slice(0,4)) })
    });
    const data = await resp.json();
    const found = data.candidates_found || 0;
    const docs = data.documents_analyzed || 0;
    msg.textContent = `Deep scan complete: analyzed ${docs} documents, found ${found} potential deduction${found!==1?'s':''}`;
    msg.className = 'text-xs mb-2 ' + (found > 0 ? 'text-green-400' : 'text-gray-500');
    await loadDeductionSummary();
    await loadCandidates();
  } catch(e) { msg.textContent = 'Deep scan error: ' + (e.message||'unknown'); msg.className = 'text-xs mb-2 text-red-400'; }
  btn.textContent = 'AI Deep Scan'; btn.disabled = false;
  document.getElementById('scan-btn').disabled = false;
}

async function loadDeductionSummary() {
  try {
    const resp = await authFetch(`/api/tax/deductions/summary?year=${selectedYear || yearStart().slice(0,4)}`);
    const data = await resp.json();
    document.getElementById('ded-pending').textContent = data.total_pending || 0;
    const appCents = data.total_approved_cents || 0;
    document.getElementById('ded-approved').textContent = Object.values(data.by_type||{}).reduce((s,t)=>s+(t.approved||0),0);
    document.getElementById('ded-denied').textContent = Object.values(data.by_type||{}).reduce((s,t)=>s+(t.denied||0),0);
    document.getElementById('ded-saved').textContent = data.total_approved_display || '$0.00';
    // Badge on the Deductions sub-tab chip (null-safe — chip only exists when section is active)
    const badge = document.getElementById('ded-badge');
    if (badge) {
      if (data.total_pending > 0) { badge.textContent = data.total_pending; badge.classList.remove('hidden'); }
      else { badge.classList.add('hidden'); }
    }
  } catch(e) {}
}

async function loadCandidates() {
  try {
    const yr = selectedYear || yearStart().slice(0,4);
    let url = `/api/tax/deductions/candidates?year=${yr}&status=${dedCurrentStatus}`;
    if (dedCurrentType) url += `&type=${dedCurrentType}`;
    const resp = await authFetch(url);
    const data = await resp.json();
    dedCandidates = data.candidates || [];
    // Update filter counts
    const c = data.counts || {};
    document.getElementById('ded-pending').textContent = c.pending || 0;
    document.getElementById('ded-approved').textContent = c.approved || 0;
    document.getElementById('ded-denied').textContent = c.denied || 0;
    renderCandidateList();
  } catch(e) {}
}

function filterDedStatus(s) {
  dedCurrentStatus = s;
  ['pending','approved','denied'].forEach(st => {
    const btn = document.getElementById('ded-filter-' + st);
    if (btn) btn.className = 'text-xs px-2 py-1 rounded-lg ' + (st === s ? 'bg-gray-700 text-yellow-400' : 'text-gray-500 hover:text-gray-300');
  });
  loadCandidates();
}

function filterDedType(t) { dedCurrentType = t; loadCandidates(); }

function renderCandidateList() {
  const list = document.getElementById('ded-candidates-list');
  if (dedCandidates.length === 0) {
    list.innerHTML = '<p class="text-xs text-gray-600 text-center py-8">No candidates found. Try scanning or adjusting filters.</p>';
    exitReviewMode();
    return;
  }
  list.innerHTML = dedCandidates.map((c, i) => `
    <div class="flex items-center gap-3 p-2.5 rounded-lg ${dedCurrentStatus==='pending'?'bg-gray-800 hover:bg-gray-700 cursor-pointer':'bg-gray-900'} transition-colors" onclick="selectCandidate(${i})">
      <span class="text-xs text-gray-500 w-20 flex-shrink-0">${c.transaction_date||''}</span>
      <span class="text-sm text-gray-300 flex-1 truncate">${c.vendor||c.description||'—'}</span>
      <span class="badge ${badgeClass(c.deduction_type)} text-[10px]">${c.deduction_type}</span>
      <span class="text-sm font-medium ${c.status==='approved'?'text-green-400':c.status==='denied'?'text-gray-500':'text-white'}">${c.amount_display}</span>
      ${c.status==='pending'?`<span class="text-[10px] text-gray-500">${Math.round(c.confidence*100)}%</span>`:''}
    </div>`).join('');
}

function badgeClass(type) {
  const m = {medical:'badge-green', health_insurance:'badge-green', vehicle:'badge-blue', home_office:'badge-blue',
    software:'badge-blue', education:'badge-yellow', charitable:'badge-yellow', professional:'badge-blue',
    retirement:'badge-gray', student_loan:'badge-gray', hsa:'badge-green'};
  return m[type] || 'badge-gray';
}

async function selectCandidate(idx) {
  if (dedCurrentStatus !== 'pending') return;
  dedSelectedIdx = idx;
  const c = dedCandidates[idx];
  if (!c) return;

  // Enter review mode
  if (!dedReviewMode) {
    dedReviewMode = true;
    document.getElementById('ded-list-mode').classList.add('hidden');
    document.getElementById('ded-review-mode').classList.remove('hidden');
  }

  // Render review list
  const rl = document.getElementById('ded-review-list');
  rl.innerHTML = dedCandidates.map((cc, i) => `
    <div class="p-2 rounded-lg text-xs cursor-pointer transition-colors ${i===idx?'bg-oc-600/20 border border-oc-600':'bg-gray-900 hover:bg-gray-800'}" onclick="selectCandidate(${i})">
      <div class="flex justify-between"><span class="text-gray-400 truncate">${cc.vendor||'—'}</span><span class="font-medium text-white">${cc.amount_display}</span></div>
      <div class="flex justify-between mt-0.5"><span class="text-gray-600">${cc.transaction_date||''}</span><span class="badge ${badgeClass(cc.deduction_type)} text-[9px]">${cc.deduction_type}</span></div>
    </div>`).join('');

  // Set category/entity dropdowns
  const catSel = document.getElementById('review-category');
  if (catSel.options.length <= 1) {
    const cats = await authFetch(`/api/tax/categories`).then(r=>r.json());
    catSel.innerHTML = (cats.categories||[]).map(cc=>`<option value="${cc.name}">${cc.name}</option>`).join('');
  }
  if (c.category_suggestion) catSel.value = c.category_suggestion;
  document.getElementById('review-entity').value = c.entity_suggestion || 'personal';

  // Load context (document viewer)
  try {
    const resp = await authFetch(`/api/tax/deductions/candidates/${c.id}/context`);
    const ctx = await resp.json();
    const viewer = document.getElementById('ded-viewer');
    if (ctx.source_document) {
      viewer.innerHTML = `
        <div class="bg-yellow-900/20 border border-yellow-800/30 rounded-lg p-2 mb-2 text-xs">
          <span class="text-yellow-400 font-medium">${c.vendor||'Unknown'}</span> &mdash; ${c.amount_display} on ${c.transaction_date||'N/A'}
          <span class="ml-2 badge ${badgeClass(c.deduction_type)}">${c.deduction_type}</span>
          <span class="ml-2 text-gray-500">${Math.round(c.confidence*100)}% confidence</span>
        </div>
        <img src="${ctx.source_document.image_url}" class="w-full rounded" alt="Source document" onerror="this.outerHTML='<p class=\\'text-xs text-gray-500 text-center py-4\\'>Document image unavailable</p>'">`;
    } else {
      viewer.innerHTML = `
        <div class="flex flex-col items-center justify-center h-full gap-3">
          <div class="bg-gray-800 rounded-xl border border-gray-700 p-6 w-full max-w-sm">
            <div class="text-center mb-4"><span class="badge ${badgeClass(c.deduction_type)} text-xs">${c.deduction_type}</span></div>
            <div class="space-y-2 text-sm">
              <div class="flex justify-between"><span class="text-gray-500">Vendor</span><span class="text-white">${c.vendor||'—'}</span></div>
              <div class="flex justify-between"><span class="text-gray-500">Description</span><span class="text-white truncate ml-4">${c.description||'—'}</span></div>
              <div class="flex justify-between"><span class="text-gray-500">Amount</span><span class="text-green-400 font-medium">${c.amount_display}</span></div>
              <div class="flex justify-between"><span class="text-gray-500">Date</span><span class="text-white">${c.transaction_date||'—'}</span></div>
              <div class="flex justify-between"><span class="text-gray-500">Confidence</span><span class="text-white">${Math.round(c.confidence*100)}%</span></div>
              <div class="flex justify-between"><span class="text-gray-500">Source</span><span class="text-gray-400">${c.source_type==='financial_transaction'?'Bank/API sync':'Statement scan'}</span></div>
            </div>
          </div>
        </div>`;
    }
  } catch(e) {
    document.getElementById('ded-viewer').innerHTML = '<p class="text-xs text-red-400 text-center py-4">Failed to load context</p>';
  }
}

function exitReviewMode() {
  dedReviewMode = false;
  dedSelectedIdx = -1;
  document.getElementById('ded-list-mode').classList.remove('hidden');
  document.getElementById('ded-review-mode').classList.add('hidden');
}

async function reviewAction(action) {
  if (dedSelectedIdx < 0) return;
  const c = dedCandidates[dedSelectedIdx];
  if (!c) return;
  try {
    await authFetch(`/api/tax/deductions/candidates/${c.id}/review`, {
      method: 'PUT', headers: {'Content-Type':'application/json'},
      body: JSON.stringify({
        token, action,
        category: document.getElementById('review-category').value,
        entity: document.getElementById('review-entity').value,
      })
    });
    // Remove from list and advance
    dedCandidates.splice(dedSelectedIdx, 1);
    if (dedCandidates.length === 0) { exitReviewMode(); loadDeductionSummary(); loadCandidates(); return; }
    if (dedSelectedIdx >= dedCandidates.length) dedSelectedIdx = dedCandidates.length - 1;
    selectCandidate(dedSelectedIdx);
    loadDeductionSummary();
  } catch(e) {}
}

// ── Helpers for new tabs ──
function toggleForm(id) {
  const el = document.getElementById(id);
  if (el) el.classList.toggle('hidden');
}
function fmtCents(c) {
  if (c === undefined || c === null) return '$0.00';
  const sign = c < 0 ? '-' : '';
  const abs = Math.abs(c) / 100;
  return sign + '$' + abs.toFixed(2).replace(/\B(?=(\d{3})+(?!\d))/g, ',');
}
function parseCurrencyInput(id) {
  const v = parseFloat(document.getElementById(id).value || '0');
  return Math.round(v * 100);
}
function escapeHtml(s) { return String(s == null ? '' : s).replace(/[&<>"']/g, c => ({'&':'&amp;','<':'&lt;','>':'&gt;','"':'&quot;',"'":'&#39;'}[c])); }

// ── Credits tab ──
async function loadCreditsTab() {
  try {
    const resp = await authFetch(`/api/tax/credits/eligibility?year=${selectedYear}`);
    const data = await resp.json();
    const grid = document.getElementById('credits-eligibility-grid');
    const credits = data.credits || [];
    if (credits.length === 0) {
      grid.innerHTML = '<p class="text-xs text-gray-600 col-span-full">No credit data available.</p>';
    } else {
      grid.innerHTML = credits.map(c => `
        <div class="p-3 rounded-lg ${c.eligible ? 'bg-green-500/10 border border-green-600/30' : 'bg-gray-900/40 border border-gray-700'}">
          <div class="flex items-center justify-between mb-1">
            <span class="text-sm font-medium text-gray-200">${escapeHtml(c.name)}</span>
            <span class="text-xs ${c.eligible ? 'text-green-400' : 'text-gray-500'}">${c.eligible ? 'Eligible' : 'Not eligible'}</span>
          </div>
          <p class="text-lg font-semibold ${c.eligible ? 'text-green-400' : 'text-gray-500'}">${escapeHtml(c.amount || '$0.00')}</p>
          <p class="text-xs text-gray-500 mt-1">${escapeHtml(c.reason || '')}</p>
        </div>
      `).join('');
    }
    document.getElementById('credits-total').textContent = data.total || '$0.00';
  } catch(e) {
    document.getElementById('credits-eligibility-grid').innerHTML = '<p class="text-xs text-red-400 col-span-full">Failed to load credits: ' + escapeHtml(e.message) + '</p>';
  }
}

async function saveEducation() {
  const body = {
    token, tax_year: selectedYear,
    student_name: document.getElementById('edu-student').value,
    institution: document.getElementById('edu-school').value,
    tuition_cents: parseCurrencyInput('edu-tuition'),
    fees_cents: parseCurrencyInput('edu-fees'),
    books_cents: parseCurrencyInput('edu-books'),
  };
  if (!body.student_name || !body.institution || body.tuition_cents === 0) {
    alert('Student name, institution, and tuition are required.');
    return;
  }
  const resp = await authFetch('/api/tax/credits/education', { method: 'POST', headers: {'Content-Type':'application/json'}, body: JSON.stringify(body) });
  const data = await resp.json();
  if (data.ok) {
    ['edu-student','edu-school','edu-tuition','edu-fees','edu-books'].forEach(id => document.getElementById(id).value = '');
    document.getElementById('edu-form').classList.add('hidden');
    loadCreditsTab();
  } else {
    alert('Save failed: ' + (data.error || 'unknown error'));
  }
}

async function saveChildcare() {
  const body = {
    token, tax_year: selectedYear,
    provider_name: document.getElementById('care-provider').value,
    amount_cents: parseCurrencyInput('care-amount'),
    dependent_id: parseInt(document.getElementById('care-dep').value) || null,
  };
  if (!body.provider_name || body.amount_cents === 0) { alert('Provider and amount required.'); return; }
  const resp = await authFetch('/api/tax/credits/childcare', { method: 'POST', headers: {'Content-Type':'application/json'}, body: JSON.stringify(body) });
  const data = await resp.json();
  if (data.ok) {
    ['care-provider','care-amount','care-dep'].forEach(id => document.getElementById(id).value = '');
    document.getElementById('care-form').classList.add('hidden');
    loadCreditsTab();
  } else { alert('Save failed'); }
}

async function saveEnergy() {
  const cost = parseCurrencyInput('energy-cost');
  const qual = parseCurrencyInput('energy-qual');
  const body = {
    token, tax_year: selectedYear,
    improvement_type: document.getElementById('energy-type').value,
    cost_cents: cost,
    qualifying_cents: qual > 0 ? qual : null,
    vendor: document.getElementById('energy-vendor').value || null,
  };
  if (cost === 0) { alert('Cost is required.'); return; }
  const resp = await authFetch('/api/tax/credits/energy', { method: 'POST', headers: {'Content-Type':'application/json'}, body: JSON.stringify(body) });
  const data = await resp.json();
  if (data.ok) {
    ['energy-cost','energy-qual','energy-vendor'].forEach(id => document.getElementById(id).value = '');
    document.getElementById('energy-form').classList.add('hidden');
    loadCreditsTab();
  } else { alert('Save failed'); }
}

// ── Quarterly tab ──
async function loadQuarterlyTab() {
  try {
    const fs = 'single'; // TODO: from profile; handler defaults this anyway
    const [recResp, listResp, projResp] = await Promise.all([
      authFetch(`/api/tax/estimated-payments/recommended?year=${selectedYear}&filing_status=${fs}`),
      authFetch(`/api/tax/estimated-payments?year=${selectedYear}`),
      authFetch(`/api/tax/projection?year=${selectedYear}`),
    ]);
    const rec = await recResp.json();
    const list = await listResp.json();
    const proj = await projResp.json().catch(() => ({}));

    document.getElementById('qtr-projected-tax').textContent = rec.projected_tax || '--';
    document.getElementById('qtr-owed').textContent = rec.projected_owed || '--';
    document.getElementById('qtr-per-quarter').textContent = rec.per_quarter || '--';
    document.getElementById('qtr-safe-harbor').textContent = rec.safe_harbor ? 'Safe harbor: ' + rec.safe_harbor : '';
    if (proj && proj.effective_rate) {
      document.getElementById('qtr-effective-rate').textContent = 'Effective rate: ' + proj.effective_rate;
    }

    const deadlines = list.deadlines || [];
    const byQ = {};
    (list.payments || []).forEach(p => { byQ[p.quarter] = p; });

    const qtrList = document.getElementById('qtr-list');
    qtrList.innerHTML = [1,2,3,4].map(q => {
      const p = byQ[q];
      const deadline = deadlines[q-1] || '';
      if (p) {
        return `<div class="flex items-center justify-between p-2 bg-gray-900/40 rounded-lg">
          <div><span class="text-sm font-medium text-gray-200">Q${q}</span> <span class="text-xs text-gray-500 ml-2">due ${escapeHtml(deadline)}</span></div>
          <div class="text-right">
            <p class="text-sm font-semibold text-green-400">${escapeHtml(p.amount)}</p>
            <p class="text-xs text-gray-500">${escapeHtml(p.payment_date || '')} · ${escapeHtml(p.method || '')} ${p.confirmation ? '· #' + escapeHtml(p.confirmation) : ''}</p>
          </div>
        </div>`;
      }
      return `<div class="flex items-center justify-between p-2 bg-gray-900/20 rounded-lg border border-gray-800">
        <div><span class="text-sm text-gray-400">Q${q}</span> <span class="text-xs text-gray-600 ml-2">due ${escapeHtml(deadline)}</span></div>
        <div class="text-xs text-gray-600">Not paid</div>
      </div>`;
    }).join('');
  } catch(e) {
    document.getElementById('qtr-list').innerHTML = '<p class="text-xs text-red-400">Failed to load: ' + escapeHtml(e.message) + '</p>';
  }
}

async function saveEstimatedPayment() {
  const body = {
    token, tax_year: selectedYear,
    quarter: parseInt(document.getElementById('qtr-q').value),
    amount_cents: parseCurrencyInput('qtr-amt'),
    payment_date: document.getElementById('qtr-date').value || null,
    payment_method: document.getElementById('qtr-method').value,
    confirmation_id: document.getElementById('qtr-conf').value || null,
  };
  if (body.amount_cents === 0) { alert('Amount required.'); return; }
  const resp = await authFetch('/api/tax/estimated-payments', { method: 'POST', headers: {'Content-Type':'application/json'}, body: JSON.stringify(body) });
  const data = await resp.json();
  if (data.ok) {
    ['qtr-amt','qtr-date','qtr-conf'].forEach(id => document.getElementById(id).value = '');
    loadQuarterlyTab();
  } else { alert('Save failed: ' + (data.error || 'unknown')); }
}

// ── Depreciation tab ──
const ASSET_CLASS_LIFE = { computer: 5, office_equipment: 7, vehicle: 5, machinery: 7, furniture: 7, improvement: 15, building_residential: 27, building_commercial: 39 };
function updateAssetLife() {
  const cls = document.getElementById('asset-class').value;
  document.getElementById('asset-life').value = ASSET_CLASS_LIFE[cls] || 5;
  document.getElementById('asset-is-vehicle').checked = (cls === 'vehicle');
}

let _assetCache = [];
async function loadDepreciationTab() {
  try {
    const resp = await authFetch(`/api/tax/assets?year=${selectedYear}`);
    const data = await resp.json();
    const assets = data.assets || [];
    _assetCache = assets;
    const list = document.getElementById('assets-list');
    const vehSelect = document.getElementById('veh-asset');
    const currentVehVal = vehSelect.value;
    vehSelect.innerHTML = '<option value="">-- pick vehicle --</option>' +
      assets.filter(a => a.is_vehicle).map(a => `<option value="${a.id}">${escapeHtml(a.description)}</option>`).join('');
    if (currentVehVal) vehSelect.value = currentVehVal;

    // Handler returns total_current_year; §179 and bonus are on each asset as display strings.
    // Sum by parsing display strings since no *_cents fields are exposed.
    const parseDisplay = s => {
      if (!s) return 0;
      const n = parseFloat(String(s).replace(/[$,]/g, ''));
      return isFinite(n) ? Math.round(n * 100) : 0;
    };
    let s179 = 0, bonus = 0;
    assets.forEach(a => { s179 += parseDisplay(a.section_179); bonus += parseDisplay(a.bonus_depr); });
    const totalYear = parseDisplay(data.total_current_year);
    const macrs = Math.max(totalYear - s179 - bonus, 0); // approximate, since totalYear is current_year_depr sum

    if (assets.length === 0) {
      list.innerHTML = '<p class="text-xs text-gray-600">No assets yet. Add a laptop, vehicle, equipment, or building above.</p>';
      document.getElementById('depr-179').textContent = '$0.00';
      document.getElementById('depr-bonus').textContent = '$0.00';
      document.getElementById('depr-macrs').textContent = '$0.00';
      document.getElementById('depr-total').textContent = '$0.00';
      return;
    }

    list.innerHTML = assets.map(a => `<div class="flex items-center justify-between p-2 bg-gray-900/40 rounded-lg">
      <div>
        <p class="text-sm font-medium text-gray-200">${escapeHtml(a.description)} ${a.is_vehicle ? '<span class="text-xs text-blue-400 ml-1">vehicle</span>' : ''}</p>
        <p class="text-xs text-gray-500">${escapeHtml(a.asset_class)} · ${a.macrs_life}yr MACRS · placed ${escapeHtml(a.placed_in_service)} · ${a.business_use_pct}% biz</p>
      </div>
      <div class="text-right">
        <p class="text-sm text-gray-300">${escapeHtml(a.cost_basis)}</p>
        <p class="text-xs text-gray-500">yr depr: ${escapeHtml(a.current_year_depr || '$0.00')}</p>
        <button onclick="viewSchedule(${a.id})" class="text-xs text-oc-400 hover:text-oc-300">schedule →</button>
      </div>
    </div>`).join('');
    document.getElementById('depr-179').textContent = fmtCents(s179);
    document.getElementById('depr-bonus').textContent = fmtCents(bonus);
    document.getElementById('depr-macrs').textContent = fmtCents(macrs);
    document.getElementById('depr-total').textContent = data.total_current_year || fmtCents(totalYear);
  } catch(e) {
    document.getElementById('assets-list').innerHTML = '<p class="text-xs text-red-400">Failed: ' + escapeHtml(e.message) + '</p>';
  }
}

async function viewSchedule(id) {
  try {
    const resp = await authFetch(`/api/tax/assets/${id}/schedule`);
    const data = await resp.json();
    const asset = _assetCache.find(a => a.id === id);
    const title = asset ? asset.description : ('Asset #' + id);
    const rows = (data.schedule || []).map(r => `<tr><td class="py-1 pr-3">${r.year}</td><td class="py-1 pr-3 text-right">${escapeHtml(r.depreciation)}</td><td class="py-1 pr-3 text-right">${escapeHtml(r.accumulated)}</td><td class="py-1 text-right">${escapeHtml(r.remaining)}</td></tr>`).join('');
    const html = `<div class="fixed inset-0 bg-black/70 z-50 flex items-center justify-center" onclick="this.remove()">
      <div class="bg-gray-900 border border-gray-700 rounded-xl p-4 max-w-xl w-full" onclick="event.stopPropagation()">
        <div class="flex items-center justify-between mb-3"><h3 class="font-medium">MACRS Schedule — ${escapeHtml(title)}</h3><button onclick="this.closest('.fixed').remove()" class="text-gray-400 hover:text-white">✕</button></div>
        <table class="w-full text-xs"><thead><tr class="text-gray-500 border-b border-gray-700"><th class="py-1 pr-3 text-left">Year</th><th class="py-1 pr-3 text-right">Depreciation</th><th class="py-1 pr-3 text-right">Accumulated</th><th class="py-1 text-right">Remaining</th></tr></thead><tbody>${rows}</tbody></table>
      </div>
    </div>`;
    document.body.insertAdjacentHTML('beforeend', html);
  } catch(e) { alert('Failed to load schedule: ' + e.message); }
}

async function saveAsset() {
  const body = {

    description: document.getElementById('asset-desc').value,
    asset_class: document.getElementById('asset-class').value,
    macrs_life_years: parseInt(document.getElementById('asset-life').value),
    cost_basis_cents: parseCurrencyInput('asset-cost'),
    placed_in_service: document.getElementById('asset-date').value,
    business_use_pct: parseInt(document.getElementById('asset-biz-pct').value) || 100,
    section_179_cents: parseCurrencyInput('asset-179'),
    use_bonus: document.getElementById('asset-bonus').checked,
    is_vehicle: document.getElementById('asset-is-vehicle').checked,
  };
  if (!body.description || !body.placed_in_service || body.cost_basis_cents === 0) {
    alert('Description, placed-in-service date, and cost are required.');
    return;
  }
  const resp = await authFetch('/api/tax/assets', { method: 'POST', headers: {'Content-Type':'application/json'}, body: JSON.stringify(body) });
  const data = await resp.json();
  if (data.ok) {
    ['asset-desc','asset-cost','asset-date','asset-179'].forEach(id => document.getElementById(id).value = '');
    document.getElementById('asset-form').classList.add('hidden');
    alert(`Saved. First-year total: ${data.first_year_total} (§179 ${data.section_179} + bonus ${data.bonus_depreciation} + MACRS ${data.first_year_macrs})`);
    loadDepreciationTab();
  } else { alert('Save failed'); }
}

async function saveVehicleUsage() {
  const body = {

    asset_id: parseInt(document.getElementById('veh-asset').value),
    tax_year: parseInt(document.getElementById('veh-year').value),
    business_miles: parseInt(document.getElementById('veh-biz-miles').value) || 0,
    total_miles: parseInt(document.getElementById('veh-total-miles').value) || 0,
  };
  if (!body.asset_id) { alert('Pick a vehicle.'); return; }
  const resp = await authFetch('/api/tax/vehicle-usage', { method: 'POST', headers: {'Content-Type':'application/json'}, body: JSON.stringify(body) });
  const data = await resp.json();
  if (data.ok !== false) {
    ['veh-biz-miles','veh-total-miles'].forEach(id => document.getElementById(id).value = '');
    alert('Vehicle usage saved.');
  } else { alert('Save failed: ' + (data.error || 'unknown')); }
}

// ── State tab ──
async function loadStateTab() {
  try {
    const [supResp, estResp, profResp] = await Promise.all([
      authFetch('/api/tax/state/supported'),
      authFetch(`/api/tax/state/estimate?year=${selectedYear}`),
      authFetch(`/api/tax/state/profile?year=${selectedYear}`),
    ]);
    const sup = await supResp.json();
    const est = await estResp.json();
    const prof = await profResp.json();

    // Populate state picker
    const sel = document.getElementById('st-state');
    if (sel.children.length === 0) {
      const bracketStates = new Set(sup.brackets_available || []);
      sel.innerHTML = (sup.states || []).map(s => {
        const label = s.has_income_tax ? (bracketStates.has(s.code) ? ` (${s.code})` : ` (${s.code}, flat/no brackets)`) : ` (${s.code}, no income tax)`;
        return `<option value="${s.code}">${escapeHtml(s.name)}${label}</option>`;
      }).join('');
    }

    document.getElementById('st-federal-agi').textContent = est.federal_agi || '--';
    document.getElementById('st-total-state-tax').textContent = est.total_state_tax || '--';
    document.getElementById('st-combined-rate').textContent = est.combined_effective_rate || '--';

    const bd = document.getElementById('state-breakdown');
    const states = est.states || [];
    const profiles = prof.profiles || [];
    if (profiles.length === 0) {
      bd.innerHTML = '<p class="text-xs text-gray-600">No state residencies added yet.</p>';
    } else {
      bd.innerHTML = profiles.map(p => {
        const s = states.find(x => x.state === p.state) || {};
        return `<div class="p-3 bg-gray-900/40 rounded-lg">
          <div class="flex items-center justify-between">
            <span class="text-sm font-medium text-gray-200">${escapeHtml(p.state)} — ${escapeHtml(p.residency)} (${p.months} mo)</span>
            <span class="text-sm text-oc-400">${escapeHtml(s.state_tax || '$0.00')}</span>
          </div>
          <p class="text-xs text-gray-500 mt-1">Wages ${escapeHtml(p.wages)} · Withheld ${escapeHtml(p.withheld)} · Owed ${escapeHtml(s.owed || '$0.00')}</p>
        </div>`;
      }).join('');
    }
  } catch(e) {
    document.getElementById('state-breakdown').innerHTML = '<p class="text-xs text-red-400">Failed: ' + escapeHtml(e.message) + '</p>';
  }
}

async function saveStateProfile() {
  const body = {
    token, tax_year: selectedYear,
    state: document.getElementById('st-state').value,
    residency_type: document.getElementById('st-residency').value,
    months_resident: parseInt(document.getElementById('st-months').value) || 12,
    state_wages_cents: parseCurrencyInput('st-wages'),
    state_withheld_cents: parseCurrencyInput('st-withheld'),
  };
  const resp = await authFetch('/api/tax/state/profile', { method: 'POST', headers: {'Content-Type':'application/json'}, body: JSON.stringify(body) });
  const data = await resp.json();
  if (data.ok) {
    ['st-wages','st-withheld'].forEach(id => document.getElementById(id).value = '');
    loadStateTab();
  } else { alert('Save failed'); }
}

// ── Entities tab ──
let currentEntityId = null;
async function loadEntitiesTab() {
  try {
    const resp = await authFetch(`/api/tax/entities`);
    const data = await resp.json();
    const list = document.getElementById('entities-list');
    const entities = data.entities || [];
    if (entities.length === 0) {
      list.innerHTML = '<p class="text-xs text-gray-600">No business entities. Add an S-Corp, LLC, or partnership to track P&amp;L separately from personal.</p>';
      return;
    }
    const typeLabels = { sole_prop:'Sole Prop', s_corp:'S-Corp', c_corp:'C-Corp', partnership:'Partnership', llc_single:'Single-Member LLC', llc_multi:'Multi-Member LLC' };
    list.innerHTML = entities.map(e => `<button onclick="showEntityDetail(${e.id})" class="w-full text-left p-2 bg-gray-900/40 hover:bg-gray-900/70 rounded-lg flex items-center justify-between transition-colors">
      <div>
        <p class="text-sm font-medium text-gray-200">${escapeHtml(e.name)}</p>
        <p class="text-xs text-gray-500">${escapeHtml(typeLabels[e.type] || e.type)} · ${escapeHtml(e.state || '')} · ${e.ownership_pct}% owned</p>
      </div>
      <span class="text-xs text-oc-400">open →</span>
    </button>`).join('');
  } catch(e) {
    document.getElementById('entities-list').innerHTML = '<p class="text-xs text-red-400">Failed: ' + escapeHtml(e.message) + '</p>';
  }
}

async function saveEntity() {
  const body = {

    entity_name: document.getElementById('ent-name').value,
    entity_type: document.getElementById('ent-type').value,
    ein: document.getElementById('ent-ein').value || null,
    state_of_formation: document.getElementById('ent-state').value || null,
    formation_date: document.getElementById('ent-formed').value || null,
    ownership_pct: parseInt(document.getElementById('ent-own').value) || 100,
  };
  if (!body.entity_name) { alert('Entity name required.'); return; }
  const resp = await authFetch('/api/tax/entities', { method: 'POST', headers: {'Content-Type':'application/json'}, body: JSON.stringify(body) });
  const data = await resp.json();
  if (data.ok) {
    ['ent-name','ent-ein','ent-state','ent-formed'].forEach(id => document.getElementById(id).value = '');
    document.getElementById('ent-form').classList.add('hidden');
    loadEntitiesTab();
  } else { alert('Save failed: ' + (data.error || 'unknown')); }
}

async function showEntityDetail(id) {
  currentEntityId = id;
  try {
    const resp = await authFetch(`/api/tax/entities/${id}/summary?year=${selectedYear}`);
    const data = await resp.json();
    document.getElementById('entity-detail').classList.remove('hidden');
    // Fetch name from the list
    const allResp = await authFetch(`/api/tax/entities`);
    const all = await allResp.json();
    const ent = (all.entities || []).find(e => e.id === id);
    document.getElementById('ent-detail-name').textContent = ent ? ent.name : 'Entity #' + id;
    document.getElementById('ent-d-income').textContent = data.income || '$0.00';
    document.getElementById('ent-d-expenses').textContent = data.expenses || '$0.00';
    document.getElementById('ent-d-net').textContent = data.net_income || '$0.00';
    document.getElementById('ent-d-tax').textContent = data.entity_tax || '$0.00';
    document.getElementById('ent-d-passthrough').textContent = data.is_pass_through ? 'Pass-through to shareholders' : 'Taxed at entity level (21%)';

    const sh = data.shareholders || [];
    document.getElementById('ent-d-shareholders').innerHTML = sh.length === 0 ? '<p class="text-xs text-gray-600">No shareholders yet.</p>' :
      sh.map(s => `<div class="p-2 bg-gray-900/40 rounded-lg text-xs">
        <div class="flex items-center justify-between"><span class="text-gray-200 font-medium">${escapeHtml(s.name)}</span><span class="text-gray-400">${s.ownership_pct}%</span></div>
        <p class="text-gray-500">Salary ${escapeHtml(s.salary)} · Distributions ${escapeHtml(s.distributions)} · K-1 ordinary ${escapeHtml(s.k1_ordinary)}</p>
      </div>`).join('');

    const cats = data.expense_categories || [];
    document.getElementById('ent-d-categories').innerHTML = cats.length === 0 ? '<p class="text-xs text-gray-600">No expenses yet</p>' :
      cats.map(c => `<div class="flex items-center justify-between text-xs"><span class="text-gray-300">${escapeHtml(c.category)}</span><span class="text-gray-400">${escapeHtml(c.amount)}</span></div>`).join('');
  } catch(e) { alert('Failed to load entity: ' + e.message); }
}
function hideEntityDetail() {
  currentEntityId = null;
  document.getElementById('entity-detail').classList.add('hidden');
}

function showAddShareholder() { document.getElementById('ent-sh-form').classList.toggle('hidden'); }
async function saveShareholder() {
  if (!currentEntityId) { alert('No entity selected.'); return; }
  const body = {
    token, entity_id: currentEntityId, tax_year: selectedYear,
    name: document.getElementById('ent-sh-name').value,
    ownership_pct: parseInt(document.getElementById('ent-sh-pct').value) || 0,
    salary_cents: parseCurrencyInput('ent-sh-salary'),
    distribution_cents: parseCurrencyInput('ent-sh-dist'),
  };
  if (!body.name) { alert('Name required.'); return; }
  const resp = await authFetch(`/api/tax/entities/${currentEntityId}/shareholders`, { method: 'POST', headers: {'Content-Type':'application/json'}, body: JSON.stringify(body) });
  const data = await resp.json();
  if (data.ok) {
    ['ent-sh-name','ent-sh-pct','ent-sh-salary','ent-sh-dist'].forEach(id => document.getElementById(id).value = '');
    document.getElementById('ent-sh-form').classList.add('hidden');
    showEntityDetail(currentEntityId);
  } else { alert('Save failed'); }
}

async function generateK1s() {
  if (!currentEntityId) return;
  try {
    const resp = await authFetch(`/api/tax/entities/${currentEntityId}/k1?year=${selectedYear}`);
    const data = await resp.json();
    const k1s = data.k1s || [];
    document.getElementById('ent-k1-results').innerHTML = k1s.length === 0 ? '<p class="text-xs text-gray-600">No shareholders — add one to generate K-1s.</p>' :
      '<div class="space-y-2">' + k1s.map(k => `<div class="p-2 bg-gray-900/40 rounded-lg text-xs">
        <p class="font-medium text-gray-200">${escapeHtml(k.form)} — ${escapeHtml(k.shareholder)} (${k.ownership_pct}%)</p>
        <p class="text-gray-500">Ordinary ${escapeHtml(k.ordinary_income)} · Salary ${escapeHtml(k.salary)} · Distributions ${escapeHtml(k.distributions)}</p>
      </div>`).join('') + '</div>';
  } catch(e) { alert('Failed: ' + e.message); }
}

async function issue1099() {
  if (!currentEntityId) return;
  const body = {
    token, entity_id: currentEntityId, tax_year: selectedYear,
    recipient_name: document.getElementById('ent-1099-name').value,
    recipient_address: document.getElementById('ent-1099-addr').value || null,
    amount_cents: parseCurrencyInput('ent-1099-amt'),
  };
  const resp = await authFetch(`/api/tax/entities/${currentEntityId}/1099`, { method: 'POST', headers: {'Content-Type':'application/json'}, body: JSON.stringify(body) });
  const data = await resp.json();
  if (data.ok) {
    ['ent-1099-name','ent-1099-addr','ent-1099-amt'].forEach(id => document.getElementById(id).value = '');
    document.getElementById('ent-1099-form').classList.add('hidden');
    loadEntity1099List();
  } else { alert('Issue failed: ' + (data.error || 'unknown')); }
}

async function loadEntity1099List() {
  if (!currentEntityId) return;
  try {
    const resp = await authFetch(`/api/tax/entities/${currentEntityId}/1099?year=${selectedYear}`);
    const data = await resp.json();
    const forms = data.forms || [];
    document.getElementById('ent-1099-results').innerHTML = forms.length === 0 ? '<p class="text-xs text-gray-600">No 1099s issued yet.</p>' :
      '<div class="space-y-1">' + forms.map(f => `<div class="flex items-center justify-between text-xs p-2 bg-gray-900/40 rounded-lg">
        <span class="text-gray-200">${escapeHtml(f.recipient)} <span class="text-gray-500">${escapeHtml(f.form)}</span></span>
        <span class="text-gray-300">${escapeHtml(f.amount)} <span class="text-xs text-gray-500">${escapeHtml(f.status)}</span></span>
      </div>`).join('') + '</div>';
  } catch(e) {}
}

async function loadEntityComparison() {
  const income = parseCurrencyInput('ent-cmp-income');
  const qs = `?year=${selectedYear}${income ? '&income=' + income : ''}`;
  try {
    const resp = await authFetch('/api/tax/entity-comparison' + qs);
    const data = await resp.json();
    const container = document.getElementById('ent-comparison');
    if (data.error) {
      container.innerHTML = `<p class="text-xs text-gray-600">${escapeHtml(data.error)}</p>`;
      return;
    }
    const sp = data.sole_proprietorship || {};
    const sc = data.s_corp || {};
    const savings = sc.savings_vs_sole_prop || '$0.00';
    const scIsBetter = savings && !savings.startsWith('-') && savings !== '$0.00';
    container.innerHTML = `<p class="text-xs text-gray-400 mb-3">Compared at SE income <span class="text-gray-200">${escapeHtml(data.income || '--')}</span></p>
      <div class="grid grid-cols-1 md:grid-cols-2 gap-3">
        <div class="p-3 bg-gray-900/40 rounded-lg border ${scIsBetter ? 'border-gray-700' : 'border-green-600/40'}">
          <p class="text-sm font-medium text-gray-200 mb-2">Sole Proprietorship</p>
          <p class="text-xs text-gray-500">SE tax</p>
          <p class="text-xl font-semibold ${scIsBetter ? 'text-red-400' : 'text-green-400'}">${escapeHtml(sp.se_tax || '--')}</p>
          <p class="text-xs text-gray-500 mt-2">${escapeHtml(sp.notes || '')}</p>
        </div>
        <div class="p-3 bg-gray-900/40 rounded-lg border ${scIsBetter ? 'border-green-600/40' : 'border-gray-700'}">
          <p class="text-sm font-medium text-gray-200 mb-2">S-Corporation</p>
          <p class="text-xs text-gray-500">Total FICA (salary portion)</p>
          <p class="text-xl font-semibold ${scIsBetter ? 'text-green-400' : 'text-red-400'}">${escapeHtml(sc.total_fica || '--')}</p>
          <p class="text-xs text-gray-500 mt-1">Reasonable salary ${escapeHtml(sc.reasonable_salary || '--')} · distribution ${escapeHtml(sc.distribution || '--')}</p>
          <p class="text-xs text-oc-400 mt-2">Savings vs Sole Prop: <span class="font-medium">${escapeHtml(savings)}</span></p>
          <p class="text-xs text-gray-500 mt-1">${escapeHtml(sc.notes || '')}</p>
        </div>
      </div>
      ${data.recommendation ? `<p class="text-xs text-gray-300 mt-3 p-2 bg-oc-500/10 border border-oc-600/30 rounded-lg">${escapeHtml(data.recommendation)}</p>` : ''}`;
  } catch(e) { document.getElementById('ent-comparison').innerHTML = '<p class="text-xs text-red-400">Failed: ' + escapeHtml(e.message) + '</p>'; }
}

// ── Insights tab ──
async function loadInsightsTab() {
  loadAuditRisk();
  loadInsightsList();
  loadTaxContext();
}

async function loadAuditRisk() {
  try {
    const resp = await authFetch(`/api/tax/audit-risk?year=${selectedYear}`);
    const data = await resp.json();
    const factors = data.factors || [];
    // The overall risk comes from the "overall" factor; others are individual factors.
    const overall = factors.find(f => f.factor === 'overall');
    const detail = factors.filter(f => f.factor !== 'overall');
    const riskLabel = (overall && overall.risk) || (detail.length === 0 ? 'low' : 'low');
    const ring = document.getElementById('audit-score-ring');
    const label = document.getElementById('audit-score-label');
    let color, text;
    if (riskLabel === 'high') { color = 'border-red-500 text-red-400'; text = 'High audit risk'; }
    else if (riskLabel === 'medium') { color = 'border-yellow-500 text-yellow-400'; text = 'Moderate audit risk'; }
    else { color = 'border-green-500 text-green-400'; text = 'Low audit risk'; }
    ring.textContent = detail.length;
    ring.className = 'w-20 h-20 rounded-full border-4 flex items-center justify-center text-2xl font-semibold ' + color;
    label.textContent = text;
    document.getElementById('audit-score-summary').textContent = (overall && overall.description) || (detail.length > 0 ? `${detail.length} factor${detail.length===1?'':'s'} identified` : 'No risk factors detected');

    document.getElementById('audit-factors').innerHTML = detail.length === 0 ? '' :
      detail.map(f => {
        const r = (f.risk || 'low').toLowerCase();
        const rcolor = r === 'high' ? 'text-red-400' : r === 'medium' ? 'text-yellow-400' : 'text-green-400';
        return `<div class="p-2 bg-gray-900/40 rounded-lg text-xs">
          <div class="flex items-center justify-between">
            <span class="text-gray-200 font-medium">${escapeHtml((f.factor || 'Factor').replace(/_/g,' '))}</span>
            <span class="${rcolor} uppercase">${escapeHtml(r)}</span>
          </div>
          <p class="text-gray-500 mt-0.5">${escapeHtml(f.description || '')}</p>
        </div>`;
      }).join('');
  } catch(e) {
    document.getElementById('audit-score-label').textContent = 'Failed to load';
    document.getElementById('audit-score-summary').textContent = e.message;
  }
}

async function loadInsightsList() {
  try {
    const resp = await authFetch(`/api/tax/insights?year=${selectedYear}`);
    const data = await resp.json();
    const insights = data.insights || [];
    const list = document.getElementById('insights-list');
    if (insights.length === 0) {
      list.innerHTML = '<p class="text-xs text-gray-600">No AI insights yet. Add receipts, income, and profile data so the advisor has something to analyze.</p>';
      return;
    }
    list.innerHTML = insights.map(i => {
      // Priority 8+ = high, 5-7 = medium, <5 = low
      const p = i.priority || 0;
      const bg = p >= 8 ? 'bg-red-500/10 border-red-600/30' : p >= 5 ? 'bg-yellow-500/10 border-yellow-600/30' : 'bg-oc-500/10 border-oc-600/30';
      const typeLabel = (i.type || '').replace(/_/g, ' ');
      return `<div class="p-3 rounded-lg border ${bg}">
        <div class="flex items-center justify-between mb-1">
          <span class="text-sm font-medium text-gray-200">${escapeHtml(i.title || 'Insight')}</span>
          ${typeLabel ? `<span class="text-xs text-gray-500 uppercase">${escapeHtml(typeLabel)}</span>` : ''}
        </div>
        <p class="text-xs text-gray-400">${escapeHtml(i.body || i.message || '')}</p>
      </div>`;
    }).join('');
  } catch(e) { document.getElementById('insights-list').innerHTML = '<p class="text-xs text-red-400">Failed: ' + escapeHtml(e.message) + '</p>'; }
}

async function runWhatIf() {
  const body = {
    token, tax_year: selectedYear,
    scenario_name: document.getElementById('wi-name').value || 'Scenario',
    additional_income_cents: parseCurrencyInput('wi-income'),
    additional_deduction_cents: parseCurrencyInput('wi-ded'),
    retirement_contribution_cents: parseCurrencyInput('wi-ret'),
    filing_status_override: document.getElementById('wi-fs').value || null,
  };
  try {
    const resp = await authFetch('/api/tax/what-if', { method: 'POST', headers: {'Content-Type':'application/json'}, body: JSON.stringify(body) });
    const data = await resp.json();
    const b = data.baseline || {}, s = data.scenario_result || {}, d = data.difference || {};
    document.getElementById('wi-result').innerHTML = `<div class="mt-3 p-3 bg-gray-900/40 rounded-lg text-xs">
      <p class="font-medium text-gray-200 mb-2">${escapeHtml(data.scenario || 'Scenario')}</p>
      <div class="grid grid-cols-3 gap-2">
        <div><p class="text-gray-500 uppercase">Baseline tax</p><p class="text-gray-300 text-sm">${escapeHtml(b.total_tax || '--')}</p></div>
        <div><p class="text-gray-500 uppercase">Scenario tax</p><p class="text-gray-300 text-sm">${escapeHtml(s.total_tax || '--')}</p></div>
        <div><p class="text-gray-500 uppercase">Change</p><p class="text-sm ${(d.tax_change||'').startsWith('-') ? 'text-green-400' : 'text-red-400'}">${escapeHtml(d.tax_change || '--')}</p></div>
      </div>
      ${d.savings && d.savings !== '$0.00' ? `<p class="mt-2 text-green-400">Potential savings: ${escapeHtml(d.savings)}</p>` : ''}
    </div>`;
  } catch(e) { document.getElementById('wi-result').innerHTML = '<p class="text-xs text-red-400 mt-2">Failed: ' + escapeHtml(e.message) + '</p>'; }
}

async function loadTaxContext() {
  try {
    const resp = await authFetch(`/api/tax/context?year=${selectedYear}`);
    const data = await resp.json();
    document.getElementById('tax-context-text').textContent = data.context || '(empty — no tax data to inject)';
  } catch(e) {
    document.getElementById('tax-context-text').textContent = 'Failed to load: ' + e.message;
  }
}

// ── Investments tab extensions ──
function showAddLotForm() { document.getElementById('add-lot-form').classList.toggle('hidden'); }
function showAddK1Form() { document.getElementById('add-k1-form').classList.toggle('hidden'); }

async function loadLots() {
  try {
    const status = document.getElementById('lots-status').value || 'open';
    const resp = await authFetch(`/api/tax/lots?status=${status}`);
    const data = await resp.json();
    const lots = data.lots || [];
    const list = document.getElementById('lots-list');
    if (lots.length === 0) {
      list.innerHTML = '<p class="text-xs text-gray-600">No tax lots. Auto-imported from brokerage syncs or manually added.</p>';
      return;
    }
    list.innerHTML = lots.map(l => `<div class="flex items-center justify-between p-2 bg-gray-900/40 rounded-lg">
      <div>
        <p class="text-sm font-medium text-gray-200">${escapeHtml(l.symbol)} <span class="text-xs text-gray-500">${escapeHtml(l.asset_type)}</span></p>
        <p class="text-xs text-gray-500">${l.quantity} @ ${escapeHtml(l.cost_per_unit)} · acquired ${escapeHtml(l.acquisition_date)} · ${escapeHtml(l.broker || 'manual')} · ${escapeHtml(l.method)}</p>
      </div>
      <div class="text-right">
        <p class="text-sm text-gray-300">${escapeHtml(l.total_basis)}</p>
        ${l.wash_sale_adj && l.wash_sale_adj !== '$0.00' ? `<p class="text-xs text-yellow-400">wash: ${escapeHtml(l.wash_sale_adj)}</p>` : ''}
        ${status === 'open' ? `<button onclick="sellLot(${l.id},'${escapeHtml(l.symbol)}',${l.quantity})" class="text-xs text-oc-400 hover:text-oc-300">sell →</button>` : ''}
      </div>
    </div>`).join('');
  } catch(e) { document.getElementById('lots-list').innerHTML = '<p class="text-xs text-red-400">Failed: ' + escapeHtml(e.message) + '</p>'; }
}

async function saveLot() {
  const body = {

    symbol: document.getElementById('lot-symbol').value.toUpperCase(),
    asset_type: document.getElementById('lot-asset-type').value,
    quantity: parseFloat(document.getElementById('lot-qty').value),
    cost_per_unit_cents: Math.round(parseFloat(document.getElementById('lot-cpu').value || '0') * 100),
    acquisition_date: document.getElementById('lot-date').value,
    broker: document.getElementById('lot-broker').value || null,
  };
  if (!body.symbol || !body.quantity || !body.cost_per_unit_cents || !body.acquisition_date) {
    alert('Symbol, quantity, cost per unit, and date are required.');
    return;
  }
  const resp = await authFetch('/api/tax/lots', { method: 'POST', headers: {'Content-Type':'application/json'}, body: JSON.stringify(body) });
  const data = await resp.json();
  if (data.ok !== false) {
    ['lot-symbol','lot-qty','lot-cpu','lot-date','lot-broker'].forEach(id => document.getElementById(id).value = '');
    document.getElementById('add-lot-form').classList.add('hidden');
    loadLots();
  } else { alert('Save failed'); }
}

async function sellLot(lotId, symbol, maxQty) {
  const qtyStr = prompt(`Sell ${symbol} — quantity (max ${maxQty}):`, maxQty);
  if (!qtyStr) return;
  const priceStr = prompt('Sell price per unit ($):');
  if (!priceStr) return;
  const dateStr = prompt('Sell date (YYYY-MM-DD):', new Date().toISOString().slice(0,10));
  if (!dateStr) return;
  const body = {
    token, lot_id: lotId,
    quantity: parseFloat(qtyStr),
    sell_price_cents: Math.round(parseFloat(priceStr) * 100),
    sell_date: dateStr,
  };
  const resp = await authFetch('/api/tax/lots/sell', { method: 'POST', headers: {'Content-Type':'application/json'}, body: JSON.stringify(body) });
  const data = await resp.json();
  if (data.ok !== false) {
    loadLots();
    loadCapitalGains();
    loadForm8949();
    loadWashSales();
  } else { alert('Sell failed: ' + (data.error || 'unknown')); }
}

async function loadWashSales() {
  try {
    const resp = await authFetch(`/api/tax/wash-sales?year=${selectedYear}`);
    const data = await resp.json();
    const matches = data.wash_sales || [];
    document.getElementById('wash-count').textContent = matches.length + ' match' + (matches.length===1?'':'es');
    const list = document.getElementById('wash-list');
    if (matches.length === 0) {
      list.innerHTML = '<p class="text-xs text-gray-600">No wash sales detected.</p>';
      return;
    }
    list.innerHTML = matches.map(m => `<div class="p-2 bg-yellow-500/10 border border-yellow-600/30 rounded-lg text-xs">
      <p class="text-gray-200 font-medium">${escapeHtml(m.symbol || m.asset || '')}</p>
      <p class="text-gray-400">Loss disallowed: ${escapeHtml(m.disallowed_loss || m.adjustment || '')} · sold ${escapeHtml(m.sell_date || '')} · repurchased ${escapeHtml(m.repurchase_date || '')}</p>
    </div>`).join('');
  } catch(e) { document.getElementById('wash-list').innerHTML = '<p class="text-xs text-red-400">Failed</p>'; }
}

async function loadForm8949() {
  try {
    const resp = await authFetch(`/api/tax/form-8949?year=${selectedYear}`);
    const data = await resp.json();
    const shortRows = data.short_term || [];
    const longRows = data.long_term || [];
    const total = shortRows.length + longRows.length;
    document.getElementById('form8949-count').textContent = total + ' disposition' + (total===1?'':'s');
    const list = document.getElementById('form8949-list');
    if (total === 0) {
      list.innerHTML = '<p class="text-xs text-gray-600">No dispositions this year. Sell a lot to populate Form 8949.</p>';
      return;
    }
    const row = r => `<tr class="border-b border-gray-800/50">
      <td class="py-1 text-gray-300">${escapeHtml(r.description || r.symbol || '')}</td>
      <td class="py-1 text-right text-gray-400">${escapeHtml(r.acquisition_date || r.acquired || '')}</td>
      <td class="py-1 text-right text-gray-400">${escapeHtml(r.sell_date || r.sold || '')}</td>
      <td class="py-1 text-right text-gray-300">${escapeHtml(r.proceeds || '')}</td>
      <td class="py-1 text-right text-gray-300">${escapeHtml(r.basis || '')}</td>
      <td class="py-1 text-right ${(r.gain_loss || '').startsWith('-') ? 'text-red-400' : 'text-green-400'}">${escapeHtml(r.gain_loss || '')}</td>
    </tr>`;
    const section = (label, rows, total) => rows.length === 0 ? '' :
      `<div class="mb-3"><p class="text-xs font-medium text-gray-400 uppercase mt-2 mb-1">${label} <span class="text-gray-500 normal-case">— total ${escapeHtml(total || '')}</span></p>
       <table class="w-full text-xs"><thead><tr class="text-gray-500 border-b border-gray-700"><th class="py-1 text-left">Description</th><th class="py-1 text-right">Acquired</th><th class="py-1 text-right">Sold</th><th class="py-1 text-right">Proceeds</th><th class="py-1 text-right">Basis</th><th class="py-1 text-right">Gain/Loss</th></tr></thead><tbody>${rows.map(row).join('')}</tbody></table></div>`;
    const net = data.net_gain_loss || '';
    list.innerHTML = section('Short-Term (held ≤ 1 year)', shortRows, data.short_term_total) + section('Long-Term (held > 1 year)', longRows, data.long_term_total) +
      (net ? `<p class="text-xs text-gray-300 mt-2 pt-2 border-t border-gray-700">Net gain/loss: <span class="${net.startsWith('-') ? 'text-red-400' : 'text-green-400'} font-medium">${escapeHtml(net)}</span></p>` : '');
  } catch(e) { document.getElementById('form8949-list').innerHTML = '<p class="text-xs text-red-400">Failed</p>'; }
}

async function loadK1s() {
  try {
    const resp = await authFetch(`/api/tax/k1?year=${selectedYear}`);
    const data = await resp.json();
    const k1s = data.k1s || [];
    const list = document.getElementById('k1-list');
    if (k1s.length === 0) {
      list.innerHTML = '<p class="text-xs text-gray-600">No K-1 income. Add partnership or S-Corp distributions you received.</p>';
      return;
    }
    list.innerHTML = k1s.map(k => `<div class="p-2 bg-gray-900/40 rounded-lg">
      <div class="flex items-center justify-between">
        <span class="text-sm font-medium text-gray-200">${escapeHtml(k.entity)} <span class="text-xs text-gray-500">${escapeHtml(k.type)}</span></span>
        <span class="text-sm text-gray-300">ordinary ${escapeHtml(k.ordinary)}</span>
      </div>
      <p class="text-xs text-gray-500">rental ${escapeHtml(k.rental)} · interest ${escapeHtml(k.interest)} · dividends ${escapeHtml(k.dividends)} · capgain ${escapeHtml(k.capital_gains)} · SE ${escapeHtml(k.se_income)}</p>
    </div>`).join('');
  } catch(e) { document.getElementById('k1-list').innerHTML = '<p class="text-xs text-red-400">Failed</p>'; }
}

async function saveK1() {
  const body = {
    token, tax_year: selectedYear,
    entity_name: document.getElementById('k1-name').value,
    entity_type: document.getElementById('k1-type').value,
    ordinary_cents: parseCurrencyInput('k1-ordinary'),
    rental_cents: parseCurrencyInput('k1-rental'),
    interest_cents: parseCurrencyInput('k1-interest'),
    dividend_cents: parseCurrencyInput('k1-dividend'),
    capital_gain_cents: parseCurrencyInput('k1-capgain'),
    se_income_cents: parseCurrencyInput('k1-se'),
  };
  if (!body.entity_name) { alert('Entity name required.'); return; }
  const resp = await authFetch('/api/tax/k1', { method: 'POST', headers: {'Content-Type':'application/json'}, body: JSON.stringify(body) });
  const data = await resp.json();
  if (data.ok) {
    ['k1-name','k1-ordinary','k1-rental','k1-interest','k1-dividend','k1-capgain','k1-se'].forEach(id => document.getElementById(id).value = '');
    document.getElementById('add-k1-form').classList.add('hidden');
    loadK1s();
  } else { alert('Save failed'); }
}

async function loadCapitalGains() {
  try {
    const resp = await authFetch(`/api/tax/capital-gains/summary?year=${selectedYear}`);
    const data = await resp.json();
    const grid = document.getElementById('cap-gains-grid');
    grid.innerHTML = `
      <div><p class="text-xs text-gray-500 uppercase">Short-Term</p><p class="text-lg font-semibold mt-1 text-gray-200">${escapeHtml(data.short_term_gains || '$0.00')}</p><p class="text-xs text-red-400">losses ${escapeHtml(data.short_term_losses || '$0.00')}</p></div>
      <div><p class="text-xs text-gray-500 uppercase">Long-Term</p><p class="text-lg font-semibold mt-1 text-oc-400">${escapeHtml(data.long_term_gains || '$0.00')}</p><p class="text-xs text-red-400">losses ${escapeHtml(data.long_term_losses || '$0.00')}</p></div>
      <div><p class="text-xs text-gray-500 uppercase">Net Gain/Loss</p><p class="text-lg font-semibold mt-1 ${(data.net_gain_loss||'').startsWith('-') ? 'text-red-400' : 'text-green-400'}">${escapeHtml(data.net_gain_loss || '$0.00')}</p></div>
      <div><p class="text-xs text-gray-500 uppercase">Usable / Carryforward</p><p class="text-lg font-semibold mt-1 text-gray-200">${escapeHtml(data.usable_loss || '$0.00')}</p><p class="text-xs text-gray-500">carry ${escapeHtml(data.carryforward_loss || '$0.00')}</p></div>
    `;
    const note = document.getElementById('cap-gains-note');
    const ws = data.wash_sale_count || 0;
    note.textContent = ws > 0 ? `${ws} wash sale match${ws===1?'':'es'} detected. Capital loss limit: $3,000/year; excess carries forward.` : 'Capital loss limit: $3,000/year; excess carries forward.';
  } catch(e) { document.getElementById('cap-gains-grid').innerHTML = '<p class="text-xs text-red-400 col-span-full">Failed</p>'; }
}

// Keyboard shortcuts for review mode
document.addEventListener('keydown', e => {
  if (!dedReviewMode || e.target.tagName === 'INPUT' || e.target.tagName === 'TEXTAREA' || e.target.tagName === 'SELECT') return;
  if (e.key === 'j' && dedSelectedIdx < dedCandidates.length - 1) selectCandidate(dedSelectedIdx + 1);
  else if (e.key === 'k' && dedSelectedIdx > 0) selectCandidate(dedSelectedIdx - 1);
  else if (e.key === 'a') reviewAction('approve');
  else if (e.key === 'd') reviewAction('deny');
});

// Init
loadProfile();
loadOverview();         // populates summary data used by the KPI strip + dashboard
loadCategories();
loadTaxConversation();
loadKpiStrip();         // fills the always-visible KPI tiles
updateDeadlinePill();   // shows next IRS deadline if within 60 days
showSection('investments'); // default landing — Sean's most-frequent use
setInterval(loadKpiStrip, 90000); // refresh portfolio every 90s
initPositronicBrain();  // neural-net canvas animation + LCARS log counter
// positronMsgCount declared at top of script alongside
// other module-level lets; do not redeclare here.
updatePositronLog();    // initial log reading

// ═══ POSITRONIC BRAIN — animated neural network behind the chat panel ═══
function initPositronicBrain() {
  const canvas = document.getElementById('positron-brain');
  if (!canvas) return;
  const ctx = canvas.getContext('2d', { alpha: true });
  let w = 0, h = 0;
  const NODE_DENSITY = 4200;  // one neuron per ~4200 sq px of panel area
  const nodes = [];   // {x,y,r,phase}
  const edges = [];   // {a,b,len}
  const pulses = [];  // {edge, t, dir}

  function resize() {
    // Use the panel parent's rect — canvas has intrinsic 300x150 that
    // getBoundingClientRect returns if CSS inset:0 hasn't kicked in yet.
    const parent = canvas.parentElement || canvas;
    const rect = parent.getBoundingClientRect();
    if (rect.width < 20 || rect.height < 20) return; // skip while hidden/laying out
    const dpr = window.devicePixelRatio || 1;
    w = canvas.width = Math.floor(rect.width * dpr);
    h = canvas.height = Math.floor(rect.height * dpr);
    canvas.style.width = rect.width + 'px';
    canvas.style.height = rect.height + 'px';
    ctx.setTransform(dpr, 0, 0, dpr, 0, 0);
    buildGraph(rect.width, rect.height);
  }

  function buildGraph(vw, vh) {
    nodes.length = 0; edges.length = 0; pulses.length = 0;
    const count = Math.max(24, Math.min(140, Math.round((vw * vh) / NODE_DENSITY)));
    for (let i = 0; i < count; i++) {
      nodes.push({
        x: Math.random() * vw,
        y: Math.random() * vh,
        r: 1.4 + Math.random() * 1.8,
        phase: Math.random() * Math.PI * 2,
      });
    }
    // Connect each node to its 2 nearest neighbors
    for (let i = 0; i < nodes.length; i++) {
      const dists = nodes
        .map((n, j) => ({ j, d: j === i ? Infinity : Math.hypot(nodes[i].x - n.x, nodes[i].y - n.y) }))
        .sort((a, b) => a.d - b.d);
      for (let k = 0; k < 2; k++) {
        const j = dists[k].j;
        // Avoid duplicate edges
        if (!edges.find(e => (e.a === i && e.b === j) || (e.a === j && e.b === i))) {
          edges.push({ a: i, b: j, len: dists[k].d });
        }
      }
    }
  }

  window.addEventListener('resize', resize);
  // Size the canvas now + after layout settles + via ResizeObserver on the
  // panel so the graph always matches the actual rendered size.
  setTimeout(resize, 50);
  setTimeout(resize, 400);
  setTimeout(resize, 1200);
  const parent = canvas.parentElement;
  if (parent && window.ResizeObserver) {
    new ResizeObserver(resize).observe(parent);
  }

  let t0 = performance.now();
  function tick(now) {
    const dt = (now - t0) / 1000; t0 = now;
    if (!w || !h) { requestAnimationFrame(tick); return; }
    const rect = canvas.getBoundingClientRect();
    ctx.clearRect(0, 0, rect.width, rect.height);

    // Draw fiber-optic edges
    for (const e of edges) {
      const a = nodes[e.a], b = nodes[e.b];
      const grad = ctx.createLinearGradient(a.x, a.y, b.x, b.y);
      grad.addColorStop(0, 'rgba(255,156,61,0.12)');
      grad.addColorStop(0.5, 'rgba(255,184,137,0.22)');
      grad.addColorStop(1, 'rgba(255,156,61,0.12)');
      ctx.strokeStyle = grad;
      ctx.lineWidth = 0.6;
      ctx.beginPath(); ctx.moveTo(a.x, a.y); ctx.lineTo(b.x, b.y); ctx.stroke();
    }

    // Spawn new pulses occasionally — scales with node density
    if (Math.random() < 0.08 && pulses.length < 14) {
      const e = edges[Math.floor(Math.random() * edges.length)];
      pulses.push({ edge: e, t: 0, dir: Math.random() < 0.5 ? 1 : -1 });
    }

    // Advance and draw pulses
    for (let i = pulses.length - 1; i >= 0; i--) {
      const p = pulses[i];
      p.t += dt * 0.6;
      if (p.t > 1) { pulses.splice(i, 1); continue; }
      const a = nodes[p.edge.a], b = nodes[p.edge.b];
      const tt = p.dir === 1 ? p.t : 1 - p.t;
      const px = a.x + (b.x - a.x) * tt;
      const py = a.y + (b.y - a.y) * tt;
      // Bright pulse head
      ctx.fillStyle = 'rgba(255,220,170,0.95)';
      ctx.shadowColor = 'rgba(255,184,137,0.9)';
      ctx.shadowBlur = 8;
      ctx.beginPath(); ctx.arc(px, py, 2.1, 0, Math.PI * 2); ctx.fill();
      ctx.shadowBlur = 0;
    }

    // Draw nodes (neurons) with breathing glow
    for (const n of nodes) {
      n.phase += dt * 1.1;
      const glow = 0.55 + 0.45 * Math.sin(n.phase);
      ctx.fillStyle = `rgba(255,184,137,${0.45 + glow * 0.4})`;
      ctx.shadowColor = `rgba(255,156,61,${glow * 0.7})`;
      ctx.shadowBlur = 6 + glow * 6;
      ctx.beginPath(); ctx.arc(n.x, n.y, n.r + glow * 0.3, 0, Math.PI * 2); ctx.fill();
      ctx.shadowBlur = 0;
    }

    requestAnimationFrame(tick);
  }
  requestAnimationFrame(tick);
}

// Session log counter — declaration hoisted above the first top-level
// updatePositronLog() call so the function body can read positronMsgCount
// without throwing ReferenceError: Cannot access 'positronMsgCount' before
// initialization. See the early `let positronMsgCount = 0;` above.
function updatePositronLog() {
  const idx = document.getElementById('positron-log-index');
  if (!idx) return;
  const base = 48000;  // aesthetic starting offset
  const now = Date.now() / 1000;
  const sessionLog = (base + (now % 100000) / 10).toFixed(1);
  idx.textContent = sessionLog;
  const cnt = document.getElementById('positron-msg-count');
  if (cnt) cnt.textContent = positronMsgCount.toString().padStart(3, '0');
}
setInterval(updatePositronLog, 3000);
"##;
