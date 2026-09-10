//! ComputerManager — the public face of v2.0's per-Bot VMs.
//!
//! The Tauri command surface (`commands/computer.rs`) calls
//! into `ComputerManager`. The manager owns:
//!
//! - the `SshPool` (server-side + per-Bot SSH/SFTP)
//! - the `LibvirtClient` (libvirt-over-SSH)
//! - the in-process `takeover_tunnels: HashMap<BotId, TakeoverHandle>`
//!   for live "Take over with Screen Sharing" SSH tunnels
//!
//! v3.7.2: the in-app preview is now a host-side QEMU
//! framebuffer poll (see `screenshot::capture_jpeg`). The
//! noVNC↔RFB WebSocket bridge that powered v3.0.x's
//! `console_url` is gone — the Tauri webview (WKWebView)
//! didn't render noVNC's canvas path reliably, and the
//! screenshot poll sidesteps that entirely. The takeover
//! tunnel is the only VNC-related state the manager holds.
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
//! - `screenshot(bot_id)` — host-side QEMU framebuffer
//!   grab; returns JPEG bytes for the in-app preview.
//! - `takeover_open(bot_id)` — opens an `ssh -L` tunnel
//!   from a local port in the configured VNC range to
//!   the VM's VNC port. Returns the local port. The
//!   renderer opens macOS `Screen Sharing` against it.
//! - `takeover_close(bot_id)` — kills the SSH tunnel
//!   child. macOS `Screen Sharing` will fail to reconnect
//!   after the port is freed.
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
use self::screenshot::ScreenshotError;
use self::ssh::{SshError, SshExecutor, SshPool};
use self::vnc::{TakeoverHandle, VncError};

pub mod input;
pub mod keys;
pub mod libvirt;
pub mod provision;
pub mod screenshot;
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
    #[error("screenshot: {0}")]
    Screenshot(String),
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
    #[error("qemu guest agent on {vm_name} did not respond within {waited:?} (last stderr: {last_stderr})")]
    QgaTimeout {
        vm_name: String,
        waited: std::time::Duration,
        last_stderr: String,
    },
    /// v3.7.2 (amended): the libvirt domain is missing on
    /// the host. This is distinct from `Libvirt` (which
    /// surfaces raw stderr): a `DomainNotFound` is a
    /// *recoverable* state — the Bot exists in the
    /// MaxBot SQLite, but its VM was never provisioned,
    /// was destroyed, or lives on a different host. The
    /// renderer should surface a "VM not provisioned"
    /// state with a Provision button rather than the raw
    /// libvirt error. Constructed explicitly by
    /// `screenshot::capture_jpeg` after matching the
    /// `failed to get domain` stderr pattern; no automatic
    /// `From` conversion (network/SSH failures should
    /// stay as `Ssh` / `Libvirt`).
    #[error("computer: domain '{vm_name}' not found on host")]
    DomainNotFound { vm_name: String },
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
impl From<ScreenshotError> for ComputerError {
    fn from(e: ScreenshotError) -> Self {
        // v3.7.2 (amended): the `DomainNotFound` variant
        // carries the `vm_name` and is the explicit
        // construction site for `ComputerError::DomainNotFound`.
        // We preserve the `vm_name` so the renderer can
        // show "VM not provisioned for maxbot-bot-1" +
        // a Provision button. Everything else falls
        // through to the generic `Screenshot` variant.
        match e {
            ScreenshotError::DomainNotFound { vm_name } => {
                Self::DomainNotFound { vm_name }
            }
            other => Self::Screenshot(other.to_string()),
        }
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
}

pub struct ComputerManager {
    pool: Arc<SshPool>,
    libvirt: libvirt::LibvirtClient,
    /// Live "Take over with Screen Sharing" SSH tunnels,
    /// keyed by bot id. The renderer calls
    /// `takeover_open(bot_id)` to start one and
    /// `takeover_close(bot_id)` to kill it. Dropping the
    /// manager drops all of them, which `start_kill`s the
    /// SSH tunnel children and frees the local ports.
    ///
    /// v3.7.2: replaces the v3.0.x `proxies: HashMap<BotId, VncProxy>`
    /// map. The VncProxy struct owned both the SSH tunnel
    /// and a noVNC-bridging WebSocket listener; we deleted
    /// the latter, so the only thing left is the tunnel
    /// child + the local port.
    takeover_tunnels: Mutex<HashMap<String, Arc<TakeoverHandle>>>,
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
            takeover_tunnels: Mutex::new(HashMap::new()),
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
                // Encrypt the per-Bot key. When the
                // v2.3.5 default-key flag is on, the
                // per-Bot key is never read on the hot
                // path — ssh falls back to the user's
                // default key. We still want to encrypt
                // + store it (so the user can toggle
                // back to the per-Bot path without
                // re-provisioning), but the "passphrase"
                // becomes a deterministic placeholder.
                // This is safe because (a) the data is
                // never decrypted on the hot path and
                // (b) the local DB is per-user protected
                // by macOS file-protection class.
                //
                // When the flag is off, fall back to the
                // user-set passphrase (v2.3.4 behavior).
                // Empty passphrase + flag off = the user
                // hasn't configured the per-Bot path
                // yet, so we error with the same
                // v2.3.4 `PassphraseMissing`.
                let settings = db.load_settings()?;
                let passphrase = resolve_provision_passphrase(&settings)
                    .ok_or(ComputerError::PassphraseMissing)?;
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
        // Drop any active takeover tunnel (kills the
        // SSH child and frees the local port).
        self.takeover_tunnels.lock().await.remove(bot_id);
        // Drop the SSH pool state.
        self.pool.forget_bot(bot_id).await;
        // Remove the DB rows.
        db.delete_computer(bot_id)?;
        if !row.ssh_key_id.is_empty() {
            db.delete_ssh_key(&row.ssh_key_id)?;
        }
        Ok(())
    }

    /// `computer_screenshot(bot_id)` — host-side QEMU
    /// framebuffer grab. Returns the encoded JPEG
    /// bytes, ready to be wrapped in a `Blob` and
    /// rendered as an `<img>`.
    ///
    /// v3.7.2: replaces the v3.0.x `console_url` path.
    /// The old path mounted a noVNC client in the Tauri
    /// webview; the new path renders a fresh JPEG every
    /// ~300ms via the `screenshot::capture_jpeg` helper.
    /// No QGA gate, no SSH into the guest, no
    /// webview-side noVNC. Just `virsh screenshot` over
    /// the server's SSH connection.
    ///
    /// Gated on `computers.state == "running"`. We do
    /// NOT gate on QGA — `virsh screenshot` is the
    /// QEMU virtual VGA capture, it works during
    /// cloud-init (the guest can still be installing
    /// LightDM and the preview will update through the
    /// boot).
    pub async fn screenshot(
        &self,
        db: &Database,
        bot_id: &str,
    ) -> Result<Vec<u8>, ComputerError> {
        let row = db
            .get_computer(bot_id)?
            .ok_or_else(|| ComputerError::NoComputer(bot_id.into()))?;
        if row.vm_name.is_empty() {
            return Err(ComputerError::NoComputer(bot_id.into()));
        }
        // We rely on `computers.state` for the running
        // guard (don't talk to QGA — it isn't ready
        // during cloud-init, which is exactly the boot
        // phase we want to show).
        let bytes = screenshot::capture_jpeg(
            &*self.pool,
            &row.vm_name,
            &row.state,
        )
        .await?;
        Ok(bytes)
    }

    /// `computer_takeover_open(bot_id)` — open an
    /// `ssh -L` tunnel from a local port in the
    /// configured VNC range to the VM's VNC port. The
    /// handle is stored in `self.takeover_tunnels` so a
    /// later `takeover_close` (or ComputerManager
    /// drop) can kill the SSH child.
    ///
    /// Returns the local loopback port — the renderer
    /// hands this to `open vnc://127.0.0.1:<port>` to
    /// launch macOS `Screen Sharing`.
    ///
    /// v3.7.2: replaces the v3.0.x `console_url`
    /// path. The local port is allocated from the
    /// same `computer_vnc_local_port_range` setting
    /// the v3.0.x path used, so two Bots don't
    /// collide on a hardcoded `:5901`.
    pub async fn takeover_open(
        &self,
        db: &Database,
        bot_id: &str,
    ) -> Result<u16, ComputerError> {
        let row = db
            .get_computer(bot_id)?
            .ok_or_else(|| ComputerError::NoComputer(bot_id.into()))?;
        let vnc_port = row
            .vnc_port
            .ok_or_else(|| ComputerError::NoComputer(bot_id.into()))?;
        // If a tunnel is already open for this bot,
        // return its port (idempotent) rather than
        // opening a second one. The renderer's
        // `open vnc://` is also idempotent.
        if let Some(existing) = self.takeover_tunnels.lock().await.get(bot_id) {
            return Ok(existing.local_port);
        }
        let range = *self.port_range.lock().await;
        let handle = match vnc::open_takeover(&*self.pool, vnc_port, range).await {
            Ok(h) => h,
            Err(VncError::TunnelAuthFailed(stderr)) => {
                // Mirror the v3.0.3 silent auto-recover
                // path. The user's default pubkey may
                // not be in the VM's `authorized_keys`
                // yet — install it via QGA and retry.
                log::info!(
                    "takeover tunnel auth failed, attempting default key install for {bot_id}: {stderr}"
                );
                if let Err(install_err) = self
                    .install_default_key_via_qga(db, bot_id)
                    .await
                {
                    return Err(ComputerError::Vnc(format!(
                        "tunnel auth failed and default key install failed: {install_err} \
                         — check that ~/.ssh/id_ed25519.pub (or id_rsa.pub / id_ecdsa.pub) exists"
                    )));
                }
                match vnc::open_takeover(&*self.pool, vnc_port, range).await {
                    Ok(h) => h,
                    Err(VncError::TunnelAuthFailed(stderr2)) => {
                        return Err(ComputerError::Vnc(format!(
                            "tunnel auth failed even after default key install: {stderr2} \
                             — verify the VM has accepted the new key (try 'ssh bot@<vm-ip>' from your shell)"
                        )));
                    }
                    Err(other_vnc_err) => {
                        return Err(ComputerError::Vnc(other_vnc_err.to_string()));
                    }
                }
            }
            Err(e) => return Err(ComputerError::from(e)),
        };
        let local_port = handle.local_port;
        self.takeover_tunnels
            .lock()
            .await
            .insert(bot_id.to_string(), Arc::new(handle));
        Ok(local_port)
    }

    /// `computer_takeover_close(bot_id)` — kill the
    /// SSH tunnel child and free the local port.
    /// macOS `Screen Sharing` will lose its connection
    /// the next time it polls. Idempotent: returns
    /// `Ok(())` whether or not a tunnel was open.
    pub async fn takeover_close(
        &self,
        bot_id: &str,
    ) -> Result<(), ComputerError> {
        self.takeover_tunnels.lock().await.remove(bot_id);
        Ok(())
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

    /// v2.3.5: bootstrap-install the user's default SSH
    /// public key into the VM's `authorized_keys` so an
    /// existing VM (provisioned with a per-Bot key) can
    /// switch to the default-key path without Destroy.
    ///
    /// v3.0.3: this is now a thin wrapper around
    /// `install_default_key_via_qga`. The Tauri command
    /// surface (the "Use my default key" toolbar button)
    /// still calls this; the auto-recover path inside
    /// `takeover_open` calls the helper directly.
    /// v3.7.2: `console_url` was replaced by
    /// `takeover_open` + `takeover_close`; the auto-
    /// recover path moved with it.
    pub async fn install_default_key(
        &self,
        db: &Database,
        bot_id: &str,
    ) -> Result<String, ComputerError> {
        self.install_default_key_via_qga(db, bot_id).await
    }

    /// v3.0.3: the QGA-based key-install logic, extracted
    /// so `takeover_open` can call it on
    /// `TunnelAuthFailed` without going through the
    /// public Tauri command.
    /// We send the install via the QEMU guest agent so we
    /// don't need SSH access — the chicken-and-egg case
    /// for a user whose passphrase is empty. The QGA
    /// command is idempotent (`grep -qxF` short-circuits
    /// when the key line already exists), so multiple
    /// calls are safe.
    ///
    /// Returns the QGA's JSON response on success.
    pub(crate) async fn install_default_key_via_qga(
        &self,
        db: &Database,
        bot_id: &str,
    ) -> Result<String, ComputerError> {
        // 1. Find the public key on the Mac. Prefer ed25519,
        //    fall back to rsa / ecdsa. The OS keychain is
        //    not used here — we read the file directly so
        //    the same key the user uses in their terminal
        //    gets installed on the VM.
        let home = std::env::var_os("HOME")
            .ok_or_else(|| ComputerError::Ssh("HOME not set".into()))?;
        let home = std::path::PathBuf::from(home);
        let pub_path = ["id_ed25519.pub", "id_rsa.pub", "id_ecdsa.pub"]
            .iter()
            .map(|name| home.join(".ssh").join(name))
            .find(|p| p.exists())
            .ok_or_else(|| {
                ComputerError::Ssh(
                    "no default SSH public key found (looked for id_ed25519.pub, id_rsa.pub, id_ecdsa.pub in ~/.ssh/)".into(),
                )
            })?;
        let pub_key = std::fs::read_to_string(&pub_path).map_err(|e| {
            ComputerError::Ssh(format!("reading {}: {e}", pub_path.display()))
        })?;
        let pub_key = pub_key.trim();
        if pub_key.is_empty() {
            return Err(ComputerError::Ssh(format!(
                "{} is empty",
                pub_path.display()
            )));
        }

        // 2. Look up the VM domain name from the computers
        //    table so we know which qemu-agent-command to
        //    address.
        let computer = db
            .get_computer(bot_id)?
            .ok_or_else(|| ComputerError::NoComputer(bot_id.into()))?;
        if computer.vm_name.is_empty() {
            return Err(ComputerError::NoComputer(bot_id.into()));
        }

        // 3. Build the QGA guest-exec command. The shell
        //    pipeline is idempotent: `grep -qxF` returns
        //    true when the key line is already present, so
        //    the `|| echo … >>` only appends on the first
        //    run.
        let script = format!(
            "mkdir -p /home/bot/.ssh && \
             chmod 700 /home/bot/.ssh && \
             grep -qxF \"{pub_key}\" /home/bot/.ssh/authorized_keys || \
             echo \"{pub_key}\" >> /home/bot/.ssh/authorized_keys ; \
             chmod 600 /home/bot/.ssh/authorized_keys ; \
             chown -R bot:bot /home/bot/.ssh"
        );
        let cmd_json = serde_json::json!({
            "execute": "guest-exec",
            "arguments": {
                "path": "/bin/sh",
                "arg": ["-c", script],
            }
        })
        .to_string();
        let response = self
            .libvirt
            .qemu_agent_command(&*self.pool, &computer.vm_name, &cmd_json)
            .await?;
        Ok(response)
    }

    /// Ensure the per-Bot key is decrypted + cached in
    /// the pool. Called before every `vm_sftp_*` and
    /// `vm_exec`. Idempotent — if the key is already
    /// cached, this is a no-op.
    ///
    /// v2.3.5: when `computer_use_default_ssh_key` is on,
    /// the SSH path uses the user's default key so we
    /// skip the per-Bot key fetch entirely. The VM endpoint
    /// is still populated (so `vm_sftp_*` and `vm_exec`
    /// can route to the right IP), but we never read
    /// `ssh_keys` on the hot path. Legacy users with the
    /// flag off keep the original behavior.
    async fn ensure_bot_unlocked(
        &self,
        db: &Database,
        bot_id: &str,
    ) -> Result<(), ComputerError> {
        // v2.3.5: when the default-key flag is on, skip the
        // per-Bot key fetch. We still need the VM endpoint
        // cached so subsequent `vm_sftp_*` calls route to
        // the right IP, but the per-Bot key is never read.
        let settings = db.load_settings()?;
        if settings.computer_use_default_ssh_key {
            if let Ok(row) = db.get_computer(bot_id) {
                if let Some(c) = row {
                    if let Some(ip) = c.vm_ip.clone() {
                        self.pool
                            .set_vm_endpoint(bot_id, ip, "bot".into())
                            .await;
                    }
                }
            }
            return Ok(());
        }
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

/// v3.0.5: wait for the QEMU guest agent (QGA) on a
/// freshly-provisioned VM to become responsive. The
/// `provision_vm` orchestrator returns once the VM is
/// `running` + has a DHCP lease, but cloud-init's
/// `packages:` block (xfce4, x11vnc, qemu-guest-agent,
/// openssh-server) and `runcmd:` `systemctl enable --now
/// qemu-guest-agent` finish well after that. Without this
/// wait, any QGA call made immediately after provision
/// (e.g. the v3.0.3 console auto-recover's
/// `install_default_key_via_qga`) hits a `Guest agent is
/// not responding` stderr from virsh. The 5-attempt retry
/// at the `qemu_agent_command` layer covers the brief
/// socket-not-ready window after QGA is "ready"; this
/// helper covers the much longer QGA-install window.
///
/// We poll with `virsh qemu-agent-command <name>
/// '{"execute":"guest-ping"}'` every 3 seconds up to 5
/// minutes. `guest-ping` is the cheapest QGA call (no
/// side effects, just a round-trip). The 5-minute ceiling
/// matches the typical worst-case cloud-init
/// packages+runcmd time on the Ubuntu 24.04 noble cloud
/// image used by `provision-vm.sh`; if it ever trips, the
/// VM's cloud-init is genuinely broken, and the
/// `ComputerError::QgaTimeout` surfaces that to the UI.
///
/// Reusable: the future console-recover path can call this
/// before its first QGA call to avoid the retry loop
/// entirely on warm-cache provisions.
pub(crate) async fn wait_for_qga_ready(
    pool: &SshPool,
    vm_name: &str,
) -> Result<(), ComputerError> {
    use std::time::{Duration, Instant};

    const POLL_INTERVAL: Duration = Duration::from_secs(3);
    const TIMEOUT: Duration = Duration::from_secs(300); // 5 minutes

    // `guest-ping` is the standard QGA liveness check.
    // It returns `{"return":{}}` on success. The shell
    // quoting is identical to `LibvirtClient::qemu_agent_command`:
    // pass the JSON body as a single quoted arg so the
    // server-side shell doesn't try to parse it.
    let cmd = format!(
        "sudo -n virsh qemu-agent-command {} '{{\"execute\":\"guest-ping\"}}'",
        shell_quote(vm_name)
    );

    let start = Instant::now();
    // Tracked across loop iterations so the QgaTimeout
    // error carries the most recent stderr from virsh
    // when we give up.
    #[allow(unused_assignments)]
    let mut last_err = String::new();
    loop {
        match SshExecutor::server_exec(pool, &cmd).await {
            Ok(out) if out.success => return Ok(()),
            Ok(out) => {
                last_err = out.stderr.trim().to_string();
            }
            Err(e) => {
                last_err = format!("ssh: {e}");
            }
        }
        if start.elapsed() >= TIMEOUT {
            return Err(ComputerError::QgaTimeout {
                vm_name: vm_name.into(),
                waited: start.elapsed(),
                last_stderr: last_err,
            });
        }
        tokio::time::sleep(POLL_INTERVAL).await;
    }
}

/// Single-quote `s` for safe interpolation into a shell
/// command. Used by `wait_for_qga_ready` (and the future
/// console-recover path) to build the `virsh
/// qemu-agent-command` invocation. Matches the
/// `shell_quote` style used in `provision.rs`.
fn shell_quote(s: &str) -> String {
    let escaped = s.replace('\'', "'\\''");
    format!("'{escaped}'")
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

/// v2.3.5/v2.3.6: pick the passphrase used to encrypt
/// the per-Bot SSH key during provisioning. Returns
/// `None` when the user hasn't configured the per-Bot
/// path and the default-key flag is off — the caller
/// surfaces `ComputerError::PassphraseMissing`.
///
/// The default-key flag is on (v2.3.5 default for new
/// installs and the post-migration value for upgraded
/// installs), so the placeholder is the common path.
/// The per-Bot key is still encrypted+stored for
/// backwards compat (the user can toggle the flag off
/// without re-provisioning), but the placeholder
/// passphrase is reproducible across launches and is
/// never used to decrypt.
fn resolve_provision_passphrase(s: &Settings) -> Option<String> {
    if s.computer_use_default_ssh_key {
        Some("default-key-path-no-passphrase-needed".to_string())
    } else if s.computer_passphrase.is_empty() {
        None
    } else {
        Some(s.computer_passphrase.clone())
    }
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
    fn computer_state_serializes_to_snake_case() {
        // JSON shape is part of the public Tauri API. The
        // frontend's `ComputerPanel` reads it as
        // `state === 'provisioning' | 'running' | 'stopped' | 'error'`.
        assert_eq!(ComputerState::Provisioning.as_str(), "provisioning");
        assert_eq!(ComputerState::Running.as_str(), "running");
        assert_eq!(ComputerState::Stopped.as_str(), "stopped");
        assert_eq!(ComputerState::Error.as_str(), "error");
    }

    // ----- v2.3.6: provision-passphrase resolver -----

    #[test]
    fn provision_passphrase_uses_placeholder_when_default_key_on() {
        // The default-key flag is on (the v2.3.5
        // post-migration default — applied via the
        // serde default fn, not `Default::default()`)
        // and the user hasn't set a passphrase. The
        // provision flow must succeed — we can't gate
        // creation on a passphrase the user has no UI
        // to set.
        let mut s = Settings::default();
        s.computer_use_default_ssh_key = true;
        assert!(s.computer_use_default_ssh_key);
        assert!(s.computer_passphrase.is_empty());
        let pw = resolve_provision_passphrase(&s);
        assert_eq!(
            pw.as_deref(),
            Some("default-key-path-no-passphrase-needed"),
            "default-key on must yield a non-empty placeholder, not None"
        );
    }

    #[test]
    fn provision_passphrase_returns_none_when_per_bot_path_unconfigured() {
        // Legacy per-Bot path: flag off, no passphrase
        // set. The provision flow must fail with
        // `PassphraseMissing` so the user gets a clear
        // error.
        let mut s = Settings::default();
        s.computer_use_default_ssh_key = false;
        s.computer_passphrase = String::new();
        assert!(resolve_provision_passphrase(&s).is_none());
    }

    #[test]
    fn provision_passphrase_uses_user_value_when_per_bot_path_configured() {
        // Legacy per-Bot path: flag off, passphrase
        // set. Provision uses the user's passphrase
        // (v2.3.4 behavior).
        let mut s = Settings::default();
        s.computer_use_default_ssh_key = false;
        s.computer_passphrase = "user-pass-2026".into();
        assert_eq!(
            resolve_provision_passphrase(&s).as_deref(),
            Some("user-pass-2026"),
        );
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
            // v3.2.0 — `computer_use` defaults to "vm" for
            // new Bots. Smoke tests don't exercise the
            // field.
            connectors_enabled: String::new(),
            computer_use: "vm".to_string(),
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

        // 5b. SFTP path. Cloud-init writes the SSH host
        // keys early (well before it installs xfce / x11vnc /
        // qemu-guest-agent), so the SFTP endpoint should be
        // reachable within ~30s of the IP lease showing up.
        // On a cold libvirt + cold caches, however, the
        // qemu-guest-agent + cloud-init meta-data fetch
        // itself can take 60-120s before sshd is up. We
        // poll the per-Bot SFTP `file_list` for up to 180s
        // and assert we can read `/home/bot/.bashrc` and
        // round-trip a write to `/home/bot/maxbot-smoke.txt`.
        eprintln!("[smoke] waiting for SFTP (up to 180s)...");
        let sftp_deadline = std::time::Instant::now()
            + std::time::Duration::from_secs(180);
        let mut sftp_ok = false;
        while std::time::Instant::now() < sftp_deadline {
            match mgr.file_list(&db, &bot_id, "/home/bot").await {
                Ok(entries) => {
                    eprintln!(
                        "[smoke] sftp /home/bot ok ({} entries)",
                        entries.len()
                    );
                    sftp_ok = true;
                    break;
                }
                Err(e) => {
                    eprintln!("[smoke] sftp not ready yet: {e}");
                    tokio::time::sleep(std::time::Duration::from_secs(3))
                        .await;
                }
            }
        }
        assert!(sftp_ok, "SFTP never became reachable in 60s");
        // Read /home/bot/.bashrc — present on every Ubuntu
        // cloud image. If this returns the standard
        // shell-init preamble, the round-trip is working.
        let bashrc = mgr
            .file_read(&db, &bot_id, "/home/bot/.bashrc")
            .await
            .expect("read .bashrc");
        assert!(
            bashrc.contains("# ~/.bashrc") || bashrc.contains("~/.bashrc"),
            ".bashrc doesn't look like the Ubuntu default; got: {}",
            &bashrc[..bashrc.len().min(120)]
        );
        // Write a marker file, read it back, delete it.
        let marker = "/home/bot/maxbot-smoke.txt";
        mgr.file_write(&db, &bot_id, marker, "smoke-ok\n")
            .await
            .expect("write marker");
        let round_trip = mgr
            .file_read(&db, &bot_id, marker)
            .await
            .expect("read marker back");
        assert_eq!(round_trip, "smoke-ok\n");
        // The Rust side doesn't ship an unlink command, so
        // leave the marker file in place — the VM is about
        // to be torn down anyway.

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

    // ----- v3.0.4: end-to-end Console smoke test against
    // the real Linux server (Tyler's `crispy` at
    // 192.168.0.49). This is the load-bearing CI gate that
    // proves the v3.0.3 auto-recover fix works end-to-end —
    // not just in unit-test isolation.
    //
    // This test:
    //   1. Builds a fresh SQLite DB in a temp dir
    //   2. Inserts a Bot row + Settings (server host, SSH user,
    //      passphrase, default disk/RAM)
    //   3. Constructs a ComputerManager pointed at the real server
    //   4. Calls `provision(bot_id, opts)` and waits for the VM
    //      to be `running` with an IP + VNC port
    //   5. Calls `console_url(bot_id)` — exercises the
    //      `TunnelAuthFailed` → `install_default_key_via_qga` →
    //      retry-tunnel path that v3.0.3 added
    //   6. Asserts the returned URL is a localhost WebSocket
    //      (`ws://localhost:<port>/`) — never a remote host
    //   7. TCP-probes the port to prove the WebSocket is
    //      actually listening, not just a string the function
    //      returned
    //   8. Tears down the libvirt domain so the test can run
    //      again without manual cleanup
    //
    // Marked `#[ignore]` so `cargo test` doesn't hit the real
    // server in CI. Run with:
    //
    //   cargo test --lib takeover_e2e_against_crispy -- --ignored --nocapture
    //
    // Prereqs on the test machine:
    //   - ssh-agent running with Tyler's `~/.ssh/id_ed25519`
    //   - the key is authorized on `tyler@192.168.0.49`
    //   - the server has libvirt + `/opt/maxbot/provision-vm.sh`
    //   - passwordless sudo for tyler on the server
    //   - the user's default pubkey (`~/.ssh/id_ed25519.pub` or
    //     `id_rsa.pub` / `id_ecdsa.pub`) must NOT yet be in the
    //     VM's authorized_keys — that's what triggers the
    //     auto-recover path. Each fresh provision uses a
    //     throwaway VM, so this is naturally the case.
    #[tokio::test]
    #[ignore]
    async fn takeover_e2e_against_crispy() {
        use super::libvirt::LibvirtClient;
        use tokio::net::TcpStream;

        let started = std::time::Instant::now();

        // 1. Fresh DB
        let dir = std::env::temp_dir().join(format!(
            "maxbot-console-smoke-{}",
            uuid::Uuid::new_v4()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("test.sqlite");
        let db = crate::storage::db::Database::open(&path).expect("open test db");

        // 2. Bot + settings. Same shape as
        //    `provision_e2e_against_crispy`.
        let bot_id = format!("console-smoke-{}", &uuid::Uuid::new_v4().to_string()[..8]);
        let now = chrono::Utc::now();
        let bot = crate::bots::Bot {
            id: bot_id.clone(),
            name: "console-smoke".to_string(),
            description: "".to_string(),
            system_prompt: "".to_string(),
            default_model: "MiniMax-M3".to_string(),
            allowed_tools: vec![],
            icon: "".to_string(),
            color: "".to_string(),
            avatar_color: "".to_string(),
            last_active_at: None,
            state: crate::bots::BotState::Idle,
            // v3.2.0 — `computer_use` defaults to "vm" for
            // new Bots. Console smoke tests don't exercise
            // the field.
            connectors_enabled: String::new(),
            computer_use: "vm".to_string(),
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

        // 4. Provision (~1-2 min for cloud-init + xfce +
        //    x11vnc + qemu-guest-agent, then IP/VNC poll).
        //    Provision polls internally and returns once
        //    state == "running" and the IP is leased.
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
        let domain = row.vm_name.clone();
        let vm_ip = row.vm_ip.clone().unwrap();
        let vnc_port = row.vnc_port.unwrap();
        eprintln!(
            "[smoke] provisioned: domain={domain} vm_ip={vm_ip} vnc_port={vnc_port}"
        );

        // 6. Call takeover_open. The VM was just
        //    provisioned and does NOT have the user's
        //    default pubkey authorized yet, so the first
        //    tunnel attempt will fail with
        //    TunnelAuthFailed. v3.0.3's auto-recover path
        //    installs the default key via QGA and retries.
        //    The user-facing contract is that this
        //    returns a working local port on success —
        //    that's the v3.0.3 fix proven end-to-end on
        //    the v3.7.2 takeover path.
        eprintln!("[smoke] calling takeover_open (auto-recover expected)...");
        let local_port = mgr
            .takeover_open(&db, &bot_id)
            .await
            .expect("takeover_open should succeed via auto-recover");
        eprintln!("[smoke] takeover_open returned local port: {local_port}");

        // 7. Port is in the configured VNC range. The
        //    brief's invariant is that the local port is
        //    allocated from the same range two Bots
        //    share, never hardcoded to :5901.
        assert!(
            (5900..=5999).contains(&local_port),
            "local port {local_port} should be in the configured 5900-5999 range"
        );

        // 8. TCP-probe the port. The SSH tunnel forwards
        //    127.0.0.1:<local> to 127.0.0.1:<vnc> on the
        //    server. A raw TCP connect succeeds even
        //    before any VNC handshake — we only need to
        //    prove the tunnel is up.
        let stream = TcpStream::connect(("127.0.0.1", local_port))
            .await
            .expect("takeover port should be accepting TCP");
        let peer = stream
            .peer_addr()
            .ok()
            .map(|a| a.to_string())
            .unwrap_or_default();
        drop(stream);
        eprintln!("[smoke] TCP probe to 127.0.0.1:{local_port} succeeded (peer={peer})");

        // 9. Close the takeover. The SSH child should
        //    be killed and the local port should refuse
        //    connections.
        mgr.takeover_close(&bot_id)
            .await
            .expect("takeover_close should be idempotent");
        // Give the kernel a moment to actually close
        // the listen socket on the local side.
        tokio::time::sleep(std::time::Duration::from_millis(250)).await;
        let probe_after = TcpStream::connect(("127.0.0.1", local_port)).await;
        assert!(
            probe_after.is_err(),
            "local port {local_port} should be freed after takeover_close"
        );
        eprintln!("[smoke] takeover_close freed local port: {local_port}");

        let elapsed = started.elapsed();
        eprintln!("[smoke] takeover_e2e_against_crispy passed in {elapsed:?}");

        // 10. Tear down. Destroy the libvirt domain so
        //     the test can run again without manual
        //     cleanup. The ssh pool is wrapped in Arc;
        //     we use a throwaway handle so we don't
        //     disturb the manager's own pool.
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

        drop(db);
        let _ = std::fs::remove_dir_all(&dir);
    }

    // ----- v3.0.3: install_default_key refactor + console_url
    // auto-recover. The QGA success path is covered by
    // `provision_e2e_against_crispy` + the v3.0.4
    // `console_e2e_against_crispy` (which exercise the
    // real SSH tunnel → QGA → libvirt flow); the unit
    // tests below pin the refactor and the error paths.

    /// v3.0.3: the helper and the public wrapper both
    /// read `$HOME` to find the user's default SSH pubkey.
    /// Rust tests run in parallel by default, so the
    /// HOME-modifying tests below race each other. We
    /// serialize them with a single static mutex — the
    /// critical sections are short, so this doesn't
    /// noticeably slow the suite.
    static HOME_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    /// RAII guard that restores the `$HOME` env var to its
    /// prior value when dropped. Test panics would
    /// otherwise leak the temp-dir HOME to subsequent
    /// tests, causing confusing failures.
    struct HomeGuard(Option<std::ffi::OsString>);
    impl HomeGuard {
        fn set(new_path: &std::path::Path) -> Self {
            let prev = std::env::var_os("HOME");
            std::env::set_var("HOME", new_path);
            Self(prev)
        }
    }
    impl Drop for HomeGuard {
        fn drop(&mut self) {
            match self.0.take() {
                Some(v) => std::env::set_var("HOME", v),
                None => std::env::remove_var("HOME"),
            }
        }
    }

    /// Build a fresh ComputerManager pointed at a fresh
    /// empty DB. Server config is unconfigured by default
    /// (we never reach the SSH path in these tests).
    fn make_test_manager(db_path: &std::path::Path) -> ComputerManager {
        let _ = crate::storage::db::Database::open(db_path).expect("open test db");
        let s = crate::storage::Settings::default();
        ComputerManager::new(&s)
    }

    /// Insert a minimal bot row so the `computers.bot_id`
    /// FK to `bots.id` is satisfied for tests that need
    /// a real computer row.
    fn insert_test_bot(db: &crate::storage::db::Database, bot_id: &str) {
        let now = chrono::Utc::now();
        let bot = crate::bots::Bot {
            id: bot_id.into(),
            name: "test".into(),
            description: String::new(),
            system_prompt: String::new(),
            default_model: "MiniMax-M3".into(),
            allowed_tools: vec![],
            icon: String::new(),
            color: String::new(),
            avatar_color: String::new(),
            last_active_at: None,
            state: crate::bots::BotState::Idle,
            // v3.2.0 — `computer_use` defaults to "vm" for
            // new Bots. This insert helper is used by
            // several test setups; the value is just here
            // so the struct literal compiles.
            connectors_enabled: String::new(),
            computer_use: "vm".to_string(),
            created_at: now,
            updated_at: now,
        };
        db.upsert_bot(&bot).expect("upsert test bot");
    }

    #[tokio::test]
    async fn install_default_key_via_qga_no_pubkey_returns_ssh_error() {
        let _lock = HOME_LOCK.lock().expect("HOME_LOCK poisoned");
        // Point HOME at an empty temp dir — the helper
        // looks for ~/.ssh/id_{ed25519,rsa,ecdsa}.pub and
        // should return a ComputerError::Ssh with the
        // "no default SSH public key" message.
        let home = tempfile::TempDir::new().expect("tempdir");
        let _home = HomeGuard::set(home.path());
        let db_dir = tempfile::TempDir::new().expect("tempdir");
        let db_path = db_dir.path().join("test.sqlite");
        let mgr = make_test_manager(&db_path);
        let db = crate::storage::db::Database::open(&db_path).expect("db");
        let result = mgr.install_default_key_via_qga(&db, "any-bot").await;
        match result {
            Err(ComputerError::Ssh(msg)) => {
                assert!(
                    msg.contains("no default SSH public key"),
                    "expected 'no default SSH public key' in error, got: {msg}"
                );
            }
            other => panic!(
                "expected ComputerError::Ssh with 'no default SSH public key', got: {other:?}"
            ),
        }
    }

    #[tokio::test]
    async fn install_default_key_wrapper_matches_helper_on_no_pubkey() {
        let _lock = HOME_LOCK.lock().expect("HOME_LOCK poisoned");
        // v3.0.3 refactor: the public Tauri command
        // `install_default_key` is a thin wrapper around
        // `install_default_key_via_qga`. The two must
        // produce identical errors for the same inputs so
        // the manual "Use my default key" toolbar button
        // behaves exactly as it did before the refactor.
        let home = tempfile::TempDir::new().expect("tempdir");
        let _home = HomeGuard::set(home.path());
        let db_dir = tempfile::TempDir::new().expect("tempdir");
        let db_path = db_dir.path().join("test.sqlite");
        let mgr = make_test_manager(&db_path);
        let db = crate::storage::db::Database::open(&db_path).expect("db");
        let wrapper_err = mgr
            .install_default_key(&db, "any-bot")
            .await
            .expect_err("wrapper should fail on no-pubkey");
        match wrapper_err {
            ComputerError::Ssh(msg) => {
                assert!(
                    msg.contains("no default SSH public key"),
                    "wrapper diverged from helper: {msg}"
                );
            }
            other => panic!("wrapper should return Ssh, got: {other:?}"),
        }
    }

    #[tokio::test]
    async fn install_default_key_via_qga_no_computer_returns_no_computer() {
        let _lock = HOME_LOCK.lock().expect("HOME_LOCK poisoned");
        // HOME has a valid pubkey, so the helper reaches
        // its DB lookup. The DB has no computer row →
        // NoComputer error. Proves the helper does the
        // DB lookup the same way the original did.
        let home = tempfile::TempDir::new().expect("tempdir");
        let ssh_dir = home.path().join(".ssh");
        std::fs::create_dir_all(&ssh_dir).expect("mkdir .ssh");
        std::fs::write(
            ssh_dir.join("id_ed25519.pub"),
            "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAITESTKEY test@local\n",
        )
        .expect("write pub");
        let _home = HomeGuard::set(home.path());
        let db_dir = tempfile::TempDir::new().expect("tempdir");
        let db_path = db_dir.path().join("test.sqlite");
        let mgr = make_test_manager(&db_path);
        let db = crate::storage::db::Database::open(&db_path).expect("db");
        let result = mgr
            .install_default_key_via_qga(&db, "no-such-bot")
            .await;
        match result {
            Err(ComputerError::NoComputer(bot)) => {
                assert_eq!(bot, "no-such-bot");
            }
            other => panic!(
                "expected NoComputer(\"no-such-bot\"), got: {other:?}"
            ),
        }
    }

    #[tokio::test]
    async fn takeover_open_passes_through_non_auth_errors_without_recover() {
        let _lock = HOME_LOCK.lock().expect("HOME_LOCK poisoned");
        // v3.7.2: the auto-recover path in takeover_open
        // is scoped to `TunnelAuthFailed` only (the
        // inherit-and-rebuild from v3.0.3). For any
        // other VncError (e.g. NoFreePort because the
        // port range is exhausted), takeover_open must
        // propagate the error directly — we don't burn
        // a QGA install attempt on unrelated failures.
        // We force NoFreePort by pre-binding the only
        // port in a 1-port range.
        use std::net::TcpListener as StdTcpListener;
        use std::sync::Mutex as StdMutex;
        // Pick a port the OS gives us, then hold the
        // listener for the test duration so
        // vnc::open_takeover can't bind it.
        let probe = StdTcpListener::bind("127.0.0.1:0").expect("probe");
        let port = probe.local_addr().expect("local_addr").port();
        let held = StdMutex::new(Some(probe));
        // Build a manager + DB with a running computer
        // row. Need a HOME with a pubkey (so a stray
        // QGA install attempt — if our recover logic
        // were mis-scoped — wouldn't crash with
        // ComputerError::Ssh, masking the actual
        // NoFreePort signal).
        let home = tempfile::TempDir::new().expect("tempdir");
        let ssh_dir = home.path().join(".ssh");
        std::fs::create_dir_all(&ssh_dir).expect("mkdir .ssh");
        std::fs::write(
            ssh_dir.join("id_ed25519.pub"),
            "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAITESTKEY test@local\n",
        )
        .expect("write pub");
        let _home = HomeGuard::set(home.path());
        let db_dir = tempfile::TempDir::new().expect("tempdir");
        let db_path = db_dir.path().join("test.sqlite");
        let mgr = make_test_manager(&db_path);
        let db = crate::storage::db::Database::open(&db_path).expect("db");
        let bot_id = "takeover-test-bot";
        insert_test_bot(&db, bot_id);
        let now = chrono::Utc::now();
        db.upsert_computer(&Computer {
            bot_id: bot_id.into(),
            vm_name: "test-vm".into(),
            vm_ip: Some("192.168.122.10".into()),
            vnc_port: Some(5900),
            ssh_key_id: String::new(),
            state: ComputerState::Running.as_str().into(),
            last_seen_at: Some(now),
            created_at: now,
        })
        .expect("upsert computer");
        // Set the port range to the single port we
        // pre-bound so vnc::open_takeover will fail
        // with NoFreePort.
        mgr.set_port_range(port, port).await;
        let result = mgr.takeover_open(&db, bot_id).await;
        // Release the held listener. The test still owns
        // the HOME_LOCK so no other test will read HOME
        // before our drop guard runs.
        drop(held.lock().expect("held lock").take());
        // vnc::open_takeover returns NoFreePort which
        // takeover_open must surface as-is (it does NOT
        // trigger the TunnelAuthFailed auto-recover
        // path).
        match result {
            Err(ComputerError::Vnc(msg)) => {
                assert!(
                    msg.contains("port allocator exhausted")
                        || msg.contains("no free ports"),
                    "expected NoFreePort-style message, got: {msg}"
                );
            }
            Err(other) => panic!(
                "expected Vnc error from NoFreePort, got: {other:?}"
            ),
            Ok(local_port) => panic!(
                "expected error from exhausted port range, got ok: {local_port}"
            ),
        }
    }
}
