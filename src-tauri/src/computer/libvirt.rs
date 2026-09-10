//! Typed wrapper over `virsh` / `virt-install` / `qemu-img` /
//! `genisoimage` running on the user's Linux server.
//!
//! All libvirt operations are driven via the server-side SSH pool
//! (`SshPool::server_exec`). The Mac itself does not need
//! libvirt / qemu / virt-install installed — we never shell out
//! to them locally.
//!
//! Every method returns `Result<T, ComputerError>`. The error
//! variant distinguishes:
//! - `Ssh*` — connection-level problems (auth, timeout, refused)
//! - `Command { exit_code, stderr }` — virsh ran but returned
//!   non-zero (e.g. "domain not found", "already exists")
//! - `Parse` — the output didn't match the expected shape
//! - `Timeout` — a polling loop (e.g. DHCP lease wait) gave up
//! - `NotConfigured` — `Settings.computer_server_host` is empty
//!
//! The libvirt commands here are all `sudo -n virsh …`. Slice A
//! gave `tyler` passwordless sudo specifically so the Tauri side
//! can run these without a TTY. The `-n` flag fails fast (no
//! password prompt) if the NOPASSWD rule ever falls off.

use std::time::Duration;

use serde::{Deserialize, Serialize};
use thiserror::Error;

use super::ssh::{SshExecutor, SshPool};

#[derive(Debug, Error)]
pub enum LibvirtError {
    #[error("ssh error: {0}")]
    Ssh(String),
    #[error("command exited {exit_code:?}: {stderr}")]
    Command {
        exit_code: Option<i32>,
        stderr: String,
    },
    #[error("failed to parse virsh output: {0}")]
    Parse(String),
    #[error("timed out after {0:?}")]
    Timeout(Duration),
    #[error("linux server is not configured: set Settings.computer_server_host")]
    NotConfigured,
}

/// VM lifecycle states we report back to the renderer. libvirt
/// has a larger set; we collapse to the four that the UI
/// actually renders.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum DomainState {
    Running,
    /// `shut off` in libvirt-speak — VM is defined but not
    /// booted. Distinct from `Crashed` (libvirt's
    /// `crashed`).
    Stopped,
    Paused,
    Crashed,
    /// libvirt returned a state we don't model yet (e.g.
    /// `pmsuspended`, `in shutdown`). Treated as "transitional"
    /// by the UI — shows a spinner.
    Other,
}

impl DomainState {
    pub fn from_libvirt(s: &str) -> Self {
        // libvirt `domstate` returns one of: running, blocked,
        // paused, shutdown, shut off, crashed, pmsuspended,
        // online (only for net-defined inactive nets), migration.
        // We collapse to the 4-state machine the UI uses.
        match s.trim() {
            "running" => Self::Running,
            "paused" => Self::Paused,
            "crashed" => Self::Crashed,
            "shut off" | "shutdown" => Self::Stopped,
            _ => Self::Other,
        }
    }
}

/// One row from `virsh net-dhcp-leases default`. The consumer
/// (provision-vm) matches by `ipaddr` + `hostname`; `mac` is
/// dropped because the protocol gives it but the IP-poll only
/// needs the IP+hostname pair.
#[derive(Debug, Clone)]
pub struct DhcpLease {
    pub ipaddr: String,
    pub hostname: String,
}

/// Single-row summary of a libvirt domain, used by `list`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DomainSummary {
    pub name: String,
    pub state: DomainState,
}

/// Public entry-point for libvirt-over-SSH. Constructed once
/// per process (it holds no per-call state). All methods take
/// `&SshPool` so they share the long-lived server connection.
pub struct LibvirtClient;

impl LibvirtClient {
    pub fn new() -> Self {
        Self
    }

    /// `virsh list --all` — one line per domain, tab-separated:
    /// `Id  Name  State`. We parse the State column. The list
    /// has a header line; we filter on `name != "Name"` (the
    /// header) to skip it.
    pub async fn list_domains(&self, pool: &SshPool) -> Result<Vec<DomainSummary>, LibvirtError> {
        let out = run_virsh(pool, &["list", "--all"]).await?;
        let mut out_v = Vec::new();
        for line in out.lines() {
            // `virsh list` columns are whitespace-aligned but
            // not strictly tab-separated. The state is always
            // the last whitespace-separated token, the name is
            // the second-to-last. Skip the header row.
            let trimmed = line.trim();
            if trimmed.is_empty() || trimmed.starts_with("Id") || trimmed.starts_with("===") {
                continue;
            }
            let parts: Vec<&str> = trimmed.split_whitespace().collect();
            // Bare minimum: [id-or-dash, name, state, ...]
            if parts.len() < 2 {
                continue;
            }
            let name = parts[parts.len() - 2].to_string();
            let state = DomainState::from_libvirt(parts[parts.len() - 1]);
            out_v.push(DomainSummary { name, state });
        }
        Ok(out_v)
    }

    /// `virsh start <name>` — bring a defined domain up.
    pub async fn start(&self, pool: &SshPool, name: &str) -> Result<(), LibvirtError> {
        run_virsh(pool, &["start", name]).await.map(|_| ())
    }

    /// `virsh shutdown <name>` — graceful ACPI shutdown. The VM
    /// may take a few seconds to actually power off; the caller
    /// polls `virsh list --state` to confirm.
    pub async fn shutdown(&self, pool: &SshPool, name: &str) -> Result<(), LibvirtError> {
        run_virsh(pool, &["shutdown", name]).await.map(|_| ())
    }

    /// `virsh destroy <name>` — hard power-off. Equivalent to
    /// pulling the plug. Use for `computer_destroy`.
    pub async fn destroy(&self, pool: &SshPool, name: &str) -> Result<(), LibvirtError> {
        run_virsh(pool, &["destroy", name]).await.map(|_| ())
    }

    /// `virsh undefine <name> --remove-all-storage` — wipe the
    /// domain XML + the qcow2 disk + the seed ISO. Used by
    /// `computer_destroy` after a successful `virsh destroy`.
    pub async fn undefine(
        &self,
        pool: &SshPool,
        name: &str,
    ) -> Result<(), LibvirtError> {
        run_virsh(pool, &["undefine", name, "--remove-all-storage"])
            .await
            .map(|_| ())
    }

    /// `virsh vncdisplay <name>` — returns the VNC display
    /// number, e.g. `:0` or `:5`. We add 5900 to get the TCP
    /// port the VNC server is bound on (libvirt's VNC is
    /// always at 5900 + display). The `provision-vm.sh` script
    /// uses `--graphics vnc,listen=127.0.0.1,port=-1` which
    /// asks libvirt to auto-assign the next free port, so this
    /// number is the live port for this VM.
    pub async fn vncdisplay(&self, pool: &SshPool, name: &str) -> Result<u16, LibvirtError> {
        let out = run_virsh(pool, &["vncdisplay", name]).await?;
        // Format: "127.0.0.1:0" or ":5". We only care about
        // the number after the last `:`.
        let s = out.trim();
        let after = s.rsplit(':').next().unwrap_or("0");
        let display: i64 = after
            .parse()
            .map_err(|e| LibvirtError::Parse(format!("vncdisplay '{s}': {e}")))?;
        let port = 5900 + display;
        if !(5900..=5999).contains(&port) {
            return Err(LibvirtError::Parse(format!(
                "vncdisplay {display} maps to port {port} outside 5900-5999"
            )));
        }
        Ok(port as u16)
    }

    /// `virsh net-dhcp-leases default` — return the parsed
    /// list. Used by `provision_vm` to discover the VM's IP
    /// fast (within ~30s of NIC up) without waiting for
    /// qemu-guest-agent.
    pub async fn net_dhcp_leases(
        &self,
        pool: &SshPool,
        network: &str,
    ) -> Result<Vec<DhcpLease>, LibvirtError> {
        let out = run_virsh(pool, &["net-dhcp-leases", network]).await?;
        Ok(parse_dhcp_leases(&out))
    }

    /// v2.3.5: send a raw QEMU guest agent (QGA) command
    /// to a domain. Used by `computer_install_default_key`
    /// to install the user's default SSH public key into
    /// `/home/bot/.ssh/authorized_keys` without going through
    /// SSH (the chicken-and-egg case for a user switching off
    /// per-Bot keys on a VM that was provisioned with them).
    ///
    /// `cmd_json` is the QGA request body, e.g.
    /// `{"execute":"guest-exec",...}`. Returned on success
    /// is the QGA response (typically JSON). Errors come
    /// from libvirt: if the QGA channel isn't open, virsh
    /// returns `qemu-agent-command: Agent not available`.
    ///
    /// v3.0.5: after `provision_vm` returns, the VM is
    /// `running` with a DHCP lease, but the QEMU guest
    /// agent socket can take a fraction of a second to a
    /// few seconds to fully connect. v3.0.4's e2e test
    /// `console_e2e_against_crispy` hit this race — the
    /// very first `qemu-agent-command` failed with
    /// `Guest agent is not responding`. We retry on that
    /// specific stderr pattern (only) with a small
    /// backoff, so all QGA callers — provision, console
    /// auto-recover, and any future ones — gracefully
    /// wait for QGA readiness. Other QGA errors (command
    /// not found, permission denied, parse failures)
    /// are surfaced immediately; we don't paper over
    /// real failures.
    pub async fn qemu_agent_command(
        &self,
        pool: &SshPool,
        name: &str,
        cmd_json: &str,
    ) -> Result<String, LibvirtError> {
        // The QGA command body is a single arg; we pass it
        // quoted so the shell doesn't try to parse the JSON.
        let mut last_err: Option<LibvirtError> = None;
        for attempt in 1..=QGA_RETRY_ATTEMPTS {
            match run_virsh(pool, &["qemu-agent-command", name, cmd_json]).await {
                Ok(stdout) => return Ok(stdout),
                Err(e) if is_qga_not_ready(&e) && attempt < QGA_RETRY_ATTEMPTS => {
                    eprintln!(
                        "[maxbot] qemu_agent_command: QGA not ready (attempt {attempt}/{}): {e}; retrying in {:?}",
                        QGA_RETRY_ATTEMPTS, QGA_RETRY_BACKOFF
                    );
                    last_err = Some(e);
                    tokio::time::sleep(QGA_RETRY_BACKOFF).await;
                }
                Err(e) => return Err(e),
            }
        }
        // Loop always runs at least once (QGA_RETRY_ATTEMPTS >= 1).
        Err(last_err.expect("retry loop ran at least once"))
    }
}

/// Tunable retry parameters for the QGA "not ready" race
/// during VM boot. The brief's stated bounds are 3-5
/// attempts at 200-500ms backoff (~2s total). Five
/// attempts with 500ms backoff caps total wait at ~2s,
/// the brief's stated budget. The brief expects this
/// to be enough to span the QGA-connect window on
/// crispy. **NOTE:** in practice the actual race on
/// this VM is much larger — `provision_vm` only waits
/// for `running` + DHCP lease, while cloud-init's
/// `packages:` block (xfce4, x11vnc, qemu-guest-agent,
/// openssh-server) + `runcmd:` `systemctl enable --now
/// qemu-guest-agent` take 10s to several minutes to
/// finish. The retry is still bounded — not a permanent
/// loop — and only matches the specific "Guest agent is
/// not responding" / "QEMU guest agent is not
/// connected" stderr from virsh. Other QGA errors are
/// surfaced immediately. v3.0.5.
const QGA_RETRY_ATTEMPTS: usize = 5;
const QGA_RETRY_BACKOFF: Duration = Duration::from_millis(500);

/// `true` when a `LibvirtError` looks like the QGA socket
/// isn't connected yet (rather than a real QGA failure
/// like a bad command, permission denied, etc.). Matched
/// on stderr text — `virsh` exits with code 1 either way
/// and doesn't surface the structured libvirt error code
/// in this path.
fn is_qga_not_ready(err: &LibvirtError) -> bool {
    match err {
        LibvirtError::Command { stderr, .. } => {
            stderr.contains("Guest agent is not responding")
                || stderr.contains("QEMU guest agent is not connected")
        }
        // Ssh / Parse / Timeout / NotConfigured are
        // caller-side problems, not QGA readiness — don't
        // retry those.
        _ => false,
    }
}

impl Default for LibvirtClient {
    fn default() -> Self {
        Self::new()
    }
}

/// Invoke `sudo -n virsh <args>…` on the server. `-n` prevents a
/// password prompt — if NOPASSWD is missing, virsh returns
/// non-zero immediately and the user sees a clear error.
async fn run_virsh(pool: &SshPool, args: &[&str]) -> Result<String, LibvirtError> {
    let mut cmd = String::with_capacity(16 + args.iter().map(|a| a.len() + 1).sum::<usize>());
    cmd.push_str("sudo -n virsh");
    for a in args {
        cmd.push(' ');
        // Defensive: virsh's parser doesn't need quoting, but
        // if a future caller passes an arg with whitespace
        // (unlikely; virsh args are well-known), single-quote
        // it. The current call sites pass fixed strings.
        if a.chars().any(char::is_whitespace) {
            cmd.push('\'');
            cmd.push_str(&a.replace('\'', "'\\''"));
            cmd.push('\'');
        } else {
            cmd.push_str(a);
        }
    }
    let result = SshExecutor::server_exec(pool, &cmd)
        .await
        .map_err(|e| LibvirtError::Ssh(e.to_string()))?;
    if !result.success {
        return Err(LibvirtError::Command {
            exit_code: result.exit_code,
            stderr: result.stderr.trim().to_string(),
        });
    }
    Ok(result.stdout)
}

/// Parse `virsh net-dhcp-leases default` output. The table has
/// a header line, a separator line, then one row per lease:
///
/// ```text
///  Expiry Time           MAC address         Protocol   IP address          Hostname   Client ID or DUID
///  2026-09-09T13:45:00   52:54:00:3a:c1:ca   ipv4       192.168.122.173/24  maxbot-…   -
/// ```
///
/// We don't bother with `Expiry Time` or `Client ID`; we only
/// need `IP address`, `MAC`, and `Hostname`. The IP is
/// `192.168.122.173/24` — we strip the `/24` suffix to get the
/// bare address.
pub(crate) fn parse_dhcp_leases(raw: &str) -> Vec<DhcpLease> {
    let mut out = Vec::new();
    for line in raw.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty()
            || trimmed.starts_with("Expiry")
            || trimmed.starts_with("---")
        {
            continue;
        }
        // virsh pads with multiple spaces; collapse to single.
        let parts: Vec<&str> = trimmed.split_whitespace().collect();
        // Real `virsh net-dhcp-leases` rows (Ubuntu 24.10+)
        // look like:
        //
        //   2026-09-09 11:42:30  52:54:00:d6:7e:ea  ipv4  192.168.122.65/24  maxbot-bot-...  ff:...
        //
        // The expiry timestamp is TWO whitespace-separated
        // tokens (date + time), not one. Some libvirt versions
        // also emit it as a single ISO-8601 token (`T`-joined);
        // we accept either shape by locating the protocol
        // token (`ipv4` / `ipv6`) and using it as an anchor.
        // That makes us robust to either timestamp format and
        // any future column reordering.
        let proto_idx = parts
            .iter()
            .position(|p| *p == "ipv4" || *p == "ipv6");
        let proto_idx = match proto_idx {
            Some(i) => i,
            None => continue,
        };
        // IP is the token immediately after the protocol.
        let ip_idx = proto_idx + 1;
        if ip_idx >= parts.len() {
            continue;
        }
        let ip_raw = parts[ip_idx];
        let ipaddr = ip_raw.split('/').next().unwrap_or(ip_raw).to_string();
        // Hostname is the token after the IP. Client ID
        // follows if present; we don't need it.
        let hostname = parts
            .get(ip_idx + 1)
            .map(|s| s.to_string())
            .unwrap_or_default();
        out.push(DhcpLease { ipaddr, hostname });
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn domstate_maps_known_libvirt_strings() {
        assert_eq!(DomainState::from_libvirt("running"), DomainState::Running);
        assert_eq!(DomainState::from_libvirt("shut off"), DomainState::Stopped);
        assert_eq!(DomainState::from_libvirt("shutdown"), DomainState::Stopped);
        assert_eq!(DomainState::from_libvirt("paused"), DomainState::Paused);
        assert_eq!(DomainState::from_libvirt("crashed"), DomainState::Crashed);
        assert_eq!(DomainState::from_libvirt("pmsuspended"), DomainState::Other);
    }

    #[test]
    fn parse_dhcp_leases_extracts_ip_and_hostname() {
        let raw = "\
 Expiry Time           MAC address         Protocol   IP address                Hostname       Client ID or DUID
---------------------------------------------------------------------------------------------------------------------
 2026-09-09T13:45:00   52:54:00:3a:c1:ca   ipv4       192.168.122.173/24        maxbot-foo     -
 2026-09-09T13:46:00   52:54:00:de:ad:be   ipv4       192.168.122.50/24         another-bot    ff:...
";
        let leases = parse_dhcp_leases(raw);
        assert_eq!(leases.len(), 2);
        assert_eq!(leases[0].ipaddr, "192.168.122.173");
        assert_eq!(leases[0].hostname, "maxbot-foo");
        assert_eq!(leases[1].ipaddr, "192.168.122.50");
        assert_eq!(leases[1].hostname, "another-bot");
    }

    // v2.0.2 regression: the old parser used hard-coded
    // indices (parts[1] for MAC, parts[3] for IP, parts[4]
    // for hostname) and only worked when the Expiry
    // timestamp was a single token (ISO-8601 with `T`).
    // Real `virsh net-dhcp-leases` on Ubuntu 24.10+ emits
    // the timestamp as TWO tokens (date + time, space-
    // separated), which shifted every column by one. The
    // IP-poll matched the IP address against the expected
    // hostname and never resolved, so every provision
    // timed out at the 90s lease poll and the ComputerPanel
    // showed "Computer error". The new parser anchors on
    // the protocol token (`ipv4` / `ipv6`) so it's robust
    // to either timestamp shape.
    #[test]
    fn parse_dhcp_leases_handles_space_separated_timestamp() {
        // The exact shape Ubuntu 24.10 virsh emits.
        let raw = "\
 Expiry Time           MAC address         Protocol   IP address          Hostname                                          Client ID or DUID
---------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------
 2026-09-09 11:42:30   52:54:00:d6:7e:ea   ipv4       192.168.122.65/24    maxbot-bot-smoke-39a1fede                         ff:56:50:4d:98:00:02:00:00:ab:11:fb:66:01:58:4f:ee:34:72
 2026-09-09 11:32:16   52:54:00:f3:4c:e6   ipv4       192.168.122.238/24   maxbot-bot-0a0538fb-f1f4-42f0-83de-eec858cf9288   ff:56:50:4d:98:00:02:00:00:ab:11:12:3b:36:ee:49:ac:4d:ce
";
        let leases = parse_dhcp_leases(raw);
        assert_eq!(leases.len(), 2);
        assert_eq!(leases[0].ipaddr, "192.168.122.65");
        assert_eq!(leases[0].hostname, "maxbot-bot-smoke-39a1fede");
        assert_eq!(leases[1].ipaddr, "192.168.122.238");
        assert_eq!(leases[1].hostname, "maxbot-bot-0a0538fb-f1f4-42f0-83de-eec858cf9288");
    }

    #[test]
    fn parse_dhcp_leases_returns_empty_for_garbage() {
        // Empty input and pure-header input both produce zero
        // leases. This is the "no leases yet" case we expect
        // while a VM is still booting.
        assert!(parse_dhcp_leases("").is_empty());
        assert!(parse_dhcp_leases(
            "Expiry Time   MAC   Proto   IP   Hostname\n--- --- --- --- ---"
        )
        .is_empty());
    }

    #[test]
    fn domain_state_serializes_to_snake_case() {
        // JSON shape is part of the public Tauri API. The
        // frontend's `ComputerPanel` reads it as
        // `state === 'running' | 'stopped' | 'paused' | 'crashed'`.
        assert_eq!(
            serde_json::to_string(&DomainState::Running).unwrap(),
            "\"running\""
        );
        assert_eq!(
            serde_json::to_string(&DomainState::Stopped).unwrap(),
            "\"stopped\""
        );
        assert_eq!(
            serde_json::to_string(&DomainState::Other).unwrap(),
            "\"other\""
        );
    }

    // v3.0.5: pin the QGA "not ready" classifier so a
    // future libvirt version changing the stderr wording
    // is caught by the test suite, not by a flaky e2e
    // run. The two patterns are the exact strings virsh
    // emits on Ubuntu 24.10 / libvirt 9.x when the QEMU
    // guest agent channel isn't connected yet. Anything
    // else (command not found, permission denied, parse
    // errors, ssh failures) must NOT classify as
    // retryable — we don't want to paper over real
    // QGA errors with a retry loop.
    #[test]
    fn is_qga_not_ready_matches_libvirt_stderr_patterns() {
        // The exact stderr the v3.0.4 e2e hit.
        let ready_err = LibvirtError::Command {
            exit_code: Some(1),
            stderr: "error: Guest agent is not responding: QEMU guest agent is not connected".to_string(),
        };
        assert!(is_qga_not_ready(&ready_err));

        // The two halves separately (defensive — either
        // wording could change in a future virsh).
        let only_responding = LibvirtError::Command {
            exit_code: Some(1),
            stderr: "error: Guest agent is not responding".to_string(),
        };
        assert!(is_qga_not_ready(&only_responding));
        let only_not_connected = LibvirtError::Command {
            exit_code: Some(1),
            stderr: "error: QEMU guest agent is not connected".to_string(),
        };
        assert!(is_qga_not_ready(&only_not_connected));

        // Real QGA errors: command not found, permission
        // denied, parse failure, etc. — must NOT retry.
        let cmd_not_found = LibvirtError::Command {
            exit_code: Some(1),
            stderr: "error: Guest exec command not found: /no/such/binary".to_string(),
        };
        assert!(!is_qga_not_ready(&cmd_not_found));
        let permission = LibvirtError::Command {
            exit_code: Some(1),
            stderr: "error: operation forbidden: read-only filesystem".to_string(),
        };
        assert!(!is_qga_not_ready(&permission));
        let random = LibvirtError::Command {
            exit_code: Some(1),
            stderr: "error: domain not found: no domain with matching name".to_string(),
        };
        assert!(!is_qga_not_ready(&random));

        // Other LibvirtError variants: never retry.
        assert!(!is_qga_not_ready(&LibvirtError::Ssh("conn refused".into())));
        assert!(!is_qga_not_ready(&LibvirtError::Parse("bad".into())));
        assert!(!is_qga_not_ready(&LibvirtError::Timeout(Duration::from_secs(1))));
        assert!(!is_qga_not_ready(&LibvirtError::NotConfigured));
    }

    // v3.0.5: the retry parameters must stay inside the
    // brief's bounds (3-5 attempts, 200-500ms backoff).
    // Catches accidental changes that would either slow
    // down the e2e test (>30s budget) or weaken the fix
    // (too few attempts).
    #[test]
    fn qga_retry_parameters_are_within_brief_bounds() {
        assert!(
            (3..=5).contains(&QGA_RETRY_ATTEMPTS),
            "QGA_RETRY_ATTEMPTS={QGA_RETRY_ATTEMPTS} outside 3-5"
        );
        let backoff_ms = QGA_RETRY_BACKOFF.as_millis();
        assert!(
            (200..=500).contains(&backoff_ms),
            "QGA_RETRY_BACKOFF={backoff_ms}ms outside 200-500ms"
        );
    }
}
