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
import { listMcpServers, metaSet } from "../lib/tauri";

interface SettingsProps {
  initial: SettingsT;
  onClose: () => void;
  onSave: (next: SettingsT) => Promise<void> | void;
}

/** Tabs surfaced in the Settings modal, in render order. The order
 *  here drives the Cmd+1..Cmd+5 jump shortcuts. */
const TABS = [
  { id: "general", label: "General" },
  { id: "providers", label: "Providers" },
  { id: "tts", label: "TTS" },
  { id: "browser", label: "Browser" },
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
      };
      await onSave(merged);
      onClose();
    } catch (e) {
      setError(String(e));
    } finally {
      setSaving(false);
    }
  };

  // Save is always allowed — a user might want to persist a non-
  // default model or a base URL without a fresh API key. The original
  // "canSave" guard assumed a single required field; the v1 UI splits
  // things into per-tab and an empty key on the active provider is
  // surfaced as a hint instead of a hard block.
  const canSave = true;

  // ---- keyboard nav ----
  // Cmd+1..Cmd+5 jumps directly to a tab from anywhere in the modal.
  // Arrow keys (←/→) move focus between tabs when the tab bar
  // itself is focused (the WAI-ARIA tabs pattern).
  const tabBarRef = useRef<HTMLDivElement | null>(null);
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      // Cmd+1..5 — global within the modal
      if ((e.metaKey || e.ctrlKey) && !e.shiftKey && !e.altKey) {
        const idx = ["1", "2", "3", "4", "5"].indexOf(e.key);
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
