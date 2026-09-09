import { useEffect, useMemo, useState } from "react";
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
import { listMcpServers } from "../lib/tauri";

interface SettingsProps {
  initial: SettingsT;
  onClose: () => void;
  onSave: (next: SettingsT) => Promise<void> | void;
}

export function Settings({ initial, onClose, onSave }: SettingsProps) {
  // Merge with defaults so older settings blobs (pre-multi-provider)
  // render without "undefined" fields.
  const base: SettingsT = useMemo(() => ({ ...DEFAULT_SETTINGS, ...initial }), [initial]);
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
  const [extraBaseUrls, setExtraBaseUrls] = useState<Record<string, string>>(() => ({
    openai: base.openai_base_url ?? "",
    anthropic: base.anthropic_base_url ?? "",
    xai: base.xai_base_url ?? "",
  }));
  const [ttsVoice, setTtsVoice] = useState(base.tts_voice ?? "");
  const [showAdvanced, setShowAdvanced] = useState(false);
  const [saving, setSaving] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [mcpServers, setMcpServers] = useState<McpServerInfo[] | null>(null);

  const preset = presetFor(providerKind);

  useEffect(() => {
    // When the user switches providers, refresh the visible fields
    // from the saved settings and from the new preset's defaults.
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
      // their stored values.
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
      };
      await onSave(merged);
      onClose();
    } catch (e) {
      setError(String(e));
    } finally {
      setSaving(false);
    }
  };

  const canSave = apiKey.trim().length > 0;

  return (
    <div className="modal-overlay" onClick={onClose}>
      <div
        className="modal"
        style={{ width: 660 }}
        onClick={(e) => e.stopPropagation()}
      >
        <h2>Settings — MaxBot</h2>

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
            autoFocus
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

        <div className="field">
          <button
            type="button"
            className="link-button"
            onClick={() => setShowAdvanced((v) => !v)}
            style={{ padding: 0, background: "none", border: "none", color: "var(--accent)", cursor: "pointer" }}
          >
            {showAdvanced ? "▾" : "▸"} Configure other providers
          </button>
          {showAdvanced && (
            <div style={{ marginTop: 8, display: "grid", gap: 12 }}>
              {PROVIDER_PRESETS.filter((p) => p.kind !== providerKind).map((p) => (
                <div key={p.kind} className="other-provider">
                  <div className="other-provider-label">{p.display_name}</div>
                  <input
                    type="password"
                    value={extraKeys[p.kind] ?? ""}
                    onChange={(e) =>
                      setExtraKeys((prev) => ({ ...prev, [p.kind]: e.target.value }))
                    }
                    placeholder={`${p.kind} API key`}
                  />
                  <input
                    type="text"
                    value={extraBaseUrls[p.kind] ?? ""}
                    onChange={(e) =>
                      setExtraBaseUrls((prev) => ({ ...prev, [p.kind]: e.target.value }))
                    }
                    placeholder={p.default_base_url}
                  />
                </div>
              ))}
              <div className="hint">
                Pre-stash keys here so switching providers is one click
                in the dropdown. Keys are saved to the same SQLite blob
                as the active provider's key.
              </div>
            </div>
          )}
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
          <label>Text-to-Speech voice</label>
          <input
            type="text"
            value={ttsVoice}
            onChange={(e) => setTtsVoice(e.target.value)}
            placeholder="Samantha"
          />
          <div className="hint">
            Voice name passed to <code>say -v</code> when you click the
            🔊 Speak button on an assistant message. Leave blank for{" "}
            <code>Samantha</code> (high-quality en-US, ships with every
            macOS install). Run <code>say -v ?</code> in Terminal to
            list installed voices.
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
          <label>MCP servers</label>
          <div className="hint">
            Model Context Protocol servers loaded from{" "}
            <code>~/Library/Application Support/com.maxbot.app/mcp_servers.json</code>.
            Each server's tools are exposed to the model with the prefix{" "}
            <code>mcp__&lt;server&gt;__&lt;tool&gt;</code> and require per-call
            consent.
          </div>
          {mcpServers === null ? (
            <div className="muted small">Loading…</div>
          ) : mcpServers.length === 0 ? (
            <div className="muted small">
              No MCP servers configured. Add a{" "}
              <code>mcp_servers.json</code> file to the data directory and
              restart MaxBot.
            </div>
          ) : (
            <ul className="mcp-server-list">
              {mcpServers.map((s) => (
                <li key={s.name} className="mcp-server-row">
                  <div className="mcp-server-name">{s.name}</div>
                  <div className="mcp-server-tools small muted">
                    {s.tool_count} tool{s.tool_count === 1 ? "" : "s"}:{" "}
                    {s.tool_names.slice(0, 6).join(", ")}
                    {s.tool_names.length > 6
                      ? `, +${s.tool_names.length - 6} more`
                      : ""}
                  </div>
                </li>
              ))}
            </ul>
          )}
        </div>

        {error && <div className="error">{error}</div>}

        <div className="actions">
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
