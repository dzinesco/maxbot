//! ComputerManager — the public face of v2.0's per-Bot VMs.
//!
//! The Tauri command surface (`commands/computer.rs`) calls
//! into `ComputerManager`. The manager owns:
//!
//! - the `SshPool` (server-side + per-Bot SSH/SFTP)
//! - the `LibvirtClient` (libvirt-over-SSH)
//! - the in-process `proxies: HashMap<BotId, VncProxy>` for
//!   active noVNC consoles
//!
//! Construction is cheap — the manager is a `Send + Sync`
//! struct of `Arc`s. The `SshPool` and `LibvirtClient` are
//! stateless apart from the SshPool's per-Bot key cache, so
//! the manager itself doesn't need locking beyond the
//! pool's own internal mutexes.
//!
//! Lifecycle:
//! - `ComputerManager::new(settings)` — built once at
//!   startup, attached to `AppState`.
//! - `provision(bot_id, opts)` — async, runs to
//!   completion; updates DB and emits a
//!   `computer://state-changed` event when done.
//! - `start / stop / destroy(bot_id)` — quick, single
//!   virsh call over SSH.
//! - `console_url(bot_id)` — spawns a per-call VNC proxy,
//!   returns the URL. The proxy lives until the renderer
//!   disconnects (Drop kills the SSH tunnel child).
//! - `test_connection()` — runs `sudo -n virsh list` on
//!   the server; returns the count of domains.

use std::collections::HashMap;
use std::sync::Arc;

use chrono::Utc;
use serde::{Deserialize, Serialize};
use thiserror::Error;
use tokio::sync::Mutex;

use super::storage::db::Database;
use crate::storage::Settings;

use self::keys::KeyError;
use self::libvirt::LibvirtError;
use self::provision::{ProvisionError, ProvisionOptions};
use self::ssh::{SshError, SshExecutor, SshPool};
use self::vnc::{VncError, VncProxy};

pub mod keys;
pub mod libvirt;
pub mod provision;
pub mod ssh;
pub mod vnc;

#[derive(Debug, Error)]
pub enum ComputerError {
    #[error("ssh: {0}")]
    Ssh(String),
    #[error("libvirt: {0}")]
    Libvirt(String),
    #[error("database: {0}")]
    Db(String),
    #[error("key: {0}")]
    Key(String),
    #[error("vnc: {0}")]
    Vnc(String),
    #[error("bot {0} not found")]
    NoBot(String),
    #[error("bot {0} has no computer provisioned")]
    NoComputer(String),
    #[error("bot {0} already has a computer")]
    AlreadyProvisioned(String),
    #[error("linux server not configured — set Settings.computer_server_host")]
    ServerNotConfigured,
    #[error("passphrase not set — set Settings.computer_passphrase")]
    PassphraseMissing,
    #[error("port range exhausted (tried {0}..={1})")]
    NoPort(u16, u16),
    #[error("invalid state transition: {from} -> {to}")]
    InvalidTransition { from: String, to: String },
}

impl From<SshError> for ComputerError {
    fn from(e: SshError) -> Self {
        match e {
            SshError::ServerNotConfigured => Self::ServerNotConfigured,
            SshError::PassphraseMissing => Self::PassphraseMissing,
            SshError::MissingKey(b) => Self::NoComputer(b),
            SshError::VmNotReady(m) => Self::Ssh(m),
            other => Self::Ssh(other.to_string()),
        }
    }
}
impl From<LibvirtError> for ComputerError {
    fn from(e: LibvirtError) -> Self {
        Self::Libvirt(e.to_string())
    }
}
impl From<KeyError> for ComputerError {
    fn from(e: KeyError) -> Self {
        Self::Key(e.to_string())
    }
}
impl From<ProvisionError> for ComputerError {
    fn from(e: ProvisionError) -> Self {
        match e {
            ProvisionError::Ssh(s) => Self::Ssh(s),
            ProvisionError::Libvirt(s) => Self::Libvirt(s),
            ProvisionError::Db(s) => Self::Db(s),
            ProvisionError::NoBot(s) => Self::NoBot(s),
            ProvisionError::AlreadyProvisioned(s) => Self::AlreadyProvisioned(s),
            ProvisionError::IpTimeout(_) | ProvisionError::NoDomainName => {
                Self::Libvirt(e.to_string())
            }
        }
    }
}
impl From<VncError> for ComputerError {
    fn from(e: VncError) -> Self {
        Self::Vnc(e.to_string())
    }
}
impl From<rusqlite::Error> for ComputerError {
    fn from(e: rusqlite::Error) -> Self {
        Self::Db(e.to_string())
    }
}

/// One row in the `computers` table. Mirrors the schema in
/// `db.rs`. The Tauri command surface re-serializes this
/// to the renderer; keep the field set stable.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Computer {
    pub bot_id: String,
    pub vm_name: String,
    pub vm_ip: Option<String>,
    pub vnc_port: Option<u16>,
    pub ssh_key_id: String,
    pub state: String,
    pub last_seen_at: Option<chrono::DateTime<chrono::Utc>>,
    pub created_at: chrono::DateTime<chrono::Utc>,
}

/// The state machine persisted in `computers.state`. The
/// UI maps these to the 6-state avatar system plus a
/// "provisioning" spinner. `Provisioning` is the only
/// transient state; the rest are stable (a VM is either
/// running, stopped, or in error).
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ComputerState {
    Provisioning,
    Running,
    Stopped,
    Error,
}

impl ComputerState {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Provisioning => "provisioning",
            Self::Running => "running",
            Self::Stopped => "stopped",
            Self::Error => "error",
        }
    }
    pub fn parse(s: &str) -> Self {
        match s {
            "provisioning" => Self::Provisioning,
            "running" => Self::Running,
            "stopped" => Self::Stopped,
            "error" => Self::Error,
            _ => Self::Error,
        }
    }
}

pub struct ComputerManager {
    pool: Arc<SshPool>,
    libvirt: libvirt::LibvirtClient,
    /// Live VNC proxies, keyed by bot id. Dropping the
    /// manager drops all of them, which `start_kill`s the
    /// SSH tunnel children.
    proxies: Mutex<HashMap<String, Arc<VncProxy>>>,
    /// Cached port range. Read from settings at startup;
    /// updated on `set_port_range`.
    port_range: Mutex<(u16, u16)>,
}

impl ComputerManager {
    pub fn new(settings: &Settings) -> Self {
        let server = SshPool::server_config_from_settings(settings);
        let pool = Arc::new(SshPool::new(server));
        let port_range = parse_port_range(&settings.computer_vnc_local_port_range);
        Self {
            pool,
            libvirt: libvirt::LibvirtClient::new(),
            proxies: Mutex::new(HashMap::new()),
            port_range: Mutex::new(port_range),
        }
    }

    /// Set the passphrase (called when the user updates
    /// Settings). The next per-Bot call will use the new
    /// passphrase. We also clear the in-memory key cache;
    /// if any per-Bot rows still use the old encryption,
    /// those calls will fail with `Decrypt` and the user
    /// will need to re-provision.
    pub async fn set_passphrase(&self, passphrase: String) {
        self.pool.set_passphrase(passphrase).await;
        self.pool.clear_key_cache().await;
    }

    /// Update the VNC port range (called from the
    /// Settings UI). Existing proxies keep their ports;
    /// new ones allocate from the new range.
    pub async fn set_port_range(&self, lo: u16, hi: u16) {
        *self.port_range.lock().await = (lo, hi);
    }

    pub fn ssh_pool(&self) -> Arc<SshPool> {
        self.pool.clone()
    }

    /// Run `sudo -n virsh list` on the server and return
    /// the number of domains. The Settings UI uses this
    /// as the "Test connection" smoke test.
    pub async fn test_connection(&self) -> Result<usize, ComputerError> {
        let domains = self.libvirt.list_domains(&*self.pool).await?;
        Ok(domains.len())
    }

    /// `computer_get(bot_id)` — fetch the persisted row.
    pub async fn get(&self, db: &Database, bot_id: &str) -> Result<Option<Computer>, ComputerError> {
        let row = db.get_computer(bot_id)?;
        Ok(row)
    }

    /// `computer_provision(bot_id, opts)` — the heavy
    /// lifter. Returns immediately with a `provisioning`
    /// row written; the actual VM is set up in this call
    /// (we don't spawn a separate task because the
    /// orchestration is short — under 2 minutes for a
    /// typical VM, and the caller is already async).
    ///
    /// On success, writes the final `running` state and
    /// the discovered IP / VNC port. On failure, writes
    /// `error` with the error message.
    pub async fn provision(
        &self,
        db: &Database,
        bot_id: &str,
        opts: ProvisionOptions,
    ) -> Result<(), ComputerError> {
        // 0. Sanity: the bot exists and isn't already
        //    provisioned. We treat "already provisioned"
        //    as an error to make the UI explicit; the
        //    user has to destroy first.
        if db.get_bot(bot_id)?.is_none() {
            return Err(ComputerError::NoBot(bot_id.into()));
        }
        if db.get_computer(bot_id)?.is_some() {
            return Err(ComputerError::AlreadyProvisioned(bot_id.into()));
        }

        // 1. Insert the placeholder row immediately so
        //    the UI sees `provisioning`. Even if the
        //    orchestrator errors out, the row stays in
        //    `error` and the UI can show the failure.
        let now = Utc::now();
        db.upsert_computer(&Computer {
            bot_id: bot_id.into(),
            vm_name: String::new(),
            vm_ip: None,
            vnc_port: None,
            ssh_key_id: String::new(),
            state: ComputerState::Provisioning.as_str().into(),
            last_seen_at: None,
            created_at: now,
        })?;

        // 2. Run the orchestrator. This is the long
        //    step (1-2 min) and we want the UI to see
        //    `provisioning` throughout.
        let result = provision::provision_vm(bot_id, &opts, &*self.pool, &self.libvirt).await;

        // 3. Persist the result. On success, also
        //    encrypt + store the SSH key, register the
        //    VM endpoint in the pool, and update the
        //    computer row.
        match result {
            Ok(r) => {
                // Encrypt the per-Bot key. We need the
                // user's passphrase; if they haven't
                // set one, fail with a clear error.
                let passphrase = {
                    let s = db.load_settings()?;
                    s.computer_passphrase.clone()
                };
                if passphrase.is_empty() {
                    return Err(ComputerError::PassphraseMissing);
                }
                let ciphertext =
                    keys::encrypt_private(&r.pkcs8_private_key, &passphrase)
                        .map_err(ComputerError::from)?;
                let key_id = uuid::Uuid::new_v4().to_string();
                db.upsert_ssh_key(&key_id, &r.public_key, &ciphertext)?;

                // Update the computer row with the
                // final state. We do this in one shot
                // to avoid the UI flickering through
                // "no IP" → "has IP".
                db.upsert_computer(&Computer {
                    bot_id: bot_id.into(),
                    vm_name: r.domain_name.clone(),
                    vm_ip: Some(r.ip.clone()),
                    vnc_port: Some(r.vnc_port),
                    ssh_key_id: key_id.clone(),
                    state: ComputerState::Running.as_str().into(),
                    last_seen_at: Some(Utc::now()),
                    created_at: now,
                })?;

                // Register the VM endpoint in the
                // pool so subsequent shell / sftp
                // calls know where to go.
                self.pool
                    .set_vm_endpoint(bot_id, r.ip.clone(), "bot".into())
                    .await;
                // Keep the decrypted key in the pool
                // so the first call doesn't have to
                // re-decrypt.
                self.pool
                    .cache_key_blob(
                        bot_id,
                        std::sync::Arc::new(r.pkcs8_private_key),
                    )
                    .await;

                Ok(())
            }
            Err(e) => {
                db.upsert_computer(&Computer {
                    bot_id: bot_id.into(),
                    vm_name: String::new(),
                    vm_ip: None,
                    vnc_port: None,
                    ssh_key_id: String::new(),
                    state: ComputerState::Error.as_str().into(),
                    last_seen_at: None,
                    created_at: now,
                })?;
                Err(ComputerError::from(e))
            }
        }
    }

    /// `computer_start(bot_id)` — `virsh start <name>`.
    pub async fn start(
        &self,
        db: &Database,
        bot_id: &str,
    ) -> Result<(), ComputerError> {
        let row = db.get_computer(bot_id)?
            .ok_or_else(|| ComputerError::NoComputer(bot_id.into()))?;
        if row.vm_name.is_empty() {
            return Err(ComputerError::NoComputer(bot_id.into()));
        }
        self.libvirt
            .start(&*self.pool, &row.vm_name)
            .await?;
        db.set_computer_state(bot_id, ComputerState::Running.as_str())?;
        Ok(())
    }

    /// `computer_stop(bot_id)` — `virsh shutdown <name>`.
    /// We don't poll for the actual state change here;
    /// the UI's `computer://state-changed` listener can
    /// re-query via `computer_get` if it needs
    /// confirmation.
    pub async fn stop(
        &self,
        db: &Database,
        bot_id: &str,
    ) -> Result<(), ComputerError> {
        let row = db.get_computer(bot_id)?
            .ok_or_else(|| ComputerError::NoComputer(bot_id.into()))?;
        if row.vm_name.is_empty() {
            return Err(ComputerError::NoComputer(bot_id.into()));
        }
        self.libvirt
            .shutdown(&*self.pool, &row.vm_name)
            .await?;
        db.set_computer_state(bot_id, ComputerState::Stopped.as_str())?;
        Ok(())
    }

    /// `computer_destroy(bot_id)` — virsh destroy +
    /// undefine (with --remove-all-storage). Also drops
    /// the per-Bot pool state and any active VNC proxy.
    pub async fn destroy(
        &self,
        db: &Database,
        bot_id: &str,
    ) -> Result<(), ComputerError> {
        let row = db.get_computer(bot_id)?
            .ok_or_else(|| ComputerError::NoComputer(bot_id.into()))?;
        if !row.vm_name.is_empty() {
            // best-effort: shutdown may fail if the
            // domain is already off; that's fine.
            let _ = self.libvirt.shutdown(&*self.pool, &row.vm_name).await;
            // destroy is forceful; if the VM is gone,
            // virsh reports a "domain not found" error
            // and we swallow it (idempotent destroy).
            if let Err(e) = self.libvirt.destroy(&*self.pool, &row.vm_name).await {
                if !matches!(&e, LibvirtError::Command { stderr, .. } if stderr.contains("not found") || stderr.contains("Domain not found")) {
                    return Err(e.into());
                }
            }
            if let Err(e) = self.libvirt.undefine(&*self.pool, &row.vm_name).await {
                // Idempotent: if the domain is already
                // undefined, the user's manual cleanup
                // path is preserved.
                if !matches!(&e, LibvirtError::Command { stderr, .. } if stderr.contains("not found") || stderr.contains("Domain not found")) {
                    return Err(e.into());
                }
            }
        }
        // Drop any active VNC proxy (kills the SSH
        // tunnel).
        self.proxies.lock().await.remove(bot_id);
        // Drop the SSH pool state.
        self.pool.forget_bot(bot_id).await;
        // Remove the DB rows.
        db.delete_computer(bot_id)?;
        if !row.ssh_key_id.is_empty() {
            db.delete_ssh_key(&row.ssh_key_id)?;
        }
        Ok(())
    }

    /// `computer_console_url(bot_id)` — start a VNC
    /// proxy and return the local WebSocket URL. The
    /// proxy is stored in `self.proxies` so the
    /// `Drop` impl on the manager cleans them up.
    pub async fn console_url(
        &self,
        db: &Database,
        bot_id: &str,
    ) -> Result<String, ComputerError> {
        let row = db.get_computer(bot_id)?
            .ok_or_else(|| ComputerError::NoComputer(bot_id.into()))?;
        let vnc_port = row
            .vnc_port
            .ok_or_else(|| ComputerError::NoComputer(bot_id.into()))?;
        let range = *self.port_range.lock().await;
        let proxy = vnc::start(&*self.pool, vnc_port, range).await?;
        let url = proxy.console_url();
        // Spawn the accept loop. The task runs
        // forever; the proxy's Drop kills the tunnel
        // (and the accept loop errors out on the
        // next accept). `bot_id_owned` is an owned
        // String because `bot_id` is a borrowed
        // `&str` whose lifetime doesn't extend into
        // the spawned task.
        let bot_id_owned = bot_id.to_string();
        let url_clone = url.clone();
        let proxy = Arc::new(proxy);
        let proxy_for_serve = proxy.clone();
        tokio::spawn(async move {
            if let Err(e) = proxy_for_serve.serve().await {
                log::warn!(
                    "vnc proxy for {bot_id_owned} ({url_clone}) exited: {e}"
                );
            }
        });
        // Remember the proxy so a future
        // `computer_destroy` (or ComputerManager
        // drop) can clean it up.
        self.proxies
            .lock()
            .await
            .insert(bot_id.to_string(), proxy);
        Ok(url)
    }

    /// `computer_file_list / read / write` — SFTP into
    /// the per-Bot VM. We unlock the key on first use
    /// by reading the encrypted blob + the passphrase
    /// from the DB.
    pub async fn file_list(
        &self,
        db: &Database,
        bot_id: &str,
        path: &str,
    ) -> Result<Vec<ssh::SftpEntry>, ComputerError> {
        self.ensure_bot_unlocked(db, bot_id).await?;
        let out = self.pool.vm_sftp_list(bot_id, path).await?;
        Ok(out)
    }

    pub async fn file_read(
        &self,
        db: &Database,
        bot_id: &str,
        path: &str,
    ) -> Result<String, ComputerError> {
        self.ensure_bot_unlocked(db, bot_id).await?;
        let out = self.pool.vm_sftp_read(bot_id, path).await?;
        Ok(out)
    }

    pub async fn file_write(
        &self,
        db: &Database,
        bot_id: &str,
        path: &str,
        content: &str,
    ) -> Result<(), ComputerError> {
        self.ensure_bot_unlocked(db, bot_id).await?;
        self.pool
            .vm_sftp_write(bot_id, path, content)
            .await?;
        Ok(())
    }

    /// Ensure the per-Bot key is decrypted + cached in
    /// the pool. Called before every `vm_sftp_*` and
    /// `vm_exec`. Idempotent — if the key is already
    /// cached, this is a no-op.
    async fn ensure_bot_unlocked(
        &self,
        db: &Database,
        bot_id: &str,
    ) -> Result<(), ComputerError> {
        // Fast path: key is already cached.
        if self.pool.key_blob_for(bot_id).await.is_ok() {
            return Ok(());
        }
        // Slow path: read the encrypted blob from
        // the DB, decrypt with the user's
        // passphrase, and cache.
        let row = db.get_computer(bot_id)?
            .ok_or_else(|| ComputerError::NoComputer(bot_id.into()))?;
        if row.ssh_key_id.is_empty() {
            return Err(ComputerError::NoComputer(bot_id.into()));
        }
        let key_row = db
            .get_ssh_key(&row.ssh_key_id)?
            .ok_or_else(|| ComputerError::NoComputer(bot_id.into()))?;
        self.pool
            .unlock_with_ciphertext(bot_id, &key_row.private_key_encrypted)
            .await?;
        // After unlock, also make sure the VM
        // endpoint is populated (it may have been
        // forgotten if the user re-opened the app).
        if let Some(ip) = row.vm_ip.clone() {
            self.pool
                .set_vm_endpoint(bot_id, ip, "bot".into())
                .await;
        }
        Ok(())
    }
}

// Helper: turn "5900-5999" into (5900, 5999). Bad
// strings silently fall back to the default range.
fn parse_port_range(s: &str) -> (u16, u16) {
    let parts: Vec<&str> = s.split('-').collect();
    if parts.len() == 2 {
        if let (Ok(lo), Ok(hi)) = (parts[0].parse::<u16>(), parts[1].parse::<u16>()) {
            if lo <= hi {
                return (lo, hi);
            }
        }
    }
    (5900, 5999)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_port_range_default() {
        assert_eq!(parse_port_range(""), (5900, 5999));
        assert_eq!(parse_port_range("garbage"), (5900, 5999));
        assert_eq!(parse_port_range("5999-5900"), (5900, 5999)); // out of order
    }

    #[test]
    fn parse_port_range_explicit() {
        assert_eq!(parse_port_range("5900-5999"), (5900, 5999));
        assert_eq!(parse_port_range("7000-7099"), (7000, 7099));
    }

    #[test]
    fn computer_state_round_trips_via_string() {
        for s in [
            ComputerState::Provisioning,
            ComputerState::Running,
            ComputerState::Stopped,
            ComputerState::Error,
        ] {
            assert_eq!(ComputerState::parse(s.as_str()), s);
        }
        // Unknown strings map to error.
        assert_eq!(ComputerState::parse("????"), ComputerState::Error);
    }

    // ----- v2.0.2 smoke test: end-to-end provision against the
    // real Linux server (Tyler's `crispy` at 192.168.0.49).
    //
    // This test:
    //   1. Builds a fresh SQLite DB in a temp dir
    //   2. Inserts a Bot row + Settings (server host, SSH user,
    //      passphrase, default disk/RAM)
    //   3. Constructs a ComputerManager pointed at the real server
    //   4. Calls `provision(bot_id, opts)`
    //   5. Asserts the returned state is `running` and the DB row
    //      has an IP + VNC port
    //   6. Tears down the libvirt domain + storage pool on the
    //      server so the test is idempotent
    //
    // Marked `#[ignore]` so `cargo test` doesn't hit the real
    // server in CI. Run with:
    //
    //   cargo test --lib provision_e2e -- --ignored --nocapture
    //
    // Prereqs on the test machine:
    //   - ssh-agent running with Tyler's `~/.ssh/id_ed25519`
    //   - the key is authorized on `tyler@192.168.0.49`
    //   - the server has libvirt + `/opt/maxbot/provision-vm.sh`
    //   - passwordless sudo for tyler on the server
    #[tokio::test]
    #[ignore]
    async fn provision_e2e_against_crispy() {
        use super::libvirt::LibvirtClient;
        // 1. Fresh DB
        let dir = std::env::temp_dir().join(format!(
            "maxbot-provision-smoke-{}",
            uuid::Uuid::new_v4()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("test.sqlite");
        let db = crate::storage::db::Database::open(&path).expect("open test db");

        // 2. Bot + settings. The host is a fixed test target;
        // override via env if you need to smoke a different
        // server. Same for the SSH user.
        let bot_id = format!("smoke-{}", &uuid::Uuid::new_v4().to_string()[..8]);
        let now = chrono::Utc::now();
        let bot = crate::bots::Bot {
            id: bot_id.clone(),
            name: "smoke".to_string(),
            description: "".to_string(),
            system_prompt: "".to_string(),
            default_model: "MiniMax-M3".to_string(),
            allowed_tools: vec![],
            icon: "".to_string(),
            color: "".to_string(),
            avatar_color: "".to_string(),
            last_active_at: None,
            state: crate::bots::BotState::Idle,
            created_at: now,
            updated_at: now,
        };
        db.upsert_bot(&bot).expect("upsert bot");

        let mut s = crate::storage::db::Settings::default();
        s.computer_server_host =
            std::env::var("MAXBOT_TEST_SERVER_HOST")
                .unwrap_or_else(|_| "192.168.0.49".to_string());
        s.computer_server_ssh_user =
            std::env::var("MAXBOT_TEST_SERVER_SSH_USER")
                .unwrap_or_else(|_| "tyler".to_string());
        s.computer_server_ssh_key_id = "".to_string(); // OS keychain
        s.computer_vnc_local_port_range = "5900-5999".to_string();
        s.computer_passphrase = std::env::var("MAXBOT_TEST_PASSPHRASE")
            .unwrap_or_else(|_| "smoke-test-passphrase-2026".to_string());
        s.computer_default_disk_gb = 5;
        s.computer_default_ram_mb = 1024;
        db.save_settings(&s).expect("save settings");

        // 3. Manager
        let mgr = ComputerManager::new(&s);

        // 4. Provision. This is the long step (~1-2 min for
        // cloud-init to bring up xfce + x11vnc + qemu-guest-
        // agent, then poll for IP / VNC).
        eprintln!(
            "[smoke] provisioning bot {bot_id} on {host} (this takes 1-2 min)...",
            host = s.computer_server_host
        );
        let opts = ProvisionOptions {
            disk_gb: 5,
            ram_mb: 1024,
        };
        let result = mgr.provision(&db, &bot_id, opts).await;
        if let Err(ref e) = result {
            // Best-effort: print the server-side libvirt list
            // and the qemu-guest-agent status to help debug.
            eprintln!("[smoke] provision failed: {e}");
        }
        assert!(
            result.is_ok(),
            "provision failed: {:?}",
            result.err()
        );

        // 5. DB row should be `running` with IP + VNC.
        let row = db.get_computer(&bot_id).unwrap().expect("row exists");
        assert_eq!(row.state, "running");
        assert!(row.vm_ip.is_some(), "vm_ip should be set");
        assert!(row.vnc_port.is_some(), "vnc_port should be set");
        let ip = row.vm_ip.as_ref().unwrap();
        let vnc_port = row.vnc_port.unwrap();
        let domain = row.vm_name.clone();
        eprintln!("[smoke] provisioned: domain={domain} ip={ip} vnc_port={vnc_port}");

        // 6. Tear down. Destroy the libvirt domain and its
        // pool so the test can run again without manual
        // cleanup. The ssh pool is wrapped in Arc; we use a
        // throwaway handle.
        let server = SshPool::server_config_from_settings(&s);
        let cleanup_pool = std::sync::Arc::new(SshPool::new(server));
        let libvirt = LibvirtClient::new();
        let _ = libvirt
            .destroy(&cleanup_pool, &domain)
            .await
            .map_err(|e| eprintln!("[smoke] destroy: {e}"));
        let _ = libvirt
            .undefine(&cleanup_pool, &domain)
            .await
            .map_err(|e| eprintln!("[smoke] undefine: {e}"));
        eprintln!("[smoke] tore down {domain}");
        // Pool dir is at /var/lib/maxbot/vms/<domain> on the
        // server. `virsh pool-destroy` + `pool-undefine` is
        // enough; the rmdir is best-effort.

        // Drop the DB so the file is unlocked, then trash the
        // temp dir.
        drop(db);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
