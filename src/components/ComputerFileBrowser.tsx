// v2.0.4 ComputerFileBrowser — minimal SFTP-backed file
// browser for a per-Bot VM.
//
// Two-pane layout:
//   ┌─ left: directory listing (path bar + entries)
//   └─ right: file viewer (or empty state)
//
// Read-only in this slice. `computerFileWrite` is wired
// through `src/lib/tauri.ts` but the UI doesn't expose an
// edit affordance yet — that's a separate slice. The
// underlying Rust write uses atomic rename, so adding
// the UI later is just a `<textarea>` + a Save button.
//
// `botId` is opaque to the browser; the parent passes
// the ID and the browser calls the SFTP Tauri commands.
// Errors surface inline (no silent failures) and the
// back/forward stack is a flat list — no need for full
// history semantics at this scope.

import { useCallback, useEffect, useState } from "react";
import type { SftpEntry } from "../lib/api";
import { computerFileList, computerFileRead } from "../lib/tauri";

interface ComputerFileBrowserProps {
  botId: string;
}

const HOME_HINT = "/home/bot";

export function ComputerFileBrowser({ botId }: ComputerFileBrowserProps) {
  const [cwd, setCwd] = useState<string>(HOME_HINT);
  // Flat path stack so "up" is O(1) and we can keep the
  // history in a single string. We don't model a full
  // history (no back/forward) — the user can re-navigate.
  const [stack, setStack] = useState<string[]>([HOME_HINT]);
  const [entries, setEntries] = useState<SftpEntry[] | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [loadingDir, setLoadingDir] = useState(false);
  const [openFile, setOpenFile] = useState<string | null>(null);
  const [fileContent, setFileContent] = useState<string | null>(null);
  const [loadingFile, setLoadingFile] = useState(false);
  const [fileError, setFileError] = useState<string | null>(null);

  const refresh = useCallback(
    async (path: string) => {
      setLoadingDir(true);
      setError(null);
      try {
        const list = await computerFileList(botId, path);
        // Sort: directories first, then by name. Stable
        // ordering matters more than alphabetical perfection
        // here — the user just wants to find a file.
        const sorted = [...list].sort((a, b) => {
          if (a.is_dir !== b.is_dir) return a.is_dir ? -1 : 1;
          return a.name.localeCompare(b.name);
        });
        setEntries(sorted);
      } catch (e) {
        setError(String(e));
        setEntries(null);
      } finally {
        setLoadingDir(false);
      }
    },
    [botId],
  );

  useEffect(() => {
    refresh(cwd);
  }, [cwd, refresh]);

  const navigateTo = useCallback(
    (path: string) => {
      setCwd(path);
      setStack((s) => [...s, path]);
      setOpenFile(null);
      setFileContent(null);
      setFileError(null);
    },
    [],
  );

  const goUp = useCallback(() => {
    if (stack.length <= 1) return;
    const next = stack[stack.length - 2];
    setStack((s) => s.slice(0, -1));
    setCwd(next);
    setOpenFile(null);
    setFileContent(null);
    setFileError(null);
  }, [stack]);

  const openEntry = useCallback(
    async (entry: SftpEntry) => {
      const path = joinPath(cwd, entry.name);
      if (entry.is_dir) {
        navigateTo(path);
        return;
      }
      setOpenFile(path);
      setFileContent(null);
      setFileError(null);
      setLoadingFile(true);
      try {
        const content = await computerFileRead(botId, path);
        setFileContent(content);
      } catch (e) {
        setFileError(String(e));
      } finally {
        setLoadingFile(false);
      }
    },
    [botId, cwd, navigateTo],
  );

  return (
    <div className="cfb" data-testid="computer-file-browser">
      <div className="cfb__pane cfb__pane--left">
        <div className="cfb__path">
          <button
            type="button"
            className="cfb__up-btn"
            onClick={goUp}
            disabled={stack.length <= 1 || loadingDir}
            aria-label="Go up one directory"
            title="Up"
          >
            ↑
          </button>
          <span className="cfb__path-text" title={cwd}>
            {cwd}
          </span>
          <button
            type="button"
            className="cfb__refresh-btn"
            onClick={() => refresh(cwd)}
            disabled={loadingDir}
            aria-label="Refresh directory listing"
            title="Refresh"
          >
            {loadingDir ? "…" : "↻"}
          </button>
        </div>
        {error ? (
          <div className="cfb__error" data-testid="cfb-error">
            {error}
          </div>
        ) : entries === null ? (
          <div className="cfb__loading">Loading…</div>
        ) : entries.length === 0 ? (
          <div className="cfb__empty">This directory is empty.</div>
        ) : (
          <ul className="cfb__list" role="list">
            {entries.map((e) => (
              <li key={e.name} className="cfb__item">
                <button
                  type="button"
                  className={
                    "cfb__item-btn" +
                    (e.is_dir ? " cfb__item-btn--dir" : "") +
                    (openFile === joinPath(cwd, e.name)
                      ? " cfb__item-btn--active"
                      : "")
                  }
                  onClick={() => openEntry(e)}
                  disabled={loadingDir}
                  data-testid={e.is_dir ? "cfb-dir" : "cfb-file"}
                >
                  <span className="cfb__item-icon" aria-hidden="true">
                    {e.is_dir ? "▸" : "·"}
                  </span>
                  <span className="cfb__item-name">{e.name}</span>
                  {!e.is_dir ? (
                    <span className="cfb__item-size">
                      {formatSize(e.size)}
                    </span>
                  ) : null}
                </button>
              </li>
            ))}
          </ul>
        )}
      </div>
      <div className="cfb__pane cfb__pane--right">
        {openFile === null ? (
          <div className="cfb__viewer-empty">
            Select a file to view its contents.
          </div>
        ) : loadingFile ? (
          <div className="cfb__viewer-loading">Loading {openFile}…</div>
        ) : fileError !== null ? (
          <div className="cfb__error" data-testid="cfb-file-error">
            {fileError}
          </div>
        ) : fileContent !== null ? (
          <>
            <div className="cfb__viewer-header" title={openFile}>
              {openFile}
            </div>
            <pre className="cfb__viewer-content">{fileContent}</pre>
          </>
        ) : null}
      </div>
    </div>
  );
}

/** POSIX-style path join. Handles trailing/leading slashes
 * so `joinPath("/home/bot", "Documents")` → `"/home/bot/Documents"`
 * and `joinPath("/", "etc")` → `"/etc"`. The Rust SFTP side
 * accepts absolute paths; we never send a relative path. */
function joinPath(parent: string, child: string): string {
  if (child.startsWith("/")) return child;
  if (parent.endsWith("/")) return parent + child;
  return `${parent}/${child}`;
}

function formatSize(n: number): string {
  if (n < 1024) return `${n} B`;
  if (n < 1024 * 1024) return `${(n / 1024).toFixed(1)} KB`;
  if (n < 1024 * 1024 * 1024)
    return `${(n / 1024 / 1024).toFixed(1)} MB`;
  return `${(n / 1024 / 1024 / 1024).toFixed(2)} GB`;
}
