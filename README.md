# MaxBot

MaxBot is a personal multi-provider AI desktop client with **per-Bot Linux computers**, browser automation, and the Grok Build CLI wired into the same chat surface. It's a Tauri 2 app (Rust core, React 19 / Vite 6 / TypeScript-strict renderer) that keeps conversations, bots, and their VMs on disk so a model can keep working across launches without leaning on a server.

## Features

- **Multi-provider LLM** — MiniMax (default), OpenAI, Anthropic, xAI; switch in Settings → General. Each provider has its own API key and base URL override.
- **Streaming chat with tool use** — SSE-driven token streaming, structured tool-call deltas, per-call consent for the tools that touch the outside world.
- **Per-Bot Linux computers (v2.0)** — each Bot gets its own QEMU/KVM VM on a Linux server you control (libvirt, virbr0, cloud-init, Ubuntu 24.04 + XFCE). MaxBot drives libvirt over SSH, generates per-Bot Ed25519 keypairs (encrypted at rest with Argon2id + ChaCha20-Poly1305), and tunnels the VNC desktop to the Mac via `ssh -L` + a local WebSocket↔RFB proxy. No SaaS, no remote-desktop provider.
- **Bot roster + 6-state presence (v2.0)** — the sidebar opens to a **Bot roster**, not a conversation list. Each Bot has an avatar that animates through `idle / thinking / working / waiting / blocked / done`. The roster is the primary surface; conversations are scoped per-Bot.
- **Three access levels per Bot (Status / Preview / Takeover)** — a tiny status chip in the title bar; a pinned Preview side panel with the noVNC viewer; full-window Takeover with mouse + keyboard. The human can see what the Bot is doing at any time.
- **Specialist-Bot templates** — one-click starters for Figma Specialist (Figma Bro), Code Reviewer (Devbot), Researcher (Detective), and Inbox Triage (Mailroom) — the names and system prompts are seeded, the user can edit before saving.
- **ego-browser** — drives a real Chromium via the [ego (lite)](https://github.com/ego-lite/ego) embedded Node.js runtime, so the agent can work with authenticated sites without competing for the user's own tabs.
- **Grok Build CLI integration** — spawns `grok agent stdio` as a JSON-RPC subprocess and persists the session id, so a relaunch resumes the same Grok Build conversation.
- **TTS via macOS `say`** — speak the last response with a single keystroke; per-message 🔊 buttons for the older turns. Voice is configurable from the Settings → TTS tab.
- **AppleScript Computer Use** — drive macOS apps (browser, windows, mail, calendar, reminders, notes, system, Things 3, app launcher) with per-app TCC consent surfaced in Settings → General.
- **MCP client** — stdio JSON-RPC client loads servers from `mcp_servers.json` and exposes their tools to the model with the `mcp__<server>__<tool>` prefix. Per-call consent.

## Install

```bash
# 1. clone
git clone git@github.com:dzinesco/maxbot.git
cd maxbot/maxbot

# 2. copy the env template and fill in your keys
cp .env.example .env
$EDITOR .env   # add at least one API key (MINIMAX_API_KEY by default)

# 3. build
npm install
npm run tauri:dev   # development with hot-reload
# or
npm run tauri:build # production .app bundle in src-tauri/target/release/bundle/macos
```

The first launch walks you through a short onboarding wizard that surfaces the macOS permissions MaxBot needs (Automation, Accessibility, Full Disk Access for some tool paths). You can revisit it from Settings → General once the v1 onboarding slice lands.

## Where data is stored

Everything lives in `~/Library/Application Support/com.maxbot.app/`:

- `maxbot.sqlite` — conversations, messages, settings, bots, schedules, runs, bot inbox, computers (per-Bot VM state), ssh_keys (encrypted per-Bot SSH keypairs)
- `bots/<bot-id>/` — per-bot folder (system prompt scratch, memory, downloaded artifacts)
- `grok/` — the working directory for the Grok Build subprocess when no override is set
- `mcp_servers.json` — the MCP server registry

**Per-Bot VMs** live on your Linux server, not on the Mac. They're created and destroyed via libvirt over SSH — see `docs/server-setup.md` for the one-time server setup.

Conversations are local-first. Nothing is sent to a third party except the configured LLM provider (the API key you set in Settings determines where the prompts and completions go), the Grok Build subprocess (when the agent invokes it), the Linux server you point Settings → Computer at, and any MCP servers you've enabled.

## Keyboard shortcuts

| Shortcut | Action |
| --- | --- |
| `⌘F` | Focus the sidebar search box |
| `⌘N` | New chat |
| `⌘⇧S` | Speak / stop the last assistant response |
| `Enter` | Send (in the composer) |
| `Shift+Enter` | Newline (in the composer) |
| `Esc` | Close the active modal |

Settings tabs (when the Settings modal is open):

| Shortcut | Tab |
| --- | --- |
| `⌘1` | General |
| `⌘2` | Providers |
| `⌘3` | TTS |
| `⌘4` | Browser |
| `⌘5` | Grok |
| `←` / `→` | Move between tabs (when the tab bar is focused) |

## License

<!-- Tyler: pick a license -->

## Contributing

MaxBot is a personal tool — the patterns inside it (multi-provider LLM client, Tauri + React 19 desktop app shape, sub-agent runtime, ego-browser bridge, MCP client) are reusable but the project itself isn't aiming to be a general-purpose product. PRs that match the personal-tool vision are welcome; large refactors that would turn this into a multi-tenant platform probably aren't. Open an issue first if you're planning something non-trivial.
