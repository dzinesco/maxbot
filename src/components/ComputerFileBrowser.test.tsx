// Component tests for the v2.0.4 `ComputerFileBrowser`.
// happy-dom doesn't ship a Tauri runtime, so we mock
// `../lib/tauri` and feed canned `computerFileList` /
// `computerFileRead` / `computerFileWrite` results.

import { describe, expect, it, vi, beforeEach } from "vitest";
import { render, screen, waitFor, fireEvent } from "@testing-library/react";

vi.mock("../lib/tauri", () => ({
  computerFileList: vi.fn(),
  computerFileRead: vi.fn(),
  computerFileWrite: vi.fn(),
}));

// ComputerFileBrowser lives at the import path below; the
// lazy import must come AFTER the mock so the mocked module
// is what the component sees.
import { ComputerFileBrowser } from "./ComputerFileBrowser";
import { computerFileList, computerFileRead, computerFileWrite } from "../lib/tauri";

beforeEach(() => {
  vi.mocked(computerFileList).mockReset();
  vi.mocked(computerFileRead).mockReset();
  vi.mocked(computerFileWrite).mockReset();
});

describe("ComputerFileBrowser", () => {
  it("lists the initial directory (defaults to /home/bot)", async () => {
    vi.mocked(computerFileList).mockResolvedValueOnce([
      { name: "Documents", is_dir: true, size: 0 },
      { name: "notes.txt", is_dir: false, size: 128 },
    ]);
    render(<ComputerFileBrowser botId="bot-1" />);
    // The path bar should show /home/bot before any
    // navigation.
    expect(
      screen.getByTitle("/home/bot"),
    ).toBeInTheDocument();
    // Wait for the entries to render.
    await waitFor(() => {
      expect(screen.getByText("Documents")).toBeInTheDocument();
    });
    expect(screen.getByText("notes.txt")).toBeInTheDocument();
    // computerFileList must be called once with the
    // default path.
    expect(vi.mocked(computerFileList)).toHaveBeenCalledWith(
      "bot-1",
      "/home/bot",
    );
  });

  it("navigates into a directory and calls computerFileList with the joined path", async () => {
    vi.mocked(computerFileList)
      .mockResolvedValueOnce([
        { name: "Documents", is_dir: true, size: 0 },
      ])
      .mockResolvedValueOnce([
        { name: "report.md", is_dir: false, size: 4096 },
      ]);
    render(<ComputerFileBrowser botId="bot-1" />);
    await waitFor(() => {
      expect(screen.getByText("Documents")).toBeInTheDocument();
    });
    fireEvent.click(screen.getByText("Documents"));
    await waitFor(() => {
      expect(
        screen.getByTitle("/home/bot/Documents"),
      ).toBeInTheDocument();
    });
    expect(vi.mocked(computerFileList)).toHaveBeenLastCalledWith(
      "bot-1",
      "/home/bot/Documents",
    );
  });

  it("reads a file and shows its content in the viewer", async () => {
    vi.mocked(computerFileList).mockResolvedValueOnce([
      { name: "hello.txt", is_dir: false, size: 13 },
    ]);
    vi.mocked(computerFileRead).mockResolvedValueOnce(
      "hello world!\n",
    );
    render(<ComputerFileBrowser botId="bot-1" />);
    await waitFor(() => {
      expect(screen.getByText("hello.txt")).toBeInTheDocument();
    });
    fireEvent.click(screen.getByText("hello.txt"));
    await waitFor(() => {
      expect(screen.getByText("hello world!")).toBeInTheDocument();
    });
    // The reader is called with the full joined path.
    expect(vi.mocked(computerFileRead)).toHaveBeenCalledWith(
      "bot-1",
      "/home/bot/hello.txt",
    );
  });

  it("surfaces an SFTP error inline instead of failing silently", async () => {
    vi.mocked(computerFileList).mockRejectedValueOnce(
      "ssh: no route to host",
    );
    render(<ComputerFileBrowser botId="bot-1" />);
    await waitFor(() => {
      expect(
        screen.getByTestId("cfb-error"),
      ).toBeInTheDocument();
    });
    expect(
      screen.getByText("ssh: no route to host"),
    ).toBeInTheDocument();
  });

  it("supports Edit / Save: clicking Edit shows a textarea, Save calls computerFileWrite", async () => {
    vi.mocked(computerFileList).mockResolvedValueOnce([
      { name: "notes.txt", is_dir: false, size: 13 },
    ]);
    vi.mocked(computerFileRead).mockResolvedValueOnce(
      "original\n",
    );
    vi.mocked(computerFileWrite).mockResolvedValueOnce(undefined);
    render(<ComputerFileBrowser botId="bot-1" />);
    // Open the file so the right-pane viewer renders.
    await waitFor(() => {
      expect(screen.getByText("notes.txt")).toBeInTheDocument();
    });
    fireEvent.click(screen.getByText("notes.txt"));
    await waitFor(() => {
      expect(screen.getByTestId("cfb-edit-btn")).toBeInTheDocument();
    });
    // No textarea yet — we're in read-only view.
    expect(
      screen.queryByTestId("cfb-editor-textarea"),
    ).not.toBeInTheDocument();
    // Click Edit → textarea appears, pre-filled with the
    // read content.
    fireEvent.click(screen.getByTestId("cfb-edit-btn"));
    const textarea = await screen.findByTestId(
      "cfb-editor-textarea",
    );
    expect(textarea).toBeInTheDocument();
    expect((textarea as HTMLTextAreaElement).value).toBe("original\n");
    // Type into the textarea, then click Save.
    fireEvent.change(textarea, {
      target: { value: "edited content\n" },
    });
    fireEvent.click(screen.getByTestId("cfb-save-btn"));
    // The write must have been called with the bot id, the
    // full joined path, and the new content.
    await waitFor(() => {
      expect(vi.mocked(computerFileWrite)).toHaveBeenCalledWith(
        "bot-1",
        "/home/bot/notes.txt",
        "edited content\n",
      );
    });
    // After a successful save we drop back to read-only
    // view, so the textarea is gone and the Edit button is
    // back.
    await waitFor(() => {
      expect(
        screen.queryByTestId("cfb-editor-textarea"),
      ).not.toBeInTheDocument();
    });
    expect(
      screen.getByTestId("cfb-edit-btn"),
    ).toBeInTheDocument();
  });
});
