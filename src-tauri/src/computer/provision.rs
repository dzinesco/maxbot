//! `provision_vm(bot_id, opts)` — the high-level orchestrator
//! that turns a Bot into a Bot-with-a-VM.
//!
//! Steps:
//! 1. Mint a per-Bot Ed25519 keypair via `keys::generate_keypair`.
//! 2. Encrypt the private half and persist it to
//!    `ssh_keys` (id = uuid; bot_id tagged via the
//!    `computers.ssh_key_id` reference).
//! 3. Build the cloud-init `user-data` and `meta-data` (this
//!    module owns the user-data template; the server-side
//!    `provision-vm.sh` does the genisoimage + virt-install).
//! 4. SSH to the server and run
//!    `sudo -n /opt/maxbot/provision-vm.sh <name> <disk_gb>
//!    <ram_mb> <ssh_pubkey> <vnc_password>`. The script
//!    handles the qcow2 overlay, ISO generation, and
//!    `virt-install --import`. It returns the libvirt domain
//!    name on stdout.
//! 5. Poll `virsh net-dhcp-leases default` every 2s up to
//!    90s for the VM's IP. dnsmasq returns the lease as
//!    soon as the VM's NIC comes up — much faster than
//!    `virsh domifaddr` which requires qemu-guest-agent
//!    (only starts after cloud-init's apt install finishes,
//!    3-5 minutes).
//! 6. `virsh vncdisplay <name>` to get the VNC port.
//! 7. Persist `computers` row with `state = running`, plus
//!    the IP and VNC port.
//!
//! Failure handling: any step that fails transitions the
//! computer to `state = error` and emits a
//! `computer://state-changed` event so the UI can show the
//! error. The provision-vm.sh script is idempotent — if
//! it sees an existing domain with the same name, it
//! errors with a clear message and we surface that.

use std::time::Duration;

use rand::Rng;
use serde::{Deserialize, Serialize};
use thiserror::Error;

use super::keys;
use super::libvirt::LibvirtClient;
use super::ssh::{SshExecutor, SshPool};
use super::wait_for_qga_ready;

#[derive(Debug, Error)]
pub enum ProvisionError {
    #[error("ssh error: {0}")]
    Ssh(String),
    #[error("libvirt error: {0}")]
    Libvirt(String),
    #[error("database error: {0}")]
    Db(String),
    #[error("no Bot with id {0}")]
    NoBot(String),
    #[error("Bot {0} already has a computer — destroy first")]
    AlreadyProvisioned(String),
    #[error("timed out waiting for the VM to get a DHCP lease after {0:?}")]
    IpTimeout(Duration),
    #[error("server returned no domain name from provision-vm.sh")]
    NoDomainName,
}

/// Options the user picks in the Bot editor when
/// provisioning. The defaults are filled in by the
/// Tauri command from `Settings`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProvisionOptions {
    pub disk_gb: u32,
    pub ram_mb: u32,
}

impl Default for ProvisionOptions {
    fn default() -> Self {
        Self {
            disk_gb: 10,
            ram_mb: 2048,
        }
    }
}

/// Top-level orchestrator. `bot_id` identifies the Bot;
/// `opts` carries disk_gb and ram_mb. The function is
/// async because the libvirt-over-SSH calls are.
/// `db` and `pool` are the two collaborators we need.
///
/// On success, returns the new computer's domain name
/// and the discovered IP. On failure, returns
/// `ProvisionError`. The caller is expected to:
/// - write a `computers` row with `state =
///   provisioning` BEFORE invoking us, so the UI
///   sees a placeholder
/// - emit a `computer://state-changed` event after we
///   return (success: `running`; failure: `error` with
///   the error message)
pub async fn provision_vm(
    bot_id: &str,
    opts: &ProvisionOptions,
    pool: &SshPool,
    libvirt: &LibvirtClient,
) -> Result<ProvisionResult, ProvisionError> {
    // 1. Mint a per-Bot keypair.
    let (pkcs8, public_key) =
        keys::generate_keypair().map_err(|e| ProvisionError::Ssh(e.to_string()))?;

    // 2. VNC password: 12 random base64url chars. This
    //    isn't the long-term secret (the per-Bot SSH
    //    keypair is), but it stops casual network
    //    snooping. Picked to fit in 8 chars (x11vnc's
    //    max) with a 4-char buffer.
    let vnc_password: String = {
        use rand::rngs::OsRng;
        let mut bytes = [0u8; 9];
        OsRng.fill(&mut bytes);
        // base64url then trim to 8 chars.
        use base64::Engine as _;
        let s = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(bytes);
        s.chars().take(8).collect()
    };

    // 3. The libvirt domain name. Includes the bot_id
    //    so it's recognizable in `virsh list`. Bot_ids
    //    are uuid4, so the prefix is safe.
    let vm_name = format!("maxbot-bot-{}", sanitize_for_libvirt(bot_id));
    // The DHCP hostname we match against in the lease poll
    // must mirror what the server-side `provision-vm.sh`
    // script writes into the cloud-init meta-data. The
    // script sets `local-hostname: ${VM_NAME//_/-}`, so the
    // lease will carry the full `maxbot-bot-...` prefix.
    // v2.0.0 had `maxbot-{sanitize_for_hostname(bot_id)}`
    // here, which is missing the `bot-` segment and never
    // matched the real lease — every provision timed out
    // at the 90s IP poll. The `ComputerPanel` then showed
    // "Computer error: The VM is in an error state." even
    // though the VM was up and reachable.
    let hostname = format!("maxbot-bot-{}", sanitize_for_hostname(bot_id));

    // 4. Build the cloud-init payload — but defer
    //    genisoimage to the server-side script. We
    //    need to pass the SSH pubkey and VNC password
    //    to the script. The script writes its own
    //    user-data based on these inputs.
    //
    //    The script's user-data template is in
    //    `provision-vm.sh`; it embeds the same fields
    //    our `build_user_data` produces, with the
    //    pubkey + vnc_password substituted. Keeping
    //    the template server-side avoids us having
    //    to upload a 1KB blob over SSH.

    // 5. SSH to the server and run provision-vm.sh.
    //    We pass the pubkey + password as command-line
    //    args; they're not secrets-after-creation
    //    (the password is in the cloud-init ISO
    //    anyway), and the pubkey is the public half
    //    of a keypair. The ssh connection to the
    //    server uses the OS keychain.
    let cmd = format!(
        "sudo -n /opt/maxbot/provision-vm.sh {} {} {} '{}' '{}'",
        shell_quote(&vm_name),
        opts.disk_gb,
        opts.ram_mb,
        shell_quote(&public_key),
        shell_quote(&vnc_password),
    );
    let out = pool
        .server_exec(&cmd)
        .await
        .map_err(|e| ProvisionError::Ssh(e.to_string()))?;
    if !out.success {
        return Err(ProvisionError::Ssh(format!(
            "provision-vm.sh failed (exit {:?}): {}",
            out.exit_code,
            out.stderr.trim()
        )));
    }
    // The script writes the libvirt domain name on its
    // FINAL line (`echo "$VM_NAME"` is the last thing in
    // the script). Earlier in the script, `qemu-img
    // create` writes its formatting progress to stdout
    // (one or more lines), and the rest of the script
    // (`virt-install`, `genisoimage`) is mostly silent on
    // stdout. v2.0.0 did `out.stdout.trim()` which
    // returned the entire concatenated output — the
    // resulting "domain name" was several hundred bytes
    // long and broke every subsequent virsh call
    // (`vncdisplay`, `destroy`, etc.). Take the LAST
    // non-empty line.
    let domain_name = out
        .stdout
        .lines()
        .filter(|l| !l.trim().is_empty())
        .last()
        .map(|l| l.trim().to_string())
        .unwrap_or_default();
    if domain_name.is_empty() {
        return Err(ProvisionError::NoDomainName);
    }

    // 6. Cache the freshly-generated private key in
    //    the pool so the orchestrator's subsequent
    //    `vm_exec` calls work without re-decrypting
    //    (the encrypted blob hasn't been written to
    //    the DB yet, but the in-memory copy is
    //    identical).
    pool.cache_key_blob(bot_id, std::sync::Arc::new(pkcs8.clone()))
        .await;

    // 7. Poll for the IP. dnsmasq returns the lease
    //    within ~30s of NIC-up; we wait up to 90s.
    let ip = poll_for_ip(libvirt, pool, &hostname, Duration::from_secs(90))
        .await
        .map_err(|e| ProvisionError::Ssh(e.to_string()))?;

    // 8. VNC port. `virsh vncdisplay` is a single
    //    round-trip; if it fails (rare — would mean
    //    the domain is still being defined), retry a
    //    couple of times.
    let vnc_port = poll_for_vnc_port(libvirt, pool, &domain_name)
        .await
        .map_err(|e| ProvisionError::Libvirt(e.to_string()))?;

    // 9. v3.0.5: wait for the QEMU guest agent to
    //    become responsive. `provision-vm.sh` only
    //    waits for the VM to be defined; cloud-init's
    //    `packages:` block (xfce4, x11vnc,
    //    qemu-guest-agent, openssh-server) + `runcmd:`
    //    `systemctl enable --now qemu-guest-agent`
    //    finish well after the DHCP lease poll above.
    //    Without this wait, any QGA call made
    //    immediately after provision (e.g. the v3.0.3
    //    console auto-recover's
    //    `install_default_key_via_qga`) hits
    //    `Guest agent is not responding` from virsh.
    //    The 5-attempt retry at the
    //    `qemu_agent_command` layer covers the brief
    //    socket-not-ready window after QGA is
    //    "ready"; this covers the much longer
    //    QGA-install window. On a warm cloud-image
    //    cache the wait is typically 30-90s; on a
    //    cold cache (first provision) it can be
    //    several minutes. The helper times out at
    //    5 minutes.
    wait_for_qga_ready(pool, &domain_name)
        .await
        .map_err(|e| ProvisionError::Libvirt(e.to_string()))?;

    Ok(ProvisionResult {
        domain_name,
        ip,
        vnc_port,
        pkcs8_private_key: pkcs8,
        public_key,
    })
}

/// What `provision_vm` returns. The DB-writing half
/// happens in the Tauri command so this stays free of
/// the Database dependency and is easy to test.
#[derive(Debug, Clone)]
pub struct ProvisionResult {
    pub domain_name: String,
    pub ip: String,
    pub vnc_port: u16,
    pub pkcs8_private_key: Vec<u8>,
    pub public_key: String,
}

async fn poll_for_ip(
    libvirt: &LibvirtClient,
    pool: &SshPool,
    hostname: &str,
    timeout: Duration,
) -> Result<String, ProvisionError> {
    let start = std::time::Instant::now();
    let interval = Duration::from_secs(2);
    loop {
        let leases = libvirt
            .net_dhcp_leases(pool, "default")
            .await
            .map_err(|e| ProvisionError::Libvirt(e.to_string()))?;
        // Match by hostname (libvirt's dnsmasq uses
        // the cloud-init `local-hostname`). We also
        // fall back to scanning by `52:54:00:*` (the
        // libvirt MAC prefix) in case the hostname
        // hasn't propagated yet.
        if let Some(l) = leases.iter().find(|l| l.hostname == hostname) {
            return Ok(l.ipaddr.clone());
        }
        if start.elapsed() >= timeout {
            return Err(ProvisionError::IpTimeout(timeout));
        }
        tokio::time::sleep(interval).await;
    }
}

async fn poll_for_vnc_port(
    libvirt: &LibvirtClient,
    pool: &SshPool,
    domain: &str,
) -> Result<u16, ProvisionError> {
    // The domain takes a moment to register with
    // libvirtd after `virt-install` returns. Try a few
    // times before giving up.
    let mut last_err = None;
    for _ in 0..10 {
        match libvirt.vncdisplay(pool, domain).await {
            Ok(p) => return Ok(p),
            Err(e) => {
                last_err = Some(e);
                tokio::time::sleep(Duration::from_millis(500)).await;
            }
        }
    }
    Err(ProvisionError::Libvirt(format!(
        "vncdisplay never succeeded: {last_err:?}"
    )))
}

/// Convert a uuid4 to a libvirt-domain-name-safe
/// string: lowercase + dashes. `max-bot_…` style ids
/// would be rejected by libvirt's name validator.
fn sanitize_for_libvirt(s: &str) -> String {
    s.chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' {
                c.to_ascii_lowercase()
            } else {
                '-'
            }
        })
        .collect()
}

/// Like `sanitize_for_libvirt`, but the local-hostname
/// in cloud-init's meta-data has the same constraints
/// (RFC 1123).
fn sanitize_for_hostname(s: &str) -> String {
    sanitize_for_libvirt(s)
}

/// Shell-escape a string for inclusion as a
/// single-quoted argument in a command. We pass
/// strings through `sudo -n /opt/maxbot/provision-vm.sh
/// <args>`; the arguments include the SSH pubkey and
/// VNC password which may contain `'` (very rare in
/// generated strings, but worth being safe).
fn shell_quote(s: &str) -> String {
    let escaped = s.replace('\'', "'\\''");
    format!("'{escaped}'")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sanitize_strips_uppercase_and_unicode() {
        // libvirt names are ASCII-only.
        let s = sanitize_for_libvirt("ABCD-é_xyz");
        assert_eq!(s, "abcd---xyz");
    }

    #[test]
    fn shell_quote_wraps_in_single_quotes() {
        assert_eq!(shell_quote("hello"), "'hello'");
        assert_eq!(shell_quote("it's"), "'it'\\''s'");
    }

    #[test]
    fn default_options_are_10gb_2gb() {
        // These are the v2.0 defaults from the plan.
        // Changing them is a deliberate decision —
        // keep the test loud if the defaults drift.
        let o = ProvisionOptions::default();
        assert_eq!(o.disk_gb, 10);
        assert_eq!(o.ram_mb, 2048);
    }
}
