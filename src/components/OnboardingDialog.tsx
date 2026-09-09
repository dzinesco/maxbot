import { useState } from "react";
import type { ProviderKind, Settings as SettingsT } from "../lib/api";
import {
  apiKeyFor,
  baseUrlFor,
  presetFor,
  PROVIDER_PRESETS,
} from "../lib/api";
import { metaSet, saveSettings } from "../lib/tauri";

interface OnboardingDialogProps {
  initialSettings: SettingsT;
  onClose: () => void;
  /**
   * Called after the final CTA — by then the key is persisted and
   * `is_onboarded` is set, so the parent can drop the welcome
   * surface and render the chat.
   */
  onComplete: () => void;
}

type Step = 1 | 2 | 3 | 4;

/**
 * The multi-step onboarding wizard. Step 1 picks the provider,
 * step 2 collects the API key (and an optional base URL), step 3
 * surfaces the optional TTS / Grok / ego-browser knobs, and step 4
 * is the "Open the chat" hand-off.
 *
 * Validation lives on step 2: the "Next" button is disabled until
 * a non-empty key is present. The user can also dismiss the dialog
 * with the close button or the overlay click — that's the same as
 * the welcome's "Skip for now" path. We intentionally don't write
 * `is_onboarded = 1` on a dismiss, so a fresh launch shows the
 * welcome again.
 */
export function OnboardingDialog({
  initialSettings,
  onClose,
  onComplete,
}: OnboardingDialogProps) {
  const [step, setStep] = useState<Step>(1);
  const [providerKind, setProviderKind] = useState<ProviderKind | string>(
    initialSettings.provider_kind || "minimax",
  );
  const [apiKey, setApiKey] = useState(
    apiKeyFor(initialSettings, initialSettings.provider_kind) ?? "",
  );
  const [baseUrl, setBaseUrl] = useState(
    baseUrlFor(initialSettings, initialSettings.provider_kind),
  );
  const [showAdvanced, setShowAdvanced] = useState(false);
  // Optional knobs — pre-populated with whatever the user already
  // had in settings (e.g. seeded from .env).
  const [ttsVoice, setTtsVoice] = useState(initialSettings.tts_voice || "");
  const [showOptional, setShowOptional] = useState(false);
  const [submitting, setSubmitting] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const preset = presetFor(providerKind);
  const canAdvanceFromKey = apiKey.trim().length > 0;

  // When the user changes provider mid-step, swap the visible API
  // key/base URL to match. Saves a re-typing flow if they had a key
  // for OpenAI pre-filled and decide to switch to MiniMax.
  const handleSelectProvider = (next: ProviderKind | string) => {
    setProviderKind(next);
    setApiKey(apiKeyFor(initialSettings, next) ?? "");
    setBaseUrl(baseUrlFor(initialSettings, next));
  };

  const buildMerged = (): SettingsT => ({
    ...initialSettings,
    provider_kind: providerKind,
    default_model:
      initialSettings.default_model || presetFor(providerKind).default_model,
    minimax_api_key:
      providerKind === "minimax"
        ? apiKey.trim() || null
        : initialSettings.minimax_api_key,
    openai_api_key:
      providerKind === "openai"
        ? apiKey.trim() || null
        : initialSettings.openai_api_key,
    anthropic_api_key:
      providerKind === "anthropic"
        ? apiKey.trim() || null
        : initialSettings.anthropic_api_key,
    xai_api_key:
      providerKind === "xai" ? apiKey.trim() || null : initialSettings.xai_api_key,
    minimax_base_url:
      providerKind === "minimax" ? baseUrl.trim() : initialSettings.minimax_base_url,
    openai_base_url:
      providerKind === "openai" ? baseUrl.trim() : initialSettings.openai_base_url,
    anthropic_base_url:
      providerKind === "anthropic" ? baseUrl.trim() : initialSettings.anthropic_base_url,
    xai_base_url:
      providerKind === "xai" ? baseUrl.trim() : initialSettings.xai_base_url,
    tts_voice: ttsVoice.trim(),
  });

  const handleFinish = async () => {
    setError(null);
    setSubmitting(true);
    try {
      await saveSettings(buildMerged());
      await metaSet("is_onboarded", "1");
      onComplete();
    } catch (e) {
      setError(String(e));
    } finally {
      setSubmitting(false);
    }
  };

  return (
    <div className="modal-overlay" onClick={onClose}>
      <div
        className="modal-stacked onboarding-dialog"
        onClick={(e) => e.stopPropagation()}
      >
        <div className="modal-stacked-header">
          <h2>Set up MaxBot</h2>
          <div className="onboarding-steps">
            <span className={step >= 1 ? "onboarding-step active" : "onboarding-step"}>
              1. Provider
            </span>
            <span className="onboarding-step-sep">›</span>
            <span className={step >= 2 ? "onboarding-step active" : "onboarding-step"}>
              2. API key
            </span>
            <span className="onboarding-step-sep">›</span>
            <span className={step >= 3 ? "onboarding-step active" : "onboarding-step"}>
              3. Optional
            </span>
            <span className="onboarding-step-sep">›</span>
            <span className={step >= 4 ? "onboarding-step active" : "onboarding-step"}>
              4. Ready
            </span>
          </div>
        </div>

        <div className="modal-stacked-body onboarding-body">
          {step === 1 && (
            <div className="onboarding-step-pane">
              <p className="onboarding-step-blurb">
                Which provider would you like to use? You can add or
                switch providers later from Settings.
              </p>
              <div className="onboarding-provider-grid">
                {PROVIDER_PRESETS.map((p) => (
                  <button
                    key={p.kind}
                    className={
                      "onboarding-provider-card" +
                      (p.kind === providerKind ? " selected" : "")
                    }
                    onClick={() => handleSelectProvider(p.kind)}
                    type="button"
                  >
                    <div className="onboarding-provider-name">
                      {p.display_name}
                    </div>
                    <div className="onboarding-provider-desc">
                      {p.description}
                    </div>
                  </button>
                ))}
              </div>
            </div>
          )}

          {step === 2 && (
            <div className="onboarding-step-pane">
              <p className="onboarding-step-blurb">
                Paste your <strong>{preset.display_name}</strong> key.
                It's stored locally on your Mac in the MaxBot data
                directory.
              </p>
              <div className="field">
                <label>API key</label>
                <input
                  type="password"
                  value={apiKey}
                  onChange={(e) => setApiKey(e.target.value)}
                  placeholder={preset.key_placeholder}
                  autoFocus
                />
                <div className="hint">{preset.key_hint}</div>
              </div>
              <button
                type="button"
                className="onboarding-optional-toggle"
                onClick={() => setShowAdvanced((v) => !v)}
              >
                {showAdvanced ? "▾" : "▸"} Advanced — base URL override
              </button>
              {showAdvanced && (
                <div className="field">
                  <label>Base URL (optional)</label>
                  <input
                    type="text"
                    value={baseUrl}
                    onChange={(e) => setBaseUrl(e.target.value)}
                    placeholder={preset.default_base_url}
                  />
                  <div className="hint">
                    Leave blank for the built-in default. Set to a
                    self-hosted proxy or alternate region endpoint.
                  </div>
                </div>
              )}
            </div>
          )}

          {step === 3 && (
            <div className="onboarding-step-pane">
              <p className="onboarding-step-blurb">
                A few optional extras. Everything here has a sensible
                default — skip and configure later from Settings.
              </p>
              <button
                type="button"
                className="onboarding-optional-toggle"
                onClick={() => setShowOptional((v) => !v)}
              >
                {showOptional ? "▾" : "▸"} Configure now
              </button>
              {showOptional && (
                <div className="onboarding-optional-grid">
                  <div className="field">
                    <label>Text-to-Speech voice</label>
                    <input
                      type="text"
                      value={ttsVoice}
                      onChange={(e) => setTtsVoice(e.target.value)}
                      placeholder="Samantha"
                    />
                    <div className="hint">
                      Voice name passed to <code>say -v</code> when you
                      click the 🔊 Speak button. Leave blank for the
                      built-in default. Run <code>say -v ?</code> in
                      Terminal to list installed voices.
                    </div>
                  </div>
                </div>
              )}
              {!showOptional && (
                <div className="onboarding-optional-summary">
                  Defaults applied: TTS uses the system default voice.
                  You can change this from Settings → Text-to-Speech
                  any time.
                </div>
              )}
            </div>
          )}

          {step === 4 && (
            <div className="onboarding-step-pane onboarding-step-ready">
              <div className="onboarding-ready-mark">✓</div>
              <h3>You're all set</h3>
              <p className="onboarding-step-blurb">
                <strong>{preset.display_name}</strong> is selected.
                Click <strong>Open the chat</strong> to start.
              </p>
            </div>
          )}

          {error && <div className="error">{error}</div>}
        </div>

        <div className="modal-stacked-footer">
          {step > 1 ? (
            <button
              onClick={() => setStep((s) => (s - 1) as Step)}
              disabled={submitting}
            >
              Back
            </button>
          ) : (
            <button onClick={onClose} disabled={submitting}>
              Cancel
            </button>
          )}
          <div style={{ flex: 1 }} />
          {step < 4 ? (
            <button
              className="primary"
              onClick={() => setStep((s) => (s + 1) as Step)}
              disabled={step === 2 && !canAdvanceFromKey}
              title={
                step === 2 && !canAdvanceFromKey
                  ? "Enter an API key to continue"
                  : undefined
              }
            >
              Next →
            </button>
          ) : (
            <button
              className="primary"
              onClick={handleFinish}
              disabled={submitting}
            >
              {submitting ? "Saving…" : "Open the chat"}
            </button>
          )}
        </div>
      </div>
    </div>
  );
}
