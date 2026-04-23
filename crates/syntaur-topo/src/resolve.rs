//! Routing resolution: given `from` + `to` + manifest, produce the
//! exact command a user/Claude should run.
//!
//! Philosophy: be pushy about the correct answer. If the manifest
//! knows claudevm → truenas must go via ha-minipc, the tool emits
//! `ssh -J root@192.168.1.3 truenas_admin@192.168.1.239` — not a
//! hint, the actual command. Claude pastes that into Bash and the
//! routing mistake is impossible.

use std::fmt;

use thiserror::Error;

use crate::manifest::{Manifest, Protocol, ReachKind};

/// Rendered recipe to reach a host or service from a given source.
pub struct PathSpec {
    /// Full shell command to run, e.g. `ssh -J root@192.168.1.3 truenas_admin@192.168.1.239`.
    pub command: String,
    /// If this is an SSH path, the raw `-J jump user@host` fragment
    /// (without the leading `ssh `) so scripts can compose:
    /// `rsync -e "ssh $(topo path truenas --as-ssh-args)" ...`.
    pub ssh_args: Option<String>,
    /// Human explanation of the routing choice.
    pub explanation: String,
}

impl fmt::Display for PathSpec {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.command)
    }
}

#[derive(Debug, Error)]
pub enum ReachabilityError {
    #[error("host `{0}` not in manifest")]
    UnknownHost(String),
    #[error("service `{0}` not in manifest")]
    UnknownService(String),
    #[error("host `{host}` has no SSH config — can't produce an ssh command")]
    NoSsh { host: String },
    #[error("no reachability rule from `{from}` to `{to}` — add one to the manifest under hosts.{to}.reachable_from")]
    NoRoute { from: String, to: String },
    #[error("route from `{from}` to `{to}` is policy-forbidden: {note}")]
    Forbidden { from: String, to: String, note: String },
    #[error("jump host `{0}` referenced but not in manifest")]
    UnknownJump(String),
    #[error("jump host `{0}` has no SSH config — can't use it as a ProxyJump")]
    JumpNoSsh(String),
}

/// Pick the Reach rule for `to` as seen from `from`. Falls back to
/// the `"*"` wildcard if no specific entry.
fn reach_for<'a>(m: &'a Manifest, from: &str, to: &str) -> Option<&'a crate::manifest::Reach> {
    let target = m.hosts.get(to)?;
    target
        .reachable_from
        .get(from)
        .or_else(|| target.reachable_from.get("*"))
}

/// Produce an SSH command to reach `to_host` from `from_host`. Same
/// host → just run locally, no SSH needed.
pub fn ssh_path(
    m: &Manifest,
    from: &str,
    to: &str,
) -> Result<PathSpec, ReachabilityError> {
    if from == to {
        return Ok(PathSpec {
            command: "# already on this host — run commands locally".into(),
            ssh_args: None,
            explanation: format!("{to} is the current host"),
        });
    }

    let target = m
        .hosts
        .get(to)
        .ok_or_else(|| ReachabilityError::UnknownHost(to.into()))?;
    let reach = reach_for(m, from, to)
        .ok_or_else(|| ReachabilityError::NoRoute {
            from: from.into(),
            to: to.into(),
        })?;

    // Check policy first — a forbidden/decommissioned host gives a
    // clearer error than "no SSH config" when both are true.
    if reach.kind == ReachKind::Forbidden {
        return Err(ReachabilityError::Forbidden {
            from: from.into(),
            to: to.into(),
            note: reach.note.clone(),
        });
    }

    let target_ssh = target
        .ssh
        .as_ref()
        .ok_or_else(|| ReachabilityError::NoSsh { host: to.into() })?;

    match reach.kind {
        ReachKind::Forbidden => unreachable!("handled above"),
        ReachKind::Direct => {
            let target_addr = &target.address;
            let user = &target_ssh.user;
            let ssh_args = format!("{user}@{target_addr}");
            Ok(PathSpec {
                command: format!("ssh {ssh_args}"),
                ssh_args: Some(ssh_args),
                explanation: format!("direct LAN route from {from} to {to}"),
            })
        }
        ReachKind::Via => {
            let jump_name = reach
                .jump
                .as_ref()
                .ok_or_else(|| ReachabilityError::UnknownJump("<unset>".into()))?;
            let jump = m
                .hosts
                .get(jump_name)
                .ok_or_else(|| ReachabilityError::UnknownJump(jump_name.clone()))?;
            let jump_ssh = jump
                .ssh
                .as_ref()
                .ok_or_else(|| ReachabilityError::JumpNoSsh(jump_name.clone()))?;
            let jump_str = format!("{}@{}", jump_ssh.user, jump.address);
            let dest_str = format!("{}@{}", target_ssh.user, target.address);
            let ssh_args = format!("-J {jump_str} {dest_str}");
            let note = if reach.note.is_empty() {
                String::new()
            } else {
                format!(" ({})", reach.note)
            };
            Ok(PathSpec {
                command: format!("ssh {ssh_args}"),
                ssh_args: Some(ssh_args),
                explanation: format!(
                    "{from} → {to} via {jump_name} jump host{note}"
                ),
            })
        }
    }
}

/// Produce the command to probe a named service from `from`.
pub fn service_path(
    m: &Manifest,
    from: &str,
    svc_name: &str,
) -> Result<PathSpec, ReachabilityError> {
    let svc = m
        .services
        .get(svc_name)
        .ok_or_else(|| ReachabilityError::UnknownService(svc_name.into()))?;
    let host = m
        .hosts
        .get(&svc.host)
        .ok_or_else(|| ReachabilityError::UnknownHost(svc.host.clone()))?;
    let addr = &host.address;
    let port = svc.port;
    let path_segment = if svc.path.is_empty() { "/" } else { &svc.path };

    // For HTTP(S), if the from-host can reach the target directly via
    // L3, just curl straight to the IP:port. If it needs to go via a
    // jump, use `ssh -J <jump> <dest> 'curl ...'` — run the curl ON
    // the destination. That's the simplest correct answer.
    match svc.protocol {
        Protocol::Http | Protocol::Https => {
            let scheme = match svc.protocol {
                Protocol::Http => "http",
                Protocol::Https => "https",
                _ => unreachable!(),
            };
            if from == &svc.host {
                let cmd = format!("curl -sf {scheme}://127.0.0.1:{port}{path_segment}");
                return Ok(PathSpec {
                    command: cmd,
                    ssh_args: None,
                    explanation: format!("service local to {}", svc.host),
                });
            }
            let reach = reach_for(m, from, &svc.host).ok_or_else(|| {
                ReachabilityError::NoRoute {
                    from: from.into(),
                    to: svc.host.clone(),
                }
            })?;
            match reach.kind {
                ReachKind::Forbidden => Err(ReachabilityError::Forbidden {
                    from: from.into(),
                    to: svc.host.clone(),
                    note: reach.note.clone(),
                }),
                ReachKind::Direct => {
                    let cmd = format!("curl -sf {scheme}://{addr}:{port}{path_segment}");
                    Ok(PathSpec {
                        command: cmd,
                        ssh_args: None,
                        explanation: format!("direct {scheme} to {}", svc.host),
                    })
                }
                ReachKind::Via => {
                    // Run the curl from inside an ssh through the jump.
                    let ssh = ssh_path(m, from, &svc.host)?;
                    let ssh_cmd = ssh.command;
                    let cmd = format!(
                        "{ssh_cmd} 'curl -sf {scheme}://127.0.0.1:{port}{path_segment}'"
                    );
                    Ok(PathSpec {
                        command: cmd,
                        ssh_args: None,
                        explanation: format!(
                            "{scheme} probe run on {} via jump (remote curl)",
                            svc.host
                        ),
                    })
                }
            }
        }
        Protocol::Ssh => ssh_path(m, from, &svc.host),
        Protocol::Tcp | Protocol::Udp | Protocol::Mqtt | Protocol::Grpc => {
            // No universal probe command; show the endpoint.
            let cmd = format!("{}:{} ({:?})", addr, port, svc.protocol);
            Ok(PathSpec {
                command: cmd,
                ssh_args: None,
                explanation: format!(
                    "no canonical probe command for {:?}; address shown",
                    svc.protocol
                ),
            })
        }
    }
}

/// Lint the manifest. Returns all warnings/errors found; caller
/// decides whether to hard-fail. None == clean.
pub fn validate(m: &Manifest) -> Vec<String> {
    let mut problems: Vec<String> = Vec::new();

    for (name, host) in &m.hosts {
        for (from, reach) in &host.reachable_from {
            if from != "*" && !m.hosts.contains_key(from) {
                problems.push(format!(
                    "host `{name}`.reachable_from mentions `{from}` but no such host in manifest"
                ));
            }
            if reach.kind == ReachKind::Via {
                match reach.jump.as_deref() {
                    None => problems.push(format!(
                        "host `{name}`.reachable_from[{from}] is `via` but has no `jump` field"
                    )),
                    Some(j) if !m.hosts.contains_key(j) => problems.push(format!(
                        "host `{name}`.reachable_from[{from}] refers to jump `{j}` not in manifest"
                    )),
                    Some(j) if m.hosts.get(j).and_then(|h| h.ssh.as_ref()).is_none() => {
                        problems.push(format!(
                            "jump host `{j}` (referenced by {name}.reachable_from[{from}]) has no ssh config"
                        ));
                    }
                    _ => {}
                }
            }
        }
    }

    for (name, svc) in &m.services {
        if !m.hosts.contains_key(&svc.host) {
            problems.push(format!(
                "service `{name}` runs on `{}` but that host isn't in manifest",
                svc.host
            ));
        }
    }

    problems
}
