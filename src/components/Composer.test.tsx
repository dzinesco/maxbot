// v2.4.0 — Composer tests for the group-mode path.
// The chat-mode path is exercised by integration
// tests elsewhere; this file pins the mention-
// routing behavior + the empty-mention hint.

import { describe, expect, it, vi } from "vitest";
import { fireEvent, render, screen } from "@testing-library/react";
import { Composer } from "./Composer";

const groupMembers = [
  { id: "researcher_id", name: "Researcher" },
  { id: "writer_id", name: "Writer" },
];

describe("Composer (group mode)", () => {
  it("shows a 'mention a Bot' hint when no mentions are present", () => {
    render(
      <Composer
        onSend={vi.fn()}
        onStop={vi.fn()}
        onOpenSendToBot={vi.fn()}
        hasBots={true}
        streaming={false}
        lastAssistantText={null}
        ttsSpeaking={false}
        onToggleSpeakLast={vi.fn()}
        mode="group"
        groupMembers={groupMembers}
        onGroupSend={vi.fn()}
      />,
    );
    expect(
      screen.getByTestId("group-composer-hint").textContent,
    ).toMatch(/Mention a Bot with/);
  });

  it("disables Send when no mentions are present", () => {
    render(
      <Composer
        onSend={vi.fn()}
        onStop={vi.fn()}
        onOpenSendToBot={vi.fn()}
        hasBots={true}
        streaming={false}
        lastAssistantText={null}
        ttsSpeaking={false}
        onToggleSpeakLast={vi.fn()}
        mode="group"
        groupMembers={groupMembers}
        onGroupSend={vi.fn()}
      />,
    );
    const send = screen.getByTestId("group-composer-send") as HTMLButtonElement;
    expect(send.disabled).toBe(true);
  });

  it("calls onGroupSend with the parsed mentions on submit", () => {
    const onGroupSend = vi.fn();
    render(
      <Composer
        onSend={vi.fn()}
        onStop={vi.fn()}
        onOpenSendToBot={vi.fn()}
        hasBots={true}
        streaming={false}
        lastAssistantText={null}
        ttsSpeaking={false}
        onToggleSpeakLast={vi.fn()}
        mode="group"
        groupMembers={groupMembers}
        onGroupSend={onGroupSend}
      />,
    );
    const ta = screen.getByTestId("group-composer-textarea") as HTMLTextAreaElement;
    fireEvent.change(ta, { target: { value: "@Researcher and @Writer please" } });
    const send = screen.getByTestId("group-composer-send") as HTMLButtonElement;
    expect(send.disabled).toBe(false);
    fireEvent.click(send);
    expect(onGroupSend).toHaveBeenCalledTimes(1);
    const [body, mentions] = onGroupSend.mock.calls[0];
    expect(body).toMatch(/@Researcher/);
    expect(mentions).toEqual(["researcher_id", "writer_id"]);
  });
});
