import { useState } from "react";
import type { Settings as SettingsT } from "../lib/api";
import { ComputerUseSettings } from "./ComputerUseSettings";

interface SettingsProps {
  initial: SettingsT;
  onClose: () => void;
  onSave: (next: SettingsT) => Promise<void> | void;
}

export function Settings({ initial, onClose, onSave }: SettingsProps) {
  const [apiKey, setApiKey] = useState(initial.minimax_api_key ?? "");
  const [defaultModel, setDefaultModel] = useState(
    initial.default_model || "MiniMax-M3",
  );
  const [baseUrl, setBaseUrl] = useState(initial.base_url ?? "");
  const [saving, setSaving] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const submit = async () => {
    setError(null);
    setSaving(true);
    try {
      await onSave({
        minimax_api_key: apiKey.trim() ? apiKey.trim() : null,
        default_model: defaultModel.trim(),
        base_url: baseUrl.trim(),
      });
      onClose();
    } catch (e) {
      setError(String(e));
    } finally {
      setSaving(false);
    }
  };

  return (
    <div className="modal-overlay" onClick={onClose}>
      <div
        className="modal"
        style={{ width: 620 }}
        onClick={(e) => e.stopPropagation()}
      >
        <h2>Settings — MaxBot</h2>

        <div className="field">
          <label>MiniMax API key</label>
          <input
            type="password"
            value={apiKey}
            onChange={(e) => setApiKey(e.target.value)}
            placeholder="eyJ…"
            autoFocus
          />
          <div className="hint">
            Saved to <code>~/Library/Application Support/com.maxbot.app/maxbot.sqlite</code>.
            Also picked up from <code>MINIMAX_API_KEY</code> in the app environment.
          </div>
        </div>

        <div className="field">
          <label>Default model</label>
          <input
            type="text"
            value={defaultModel}
            onChange={(e) => setDefaultModel(e.target.value)}
            placeholder="MiniMax-M3"
          />
          <div className="hint">
            MiniMax-M3 (1M ctx, tools + vision), MiniMax-M2.7 (204K, tools),
            or MiniMax-M2.7-highspeed (fastest).
          </div>
        </div>

        <div className="field">
          <label>Base URL override (optional)</label>
          <input
            type="text"
            value={baseUrl}
            onChange={(e) => setBaseUrl(e.target.value)}
            placeholder="https://api.minimax.io/v1"
          />
          <div className="hint">
            Leave empty for the international endpoint. Set to
            <code> https://api.minimaxi.com/v1</code> for the China region, or
            a self-hosted proxy URL.
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

        {error && <div className="error">{error}</div>}

        <div className="actions">
          <button onClick={onClose} disabled={saving}>
            Cancel
          </button>
          <button
            className="primary"
            onClick={submit}
            disabled={saving || !apiKey.trim()}
          >
            {saving ? "Saving…" : "Save"}
          </button>
        </div>
      </div>
    </div>
  );
}
