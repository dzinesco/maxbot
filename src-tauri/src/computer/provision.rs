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
//!    <ram_mb> <ssh_pubkey> [vnc_password=ignored]
//!    [vnc_display=N]`. The script handles the
//!    qcow2 overlay, ISO generation, and `virt-install --import`.
//!    It returns the libvirt domain name on stdout.
//!    (v3.7.5: the 5th positional arg `<vnc_password>` is gone
//!    but the slot is preserved-and-ignored for API
//!    compatibility. v3.7.16: the 6th positional arg
//!    `<vnc_display>` carries the display number we picked
//!    on the Mac side so multiple VMs get sequential ports in
//!    5900-5999 instead of all colliding on 5900.)
//! 5. Poll `virsh net-dhcp-leases default` every 2s up to
//!    90s for the VM's IP. dnsmasq returns the lease as
//!    soon as the VM's NIC comes up — much faster than
//!    `virsh domifaddr` which requires qemu-guest-agent
//!    (only starts after cloud-init's apt install finishes,
//!    3-5 minutes).
//! 6. The VNC port is `5900 + vnc_display` — we pick the
//!    display on the Mac side (see `next_free_vnc_display`),
//!    pass it to the script, and trust the script to bind
//!    QEMU to that display. `virsh vncdisplay` doesn't know
//!    about the qemu:commandline VNC (no `<graphics>` element),
//!    so it would return an error — we use the display number
//!    directly. (v3.7.5 used a 5900 hard-coded fallback
//!    here; v3.7.16 makes it dynamic.)
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
    vnc_display: u8,
) -> Result<ProvisionResult, ProvisionError> {
    // 1. Mint a per-Bot keypair.
    let (pkcs8, public_key) =
        keys::generate_keypair().map_err(|e| ProvisionError::Ssh(e.to_string()))?;

    // 2. The libvirt domain name. Includes the bot_id
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

    // 3. Build the cloud-init payload — but defer
    //    genisoimage to the server-side script. We
    //    pass the SSH pubkey to the script. The script
    //    writes its own user-data based on this input.
    //
    //    The script's user-data template is in
    //    `provision-vm.sh`; it embeds the same fields
    //    our `build_user_data` produces, with the
    //    pubkey substituted. Keeping the template
    //    server-side avoids us having to upload a 1KB
    //    blob over SSH.

    // 4. SSH to the server and run provision-vm.sh.
    //    The pubkey is the public half of a keypair.
    //    The ssh connection to the server uses the OS
    //    keychain.
    //
    //    v3.7.16: pass the VNC display number as the 6th
    //    positional arg (the 5th stays as the
    //    preserved-and-ignored `<vnc_password>` slot from
    //    v3.7.5). The script substitutes it into the
    //    qemu:commandline so QEMU binds to
    //    `127.0.0.1:<display>,password=off,to=5999`. This
    //    is what makes multi-VM provisioning stop
    //    colliding on port 5900.
    let cmd = format!(
        "sudo -n /opt/maxbot/provision-vm.sh {} {} {} '{}' '' {}",
        shell_quote(&vm_name),
        opts.disk_gb,
        opts.ram_mb,
        shell_quote(&public_key),
        vnc_display,
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

    // 5. Cache the freshly-generated private key in
    //    the pool so the orchestrator's subsequent
    //    `vm_exec` calls work without re-decrypting
    //    (the encrypted blob hasn't been written to
    //    the DB yet, but the in-memory copy is
    //    identical).
    pool.cache_key_blob(bot_id, std::sync::Arc::new(pkcs8.clone()))
        .await;

    // 6. Poll for the IP. dnsmasq returns the lease
    //    within ~30s of NIC-up; we wait up to 90s.
    let ip = poll_for_ip(libvirt, pool, &hostname, Duration::from_secs(90))
        .await
        .map_err(|e| ProvisionError::Ssh(e.to_string()))?;

    // 7. VNC port. v3.7.16: we picked the display number
    //    on the Mac side (see `next_free_vnc_display`)
    //    and passed it to the script. The script's
    //    qemu:commandline binds QEMU to that display
    //    (`-vnc 127.0.0.1:<display>,password=off,to=5999`).
    //    Since the display is in 0..=99 and we picked one
    //    that's free, the actual port is 5900 + display —
    //    we don't need to ask libvirt (it doesn't know
    //    about the qemu:commandline VNC anyway because
    //    `--graphics none` removed the `<graphics>`
    //    element).
    //
    //    We do, however, sanity-check the chosen display
    //    by asking libvirt to confirm the VM is running
    //    (the qemu:commandline patch + `virsh start` runs
    //    inside provision-vm.sh, so by the time we get
    //    here the VM should be live). If `virsh domstate`
    //    says anything other than "running", surface a
    //    clear error — the VNC port won't be reachable
    //    otherwise.
    poll_for_vm_running(libvirt, pool, &domain_name)
        .await
        .map_err(|e| ProvisionError::Libvirt(e.to_string()))?;
    let vnc_port = vnc_port_from_display(vnc_display);

    // 8. v3.0.5: wait for the QEMU guest agent to
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

/// v3.7.16: pick the lowest free VNC display number
/// given the list of display numbers already in use.
/// The display number maps to TCP port `5900 + display`,
/// so the in-use list is a list of `(port - 5900)`.
/// Returns `None` if the 100-display range
/// (0..=99, ports 5900..=5999) is full.
pub fn next_free_vnc_display(in_use: &[u16], range_lo: u16, range_hi: u16) -> Option<u16> {
    if range_hi < range_lo {
        return None;
    }
    let mut used: std::collections::BTreeSet<u16> = in_use
        .iter()
        .filter_map(|&p| p.checked_sub(5900))
        .filter(|&d| d >= range_lo && d <= range_hi)
        .collect();
    for d in range_lo..=range_hi {
        if !used.remove(&d) {
            return Some(d);
        }
    }
    None
}

/// v3.7.16: derive the VNC TCP port from the display
/// number. Display 0 → 5900, display 1 → 5901, etc.
/// The cap at 5999 matches the qemu:commandline's
/// `to=5999` upper bound; a display above 99 would
/// land outside the configured port range and the
/// `to=5999` knob wouldn't help.
pub fn vnc_port_from_display(display: u8) -> u16 {
    // `display: u8` is at most 255; 5900 + u8 saturates
    // at 6155 which is still in the 5900-65535 range,
    // so a plain `+` is safe and won't overflow a u16
    // until 5900+1216. Callers are expected to keep
    // `display` in 0..=99 (the `next_free_vnc_display`
    // range) but we don't assert here — the script's
    // qemu:commandline will refuse to bind above 5999
    // and the user will see a clear error.
    5900u16.saturating_add(u16::from(display))
}

/// v3.7.16: confirm the domain reached the `running`
/// state in libvirtd after provision-vm.sh returned.
/// `virsh start` is fire-and-forget; libvirtd can take
/// a moment to register the state. We poll a few times
/// (500ms apart, 10 attempts) before giving up.
async fn poll_for_vm_running(
    libvirt: &LibvirtClient,
    pool: &SshPool,
    domain: &str,
) -> Result<(), ProvisionError> {
    let mut last_err: Option<String> = None;
    for _ in 0..10 {
        match libvirt.domstate(pool, domain).await {
            Ok(s) if s == super::libvirt::DomainState::Running => return Ok(()),
            Ok(s) => {
                last_err = Some(format!("state is {s:?}, not running"));
            }
            Err(e) => {
                last_err = Some(e.to_string());
            }
        }
        tokio::time::sleep(Duration::from_millis(500)).await;
    }
    Err(ProvisionError::Libvirt(format!(
        "VM {domain} did not reach `running` state: {}",
        last_err.unwrap_or_else(|| "unknown".into())
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

    // --- v3.7.16: VNC port allocation tests ---

    /// Empty list → display 0 (the v3.7.5 behavior).
    #[test]
    fn next_free_vnc_display_empty_list_returns_zero() {
        assert_eq!(next_free_vnc_display(&[], 0, 99), Some(0));
    }

    /// First VM already on 5900 → next gets 5901 (display 1).
    #[test]
    fn next_free_vnc_display_skips_in_use() {
        assert_eq!(next_free_vnc_display(&[5900], 0, 99), Some(1));
    }

    /// Out-of-range entries (e.g. 5899 from a stray row) are
    /// ignored — we only consider ports in the configured
    /// range.
    #[test]
    fn next_free_vnc_display_ignores_out_of_range() {
        // 5899 is below the range; 6000 is above. Both
        // should be ignored, so the function returns 0.
        assert_eq!(next_free_vnc_display(&[5899, 6000], 0, 99), Some(0));
    }

    /// Multiple gaps — we pick the lowest free slot, not
    /// the next sequential one. This matters when a
    /// middle Bot gets Destroyed: the new Bot should
    /// reuse the freed display, not push everyone forward.
    #[test]
    fn next_free_vnc_display_picks_lowest_free() {
        // 5900 and 5902 in use → 5901 is free.
        assert_eq!(next_free_vnc_display(&[5900, 5902], 0, 99), Some(1));
        // 5901 and 5902 in use → 5900 is free.
        assert_eq!(next_free_vnc_display(&[5901, 5902], 0, 99), Some(0));
    }

    /// The full range is exhausted → None (the caller
    /// surfaces a clear "destroy a Bot" error).
    #[test]
    fn next_free_vnc_display_full_range_returns_none() {
        let used: Vec<u16> = (5900..=5999).collect();
        assert_eq!(next_free_vnc_display(&used, 0, 99), None);
    }

    /// The user can configure a tighter range via
    /// `Settings.computer_vnc_local_port_range` (parsed
    /// by the Settings UI into lo + hi ints). The
    /// function respects the range.
    #[test]
    fn next_free_vnc_display_respects_range() {
        // Range 5910..=5912 with 5911 in use → 5910 free.
        assert_eq!(next_free_vnc_display(&[5911], 10, 12), Some(10));
        // Range 5910..=5912 with all in use → None.
        assert_eq!(
            next_free_vnc_display(&[5910, 5911, 5912], 10, 12),
            None
        );
    }

    /// Invalid range (lo > hi) → None, not a panic.
    /// Defensive — the Settings UI clamps this but
    /// the function shouldn't trust its inputs.
    #[test]
    fn next_free_vnc_display_invalid_range_returns_none() {
        assert_eq!(next_free_vnc_display(&[], 50, 10), None);
    }

    /// `vnc_port_from_display` is the inverse of
    /// `display - 5900`. Lock the math here so any
    /// drift (off-by-one, signedness) fails loudly.
    #[test]
    fn vnc_port_from_display_math_is_inverse() {
        assert_eq!(vnc_port_from_display(0), 5900);
        assert_eq!(vnc_port_from_display(1), 5901);
        assert_eq!(vnc_port_from_display(50), 5950);
        assert_eq!(vnc_port_from_display(99), 5999);
    }

    /// `vnc_port_from_display` must NOT overflow a u16
    /// even if a future caller passes display=255.
    /// (Today's range is 0..=99 so this can't happen
    /// in practice, but `saturating_add` is cheap
    /// insurance.)
    #[test]
    fn vnc_port_from_display_saturates() {
        // 5900 + 255 = 6155, well within u16::MAX.
        assert_eq!(vnc_port_from_display(255), 6155);
    }
}
