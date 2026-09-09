import { useState } from "react";
import { OnboardingDialog } from "./OnboardingDialog";
import type { Settings as SettingsT } from "../lib/api";

interface WelcomeProps {
  /**
   * The current settings (may already have a key seeded from .env
   * on the Rust side). We pass it through to the dialog so the user
   * doesn't have to retype a key that's already there.
   */
  initialSettings: SettingsT;
  /**
   * Called when onboarding finishes (either via the dialog's "Open
   * the chat" CTA, the "I have a .env file" Done button, or the
   * "Skip for now" link). The parent flips `is_onboarded` locally
   * and replaces this view with the normal chat surface.
   */
  onComplete: () => void;
  /**
   * The persisted path the host will look at for the .env file. We
   * show this so the user can verify where to drop the file before
   * the Rust env_loader picks it up.
   */
  envHintPath: string;
}

/**
 * The first-run welcome surface. Centered, polished, a single
 * decision: "I have an API key" → open the onboarding wizard;
 * "I have a .env file" → tell the user where it goes and let
 * them carry on. "Skip for now" is the easy out for power users
 * who want to land in the empty state and configure later.
 */
export function Welcome({
  initialSettings,
  onComplete,
  envHintPath,
}: WelcomeProps) {
  const [showOnboarding, setShowOnboarding] = useState(false);
  const [showEnvHint, setShowEnvHint] = useState(false);

  return (
    <div className="welcome">
      <div className="welcome-mark">M</div>
      <h1 className="welcome-title">Welcome to MaxBot</h1>
      <p className="welcome-subtitle">
        A multi-provider AI desktop client with sub-agents, browser
        automation, and the Grok Build CLI in your toolbelt.
      </p>

      <div className="welcome-paths">
        <button
          className="welcome-path-card"
          onClick={() => setShowOnboarding(true)}
        >
          <div className="welcome-path-card-title">I have an API key</div>
          <div className="welcome-path-card-desc">
            Set up MiniMax, OpenAI, Anthropic, or xAI in a couple of clicks.
          </div>
        </button>
        <button
          className="welcome-path-card"
          onClick={() => setShowEnvHint((v) => !v)}
        >
          <div className="welcome-path-card-title">I have a .env file</div>
          <div className="welcome-path-card-desc">
            Drop it where MaxBot will find it on the next launch.
          </div>
        </button>
      </div>

      {showEnvHint && (
        <div className="welcome-env-hint">
          <div className="welcome-env-hint-label">
            MaxBot looks for a <code>.env</code> file in this order:
          </div>
          <ol className="welcome-env-hint-list">
            <li>
              <code>$MAXBOT_ENV</code>{" "}
              <span className="muted">(if set)</span>
            </li>
            <li>
              <code>./.env</code>{" "}
              <span className="muted">(current working directory)</span>
            </li>
            <li>
              <code>~/.maxbot.env</code>
            </li>
            <li>
              <code>~/dev/maxbot/maxbot/.env</code>
            </li>
          </ol>
          <div className="welcome-env-hint-actions">
            <code className="welcome-env-hint-path">{envHintPath}</code>
            <button
              className="ghost small"
              onClick={() => {
                // Best-effort: copy the path to clipboard. Reveal in
                // Finder is intentionally not wired up here because
                // we don't have a Tauri command for arbitrary
                // folders; users can ⌘-Shift-G in Finder and paste.
                if (navigator.clipboard) {
                  navigator.clipboard.writeText(envHintPath).catch(() => {});
                }
              }}
              title="Copy the first candidate path to the clipboard"
            >
              Copy path
            </button>
            <button className="primary" onClick={onComplete}>
              Done
            </button>
          </div>
          <div className="welcome-env-hint-foot">
            Restart MaxBot after saving the file so the keys seed into
            settings.
          </div>
        </div>
      )}

      <button className="welcome-skip" onClick={onComplete}>
        Skip for now →
      </button>

      {showOnboarding && (
        <OnboardingDialog
          initialSettings={initialSettings}
          onClose={() => setShowOnboarding(false)}
          onComplete={() => {
            setShowOnboarding(false);
            onComplete();
          }}
        />
      )}
    </div>
  );
}
