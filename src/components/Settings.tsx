import { useEffect, useMemo, useRef, useState } from "react";
import type { McpServerInfo, ProviderKind, Settings as SettingsT } from "../lib/api";
import {
  apiKeyFor,
  baseUrlFor,
  DEFAULT_SETTINGS,
  isConfigured,
  presetFor,
  PROVIDER_PRESETS,
} from "../lib/api";
import { ComputerUseSettings } from "./ComputerUseSettings";
import { computerTestConnection, listMcpServers, metaSet } from "../lib/tauri";

interface SettingsProps {
  initial: SettingsT;
  onClose: () => void;
  onSave: (next: SettingsT) => Promise<void> | void;
}

/** Tabs surfaced in the Settings modal, in render order. The order
 *  here drives the Cmd+1..Cmd+6 jump shortcuts. The Computer tab
 *  was added in v2.0 Slice D (between Browser and Grok) and is
 *  gated on `Settings.computer_server_host` being set — see
 *  `renderComputerTab()`. */
const TABS = [
  { id: "general", label: "General" },
  { id: "providers", label: "Providers" },
  { id: "tts", label: "TTS" },
  { id: "browser", label: "Browser" },
  { id: "computer", label: "Computer" },
  { id: "grok", label: "Grok" },
] as const;

type TabId = (typeof TABS)[number]["id"];

/** Common macOS `say` voices — surfaced in the TTS tab as a quick
 *  dropdown alongside the free-form text input. The hardcoded list
 *  covers the languages Tyler actually uses; the text input above
 *  accepts any voice the user has installed (run `say -v ?` in
 *  Terminal to enumerate). When the user's typed value isn't on the
 *  list, the dropdown shows "Custom" so the input wins visually. */
const COMMON_VOICES = [
  "Samantha",
  "Daniel",
  "Karen",
  "Ava",
  "Allison",
  "Tessa",
  "Rishi",
  "Serena",
  "Victoria",
  "Nicky",
  "Alex",
  "Fred",
  "Whisper",
  "Tingting",
  "Mei-Jia",
];

export function Settings({ initial, onClose, onSave }: SettingsProps) {
  // Merge with defaults so older settings blobs (pre-multi-provider,
  // pre-grok) render without "undefined" fields.
  const base: SettingsT = useMemo(
    () => ({ ...DEFAULT_SETTINGS, ...initial }),
    [initial],
  );
  const [activeTab, setActiveTab] = useState<TabId>("general");

  // ---- form state ----
  const [providerKind, setProviderKind] = useState<ProviderKind | string>(
    base.provider_kind || "minimax",
  );
  const [apiKey, setApiKey] = useState(apiKeyFor(base, providerKind) ?? "");
  const [defaultModel, setDefaultModel] = useState(
    base.default_model || presetFor(providerKind).default_model,
  );
  const [baseUrl, setBaseUrl] = useState(baseUrlFor(base, providerKind));
  const [extraKeys, setExtraKeys] = useState<Record<string, string>>(() => ({
    openai: base.openai_api_key ?? "",
    anthropic: base.anthropic_api_key ?? "",
    xai: base.xai_api_key ?? "",
  }));
  const [extraBaseUrls, setExtraBaseUrls] = useState<Record<string, string>>(
    () => ({
      openai: base.openai_base_url ?? "",
      anthropic: base.anthropic_base_url ?? "",
      xai: base.xai_base_url ?? "",
    }),
  );
  const [ttsVoice, setTtsVoice] = useState(base.tts_voice ?? "");
  // Browser tab — both are optional overrides; the Rust side currently
  // auto-resolves from $HOME/.local/bin and PATH. Surfaced here so a
  // future Rust change can pick them up without another UI pass.
  const [egoBrowserPath, setEgoBrowserPath] = useState(base.ego_browser_path ?? "");
  const [nodejsPath, setNodejsPath] = useState(base.nodejs_path ?? "");
  // Grok tab — three string fields from the Rust settings blob, all
  // default to "" (Rust side resolves sensible defaults when empty).
  const [grokBinary, setGrokBinary] = useState(base.grok_build_binary ?? "");
  const [grokModel, setGrokModel] = useState(base.grok_build_model ?? "");
  const [grokCwd, setGrokCwd] = useState(base.grok_cwd ?? "");
  // v2.0 Slice D — Computer tab. The Rust side stores the VNC
  // port range as a single `"lo-hi"` string; we keep it as two
  // numeric inputs locally and serialize back on submit. The
  // `computerServerHost` gates the empty-state in the tab.
  const [computerServerHost, setComputerServerHost] = useState(
    base.computer_server_host ?? "",
  );
  const [computerServerSshUser, setComputerServerSshUser] = useState(
    base.computer_server_ssh_user ?? "tyler",
  );
  const [computerServerSshKeyId, setComputerServerSshKeyId] = useState(
    base.computer_server_ssh_key_id ?? "",
  );
  const [computerPassphrase, setComputerPassphrase] = useState(
    base.computer_passphrase ?? "",
  );
  // v2.3.5: when true, the SSH path uses the user's
  // default `~/.ssh/id_*` key. The passphrase field is
  // hidden when this is on because the per-Bot key
  // path is skipped entirely. New installs default
  // to true (matches the Rust `default_computer_use_default_ssh_key`
  // helper in `db.rs`).
  const [computerUseDefaultSshKey, setComputerUseDefaultSshKey] = useState(
    base.computer_use_default_ssh_key ?? true,
  );
  const initialPortRange = base.computer_vnc_local_port_range || "5900-5999";
  const [computerPortLo, setComputerPortLo] = useState<number>(
    parsePortLo(initialPortRange),
  );
  const [computerPortHi, setComputerPortHi] = useState<number>(
    parsePortHi(initialPortRange),
  );
  const [computerDiskGb, setComputerDiskGb] = useState<number>(
    base.computer_default_disk_gb ?? 10,
  );
  const [computerRamMb, setComputerRamMb] = useState<number>(
    base.computer_default_ram_mb ?? 2048,
  );
  // Test-connection state: `null` = idle, `{ kind: "ok", count }` =
  // last call succeeded, `{ kind: "err", message }` = failed. The
  // banner shows up while `kind` is set and is dismissable.
  const [testState, setTestState] = useState<
    | null
    | { kind: "ok"; count: number }
    | { kind: "err"; message: string }
  >(null);
  const [testPending, setTestPending] = useState(false);

  const [saving, setSaving] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [mcpServers, setMcpServers] = useState<McpServerInfo[] | null>(null);

  const preset = presetFor(providerKind);

  // When the user switches the active provider in the General tab,
  // refresh the visible fields from the saved settings and from the
  // new preset's defaults.
  useEffect(() => {
    setApiKey(apiKeyFor(base, providerKind) ?? "");
    setBaseUrl(baseUrlFor(base, providerKind));
    if (!base.default_model) {
      setDefaultModel(presetFor(providerKind).default_model);
    }
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [providerKind]);

  useEffect(() => {
    listMcpServers()
      .then(setMcpServers)
      .catch(() => setMcpServers([]));
  }, []);

  const submit = async () => {
    setError(null);
    setSaving(true);
    try {
      // Build the per-provider payload: the active provider's fields
      // come from the visible inputs; the inactive providers keep
      // their stored values. The new (v1) tabs add their own fields
      // to the same payload.
      //
      // The Computer tab fields are serialized last. The port range
      // is stored as a single `"lo-hi"` string in SQLite (Rust's
      // `computer_vnc_local_port_range: String`); the UI keeps the
      // two ends as separate numeric inputs and joins them on
      // submit. `lo` is clamped ≤ `hi` to mirror the Rust
      // `parse_port_range` helper's behavior on bad input.
      const safeLo = Math.max(1, Math.min(65535, Math.floor(computerPortLo)));
      const safeHi = Math.max(safeLo, Math.min(65535, Math.floor(computerPortHi)));
      const merged: SettingsT = {
        ...base,
        provider_kind: providerKind,
        default_model: defaultModel.trim(),
        minimax_api_key:
          providerKind === "minimax"
            ? apiKey.trim() || null
            : base.minimax_api_key,
        openai_api_key:
          providerKind === "openai"
            ? apiKey.trim() || null
            : extraKeys.openai.trim() || null,
        anthropic_api_key:
          providerKind === "anthropic"
            ? apiKey.trim() || null
            : extraKeys.anthropic.trim() || null,
        xai_api_key:
          providerKind === "xai" ? apiKey.trim() || null : extraKeys.xai.trim() || null,
        minimax_base_url:
          providerKind === "minimax" ? baseUrl.trim() : base.minimax_base_url,
        openai_base_url:
          providerKind === "openai" ? baseUrl.trim() : extraBaseUrls.openai.trim(),
        anthropic_base_url:
          providerKind === "anthropic" ? baseUrl.trim() : extraBaseUrls.anthropic.trim(),
        xai_base_url:
          providerKind === "xai" ? baseUrl.trim() : extraBaseUrls.xai.trim(),
        tts_voice: ttsVoice.trim(),
        grok_build_binary: grokBinary.trim(),
        grok_build_model: grokModel.trim(),
        grok_cwd: grokCwd.trim(),
        ego_browser_path: egoBrowserPath.trim(),
        nodejs_path: nodejsPath.trim(),
        // v2.0 Slice D: Computer tab. Field names match the Rust
        // struct in `db.rs` exactly.
        computer_server_host: computerServerHost.trim(),
        computer_server_ssh_user: computerServerSshUser.trim() || "tyler",
        computer_server_ssh_key_id: computerServerSshKeyId.trim(),
        computer_vnc_local_port_range: `${safeLo}-${safeHi}`,
        computer_passphrase: computerPassphrase,
        // v2.3.5: default-key flag. The Rust side
        // serializes this into the JSON settings blob
        // and `ServerConfig::from_settings` zeros out
        // `identity_file` when it's on. Stored as a
        // boolean so the React state round-trips cleanly.
        computer_use_default_ssh_key: computerUseDefaultSshKey,
        computer_default_disk_gb: Math.max(
          1,
          Math.floor(computerDiskGb || 10),
        ),
        computer_default_ram_mb: Math.max(
          256,
          Math.floor(computerRamMb || 2048),
        ),
      };
      await onSave(merged);
      onClose();
    } catch (e) {
      setError(String(e));
    } finally {
      setSaving(false);
    }
  };

  /** Run the "Test connection" smoke test against the
   * `computer_test_connection` Tauri command. The Rust side runs
   * `sudo -n virsh list` on the server (over SSH) and returns the
   * number of domains. The banner below the button shows the
   * result; the X dismisses it. */
  const runTestConnection = async () => {
    setTestPending(true);
    setTestState(null);
    try {
      // Persist the form first so the Rust side sees the user's
      // current host / user settings before it tries to connect.
      // We do this by calling the same payload construction as
      // `submit` (minus `await onSave(merged); onClose();`).
      const safeLo = Math.max(1, Math.min(65535, Math.floor(computerPortLo)));
      const safeHi = Math.max(safeLo, Math.min(65535, Math.floor(computerPortHi)));
      const merged: SettingsT = {
        ...base,
        provider_kind: providerKind,
        default_model: defaultModel.trim(),
        minimax_api_key:
          providerKind === "minimax"
            ? apiKey.trim() || null
            : base.minimax_api_key,
        openai_api_key:
          providerKind === "openai"
            ? apiKey.trim() || null
            : extraKeys.openai.trim() || null,
        anthropic_api_key:
          providerKind === "anthropic"
            ? apiKey.trim() || null
            : extraKeys.anthropic.trim() || null,
        xai_api_key:
          providerKind === "xai" ? apiKey.trim() || null : extraKeys.xai.trim() || null,
        minimax_base_url:
          providerKind === "minimax" ? baseUrl.trim() : base.minimax_base_url,
        openai_base_url:
          providerKind === "openai" ? baseUrl.trim() : extraBaseUrls.openai.trim(),
        anthropic_base_url:
          providerKind === "anthropic" ? baseUrl.trim() : extraBaseUrls.anthropic.trim(),
        xai_base_url:
          providerKind === "xai" ? baseUrl.trim() : extraBaseUrls.xai.trim(),
        tts_voice: ttsVoice.trim(),
        grok_build_binary: grokBinary.trim(),
        grok_build_model: grokModel.trim(),
        grok_cwd: grokCwd.trim(),
        ego_browser_path: egoBrowserPath.trim(),
        nodejs_path: nodejsPath.trim(),
        computer_server_host: computerServerHost.trim(),
        computer_server_ssh_user: computerServerSshUser.trim() || "tyler",
        computer_server_ssh_key_id: computerServerSshKeyId.trim(),
        computer_vnc_local_port_range: `${safeLo}-${safeHi}`,
        computer_passphrase: computerPassphrase,
        // v2.3.5: default-key flag. The Rust side
        // serializes this into the JSON settings blob
        // and `ServerConfig::from_settings` zeros out
        // `identity_file` when it's on. Stored as a
        // boolean so the React state round-trips cleanly.
        computer_use_default_ssh_key: computerUseDefaultSshKey,
        computer_default_disk_gb: Math.max(
          1,
          Math.floor(computerDiskGb || 10),
        ),
        computer_default_ram_mb: Math.max(
          256,
          Math.floor(computerRamMb || 2048),
        ),
      };
      await onSave(merged);
      const count = await computerTestConnection();
      setTestState({ kind: "ok", count });
    } catch (e) {
      setTestState({ kind: "err", message: String(e) });
    } finally {
      setTestPending(false);
    }
  };

  // Save is always allowed — a user might want to persist a non-
  // default model or a base URL without a fresh API key. The original
  // "canSave" guard assumed a single required field; the v1 UI splits
  // things into per-tab and an empty key on the active provider is
  // surfaced as a hint instead of a hard block.
  const canSave = true;

  // ---- keyboard nav ----
  // Cmd+1..Cmd+6 jumps directly to a tab from anywhere in the modal.
  // Arrow keys (←/→) move focus between tabs when the tab bar
  // itself is focused (the WAI-ARIA tabs pattern).
  const tabBarRef = useRef<HTMLDivElement | null>(null);
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      // Cmd+1..6 — global within the modal. v2.0 Slice D
      // bumped the upper bound from 5 to 6 when the Computer
      // tab landed between Browser (4) and Grok (6).
      if ((e.metaKey || e.ctrlKey) && !e.shiftKey && !e.altKey) {
        const idx = ["1", "2", "3", "4", "5", "6"].indexOf(e.key);
        if (idx >= 0) {
          e.preventDefault();
          setActiveTab(TABS[idx].id);
          // Move focus to the new tab so screen readers announce it.
          const btn = tabBarRef.current?.querySelectorAll<HTMLButtonElement>(
            "button[role='tab']",
          )[idx];
          btn?.focus();
          return;
        }
      }
      // ←/→ — only when focus is in the tab bar
      if (
        (e.key === "ArrowLeft" || e.key === "ArrowRight") &&
        tabBarRef.current?.contains(document.activeElement)
      ) {
        e.preventDefault();
        const cur = TABS.findIndex((t) => t.id === activeTab);
        const next =
          e.key === "ArrowRight"
            ? (cur + 1) % TABS.length
            : (cur - 1 + TABS.length) % TABS.length;
        setActiveTab(TABS[next].id);
        const btns = tabBarRef.current.querySelectorAll<HTMLButtonElement>(
          "button[role='tab']",
        );
        btns[next]?.focus();
      }
    };
    document.addEventListener("keydown", onKey);
    return () => document.removeEventListener("keydown", onKey);
  }, [activeTab]);

  const isReady = isConfigured(base);

  return (
    <div className="modal-overlay" onClick={onClose}>
      <div
        className="modal-stacked modal-stacked-wide"
        onClick={(e) => e.stopPropagation()}
      >
        <div className="modal-stacked-header">
          <h2>Settings — MaxBot</h2>
        </div>

        <div
          className="modal-tabs"
          ref={tabBarRef}
          role="tablist"
          aria-label="Settings categories"
        >
          {TABS.map((t) => (
            <button
              key={t.id}
              type="button"
              role="tab"
              aria-selected={activeTab === t.id}
              tabIndex={activeTab === t.id ? 0 : -1}
              className={`modal-tab${activeTab === t.id ? " active" : ""}`}
              onClick={() => setActiveTab(t.id)}
            >
              {t.label}
            </button>
          ))}
        </div>

        <div className="modal-stacked-body">
          {activeTab === "general" && (
            <div role="tabpanel" className="modal-tab-pane">
              <div className="field">
                <label>Status</label>
                <div
                  className={`status-row ${isReady ? "online" : "missing"}`}
                  title={
                    isReady
                      ? "Active LLM provider is configured and ready"
                      : "No API key for the active provider"
                  }
                >
                  <span className="indicator" />
                  <span>
                    {isReady
                      ? `${preset.display_name} — ready`
                      : "API key not set for the active provider"}
                  </span>
                </div>
                <div className="hint">
                  The active provider is used for new chats and bot runs
                  unless an individual bot overrides the model.
                </div>
              </div>

              <div className="field">
                <label>LLM provider</label>
                <select
                  value={providerKind}
                  onChange={(e) => setProviderKind(e.target.value)}
                >
                  {PROVIDER_PRESETS.map((p) => (
                    <option key={p.kind} value={p.kind}>
                      {p.display_name}
                    </option>
                  ))}
                </select>
                <div className="hint">{preset.description}</div>
              </div>

              <div className="field">
                <label>{preset.display_name} API key</label>
                <input
                  type="password"
                  value={apiKey}
                  onChange={(e) => setApiKey(e.target.value)}
                  placeholder={preset.key_placeholder}
                />
                <div className="hint">{preset.key_hint}</div>
              </div>

              <div className="field">
                <label>Default model</label>
                <input
                  type="text"
                  value={defaultModel}
                  onChange={(e) => setDefaultModel(e.target.value)}
                  placeholder={preset.default_model}
                />
                <div className="hint">
                  Leave blank to use <code>{preset.default_model}</code>.
                </div>
              </div>

              <div className="field">
                <label>Base URL override (optional)</label>
                <input
                  type="text"
                  value={baseUrl}
                  onChange={(e) => setBaseUrl(e.target.value)}
                  placeholder={preset.default_base_url}
                />
                <div className="hint">
                  Leave blank for the built-in default. Set to a self-hosted
                  proxy or alternate region endpoint if needed.
                </div>
              </div>

              <hr
                style={{
                  border: 0,
                  borderTop: "1px solid var(--border)",
                  margin: "6px 0",
                }}
              />

              <ComputerUseSettings />

              <hr
                style={{
                  border: 0,
                  borderTop: "1px solid var(--border)",
                  margin: "6px 0",
                }}
              />

              <div className="field">
                {/* AGENT_C: reset-onboarding */}
                <label>Onboarding</label>
                <button
                  type="button"
                  onClick={() => {
                    const ok = window.confirm(
                      "Reset onboarding? You'll see the welcome screen on next launch.",
                    );
                    if (!ok) return;
                    metaSet("is_onboarded", "").catch((e) => {
                      console.error("reset onboarding failed:", e);
                      alert(`Failed to reset onboarding: ${e}`);
                    });
                  }}
                >
                  Reset onboarding
                </button>
                <div className="hint">
                  Re-runs the first-launch wizard on the next app
                  launch so you can re-pick a provider or revisit the
                  .env-file path. Settings are preserved — only the
                  welcome screen flag is cleared.
                </div>
              </div>
            </div>
          )}

          {activeTab === "providers" && (
            <div role="tabpanel" className="modal-tab-pane">
              {PROVIDER_PRESETS.map((p) => {
                const isActive = p.kind === providerKind;
                return (
                  <div key={p.kind} className="provider-section">
                    <div className="provider-section-header">
                      <h3>{p.display_name}</h3>
                      {isActive && (
                        <span className="badge small">active</span>
                      )}
                    </div>
                    <div className="hint provider-section-desc">
                      {p.description}
                    </div>
                    <div className="field">
                      <label>API key</label>
                      <input
                        type="password"
                        value={
                          isActive
                            ? apiKey
                            : extraKeys[p.kind] ?? ""
                        }
                        onChange={(e) => {
                          if (isActive) {
                            setApiKey(e.target.value);
                          } else {
                            setExtraKeys((prev) => ({
                              ...prev,
                              [p.kind]: e.target.value,
                            }));
                          }
                        }}
                        placeholder={p.key_placeholder}
                      />
                      <div className="hint">{p.key_hint}</div>
                    </div>
                    <div className="field">
                      <label>Base URL override (optional)</label>
                      <input
                        type="text"
                        value={
                          isActive
                            ? baseUrl
                            : extraBaseUrls[p.kind] ?? ""
                        }
                        onChange={(e) => {
                          if (isActive) {
                            setBaseUrl(e.target.value);
                          } else {
                            setExtraBaseUrls((prev) => ({
                              ...prev,
                              [p.kind]: e.target.value,
                            }));
                          }
                        }}
                        placeholder={p.default_base_url}
                      />
                    </div>
                  </div>
                );
              })}
            </div>
          )}

          {activeTab === "tts" && (
            <div role="tabpanel" className="modal-tab-pane">
              <div className="field">
                <label>Voice</label>
                <input
                  type="text"
                  value={ttsVoice}
                  onChange={(e) => setTtsVoice(e.target.value)}
                  placeholder="Samantha"
                  list="maxbot-common-voices"
                />
                <datalist id="maxbot-common-voices">
                  {COMMON_VOICES.map((v) => (
                    <option key={v} value={v} />
                  ))}
                </datalist>
                <div className="hint">
                  Voice name passed to <code>say -v</code> when you click
                  the 🔊 button on an assistant message. Leave blank for{" "}
                  <code>Samantha</code> (high-quality en-US, ships with
                  every macOS install). Run <code>say -v ?</code> in
                  Terminal for the full list of installed voices.
                </div>
              </div>

              <div className="field">
                <label>Common voices (one click)</label>
                <div className="voice-pills">
                  {COMMON_VOICES.map((v) => (
                    <button
                      key={v}
                      type="button"
                      className={`voice-pill${ttsVoice === v ? " active" : ""}`}
                      onClick={() => setTtsVoice(v)}
                    >
                      {v}
                    </button>
                  ))}
                </div>
                <div className="hint">
                  Tapping a pill sets the voice above. Free-form input
                  above accepts any name <code>say -v ?</code> reports
                  on your machine.
                </div>
              </div>
            </div>
          )}

          {activeTab === "browser" && (
            <div role="tabpanel" className="modal-tab-pane">
              <div className="field">
                <label>ego-browser path (optional)</label>
                <input
                  type="text"
                  value={egoBrowserPath}
                  onChange={(e) => setEgoBrowserPath(e.target.value)}
                  placeholder="/Users/you/.local/bin/ego-browser"
                />
                <div className="hint">
                  The <code>ego_browser</code> tool spawns{" "}
                  <code>ego-browser nodejs -e &lt;script&gt;</code> in a
                  sandboxed Chromium. Leave blank to use the auto-detected
                  path: <code>$HOME/.local/bin/ego-browser</code> if
                  present, otherwise the bare name on <code>$PATH</code>.
                  Install ego (lite) via{" "}
                  <code>~/.agents/skills/ego-browser/</code>.
                </div>
              </div>

              <div className="field">
                <label>nodejs path override (optional)</label>
                <input
                  type="text"
                  value={nodejsPath}
                  onChange={(e) => setNodejsPath(e.target.value)}
                  placeholder="(not currently used)"
                  disabled
                />
                <div className="hint">
                  ego (lite) embeds its own Node.js runtime via the{" "}
                  <code>nodejs</code> subcommand, so a Node.js binary
                  override isn't currently exposed. Field is reserved for
                  a future escape hatch.
                </div>
              </div>
            </div>
          )}

          {activeTab === "computer" && (
            <div role="tabpanel" className="modal-tab-pane settings__computer-tab">
              {computerServerHost.trim() === "" ? (
                <div
                  className="settings__computer-empty-state"
                  data-testid="computer-tab-empty-state"
                >
                  <h3>Set up your Linux server</h3>
                  <p>
                    Each Bot in MaxBot can have its own Linux VM
                    (QEMU/KVM + libvirt on your server). The Bots run
                    inside these VMs, the human operator can preview or
                    take over the desktop in real time, and the Bot's
                    tools route to its VM instead of your Mac.
                  </p>
                  <p>
                    Before the Computer tab unlocks, run the{" "}
                    <a
                      href="docs/server-setup.md"
                      onClick={(e) => {
                        // File-system link: in dev, this is served by
                        // Vite; in a built Tauri app, the docs ship
                        // with the bundle. Tauri doesn't open
                        // arbitrary file:// links in a webview, but
                        // the user can also navigate to the file in
                        // Finder. We preventDefault so the modal
                        // doesn't try to navigate the webview itself.
                        e.preventDefault();
                        alert(
                          "Open docs/server-setup.md from the project root.",
                        );
                      }}
                    >
                      Linux server setup runbook
                    </a>{" "}
                    on the machine that will host the VMs (one-time,
                    ~10 minutes). When the runbook is done, set the
                    server host below and Save.
                  </p>
                </div>
              ) : (
                <>
                  <div className="field">
                    <label>Server host</label>
                    <input
                      type="text"
                      value={computerServerHost}
                      onChange={(e) => setComputerServerHost(e.target.value)}
                      placeholder="192.168.0.49"
                      data-testid="computer-server-host"
                    />
                    <div className="hint">
                      Hostname or IP of the Linux server that hosts
                      per-Bot libvirt VMs. Same machine the runbook
                      was run on. Cleared = disable the Computer
                      feature (every <code>computer_*</code> command
                      returns <code>ServerNotConfigured</code>).
                    </div>
                  </div>

                  <div className="field">
                    <label>Server SSH user</label>
                    <input
                      type="text"
                      value={computerServerSshUser}
                      onChange={(e) =>
                        setComputerServerSshUser(e.target.value)
                      }
                      placeholder="tyler"
                    />
                    <div className="hint">
                      SSH user on the server. Defaults to{" "}
                      <code>tyler</code>; the runbook sets this user
                      up with passwordless sudo + libvirt group
                      membership.
                    </div>
                  </div>

                  <div className="field">
                    <label>SSH key id (optional)</label>
                    <input
                      type="text"
                      value={computerServerSshKeyId}
                      onChange={(e) =>
                        setComputerServerSshKeyId(e.target.value)
                      }
                      placeholder="(leave blank for now)"
                      disabled
                    />
                    <div className="hint">
                      <strong>Coming soon:</strong> manage keypairs
                      here. For now the Rust side relies on the OS
                      keychain / ssh-agent — most Tyler-style
                      installs have the Mac key at{" "}
                      <code>~/.ssh/id_ed25519</code> already and
                      leave this blank.
                    </div>
                  </div>

                  {/* v2.3.5: default-key toggle. The per-Bot
                    key + passphrase model was overkill for a
                    personal tool. When this is on, the Rust
                    side's `ServerConfig::from_settings` leaves
                    `identity_file` empty so ssh falls back to
                    the user's default key. The passphrase
                    field is hidden in that mode because the
                    per-Bot key is never generated or stored. */}
                  <div className="field">
                    <label>
                      <input
                        type="checkbox"
                        checked={computerUseDefaultSshKey}
                        onChange={(e) =>
                          setComputerUseDefaultSshKey(e.target.checked)
                        }
                        data-testid="computer-use-default-ssh-key"
                      />{" "}
                      Use my default SSH key (<code>~/.ssh/id_*</code>)
                      instead of a per-Bot key — no passphrase needed
                    </label>
                    <div className="hint">
                      When checked, MaxBot's SSH path skips the
                      per-Bot key model entirely. The
                      <code>computer_install_default_key</code>{" "}
                      button on the ComputerPanel adds your
                      <code>~/.ssh/id_ed25519.pub</code> to a
                      VM's <code>authorized_keys</code> via the
                      QEMU guest agent, so existing VMs switch
                      without Destroy.
                    </div>
                  </div>

                  {!computerUseDefaultSshKey && (
                    <div className="field">
                      <label>Passphrase</label>
                      <input
                        type="password"
                        value={computerPassphrase}
                        onChange={(e) => setComputerPassphrase(e.target.value)}
                        placeholder="(set once; used to encrypt per-Bot SSH keys)"
                      />
                      <div className="hint">
                        Used to derive an Argon2id key that encrypts
                        each Bot's SSH keypair. Set this once; if you
                        forget it, every Bot's computer becomes
                        inaccessible (recovery is a v2.8 feature).
                      </div>
                    </div>
                  )}

                  <div className="field">
                    <label>VNC local port range</label>
                    <div
                      className="form-row inline"
                      style={{ alignItems: "center" }}
                    >
                      <input
                        type="number"
                        min={1}
                        max={65535}
                        value={computerPortLo}
                        onChange={(e) =>
                          setComputerPortLo(
                            Math.max(1, parseInt(e.target.value || "5900", 10)),
                          )
                        }
                        style={{ width: 100 }}
                        data-testid="computer-vnc-port-lo"
                      />
                      <span className="muted small">to</span>
                      <input
                        type="number"
                        min={1}
                        max={65535}
                        value={computerPortHi}
                        onChange={(e) =>
                          setComputerPortHi(
                            Math.max(
                              computerPortLo,
                              parseInt(e.target.value || "5999", 10),
                            ),
                          )
                        }
                        style={{ width: 100 }}
                        data-testid="computer-vnc-port-hi"
                      />
                    </div>
                    <div className="hint">
                      Local TCP ports the SSH-tunneled VNC proxy
                      binds to. The renderer gets one port per
                      active VNC console (one per Bot that has a
                      preview / takeover open). 100 ports leaves
                      plenty of headroom. The Rust side stores this
                      as a single <code>"lo-hi"</code> string and
                      silently falls back to <code>5900-5999</code>{" "}
                      on bad input.
                    </div>
                  </div>

                  <div className="field">
                    <label>Default per-Bot disk (GB)</label>
                    <input
                      type="number"
                      min={1}
                      value={computerDiskGb}
                      onChange={(e) =>
                        setComputerDiskGb(
                          Math.max(1, parseInt(e.target.value || "10", 10)),
                        )
                      }
                      style={{ width: 100 }}
                    />
                    <div className="hint">
                      Used when a Bot is created with "Provision a
                      computer" without an explicit disk size. The
                      Linux server at <code>192.168.0.49</code> has
                      ~491 GB free on <code>/</code>, so 10 GB per
                      Bot leaves room for ~49 concurrent VMs.
                    </div>
                  </div>

                  <div className="field">
                    <label>Default per-Bot RAM (MB)</label>
                    <input
                      type="number"
                      min={256}
                      step={256}
                      value={computerRamMb}
                      onChange={(e) =>
                        setComputerRamMb(
                          Math.max(
                            256,
                            parseInt(e.target.value || "2048", 10),
                          ),
                        )
                      }
                      style={{ width: 100 }}
                    />
                    <div className="hint">
                      Used when a Bot is created without an explicit
                      RAM. 2048 MiB is comfortable for XFCE + a few
                      apps; the runbook recommends 3072 MiB to
                      silence <code>virt-install</code>'s warning.
                      Override per-Bot in the Bot editor.
                    </div>
                  </div>

                  <div className="field">
                    <label>Test connection</label>
                    <div
                      style={{
                        display: "flex",
                        alignItems: "center",
                        gap: 8,
                        flexWrap: "wrap",
                      }}
                    >
                      <button
                        type="button"
                        className="settings__test-connection-btn"
                        onClick={runTestConnection}
                        disabled={testPending}
                        data-testid="computer-test-connection"
                      >
                        {testPending ? "Testing…" : "Test connection"}
                      </button>
                      {testState && testState.kind === "ok" && (
                        <div
                          className="settings__test-connection-banner settings__test-connection-banner--ok"
                          data-testid="computer-test-connection-ok"
                        >
                          <span>
                            Connected — {testState.count} libvirt domain
                            {testState.count === 1 ? "" : "s"} on the
                            server.
                          </span>
                          <button
                            type="button"
                            className="ghost small"
                            onClick={() => setTestState(null)}
                            aria-label="Dismiss"
                          >
                            ✕
                          </button>
                        </div>
                      )}
                      {testState && testState.kind === "err" && (
                        <div
                          className="settings__test-connection-banner settings__test-connection-banner--err"
                          data-testid="computer-test-connection-err"
                        >
                          <span>
                            <strong>Connection failed.</strong>{" "}
                            {testState.message}
                          </span>
                          <button
                            type="button"
                            className="ghost small"
                            onClick={() => setTestState(null)}
                            aria-label="Dismiss"
                          >
                            ✕
                          </button>
                        </div>
                      )}
                    </div>
                    <div className="hint">
                      Saves the current Computer-tab fields and then
                      runs <code>virsh list</code> on the server over
                      SSH. A green banner means the Mac can reach
                      the server and libvirt is wired up; a red
                      banner surfaces the SSH / libvirt error
                      verbatim.
                    </div>
                  </div>
                </>
              )}
            </div>
          )}

          {activeTab === "grok" && (
            <div role="tabpanel" className="modal-tab-pane">
              <div className="field">
                <label>Binary path</label>
                <input
                  type="text"
                  value={grokBinary}
                  onChange={(e) => setGrokBinary(e.target.value)}
                  placeholder="grok"
                />
                <div className="hint">
                  The <code>grok</code> CLI binary the{" "}
                  <code>grok_prompt</code> tool spawns. Leave blank to
                  resolve <code>grok</code> on <code>$PATH</code>. If
                  your install lives elsewhere (e.g.{" "}
                  <code>~/.grok/bin/grok</code>), set the absolute path
                  here.
                </div>
              </div>

              <div className="field">
                <label>Model alias</label>
                <input
                  type="text"
                  value={grokModel}
                  onChange={(e) => setGrokModel(e.target.value)}
                  placeholder="minimax"
                />
                <div className="hint">
                  The alias passed to <code>grok --model &lt;alias&gt;</code>.
                  Resolved via the{" "}
                  <code>[model.&lt;alias&gt;]</code> table in{" "}
                  <code>~/.grok/config.toml</code>. Default{" "}
                  <code>minimax</code> matches the official install.
                </div>
              </div>

              <div className="field">
                <label>Working directory</label>
                <input
                  type="text"
                  value={grokCwd}
                  onChange={(e) => setGrokCwd(e.target.value)}
                  placeholder="(default: app data dir / grok/)"
                />
                <div className="hint">
                  Working directory for the <code>grok agent stdio</code>{" "}
                  subprocess. Leave blank to use the per-app data dir's{" "}
                  <code>grok/</code> subfolder; set to an absolute path
                  to point the agent at a specific project.
                </div>
              </div>

              <hr
                style={{
                  border: 0,
                  borderTop: "1px solid var(--border)",
                  margin: "6px 0",
                }}
              />

              <div className="field">
                <label>Session</label>
                <button
                  type="button"
                  disabled
                  title="Reset Grok session Tauri command lands in a follow-up slice"
                >
                  Reset Grok session
                </button>
                <div className="hint">
                  Drops the cached <code>grok agent stdio</code> session
                  and clears the persisted session id, so the next
                  prompt starts a fresh conversation. Use this when
                  you've drifted the agent off-topic and want a clean
                  slate. The Tauri command lands in a follow-up slice;
                  for now you can relaunch MaxBot to force a reset.
                </div>
              </div>
            </div>
          )}

          {error && <div className="error">{error}</div>}
        </div>

        <div className="modal-stacked-footer">
          <button onClick={onClose} disabled={saving}>
            Cancel
          </button>
          <button
            className="primary"
            onClick={submit}
            disabled={saving || !canSave}
            title={!canSave ? "Enter an API key for the selected provider" : undefined}
          >
            {saving ? "Saving…" : "Save"}
          </button>
        </div>
      </div>
    </div>
  );
}

// Re-export the isConfigured helper so App.tsx can use the same
// "is the active provider keyed" check without importing from api
// twice. (Pure re-export keeps the existing import surface small.)
export { isConfigured };

// v2.0 Slice D — Computer tab port-range helpers. The Rust
// struct stores a single `"lo-hi"` string; we parse the two
// ends for the two numeric inputs. Bad input (empty,
// non-numeric, reversed) falls back to the default 5900-5999
// range, mirroring the Rust `parse_port_range` helper.
function parsePortLo(s: string): number {
  const parts = s.split("-");
  if (parts.length === 2) {
    const n = parseInt(parts[0], 10);
    if (Number.isFinite(n) && n >= 1 && n <= 65535) return n;
  }
  return 5900;
}

function parsePortHi(s: string): number {
  const parts = s.split("-");
  if (parts.length === 2) {
    const n = parseInt(parts[1], 10);
    if (Number.isFinite(n) && n >= 1 && n <= 65535) return n;
  }
  return 5999;
}
