// v2.0 Slice E — backward-compat wrapper around `BotRoster`.
//
// Before Slice E, the sidebar's Bot sidebar panel was a
// component called `BotsPanel` that lived next to
// `ChatView`. Slice E replaces it with the `BotRoster` (the
// roster *is* the sidebar now — the conversation list is
// gone). Anything that still imports `BotsPanel` (e.g. older
// entry points, tests, or external tools) keeps working:
//
//   - `BotsPanel` is re-exported as an alias of `BotRoster`,
//     so `import { BotsPanel } from "./BotsPanel"` resolves
//     to the same component.
//   - The legacy prop names (`onCreate`, `bots`, etc.) are
//     mapped to `BotRoster`'s new prop names in the wrapper
//     below, so old call sites don't need a rewrite.
//
// The component the renderer actually mounts is `BotRoster`
// (see `Sidebar.tsx`). This file exists so a casual search
// for "BotsPanel" still finds the active surface.

import {
  BotRoster,
  type BotRosterProps,
} from "./BotRoster";

// Legacy prop names mapped to the new ones. Kept as a
// separate interface so old call sites get a clear type
// error if they pass a removed prop.
export interface BotsPanelProps {
  bots: BotRosterProps["bots"];
  selectedBotId?: BotRosterProps["selectedBotId"];
  lastRunsByBot?: BotRosterProps["lastRunsByBot"];
  computersByBot?: BotRosterProps["computersByBot"];
  lastActionsByBot?: BotRosterProps["lastActionsByBot"];
  onSelectBot: BotRosterProps["onSelectBot"];
  /** Legacy alias for `onCreateBot`. */
  onCreate?: BotRosterProps["onCreateBot"];
  /** New name. Takes priority over `onCreate` if both are
   *  passed (defensive — callers shouldn't pass both). */
  onCreateBot?: BotRosterProps["onCreateBot"];
  onOpenComputer?: BotRosterProps["onOpenComputer"];
}

export function BotsPanel(props: BotsPanelProps) {
  return (
    <BotRoster
      bots={props.bots}
      selectedBotId={props.selectedBotId ?? null}
      lastRunsByBot={props.lastRunsByBot}
      computersByBot={props.computersByBot}
      lastActionsByBot={props.lastActionsByBot}
      onSelectBot={props.onSelectBot}
      onCreateBot={props.onCreateBot ?? props.onCreate ?? (() => {})}
      onOpenComputer={props.onOpenComputer}
    />
  );
}

// Re-export `BotRoster` so any old import that wants to
// reach the new component via this file finds it under the
// same path.
export { BotRoster };
export type { BotRosterProps } from "./BotRoster";
