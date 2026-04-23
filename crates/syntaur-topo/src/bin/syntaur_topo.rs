//! `syntaur-topo` — LAN routing + service catalog CLI.
//!
//! Flows:
//!   syntaur-topo path <to>              # auto from current host
//!   syntaur-topo path <from> <to>       # explicit
//!   syntaur-topo host <name>            # show host facts
//!   syntaur-topo service <name>         # show service + probe command
//!   syntaur-topo list                   # everything
//!   syntaur-topo validate               # lint manifest

use std::path::PathBuf;
use std::process::ExitCode;

use anyhow::{anyhow, bail, Context, Result};
use clap::{Parser, Subcommand};

use syntaur_topo_core::{
    current_hostname, default_manifest_path,
    manifest::{Manifest, ReachKind},
    resolve, resolve_manifest_key,
};

#[derive(Parser, Debug)]
#[command(
    name = "syntaur-topo",
    version,
    about = "LAN topology + routing rules — single source of truth for host/jump/auth info",
    long_about = None
)]
struct Cli {
    /// Override manifest path (otherwise $SYNTAUR_TOPO_PATH or ~/vault/syntaur-topology.yaml).
    #[arg(long, global = true)]
    manifest: Option<PathBuf>,

    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand, Debug)]
enum Commands {
    /// Print the exact ssh/curl command to reach a host from another.
    Path {
        /// Destination host short-name OR `service:<name>`.
        target: String,
        /// Source host short-name. Defaults to the current machine's hostname.
        #[arg(long)]
        from: Option<String>,
        /// Print only the ssh args fragment (`user@host` or `-J jump user@host`),
        /// without the leading `ssh`. Use in scripts: `rsync -e "ssh $(topo path X --as-ssh-args)" ...`.
        #[arg(long)]
        as_ssh_args: bool,
        /// Print the explanation line after the command (why this route).
        #[arg(long)]
        explain: bool,
    },
    /// Show facts about a host.
    Host {
        name: String,
    },
    /// Show facts about a service + its probe command from here.
    Service {
        name: String,
        #[arg(long)]
        from: Option<String>,
    },
    /// Dump the whole manifest (hosts + services).
    List {
        /// Filter by role (e.g. `prod`, `always-on`, `jump-host`).
        #[arg(long)]
        role: Option<String>,
        /// JSON output (default: human tables).
        #[arg(long)]
        json: bool,
    },
    /// Lint the manifest for broken references + missing fields.
    Validate,
    /// Print the current hostname as this tool understands it — useful
    /// when debugging "why did topo pick that from?".
    Whoami,
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    let manifest_path = cli.manifest.unwrap_or_else(default_manifest_path);

    let result = match cli.command {
        Commands::Whoami => {
            println!("{}", current_hostname());
            Ok(())
        }
        Commands::Path {
            target,
            from,
            as_ssh_args,
            explain,
        } => cmd_path(&manifest_path, &target, from.as_deref(), as_ssh_args, explain),
        Commands::Host { name } => cmd_host(&manifest_path, &name),
        Commands::Service { name, from } => cmd_service(&manifest_path, &name, from.as_deref()),
        Commands::List { role, json } => cmd_list(&manifest_path, role.as_deref(), json),
        Commands::Validate => cmd_validate(&manifest_path),
    };

    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("syntaur-topo: {e:#}");
            ExitCode::from(1)
        }
    }
}

fn load(path: &std::path::Path) -> Result<Manifest> {
    if !path.exists() {
        bail!(
            "no topology manifest at {} — create one at ~/vault/syntaur-topology.yaml (see the crate README for schema)",
            path.display()
        );
    }
    Manifest::load(path)
}

fn cmd_path(
    manifest_path: &std::path::Path,
    target: &str,
    from_arg: Option<&str>,
    as_ssh_args: bool,
    explain: bool,
) -> Result<()> {
    let m = load(manifest_path)?;
    let raw_from = from_arg.map(String::from).unwrap_or_else(current_hostname);
    let from = resolve_manifest_key(&m, &raw_from);

    // Verify `from` is known — else the routing rules can't apply.
    if !m.hosts.contains_key(&from) {
        bail!(
            "host `{raw_from}` not in manifest — known hosts: {}. Pass --from <name> to override, or add `{raw_from}` to the matching host's `aliases` list.",
            m.hosts.keys().cloned().collect::<Vec<_>>().join(", ")
        );
    }

    // Allow `service:<name>` shorthand.
    let spec = if let Some(svc) = target.strip_prefix("service:") {
        resolve::service_path(&m, &from, svc).map_err(|e| anyhow!("{e}"))?
    } else {
        resolve::ssh_path(&m, &from, target).map_err(|e| anyhow!("{e}"))?
    };

    if as_ssh_args {
        if let Some(a) = &spec.ssh_args {
            print!("{a}");
            if std::io::IsTerminal::is_terminal(&std::io::stdout()) {
                println!();
            }
        } else {
            bail!("path is not an ssh command; --as-ssh-args doesn't apply");
        }
    } else {
        println!("{}", spec.command);
    }
    if explain {
        eprintln!("# {}", spec.explanation);
    }
    Ok(())
}

fn cmd_host(manifest_path: &std::path::Path, name: &str) -> Result<()> {
    let m = load(manifest_path)?;
    let h = m
        .hosts
        .get(name)
        .ok_or_else(|| anyhow!("unknown host `{name}` — try `syntaur-topo list`"))?;
    println!("{} ({})", h.display_name, name);
    println!("  status:        {:?}", h.status);
    if !h.os.is_empty() {
        println!("  os:            {}", h.os);
    }
    println!("  address:       {}", h.address);
    for (k, v) in &h.alt_addresses {
        println!("  alt.{k}: {v}");
    }
    if !h.roles.is_empty() {
        let roles: Vec<String> = h.roles.iter().map(|r| format!("{r:?}")).collect();
        println!("  roles:         {}", roles.join(", "));
    }
    if let Some(ssh) = &h.ssh {
        println!("  ssh:           {}@{} ({})", ssh.user, h.address, ssh.auth);
        if let Some(fb) = &ssh.fallback_password_vault_key {
            println!("  ssh fallback:  `syntaur-vault get {fb}`");
        }
    }
    if !h.reachable_from.is_empty() {
        println!("  reachable_from:");
        for (from, r) in &h.reachable_from {
            let via = match r.kind {
                ReachKind::Direct => "direct".to_string(),
                ReachKind::Via => {
                    format!("via {}", r.jump.as_deref().unwrap_or("?"))
                }
                ReachKind::Forbidden => "FORBIDDEN".to_string(),
            };
            let note = if r.note.is_empty() {
                String::new()
            } else {
                format!("  — {}", r.note)
            };
            println!("    {from:12} → {via}{note}");
        }
    }
    if !h.notes.is_empty() {
        println!("  notes:         {}", h.notes);
    }
    Ok(())
}

fn cmd_service(
    manifest_path: &std::path::Path,
    name: &str,
    from_arg: Option<&str>,
) -> Result<()> {
    let m = load(manifest_path)?;
    let svc = m
        .services
        .get(name)
        .ok_or_else(|| anyhow!("unknown service `{name}` — try `syntaur-topo list`"))?;
    let raw_from = from_arg.map(String::from).unwrap_or_else(current_hostname);
    let from = resolve_manifest_key(&m, &raw_from);

    println!("{name}");
    println!("  host:          {}", svc.host);
    println!(
        "  endpoint:      {}:{}",
        m.hosts
            .get(&svc.host)
            .map(|h| h.address.as_str())
            .unwrap_or("?"),
        svc.port
    );
    println!("  protocol:      {:?}", svc.protocol);
    if !svc.path.is_empty() {
        println!("  path:          {}", svc.path);
    }
    if !svc.description.is_empty() {
        println!("  description:   {}", svc.description);
    }

    // Best-effort probe command from `from`.
    match resolve::service_path(&m, &from, name) {
        Ok(spec) => {
            println!("  from {from}:");
            println!("    {}", spec.command);
            println!("    # {}", spec.explanation);
        }
        Err(e) => {
            println!("  from {from}:  (no route — {e})");
        }
    }
    Ok(())
}

fn cmd_list(
    manifest_path: &std::path::Path,
    role_filter: Option<&str>,
    json: bool,
) -> Result<()> {
    let m = load(manifest_path)?;

    if json {
        println!("{}", serde_json::to_string_pretty(&m)?);
        return Ok(());
    }

    let role_match = |h: &syntaur_topo_core::Host| match role_filter {
        None => true,
        Some(f) => h.roles.iter().any(|r| {
            let normalized = format!("{r:?}").to_lowercase().replace('_', "-");
            let f_norm = f.to_lowercase().replace('_', "-");
            normalized.contains(&f_norm)
        }),
    };

    println!("HOSTS");
    println!("{:<14}  {:<16}  {:<14}  ROLES", "NAME", "ADDRESS", "STATUS");
    for (name, h) in &m.hosts {
        if !role_match(h) {
            continue;
        }
        let roles: Vec<String> = h.roles.iter().map(|r| format!("{r:?}")).collect();
        println!(
            "{:<14}  {:<16}  {:<14}  {}",
            name,
            h.address,
            format!("{:?}", h.status),
            roles.join(",")
        );
    }

    if !m.services.is_empty() {
        println!();
        println!("SERVICES");
        println!(
            "{:<28}  {:<12}  {:<7}  {:<8}  DESCRIPTION",
            "NAME", "HOST", "PORT", "PROTO"
        );
        for (name, svc) in &m.services {
            println!(
                "{:<28}  {:<12}  {:<7}  {:<8}  {}",
                name,
                svc.host,
                svc.port,
                format!("{:?}", svc.protocol),
                svc.description
            );
        }
    }
    Ok(())
}

fn cmd_validate(manifest_path: &std::path::Path) -> Result<()> {
    let m = load(manifest_path).context("loading manifest")?;
    let problems = resolve::validate(&m);
    if problems.is_empty() {
        println!("✓ manifest clean: {} hosts, {} services", m.hosts.len(), m.services.len());
        return Ok(());
    }
    for p in &problems {
        println!("  ⚠ {p}");
    }
    bail!("{} problem(s) in manifest", problems.len())
}

