//! Standalone CLI for the one-time aidot cloud harvest.
//!
//! Usage:  aidot-harvest <email> <password>
//!
//! Writes `$AIDOT_INVENTORY` (or `~/.syntaur/aidot_inventory.json` by
//! default) at mode 0600. Runtime LAN control uses `aidot-ctl` against
//! that file and never touches the cloud again.

use std::env;
use std::path::PathBuf;

use anyhow::Context;

#[tokio::main(flavor = "current_thread")]
async fn main() -> anyhow::Result<()> {
    let args: Vec<String> = env::args().skip(1).collect();
    let argv: Vec<&str> = args.iter().map(|s| s.as_str()).collect();

    let (email, password) = match argv.as_slice() {
        [email, password] => (*email, *password),
        _ => {
            eprintln!("usage: aidot-harvest <email> <password>");
            eprintln!();
            eprintln!(
                "One-time cloud harvest of aidot device credentials. Writes an \
                 inventory file used by `aidot-ctl` for LAN control."
            );
            std::process::exit(2);
        }
    };

    let country = env::var("AIDOT_COUNTRY").unwrap_or_else(|_| "United States".into());
    eprintln!("[aidot-harvest] contacting prod-us-api.arnoo.com as {email}");
    let inv = rust_aidot_harvest::harvest(email, password, &country).await?;

    let path = inventory_path();
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).ok();
    }
    let bytes = serde_json::to_vec_pretty(&inv)?;
    std::fs::write(&path, &bytes).with_context(|| format!("writing {}", path.display()))?;

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600))?;
    }
    eprintln!(
        "[aidot-harvest] wrote {} devices to {}",
        inv.devices.len(),
        path.display()
    );
    Ok(())
}

fn inventory_path() -> PathBuf {
    if let Ok(p) = env::var("AIDOT_INVENTORY") {
        PathBuf::from(p)
    } else {
        let home = env::var("HOME").unwrap_or_else(|_| "/home/sean".into());
        PathBuf::from(home).join(".syntaur").join("aidot_inventory.json")
    }
}
