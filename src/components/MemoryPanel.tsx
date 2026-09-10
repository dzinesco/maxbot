// v2.5.0 — MemoryPanel.
//
// Read-only browser for a single Bot's persistent memory. Three
// sections (facts, preferences, history) rendered as columns on
// wide screens and stacked on narrow ones. The user can add new
// facts / preferences via a small form, and delete any entry
// with the 🗑 button.
//
// Memory lives on the Bot's VM (JSONL files accessed via SFTP).
// Read failures collapse to an empty list — a Bot without a
// provisioned VM simply shows "no entries yet" in each column.

import { useCallback, useEffect, useState } from "react";
import {
  memoryForget,
  memoryList,
  memoryRemember,
} from "../lib/tauri";
import type { MemEntry, MemKind } from "../lib/api";

interface MemoryPanelProps {
  /** Which Bot's memory to show. If null/empty, the panel
   *  renders a friendly "select a Bot" placeholder. */
  botId: string | null;
}

const KIND_LABEL: Record<MemKind, string> = {
  fact: "Facts",
  preference: "Preferences",
  history: "History",
};

const KIND_GLYPH: Record<MemKind, string> = {
  fact: "🧠",
  preference: "⚙️",
  history: "🕘",
};

export function MemoryPanel({ botId }: MemoryPanelProps) {
  const [facts, setFacts] = useState<MemEntry[]>([]);
  const [prefs, setPrefs] = useState<MemEntry[]>([]);
  const [history, setHistory] = useState<MemEntry[]>([]);
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState<string | null>(null);
  // The active "add" form. We keep the form inputs local to
  // each column so the user can leave one half-filled while
  // looking at another.
  const [factKey, setFactKey] = useState("");
  const [factContent, setFactContent] = useState("");
  const [prefKey, setPrefKey] = useState("");
  const [prefContent, setPrefContent] = useState("");
  const [savedFlash, setSavedFlash] = useState<{ id: number; text: string } | null>(null);

  const refresh = useCallback(async () => {
    if (!botId) return;
    setLoading(true);
    setError(null);
    try {
      const [f, p, h] = await Promise.all([
        memoryList(botId, "fact"),
        memoryList(botId, "preference"),
        memoryList(botId, "history"),
      ]);
      setFacts(f);
      setPrefs(p);
      setHistory(h);
    } catch (e) {
      setError(String(e));
    } finally {
      setLoading(false);
    }
  }, [botId]);

  useEffect(() => {
    void refresh();
  }, [refresh]);

  // Show a "Saved ✓" toast for 1.5s after a successful add.
  useEffect(() => {
    if (!savedFlash) return;
    const t = setTimeout(() => setSavedFlash(null), 1500);
    return () => clearTimeout(t);
  }, [savedFlash]);

  const handleAdd = useCallback(
    async (kind: "fact" | "preference", key: string, content: string) => {
      if (!botId) return;
      const k = key.trim();
      const c = content.trim();
      if (!k || !c) return;
      try {
        await memoryRemember(botId, kind, k, c);
        const flashId = Date.now();
        setSavedFlash({ id: flashId, text: `Saved ${kind} “${k}”` });
        await refresh();
      } catch (e) {
        setError(String(e));
      }
    },
    [botId, refresh],
  );

  const handleDelete = useCallback(
    async (kind: MemKind, key: string) => {
      if (!botId) return;
      // v3.6.0 (Phase 7) — Confirm before delete.
      // Per the brief, the per-row 🗑 button in the
      // panel needs a confirmation prompt so a
      // stray click can't lose a confirmed fact /
      // preference. History rows are exempt — they
      // have no delete button (the panel only
      // renders one for fact / preference).
      //
      // We use `window.confirm` for symmetry with
      // the rest of the renderer (no modal
      // library is currently in use). The message
      // is short and includes the entry's key so
      // the user can verify what they're about to
      // lose.
      const ok = window.confirm(
        `Delete ${kind} “${key}” from this Bot's memory? This cannot be undone.`,
      );
      if (!ok) return;
      try {
        await memoryForget(botId, kind, key);
        await refresh();
      } catch (e) {
        setError(String(e));
      }
    },
    [botId, refresh],
  );

  if (!botId) {
    return (
      <div className="memory-panel empty">
        <p>Select a Bot to browse its memory.</p>
      </div>
    );
  }

  return (
    <div className="memory-panel" data-testid="memory-panel" data-setting-key="panel.memory">
      <header className="memory-panel-header">
        <h2>Memory</h2>
        <p className="memory-panel-sub">
          Persistent across app restarts. Stored on this Bot's VM
          ({KIND_GLYPH.fact} facts, {KIND_GLYPH.preference} preferences,{" "}
          {KIND_GLYPH.history} history).
        </p>
        {savedFlash && (
          <div className="memory-panel-flash" data-testid="memory-flash">
            ✓ {savedFlash.text}
          </div>
        )}
        {error && <p className="memory-panel-error">Error: {error}</p>}
        {loading && <p className="memory-panel-loading">Loading…</p>}
      </header>

      <div className="memory-panel-columns">
        <MemoryColumn
          kind="fact"
          entries={facts}
          formKey={factKey}
          formContent={factContent}
          onChangeKey={setFactKey}
          onChangeContent={setFactContent}
          onSubmit={() => {
            void handleAdd("fact", factKey, factContent);
            setFactKey("");
            setFactContent("");
          }}
          onDelete={handleDelete}
        />
        <MemoryColumn
          kind="preference"
          entries={prefs}
          formKey={prefKey}
          formContent={prefContent}
          onChangeKey={setPrefKey}
          onChangeContent={setPrefContent}
          onSubmit={() => {
            void handleAdd("preference", prefKey, prefContent);
            setPrefKey("");
            setPrefContent("");
          }}
          onDelete={handleDelete}
        />
        <MemoryColumn
          kind="history"
          entries={history}
          formKey={null}
          formContent={null}
          onChangeKey={null}
          onChangeContent={null}
          onSubmit={null}
          onDelete={handleDelete}
        />
      </div>
    </div>
  );
}

interface MemoryColumnProps {
  kind: MemKind;
  entries: MemEntry[];
  formKey: string | null;
  formContent: string | null;
  onChangeKey: ((v: string) => void) | null;
  onChangeContent: ((v: string) => void) | null;
  onSubmit: (() => void) | null;
  onDelete: (kind: MemKind, key: string) => void | Promise<void>;
}

function MemoryColumn({
  kind,
  entries,
  formKey,
  formContent,
  onChangeKey,
  onChangeContent,
  onSubmit,
  onDelete,
}: MemoryColumnProps) {
  return (
    <section className={`memory-column memory-column--${kind}`} data-testid={`memory-col-${kind}`}>
      <h3>
        <span className="memory-column-glyph">{KIND_GLYPH[kind]}</span>{" "}
        {KIND_LABEL[kind]}
        <span className="memory-column-count"> ({entries.length})</span>
      </h3>

      {onSubmit && onChangeKey && onChangeContent && formKey !== null && formContent !== null && (
        <form
          className="memory-form"
          onSubmit={(e) => {
            e.preventDefault();
            onSubmit();
          }}
          data-testid={`memory-form-${kind}`}
        >
          <input
            type="text"
            placeholder="key (snake_case)"
            value={formKey}
            onChange={(e) => onChangeKey(e.target.value)}
            className="memory-form-key"
            aria-label={`${kind} key`}
          />
          <textarea
            placeholder="value"
            value={formContent}
            onChange={(e) => onChangeContent(e.target.value)}
            className="memory-form-content"
            rows={2}
            aria-label={`${kind} content`}
          />
          <button
            type="submit"
            className="primary small"
            disabled={!formKey.trim() || !formContent.trim()}
            data-testid={`memory-add-${kind}`}
          >
            Add {kind}
          </button>
        </form>
      )}

      {entries.length === 0 ? (
        <p className="memory-empty">
          No {KIND_LABEL[kind].toLowerCase()} yet.
          {kind === "history"
            ? " History auto-writes after each turn."
            : ""}
        </p>
      ) : (
        <ul className="memory-list" data-testid={`memory-list-${kind}`}>
          {entries.map((e, idx) => (
            <li key={`${e.kind}-${e.key || "history"}-${idx}`} className="memory-item">
              <div className="memory-item-body">
                {kind !== "history" && (
                  <div className="memory-item-key">{e.key}</div>
                )}
                <div className="memory-item-content">{e.content}</div>
                <div className="memory-item-meta">
                  {new Date(e.created_at).toLocaleString()}
                </div>
              </div>
              {kind !== "history" && (
                <button
                  type="button"
                  className="memory-item-delete"
                  onClick={() => onDelete(kind, e.key)}
                  title={`Delete ${kind} "${e.key}"`}
                  aria-label={`Delete ${kind} ${e.key}`}
                >
                  🗑
                </button>
              )}
            </li>
          ))}
        </ul>
      )}
    </section>
  );
}
