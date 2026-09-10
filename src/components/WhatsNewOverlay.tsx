// v3.0.1 — "What's new in v3" first-launch overlay.
//
// A modal that summarizes the v3.0 design polish the user just
// upgraded to. On dismiss, the parent persists a "seen" flag in
// the `meta` table (key: `seen_v3_intro`) so the overlay never
// reappears on subsequent launches.
//
// Scope: the overlay is purely presentational. It owns the
// backdrop click, the Esc keypress, and the "Got it" button. It
// calls the `onDismiss` prop to tell the parent "go persist the
// flag and unmount me". The parent is responsible for the
// persistence + re-render decision.

import { useEffect } from "react";

export const SEEN_V3_INTRO_META_KEY = "seen_v3_intro";

interface WhatsNewOverlayProps {
  /** Called when the user dismisses via Got-it button, Esc, or
   * backdrop click. The parent should persist the seen flag and
   * unmount the overlay. */
  onDismiss: () => void;
}

/** The 5 polish bullets. Kept as a module constant so the
 * vitest suite can assert the overlay renders them verbatim
 * without re-importing the component's render. Each line is one
 * sentence — short enough to read in <5s, specific enough to
 * give the user a mental anchor for the change. */
export const WHATS_NEW_V3_BULLETS: readonly string[] = [
  "Brand palette: violet → Electric Blue.",
  "Body font: Inter → system sans (no webfont download).",
  "Tactile button feedback on every click.",
  "Skeleton shimmer for loading states.",
  "ActivityFeed keeps last good data if a poll fails.",
] as const;

export function WhatsNewOverlay({ onDismiss }: WhatsNewOverlayProps) {
  // Esc dismisses. We register on `window` (not the overlay
  // div) so the event fires regardless of which child has
  // focus — the Got-it button is auto-focused on mount, but
  // a click on the backdrop can leave focus on body and we
  // still want Esc to work.
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape") {
        e.stopPropagation();
        onDismiss();
      }
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [onDismiss]);

  return (
    <div
      className="whats-new-overlay"
      onClick={onDismiss}
      data-testid="whats-new-overlay"
      role="dialog"
      aria-modal="true"
      aria-labelledby="whats-new-title"
    >
      <div
        className="whats-new-modal"
        onClick={(e) => e.stopPropagation()}
        data-testid="whats-new-modal"
      >
        <div className="whats-new-mark" aria-hidden="true">
          M
        </div>
        <h2 id="whats-new-title" className="whats-new-title">
          What's new in v3
        </h2>
        <p className="whats-new-subtitle">
          MaxBot 3.0 is a design-polish release — same features,
          a tighter, more focused feel.
        </p>
        <ul className="whats-new-bullets" data-testid="whats-new-bullets">
          {WHATS_NEW_V3_BULLETS.map((bullet, idx) => (
            <li
              key={idx}
              className="whats-new-bullet"
              data-testid="whats-new-bullet"
            >
              {bullet}
            </li>
          ))}
        </ul>
        <div className="whats-new-footer">
          <button
            type="button"
            className="primary whats-new-got-it"
            onClick={onDismiss}
            data-testid="whats-new-got-it"
            autoFocus
          >
            Got it
          </button>
        </div>
      </div>
    </div>
  );
}
