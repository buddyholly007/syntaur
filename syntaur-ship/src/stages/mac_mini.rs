//! Mac Mini smoke stage — rsync binary, launch, retry /health, run the
//! four-check auth smoke (HSTS absent, login 2xx/4xx, smart-home page,
//! smart-home API). Abort deploy on any failure.
//!
//! Phase 1 ports deploy.sh lines 96-176, incorporating the exit-code
//! fix for the retry loop I made today (explicit `[ $? -eq 0 ]` instead
//! of the pipe-to-head trick which masked curl failures).

use anyhow::{Context, Result};
use std::process::Command;

use crate::pipeline::StageContext;

pub fn run(ctx: &StageContext) -> Result<()> {
    let mac = &ctx.cfg.mac_mini;
    let ws = &ctx.cfg.workspace;

    log::info!(">> rsync gateway → {mac}");
    if !ctx.opts.dry_run {
        let status = Command::new("rsync")
            .args([
                "-az",
                ws.join("target/release/syntaur-gateway").to_str().unwrap(),
                &format!("{mac}:/tmp/syntaur-gateway.new"),
            ])
            .status()
            .context("rsync gateway")?;
        if !status.success() {
            anyhow::bail!("rsync to Mac Mini exited {status}");
        }
    }

    log::info!(">> Mac Mini: swap binary + launch");
    if !ctx.opts.dry_run {
        ssh_run(
            mac,
            "pkill -x syntaur-gateway 2>/dev/null; sleep 1; \
             mv -f /tmp/syntaur-gateway.new /tmp/syntaur-gateway && \
             chmod +x /tmp/syntaur-gateway",
        )?;
        ssh_run_background(
            mac,
            "cd /tmp && setsid nohup ./syntaur-gateway > /tmp/syntaur-gateway.log 2>&1 < /dev/null & exit 0",
        )?;
    }

    log::info!(">> Mac Mini: /health retry up to 40s");
    if !ctx.opts.dry_run {
        let health_script = r#"
            for i in $(seq 1 20); do
                body=$(curl -sf --max-time 3 http://127.0.0.1:18789/health)
                if [ $? -eq 0 ] && [ -n "$body" ]; then
                    printf "%s" "$body" | head -c 120; echo
                    exit 0
                fi
                sleep 2
            done
            echo "/health unreachable after 40s" >&2
            exit 1
        "#;
        ssh_run(mac, health_script)?;
    }

    log::info!(">> Mac Mini: auth + HSTS + smart-home smoke");
    if !ctx.opts.dry_run {
        let smoke_script = r#"
            set -e
            resp_headers=$(curl -sS -I --max-time 5 http://127.0.0.1:18789/)
            if printf "%s" "$resp_headers" | grep -qi "^strict-transport-security:"; then
                echo "FAIL_HSTS_ON_HTTP"
                printf "%s" "$resp_headers" | grep -i "strict-transport"
                exit 11
            fi
            login_code=$(curl -sS -o /dev/null -w "%{http_code}" --max-time 5 \
                -X POST http://127.0.0.1:18789/api/auth/login \
                -H "Content-Type: application/json" \
                -H "Origin: http://127.0.0.1:18789" \
                -d "{\"password\":\"__syntaur_ship_smoke_wrong_password__\"}")
            if [[ "$login_code" =~ ^5 ]] || [[ -z "$login_code" ]]; then
                echo "FAIL_LOGIN_UNREACHABLE:$login_code"
                exit 12
            fi
            sh_page=$(curl -sS -o /dev/null -w "%{http_code}" --max-time 5 http://127.0.0.1:18789/smart-home)
            if [[ "$sh_page" =~ ^5 ]] || [[ -z "$sh_page" ]]; then
                echo "FAIL_SMART_HOME_PAGE:$sh_page"; exit 13
            fi
            sh_api=$(curl -sS -o /dev/null -w "%{http_code}" --max-time 5 http://127.0.0.1:18789/api/smart-home/rooms)
            if [[ "$sh_api" =~ ^5 ]] || [[ -z "$sh_api" ]]; then
                echo "FAIL_SMART_HOME_API:$sh_api"; exit 14
            fi
            echo "OK hsts=absent login_code=$login_code sh_page=$sh_page sh_api=$sh_api"
        "#;
        let output = ssh_capture(mac, smoke_script).context("Mac Mini smoke")?;
        println!("   {output}");
        if !output.contains("OK hsts=absent") {
            anyhow::bail!("Mac Mini auth smoke failed — prod not touched");
        }
    }

    Ok(())
}

fn ssh_run(target: &str, script: &str) -> Result<()> {
    let status = Command::new("ssh")
        .args(["-n", target, script])
        .status()
        .context("ssh")?;
    if !status.success() {
        anyhow::bail!("ssh {target} '{}' exited {status}", first_line(script));
    }
    Ok(())
}

fn ssh_run_background(target: &str, script: &str) -> Result<()> {
    let status = Command::new("ssh")
        .args(["-f", "-n", target, script])
        .status()
        .context("ssh -f")?;
    if !status.success() {
        anyhow::bail!("ssh -f {target} exited {status}");
    }
    Ok(())
}

fn ssh_capture(target: &str, script: &str) -> Result<String> {
    let output = Command::new("ssh")
        .args([target, script])
        .output()
        .context("ssh capture")?;
    if !output.status.success() {
        anyhow::bail!(
            "ssh {target} exited {} — stderr: {}",
            output.status,
            String::from_utf8_lossy(&output.stderr)
        );
    }
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
}

fn first_line(s: &str) -> &str {
    s.lines().next().unwrap_or("").trim()
}
