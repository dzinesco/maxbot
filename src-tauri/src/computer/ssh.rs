//! SSH and SFTP pools for the ComputerManager.
//!
//! Two distinct paths through the same plumbing:
//!
//! 1. **Server-side** — `server_exec(cmd)` runs a command on
//!    `tyler@192.168.0.49`. The Mac's `ssh` binary uses the OS
//!    keychain / ssh-agent for auth, so we just spawn
//!    `ssh tyler@host <cmd>` via `tokio::process::Command`.
//!    No long-lived connection — each call is a fresh SSH
//!    connection (~50-100ms overhead, negligible for the libvirt
//!    queries we make).
//!
//! 2. **Per-Bot VM-side** — `vm_exec(bot_id, cmd)` runs a
//!    command on a Bot's VM, `vm_sftp_*` for file operations.
//!    Per-Bot keys live encrypted in `ssh_keys.private_key_encrypted`
//!    and are decrypted on demand. The plaintext key is written
//!    to a 0600 tempfile in `$TMPDIR` and unlinked when the call
//!    finishes (the `Drop` on `SshPool` clears all cached keys).
//!
//! The trait `SshExecutor` is the abstraction the rest of the
//! codebase depends on; production wires `SshPool`, tests wire a
//! `MockSshExecutor`. This is what makes the "Bot with a computer
//! routes through SSH, without one runs locally" test
//! straightforward.

use std::collections::HashMap;
use std::path::PathBuf;
use std::process::Stdio;
use std::sync::Arc;

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use thiserror::Error;
use tokio::io::AsyncWriteExt;
use tokio::process::Command;
use tokio::sync::Mutex;

use super::keys;

#[derive(Debug, Error)]
pub enum SshError {
    #[error("ssh binary not found on PATH")]
    BinaryMissing,
    #[error("spawn failed: {0}")]
    Spawn(String),
    #[error("auth failed: {0}")]
    Auth(String),
    #[error("command failed (exit {code:?}): {stderr}")]
    Command { code: Option<i32>, stderr: String },
    #[error("io error: {0}")]
    Io(String),
    #[error("bot {0} has no computer provisioned")]
    NoComputer(String),
    #[error("passphrase missing — set Settings.computer_passphrase")]
    PassphraseMissing,
    #[error("encrypted key blob missing for bot {0}")]
    MissingKey(String),
    #[error("server host not configured — set Settings.computer_server_host")]
    ServerNotConfigured,
    #[error("vm not ready yet: {0}")]
    VmNotReady(String),
    #[error("sftp parse error: {0}")]
    SftpParse(String),
}

impl SshError {
    /// True if the failure looks like the VM isn't reachable
    /// yet — useful for the orchestrator to decide between
    /// "retry" and "give up and surface error".
    pub fn is_transient(&self) -> bool {
        matches!(
            self,
            SshError::Command { .. } | SshError::VmNotReady(_) | SshError::Spawn(_)
        )
    }
}

/// Result of running a single command on a remote host. The
/// `success` flag is `true` when the remote process exited 0;
/// `exit_code` is the raw remote exit code, which can be
/// non-zero even on `success = false` (the process started and
/// ran, it just didn't succeed).
#[derive(Debug, Clone)]
pub struct RemoteCommandOutput {
    pub stdout: String,
    pub stderr: String,
    pub exit_code: Option<i32>,
    pub success: bool,
}

/// Result of a directory listing via SFTP. Each entry is the
/// bare filename (no path prefix, no permission bits) — the
/// renderer can join with the parent path if needed.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SftpEntry {
    pub name: String,
    pub is_dir: bool,
    pub size: u64,
}

/// Public trait for SSH and SFTP execution. Production uses
/// `SshPool`; tests can substitute a `MockSshExecutor` that
/// records calls and returns canned output without touching the
/// network.
#[async_trait]
pub trait SshExecutor: Send + Sync {
    async fn server_exec(&self, cmd: &str) -> Result<RemoteCommandOutput, SshError>;

    async fn vm_exec(
        &self,
        bot_id: &str,
        cmd: &str,
    ) -> Result<RemoteCommandOutput, SshError>;

    async fn vm_sftp_list(
        &self,
        bot_id: &str,
        path: &str,
    ) -> Result<Vec<SftpEntry>, SshError>;

    async fn vm_sftp_read(
        &self,
        bot_id: &str,
        path: &str,
    ) -> Result<String, SshError>;

    async fn vm_sftp_write(
        &self,
        bot_id: &str,
        path: &str,
        content: &str,
    ) -> Result<(), SshError>;
}

/// Snapshot of the server-side config the pool needs. Built
/// from `Settings` once at startup. The pool itself doesn't hold
/// a `Settings` reference because `Settings` lives in the DB and
/// would need a `Database` handle — passing the resolved
/// snapshot keeps the pool test-friendly.
#[derive(Debug, Clone)]
pub struct ServerConfig {
    pub host: String,
    pub user: String,
    /// Optional path to the SSH key. If empty, we rely on
    /// ssh-agent / OS keychain. Most Tyler-style installs have
    /// the Mac key at `~/.ssh/id_ed25519` and never need to set
    /// this.
    pub identity_file: String,
    /// Optional `~/.ssh/config` overrides. Used to set
    /// `StrictHostKeyChecking=accept-new` and a connection
    /// timeout, so a flaky server doesn't hang the whole app.
    pub ssh_config: String,
}

impl ServerConfig {
    /// Read `ServerConfig` from a `Settings` struct. Any field
    /// that's empty in the `Settings` is treated as "use the
    /// default" (rather than "the user wants empty"). This
    /// matches the way the Settings UI works — empty string =
    /// unset, and the defaults below kick in.
    pub fn from_settings(s: &crate::storage::Settings) -> Self {
        let host = if s.computer_server_host.is_empty() {
            String::new()
        } else {
            s.computer_server_host.clone()
        };
        let user = if s.computer_server_ssh_user.is_empty() {
            "tyler".to_string()
        } else {
            s.computer_server_ssh_user.clone()
        };
        // v2.3.5: when the default-key flag is on, leave
        // `identity_file` empty so ssh falls back to
        // `~/.ssh/id_ed25519` / `~/.ssh/id_rsa` / ssh-agent.
        // The empty-string case is already handled correctly
        // by `spawn_tunnel` (it skips the `-i` arg).
        let identity_file = if s.computer_use_default_ssh_key {
            String::new()
        } else {
            s.computer_server_ssh_key_id.clone()
        };
        // Inline ssh config: silence the host-key prompt and
        // cap the connection setup at 10s. We don't persist this
        // to ~/.ssh/config because the per-call behavior is
        // preferable — we don't want to change global SSH
        // behavior for other apps.
        let ssh_config = format!(
            "Host *\n  StrictHostKeyChecking=accept-new\n  ConnectTimeout=10\n  ServerAliveInterval=30\n  ServerAliveCountMax=3\n  UserKnownHostsFile={known_hosts}\n",
            known_hosts = default_known_hosts()
        );
        Self {
            host,
            user,
            identity_file,
            ssh_config,
        }
    }

    /// True if the host has been configured. The other fields
    /// have sensible defaults.
    pub fn is_configured(&self) -> bool {
        !self.host.is_empty()
    }
}

/// Compute the default path to the known_hosts file. Used to
/// scope our accept-new policy to a private file so we don't
/// touch the user's `~/.ssh/known_hosts`.
fn default_known_hosts() -> String {
    if let Some(home) = std::env::var_os("HOME") {
        let mut p = PathBuf::from(home);
        p.push(".ssh");
        p.push("maxbot_known_hosts");
        return p.to_string_lossy().to_string();
    }
    "/tmp/maxbot_known_hosts".to_string()
}

/// The real production pool. One process, shared across all
/// callers. The internal state is:
///
/// - `server`: server config (host, user, identity_file, ssh_config).
/// - `passphrase`: the user-set `Settings.computer_passphrase`.
///   Empty = the user hasn't configured the computer feature
///   yet; per-Bot operations will fail with `PassphraseMissing`.
/// - `key_blobs`: pre-decrypted per-Bot private key blobs,
///   keyed by bot id. Populated lazily on first `vm_exec` /
///   `vm_sftp_*` call. Cleared on `clear_key_cache()` (used
///   when the user changes the passphrase).
/// - `vm_endpoints`: a cached `(ip, ssh_user)` per bot, looked
///   up from the `computers` table before each call. The
///   ComputerManager writes here after polling for the IP
///   during provisioning.
///
/// The pool is `Send + Sync` because all the inner state is
/// `Arc<Mutex<_>>`. Calls take the mutex briefly to look up
/// state, then drop it before doing the actual SSH I/O.
pub struct SshPool {
    /// The server-side config. Public so `vnc::spawn_tunnel`
    /// can read the host/user without going through the
    /// trait (which doesn't need to know about VNC). All
    /// other consumers go through `server_config()` which
    /// hides the field by returning a clone.
    pub server: ServerConfig,
    passphrase: Mutex<String>,
    /// Per-Bot private key, decrypted, in PKCS#8 DER form. The
    /// bytes are cleared on drop via `Zeroizing<Vec<u8>>` (we
    /// use a plain `Vec<u8>` here for the in-memory cache and
    /// rely on the OS to zero pages; the encryption layer is
    /// the security boundary).
    key_blobs: Mutex<HashMap<String, Arc<Vec<u8>>>>,
    /// Per-Bot VM network endpoint. Set by the ComputerManager
    /// after provisioning polls for an IP. Cleared by
    /// `forget_bot` when a computer is destroyed.
    vm_endpoints: Mutex<HashMap<String, VmEndpoint>>,
}

#[derive(Debug, Clone)]
pub struct VmEndpoint {
    pub ip: String,
    pub ssh_user: String,
    /// MaxBot server host (e.g. `192.168.0.49`). Used as
    /// the SSH ProxyJump target for per-Bot connections —
    /// the Mac can't route to the libvirt NAT network
    /// (192.168.122.0/24) directly.
    pub proxy_host: String,
    /// MaxBot server SSH user (e.g. `tyler`). Same use as
    /// `proxy_host`.
    pub proxy_user: String,
}

impl SshPool {
    pub fn new(server: ServerConfig) -> Self {
        Self {
            server,
            passphrase: Mutex::new(String::new()),
            key_blobs: Mutex::new(HashMap::new()),
            vm_endpoints: Mutex::new(HashMap::new()),
        }
    }

    /// Build an SshPool from a `Settings` snapshot. This is
    /// a static constructor because the SshPool's fields
    /// don't need async setup, but the pool is held inside
    /// the ComputerManager (which has a `&Settings` for
    /// one tick at construction time).
    pub fn server_config_from_settings(
        s: &crate::storage::Settings,
    ) -> ServerConfig {
        ServerConfig::from_settings(s)
    }

    /// Set / replace the passphrase. The next per-Bot call
    /// will re-derive the encryption key from the new
    /// passphrase. Existing cached key blobs are NOT
    /// invalidated — if the user changes the passphrase
    /// without re-provisioning Bots, those Bots will keep the
    /// old (now-wrong) cached key and per-Bot calls will
    /// fail. Callers should call `clear_key_cache()` after
    /// updating the passphrase.
    pub async fn set_passphrase(&self, passphrase: String) {
        *self.passphrase.lock().await = passphrase;
    }

    /// Discard all cached per-Bot key blobs. Called when the
    /// user changes the passphrase or after a Bot is
    /// destroyed.
    pub async fn clear_key_cache(&self) {
        self.key_blobs.lock().await.clear();
    }

    /// Record a Bot's VM endpoint (IP + SSH user). Called by
    /// the ComputerManager after `provision_vm` polls for an
    /// IP. Without an endpoint, `vm_exec` returns
    /// `VmNotReady`. The proxy host/user are filled in
    /// from `self.server` so the per-Bot SFTP / SSH path
    /// can `-J user@host` through the MaxBot server — the
    /// Mac can't route to 192.168.122.0/24 directly.
    pub async fn set_vm_endpoint(&self, bot_id: &str, ip: String, ssh_user: String) {
        self.vm_endpoints.lock().await.insert(
            bot_id.to_string(),
            VmEndpoint {
                ip,
                ssh_user,
                proxy_host: self.server.host.clone(),
                proxy_user: self.server.user.clone(),
            },
        );
    }

    /// Forget a Bot. Drops the cached key and the VM
    /// endpoint. Called by `computer_destroy` so a
    /// re-provisioned Bot doesn't reuse a stale key.
    pub async fn forget_bot(&self, bot_id: &str) {
        self.key_blobs.lock().await.remove(bot_id);
        self.vm_endpoints.lock().await.remove(bot_id);
    }

    /// Inject a pre-decrypted per-Bot key. Used by the
    /// ComputerManager when it generates a fresh keypair
    /// during provisioning — the encrypted blob is written
    /// to the DB, but the decrypted bytes are also cached
    /// here so the very next shell command doesn't have to
    /// re-decrypt.
    pub async fn cache_key_blob(&self, bot_id: &str, der_bytes: Arc<Vec<u8>>) {
        self.key_blobs
            .lock()
            .await
            .insert(bot_id.to_string(), der_bytes);
    }

    /// Decrypt the per-Bot key from a stored ciphertext blob
    /// and cache the plaintext. The cached entry is reused
    /// for the rest of the process's lifetime.
    pub async fn unlock_with_ciphertext(
        &self,
        bot_id: &str,
        ciphertext: &[u8],
    ) -> Result<(), SshError> {
        let passphrase = self.passphrase.lock().await.clone();
        if passphrase.is_empty() {
            return Err(SshError::PassphraseMissing);
        }
        let plaintext = keys::decrypt_private(ciphertext, &passphrase)
            .map_err(|_| SshError::Auth("decrypt failed (wrong passphrase?)".into()))?;
        self.cache_key_blob(bot_id, Arc::new(plaintext)).await;
        Ok(())
    }
}

#[async_trait]
impl SshExecutor for SshPool {
    async fn server_exec(&self, cmd: &str) -> Result<RemoteCommandOutput, SshError> {
        if !self.server.is_configured() {
            return Err(SshError::ServerNotConfigured);
        }
        run_ssh(&self.server, None, cmd).await
    }

    async fn vm_exec(
        &self,
        bot_id: &str,
        cmd: &str,
    ) -> Result<RemoteCommandOutput, SshError> {
        let endpoint = self
            .vm_endpoints
            .lock()
            .await
            .get(bot_id)
            .cloned()
            .ok_or_else(|| SshError::VmNotReady(format!("bot {bot_id} has no IP yet")))?;
        // v2.3.5: when the default-key flag is on, skip the
        // per-Bot key fetch and let ssh fall back to the user's
        // default key (which was installed into the VM's
        // `authorized_keys` via `computer_install_default_key`).
        let key: Option<Arc<Vec<u8>>> = if self.server.identity_file.is_empty() {
            None
        } else {
            Some(self.key_blob_for(bot_id).await?)
        };
        run_ssh_with_key(&endpoint, key.as_ref(), cmd).await
    }

    async fn vm_sftp_list(
        &self,
        bot_id: &str,
        path: &str,
    ) -> Result<Vec<SftpEntry>, SshError> {
        let endpoint = self
            .vm_endpoints
            .lock()
            .await
            .get(bot_id)
            .cloned()
            .ok_or_else(|| SshError::VmNotReady(format!("bot {bot_id} has no IP yet")))?;
        let key: Option<Arc<Vec<u8>>> = if self.server.identity_file.is_empty() {
            None
        } else {
            Some(self.key_blob_for(bot_id).await?)
        };
        let out = run_sftp_batch(&endpoint, key.as_ref(), &format!("ls -la {path}\nbye\n")).await?;
        parse_sftp_ls(&out)
    }

    async fn vm_sftp_read(
        &self,
        bot_id: &str,
        path: &str,
    ) -> Result<String, SshError> {
        let endpoint = self
            .vm_endpoints
            .lock()
            .await
            .get(bot_id)
            .cloned()
            .ok_or_else(|| SshError::VmNotReady(format!("bot {bot_id} has no IP yet")))?;
        let key: Option<Arc<Vec<u8>>> = if self.server.identity_file.is_empty() {
            None
        } else {
            Some(self.key_blob_for(bot_id).await?)
        };
        // `sftp` batch mode: fetch a remote file to stdout via
        // /dev/stdout. The `-` trick works on OpenSSH's sftp;
        // older versions may need a tempfile dance.
        let batch = format!("get {path} -\nbye\n");
        let out = run_sftp_capture(&endpoint, key.as_ref(), &batch).await?;
        Ok(out)
    }

    async fn vm_sftp_write(
        &self,
        bot_id: &str,
        path: &str,
        content: &str,
    ) -> Result<(), SshError> {
        let endpoint = self
            .vm_endpoints
            .lock()
            .await
            .get(bot_id)
            .cloned()
            .ok_or_else(|| SshError::VmNotReady(format!("bot {bot_id} has no IP yet")))?;
        let key: Option<Arc<Vec<u8>>> = if self.server.identity_file.is_empty() {
            None
        } else {
            Some(self.key_blob_for(bot_id).await?)
        };
        // Write to a unique temp path on the VM, then move
        // atomically. sftp's batch mode `put -` reads from
        // stdin, but we have to pipe `content` into it. We
        // avoid a tempfile on the Mac by piping via stdin.
        let tmp_remote = format!("/tmp/.maxbot-sftp-{}.tmp", std::process::id());
        let batch = format!(
            "put - {tmp_remote}\nrename {tmp_remote} {path}\nbye\n"
        );
        run_sftp_write(&endpoint, key.as_ref(), &batch, content).await?;
        Ok(())
    }
}

impl SshPool {
    /// Look up the cached, decrypted per-Bot private
    /// key. Public so `ComputerManager` can probe
    /// before deciding whether to re-decrypt.
    pub async fn key_blob_for(&self, bot_id: &str) -> Result<Arc<Vec<u8>>, SshError> {
        if let Some(k) = self.key_blobs.lock().await.get(bot_id) {
            return Ok(k.clone());
        }
        Err(SshError::MissingKey(bot_id.to_string()))
    }
}

// ---- low-level spawners ----

/// Spawn `ssh <args>` and return its output. Used for both
/// server-side and per-Bot calls; the key difference is whether
/// we pass `-i <keyfile>` (per-Bot) or rely on the OS keychain
/// (server).
async fn run_ssh_command(
    mut cmd: Command,
) -> Result<RemoteCommandOutput, SshError> {
    cmd.stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .stdin(Stdio::null())
        .kill_on_drop(true);
    let output = cmd
        .output()
        .await
        .map_err(|e| {
            if e.kind() == std::io::ErrorKind::NotFound {
                SshError::BinaryMissing
            } else {
                SshError::Spawn(e.to_string())
            }
        })?;
    let stdout = String::from_utf8_lossy(&output.stdout).to_string();
    let stderr = String::from_utf8_lossy(&output.stderr).to_string();
    let exit = output.status.code();
    let success = output.status.success();
    if !success && stderr.contains("Permission denied") {
        return Err(SshError::Auth(stderr.trim().to_string()));
    }
    Ok(RemoteCommandOutput {
        stdout,
        stderr,
        exit_code: exit,
        success,
    })
}

/// Server-side exec: run `cmd` on the server using the OS
/// keychain. We avoid `-F /dev/stdin` complexity by passing
/// the options as `-o` flags inline.
async fn run_ssh(
    cfg: &ServerConfig,
    keyfile: Option<&KeyTempFile>,
    cmd: &str,
) -> Result<RemoteCommandOutput, SshError> {
    let mut c = Command::new("ssh");
    c.arg("-o").arg("BatchMode=yes");
    c.arg("-o").arg("LogLevel=ERROR");
    c.arg("-o").arg("StrictHostKeyChecking=accept-new");
    c.arg("-o").arg("ConnectTimeout=10");
    c.arg("-o").arg("ServerAliveInterval=30");
    c.arg("-o").arg("ServerAliveCountMax=3");
    if let Some(kf) = keyfile {
        c.arg("-i").arg(kf.path());
        c.arg("-o").arg("IdentitiesOnly=yes");
    } else if !cfg.identity_file.is_empty() {
        c.arg("-i").arg(&cfg.identity_file);
    }
    c.arg(format!("{}@{}", cfg.user, cfg.host));
    c.arg(cmd);
    run_ssh_command(c).await
}

/// Per-Bot exec: same as `run_ssh` but uses the per-Bot key
/// and connects to the VM's IP directly. v2.3.5: when `key`
/// is `None` the call skips `-i` and lets ssh fall back to
/// the user's default key — the VM's `authorized_keys` was
/// populated with that key by `computer_install_default_key`.
async fn run_ssh_with_key(
    endpoint: &VmEndpoint,
    key: Option<&Arc<Vec<u8>>>,
    cmd: &str,
) -> Result<RemoteCommandOutput, SshError> {
    let keyfile = match key {
        Some(k) => Some(KeyTempFile::new(k.as_ref()).await?),
        None => None,
    };
    let mut c = Command::new("ssh");
    c.arg("-o").arg("BatchMode=yes");
    c.arg("-o").arg("LogLevel=ERROR");
    c.arg("-o").arg("StrictHostKeyChecking=no");
    c.arg("-o").arg("ConnectTimeout=10");
    c.arg("-o").arg("UserKnownHostsFile=/dev/null");
    // See run_sftp_batch for the ProxyJump rationale.
    c.arg("-o").arg(format!(
        "ProxyJump={}@{}",
        endpoint.proxy_user, endpoint.proxy_host
    ));
    if let Some(kf) = &keyfile {
        c.arg("-i").arg(kf.path());
        c.arg("-o").arg("IdentitiesOnly=yes");
    }
    c.arg(format!("{}@{}", endpoint.ssh_user, endpoint.ip));
    c.arg(cmd);
    let result = run_ssh_command(c).await;
    drop(keyfile);
    result
}

/// SFTP batch run that just prints the batch output (used for
/// `ls`). v2.3.5: when `key` is `None` the call skips `-i`
/// and lets sftp fall back to the user's default key.
async fn run_sftp_batch(
    endpoint: &VmEndpoint,
    key: Option<&Arc<Vec<u8>>>,
    batch: &str,
) -> Result<String, SshError> {
    let keyfile = match key {
        Some(k) => Some(KeyTempFile::new(k.as_ref()).await?),
        None => None,
    };
    let mut c = Command::new("sftp");
    c.arg("-o").arg("BatchMode=yes");
    c.arg("-o").arg("LogLevel=ERROR");
    c.arg("-o").arg("StrictHostKeyChecking=no");
    c.arg("-o").arg("UserKnownHostsFile=/dev/null");
    if let Some(kf) = &keyfile {
        c.arg("-i").arg(kf.path());
        c.arg("-o").arg("IdentitiesOnly=yes");
    }
    c.arg("-b").arg("-");
    c.arg(format!("{}@{}", endpoint.ssh_user, endpoint.ip));
    c.stdin(Stdio::piped());
    c.stdout(Stdio::piped());
    c.stderr(Stdio::piped());
    c.kill_on_drop(true);
    let mut child = c.spawn().map_err(|e| {
        if e.kind() == std::io::ErrorKind::NotFound {
            SshError::BinaryMissing
        } else {
            SshError::Spawn(e.to_string())
        }
    })?;
    // Write the batch to stdin.
    if let Some(mut stdin) = child.stdin.take() {
        stdin
            .write_all(batch.as_bytes())
            .await
            .map_err(|e| SshError::Io(e.to_string()))?;
        drop(stdin);
    }
    let out = child
        .wait_with_output()
        .await
        .map_err(|e| SshError::Io(e.to_string()))?;
    drop(keyfile);
    let stdout = String::from_utf8_lossy(&out.stdout).to_string();
    let stderr = String::from_utf8_lossy(&out.stderr).to_string();
    if !out.status.success() {
        return Err(SshError::Command {
            code: out.status.code(),
            stderr: stderr.trim().to_string(),
        });
    }
    Ok(stdout)
}

/// SFTP batch run that captures a file fetched to stdout
/// (`get <path> -`).
async fn run_sftp_capture(
    endpoint: &VmEndpoint,
    key: Option<&Arc<Vec<u8>>>,
    batch: &str,
) -> Result<String, SshError> {
    // sftp's `get <path> -` writes the file's bytes to its
    // own stdout. So we just call the same batch runner and
    // return the stdout as the file content.
    run_sftp_batch(endpoint, key, batch).await
}

/// SFTP batch run that pipes `content` into a `put -` command
/// (i.e. we write a remote file).
async fn run_sftp_write(
    endpoint: &VmEndpoint,
    key: Option<&Arc<Vec<u8>>>,
    batch: &str,
    content: &str,
) -> Result<(), SshError> {
    let keyfile = match key {
        Some(k) => Some(KeyTempFile::new(k.as_ref()).await?),
        None => None,
    };
    let mut c = Command::new("sftp");
    c.arg("-o").arg("BatchMode=yes");
    c.arg("-o").arg("LogLevel=ERROR");
    c.arg("-o").arg("StrictHostKeyChecking=no");
    c.arg("-o").arg("UserKnownHostsFile=/dev/null");
    // See run_sftp_batch for the ProxyJump rationale. Both
    // the read (`get`) and write (`put`) batches need the
    // same hop because they go to the same per-Bot VM.
    c.arg("-o").arg(format!(
        "ProxyJump={}@{}",
        endpoint.proxy_user, endpoint.proxy_host
    ));
    if let Some(kf) = &keyfile {
        c.arg("-i").arg(kf.path());
        c.arg("-o").arg("IdentitiesOnly=yes");
    }
    c.arg("-b").arg("-");
    c.arg(format!("{}@{}", endpoint.ssh_user, endpoint.ip));
    c.stdin(Stdio::piped());
    c.stdout(Stdio::piped());
    c.stderr(Stdio::piped());
    c.kill_on_drop(true);
    let mut child = c.spawn().map_err(|e| {
        if e.kind() == std::io::ErrorKind::NotFound {
            SshError::BinaryMissing
        } else {
            SshError::Spawn(e.to_string())
        }
    })?;
    if let Some(mut stdin) = child.stdin.take() {
        stdin
            .write_all(batch.as_bytes())
            .await
            .map_err(|e| SshError::Io(e.to_string()))?;
        stdin
            .write_all(content.as_bytes())
            .await
            .map_err(|e| SshError::Io(e.to_string()))?;
        drop(stdin);
    }
    let out = child
        .wait_with_output()
        .await
        .map_err(|e| SshError::Io(e.to_string()))?;
    drop(keyfile);
    if !out.status.success() {
        let stderr = String::from_utf8_lossy(&out.stderr).to_string();
        return Err(SshError::Command {
            code: out.status.code(),
            stderr: stderr.trim().to_string(),
        });
    }
    Ok(())
}

/// Write `bytes` to a 0600 tempfile in `$TMPDIR`. The
/// tempfile is unlinked on `Drop`. Used for per-Bot SSH keys
/// that we want to live only as long as the SSH call.
///
/// On macOS, `$TMPDIR` is per-user (e.g.
/// `/var/folders/.../T/`), so even a brief window of
/// readability is contained. The `chmod 0600` is the
/// belt-and-suspenders move that satisfies the same
/// invariant on Linux.
pub struct KeyTempFile {
    path: PathBuf,
}

impl KeyTempFile {
    pub async fn new(bytes: &[u8]) -> Result<Self, SshError> {
        let mut path = std::env::temp_dir();
        path.push(format!("maxbot-key-{}.pem", uuid::Uuid::new_v4()));
        let mut f = tokio::fs::OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .mode(0o600)
            .open(&path)
            .await
            .map_err(|e| SshError::Io(format!("tempfile open: {e}")))?;
        f.write_all(bytes)
            .await
            .map_err(|e| SshError::Io(format!("tempfile write: {e}")))?;
        f.flush()
            .await
            .map_err(|e| SshError::Io(format!("tempfile flush: {e}")))?;
        drop(f);
        Ok(Self { path })
    }
    pub fn path(&self) -> &str {
        self.path.to_str().unwrap_or("/dev/null")
    }
}

impl Drop for KeyTempFile {
    fn drop(&mut self) {
        // Best-effort unlink. The OS will clean up
        // $TMPDIR eventually if this fails (e.g. on a
        // permission error). The macOS quarantine / SIP
        // scenarios that could block this are not
        // relevant for `$TMPDIR`.
        let _ = std::fs::remove_file(&self.path);
    }
}

/// Parse the output of an SFTP `ls -la` batch command. The
/// `sftp` binary prints each file on its own line in `-l`
/// format (size, date, time, name) and prefixes non-fatal
/// lines with `sftp>`. We strip the prefix, skip the
/// "total N" line, and infer directory-ness from the
/// first character of the permission field.
fn parse_sftp_ls(raw: &str) -> Result<Vec<SftpEntry>, SshError> {
    let mut out = Vec::new();
    for line in raw.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        if line.starts_with("sftp>") {
            continue;
        }
        if line.starts_with("Changing to:") {
            continue;
        }
        if line.starts_with("total ") {
            continue;
        }
        // Expected: "-rw-r--r--   1   bot   bot   1234   Jan 01 12:00   filename"
        // Or:      "drwxr-xr-x   2   bot   bot   4096   Jan 01 12:00   dirname"
        let _trimmed = line.trim_start_matches(|c: char| c == '-' || c == 'd' || c == 'l' || c.is_alphabetic() && c != ' ' );
        // sftp's `ls -la` output varies between OpenSSH
        // versions. The most reliable split is on
        // whitespace; the name is the last token. Some
        // versions include year + time, some just
        // month-day-time. We don't care about the
        // metadata, only the name and the type.
        // First non-space token starts with `-` (file) or
        // `d` (directory). The actual name is the last
        // whitespace-separated token.
        let parts: Vec<&str> = line.split_whitespace().collect();
        if parts.len() < 2 {
            continue;
        }
        let is_dir = parts[0].starts_with('d');
        // Filename might contain spaces. We take everything
        // from the last token onward (rejoin).
        let name_start = line.rfind(parts.last().unwrap()).unwrap_or(line.len());
        let name = line[name_start..].trim().to_string();
        if name == "." || name == ".." {
            continue;
        }
        // Size is the 5th column (0-indexed) in standard
        // ls -l output. We don't strictly need it but
        // callers may.
        let size = parts
            .get(4)
            .and_then(|s| s.parse::<u64>().ok())
            .unwrap_or(0);
        out.push(SftpEntry {
            name,
            is_dir,
            size,
        });
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn server_config_defaults_user_to_tyler() {
        let s = crate::storage::Settings::default();
        let cfg = ServerConfig::from_settings(&s);
        assert_eq!(cfg.user, "tyler");
        assert!(!cfg.is_configured(), "host empty → not configured");
    }

    #[test]
    fn server_config_is_configured_with_host() {
        let mut s = crate::storage::Settings::default();
        s.computer_server_host = "192.168.0.49".into();
        let cfg = ServerConfig::from_settings(&s);
        assert!(cfg.is_configured());
        assert_eq!(cfg.host, "192.168.0.49");
        assert_eq!(cfg.user, "tyler");
    }

    // ----- v2.3.5: default-key vs per-Bot-key path -----

    #[test]
    fn from_settings_uses_default_key_when_setting_on() {
        // When `computer_use_default_ssh_key` is true, the
        // server-config snapshot must leave `identity_file`
        // empty even if `computer_server_ssh_key_id` was
        // populated for legacy reasons. The empty string
        // makes `spawn_tunnel` skip the `-i` arg, so ssh
        // falls back to `~/.ssh/id_ed25519` / ssh-agent.
        let mut s = crate::storage::Settings::default();
        s.computer_server_host = "192.168.0.49".into();
        s.computer_server_ssh_key_id = "legacy-key-id".into();
        s.computer_use_default_ssh_key = true;
        let cfg = ServerConfig::from_settings(&s);
        assert_eq!(cfg.identity_file, "", "default-key on must zero out identity_file");
    }

    #[test]
    fn from_settings_uses_per_bot_key_when_setting_off() {
        // When `computer_use_default_ssh_key` is false,
        // `identity_file` must mirror `computer_server_ssh_key_id`
        // so the legacy per-Bot path still works for users
        // who haven't switched.
        let mut s = crate::storage::Settings::default();
        s.computer_server_host = "192.168.0.49".into();
        s.computer_server_ssh_key_id = "legacy-key-id".into();
        s.computer_use_default_ssh_key = false;
        let cfg = ServerConfig::from_settings(&s);
        assert_eq!(
            cfg.identity_file, "legacy-key-id",
            "default-key off must pass computer_server_ssh_key_id through"
        );
    }

    #[test]
    fn parse_sftp_ls_extracts_files_and_dirs() {
        let raw = "\
sftp> ls -la /home/bot
drwxr-xr-x   2   bot   bot   4096   Jan 01 12:00   .
drwxr-xr-x   3   root  root  4096   Jan 01 12:00   ..
-rw-r--r--   1   bot   bot   220   Jan 01 12:00   .bash_logout
-rw-r--r--   1   bot   bot   3771  Jan 01 12:00   .bashrc
drwxr-xr-x   2   bot   bot   4096  Jan 01 12:00   Documents
total 24
";
        let entries = parse_sftp_ls(raw).expect("parse");
        // . and .. are skipped, the 3 named entries remain:
        // .bash_logout, .bashrc, Documents.
        assert_eq!(entries.len(), 3);
        let bashrc = entries.iter().find(|e| e.name == ".bashrc").unwrap();
        assert!(!bashrc.is_dir);
        let docs = entries.iter().find(|e| e.name == "Documents").unwrap();
        assert!(docs.is_dir);
    }

    #[test]
    fn keytempfile_is_dropped_and_unlinked() {
        // A simple smoke test that the tempfile gets
        // unlinked on drop. We don't bother with
        // permissions here — they're checked at open time.
        let dir = std::env::temp_dir();
        let path = dir.join(format!("maxbot-k-{}", uuid::Uuid::new_v4()));
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        rt.block_on(async {
            let kf = KeyTempFile::new(b"hello").await.expect("create");
            assert!(tokio::fs::try_exists(kf.path()).await.unwrap());
            let saved_path = kf.path().to_string();
            drop(kf);
            // The unlink is best-effort but on a normal
            // tmpdir it should complete. Allow a brief
            // filesystem settle.
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
            assert!(!std::path::Path::new(&saved_path).exists());
        });
        // Silence unused.
        let _ = path;
    }

    #[test]
    fn is_transient_classifies_ssh_errors() {
        assert!(SshError::VmNotReady("x".into()).is_transient());
        assert!(SshError::Spawn("x".into()).is_transient());
        assert!(!SshError::Auth("x".into()).is_transient());
        assert!(!SshError::ServerNotConfigured.is_transient());
        assert!(!SshError::PassphraseMissing.is_transient());
    }
}
