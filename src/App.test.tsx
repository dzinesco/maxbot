import { act, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { DEFAULT_SETTINGS, blankBot } from "./lib/api";
import * as api from "./lib/tauri";
import App from "./App";

vi.mock("./lib/tauri", async (importOriginal) => {
  const original = await importOriginal<typeof import("./lib/tauri")>();
  return Object.fromEntries(Object.keys(original).map((key) => [key, vi.fn().mockResolvedValue([])]));
});
vi.mock("./components/Sidebar", () => ({ Sidebar: (props: any) => <div>
  <button onClick={() => props.onSelectBot("b2")}>Select second bot</button>
  <button onClick={() => props.onSelectView("memory")}>Memory view</button>
</div> }));
vi.mock("./components/ChatView", () => ({ ChatView: ({ messages }: any) => <div>{messages.map((m: any) => <p key={m.id}>{m.content}</p>)}</div> }));
vi.mock("./components/Composer", () => ({ Composer: (props: any) => <div>Chat composer{props.mode === "group" && <button disabled={props.streaming} onClick={() => props.onGroupSend("hello", ["b1"])}>Send group test</button>}</div> }));
vi.mock("./components/MemoryPanel", () => ({ MemoryPanel: () => <div>Memory panel</div> }));

const conversation = (id: string, bot_id: string) => ({ id, bot_id, title: id, created_at: "2026-01-01", updated_at: "2026-01-01" });
const message = (content: string) => ({ id: content, conversation_id: "c1", role: "assistant", content, tool_calls: [], created_at: "2026-01-01", error_message: null });
let unlisten: ReturnType<typeof vi.fn<() => void>>;
beforeEach(() => {
  vi.clearAllMocks();
  unlisten = vi.fn();
  for (const fn of [api.onChunk, api.onDone, api.onError, api.onBotChunk, api.onBotDone, api.onBotError]) vi.mocked(fn).mockResolvedValue(unlisten);
  vi.mocked(api.getSettings).mockResolvedValue(DEFAULT_SETTINGS);
  vi.mocked(api.metaGet).mockResolvedValue("1");
  vi.mocked(api.listBots).mockResolvedValue([ { ...blankBot(), id: "b1", name: "First" }, { ...blankBot(), id: "b2", name: "Second" } ]);
  vi.mocked(api.listConversations).mockResolvedValue([conversation("c1", "b1"), conversation("c2", "b2")] as any);
  vi.mocked(api.getMessages).mockResolvedValue([]);
});

describe("App integration", () => {
  it("ignores an old transcript that arrives after selecting another bot and returns from memory to chat", async () => {
    let resolveOld!: (messages: any[]) => void;
    vi.mocked(api.getMessages).mockImplementation((id) => id === "c1" ? new Promise((resolve) => { resolveOld = resolve; }) : Promise.resolve([message("Second transcript")] as any));
    render(<App />);
    await waitFor(() => expect(api.getMessages).toHaveBeenCalledWith("c1"));
    fireEvent.click(screen.getByText("Memory view"));
    expect(screen.getByText("Memory panel")).toBeInTheDocument();
    fireEvent.click(screen.getByText("Select second bot"));
    await screen.findByText("Second transcript");
    await act(async () => resolveOld([message("Stale transcript")]));
    expect(screen.queryByText("Stale transcript")).toBeNull();
    expect(screen.getByText("Second transcript")).toBeInTheDocument();
  });

  it("releases subscriptions when a later registration fails", async () => {
    vi.mocked(api.onError).mockRejectedValueOnce(new Error("listener failure"));
    const view = render(<App />);
    await waitFor(() => expect(unlisten).toHaveBeenCalledTimes(2));
    view.unmount();
    expect(unlisten).toHaveBeenCalledTimes(2);
  });

  it("releases listeners that finish registering after unmount", async () => {
    let resolve!: (fn: () => void) => void;
    vi.mocked(api.onChunk).mockReturnValueOnce(new Promise((r) => { resolve = r; }));
    const view = render(<App />);
    view.unmount();
    await act(async () => resolve(unlisten));
    expect(unlisten).toHaveBeenCalledTimes(6);
  });
});

it("refreshes group history after persistence and clears the busy state", async () => {
  vi.mocked(api.groupGet).mockResolvedValue({ chat: { id: "g1", name: "Test group", owner_bot_id: "b1" }, member_bot_ids: ["b1"] } as any);
  let finish!: (id: string) => void;
  vi.mocked(api.groupRunTurn).mockReturnValueOnce(new Promise((resolve) => { finish = resolve; }));
  render(<App />);
  await screen.findByText("Chat composer");
  act(() => window.dispatchEvent(new CustomEvent("maxbot:select-group", { detail: { groupId: "g1" } })));
  await screen.findByTestId("group-chat-header");
  fireEvent.click(screen.getByText("Send group test"));
  await waitFor(() => expect(api.groupRunTurn).toHaveBeenCalled());
  expect(screen.getByText("Send group test")).toBeDisabled();
  vi.mocked(api.groupHistory).mockResolvedValue([{ id: "gm", group_id: "g1", bot_id: "b1", role: "assistant", content: "Persisted group response", mentions: [], handoff_to: null, created_at: "2026-01-01" }]);
  await act(async () => finish("gm"));
  expect(await screen.findByText("Persisted group response")).toBeInTheDocument();
  expect(screen.getByText("Send group test")).not.toBeDisabled();
});
