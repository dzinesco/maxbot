# Grok and Grok Bot — practical reference

A practical guide to **Grok** (the chat assistant) and **Grok Bot**
(the newer AI-teammate product). People mix the two names up, so
both are covered. The "how the bots interact" part is specifically
about Grok Bot.

Useful as a design reference for MaxBot's per-Bot ComputerPanel
work: MaxBot's Slice C/D/E (per-Bot Linux VM, noVNC console, SFTP
file browser) is a simpler, single-Bot take on the same idea.

---

## Two different products

| | **Grok** | **Grok Bot** |
|---|---|---|
| What it is | Conversational assistant | Persistent AI teammates that do real work |
| Where | grok.com, X, iOS/Android Grok apps | Separate Grok Bot desktop + phone apps |
| Main job | Answer, write, search, generate media | Log into *your* tools and finish the job |
| Runs on | Chat session | A persistent cloud computer (VM) |
| Stops when you leave? | Chat waits | Keeps working 24/7 |
| Access | Free to start; SuperGrok raises limits | Beta: SuperGrok Plus/Heavy and certain Cursor plans |

**Grok Bot** is the product whose official tagline is "hand real
work to AI teammates on a persistent cloud computer."

---

## Grok (the assistant) — feature map

This is the Grok you talk to here, on grok.com, in the Grok apps,
and as **@grok** on X.

**Chat and reasoning**
- Back-and-forth chat, writing, coding, analysis
- Configurable reasoning / "think" modes
- Memory across chats (preferences and past context)
- File uploads: PDFs, images, spreadsheets, code, audio
- Vision: screenshots, photos, diagrams
- Multi-agent / Heavy-style modes on higher plans for hard problems

**Live information**
- Real-time web search with citations
- Native live **X** search (posts, sentiment, trends) — this is still
  Grok's unique data edge
- Deep / agentic research that browses and synthesizes sources

**Create**
- **Grok Imagine**: text-to-image, image edit, text-to-video /
  image-to-video (short clips; higher resolutions on paid tiers)
- Code generation, debug, and explanation in the same thread

**Voice**
- Hands-free voice with low latency
- Many built-in voices, multilingual
- On mobile, live camera / visual Q&A in some builds

**Connected work**
- Connectors for email, files, calendar, and other apps so Grok can
  act *inside* a chat
- Scheduled automations / recurring prompts on some plans

**On X specifically**
- Sidebar / Grok icon for a private chat
- Reply to any post with **@grok** and it can answer in-thread
- Post-level Grok button to explain text or images
- Premium+ "enhance your post" on web

**What Grok is *not***  
It does not have its own always-on desktop. It drafts and
researches; it does not reliably log into Salesforce, click through
Zendesk, or keep working after you close the laptop. That is Grok
Bot.

---

## Grok Bot — full feature guide

A **Bot** = one named, persistent teammate (Sales Outbound, Chief of
Staff, Expense Manager, etc.). You message it like a coworker. It
finishes work in real apps and only pings you when it needs a
decision.

### 1. Its own cloud computer
Each account gets **one persistent cloud VM** with:
- Browser
- Filesystem
- Terminal
- Signed-in sessions and cookies

Work happens in the actual tools — Gmail, Salesforce, Sheets,
LinkedIn, Zendesk, internal sites — even when there is **no API**.
You can open **Agent Computer**, watch clicks and typing, take over
for a password / 2FA / CAPTCHA, then hand control back. Closing your
laptop does not stop it.

Important security fact: **all of *your* Bots share that one
computer.** Logins and files are shared so handoffs work. Do not
treat two Bots as a security wall. Other users cannot see your VM.

### 2. Computer use + connectors
- **Connectors / MCP** when a clean integration exists
- **Computer use** (click, type, navigate) when it doesn't
- Optional local-computer execution, with approval, if you
  explicitly enable it
- Plugins / marketplace skills
- Cloud coding agents can be spawned for engineering work (admins
  can disable this)

### 3. Persistent memory and identity
Each Bot keeps:
- Name, title, description, avatar
- Role context and preferences
- Conversation history
- Learned workflows

Context **compounds**. It is not a fresh sandbox every task.
Different Bots have separate personalities and chat memory, but
they share the computer's files and logins.

### 4. Skills and routines
- **Skill** = reusable "how we do this" playbook (steps, inputs,
  validation, what needs approval)
- **Routine** = run that skill on a schedule (e.g. every weekday at
  8am) or on demand

Typical path: do a task once → correct it → "save this as a skill"
→ turn it into a routine. You can also **show** a Bot a workflow
once and have it follow along.

### 5. Approvals and control
You stay in the loop for:
- Logins, 2FA, payments, CAPTCHAs
- Anything you mark as requiring approval (customer emails,
  publishes, money)
- "Stop now" (halts new work; does not undo finished actions)

Bots are designed to come back when a human judgment is needed, not
to spam you for every click.

### 6. Surfaces
- Desktop app: macOS, Windows, Linux
- Mobile: iOS 18+ / Android 9+ (phones, not iPad)
- Same Bots, chats, routines, and computer sync across devices
- Team / Enterprise admin controls live in the Cursor dashboard
  (network, plugins, recording, identity)

### 7. Example jobs xAI actually cites
- Overnight pipeline: research accounts, score contacts, draft
  email + LinkedIn in your voice, leave an approval list
- CRM hygiene and weekly scoreboards
- Ops: seat new hires, process invoices from Gmail
- Support: work a Zendesk queue after you sign it in
- Eng: reproduce a UI bug, file the ticket, hand the fix to a
  debugger Bot

---

## How the bots interact

This is the part that is actually new.

### Shared computer = shared workplace
All Bots on your account sit on the **same VM**. If Sales signs
into Salesforce, Ops can use that session. If Research saves a
spreadsheet, Writer can open it. Each Bot has its **own screen**,
so several can browse in parallel. One Bot = one computer-use task
on its screen at a time.

### Direct Bot-to-Bot messages
Bots can message each other asynchronously. The receiving Bot wakes,
does its piece, and replies later. You can see that in the
transcript. Use this when:
- One Bot owns the source system, another owns the deliverable
- A specialist should review a draft
- A long job should continue without you as the router

Ask for **one owner per stage** so they don't duplicate work.

### Group chats (2–6 Bots)
Create a group, name the shared outcome, assign the next owner.

Kickoff pattern from the docs:

> @Researcher gather the source material and link every claim.  
> @Writer turn the findings into a launch draft.  
> @Reviewer check the draft against the sources and list only
> blocking issues.  
> Do not publish anything.

In groups you can:
- `@BotName` to assign
- `@everyone` for a rare all-hands update
- Threads for feedback on one result
- Reactions as lightweight "got it"
- Written replies for anything that changes instructions or safety

Handoffs in a group are text; images should be sent Bot-to-Bot
directly.

### Typical multi-bot pipeline

```text
You
  └─ group: "Q3 outbound"
        ├─ Research Bot  → accounts + sources
        ├─ Writer Bot    → email / LinkedIn drafts in your voice
        ├─ Reviewer Bot  → blocking issues only
        └─ Chief of Staff Bot → sequence, CRM notes, what needs you
```

You are not the middleware. They pass ownership. You approve the
customer-facing step.

### What they remember vs what they share

| Stays with one Bot | Shared across your Bots |
|---|---|
| Personality, role, private chat | Cloud files |
| Learned preferences for that role | Browser sessions / logins |
| Its own conversation | Group messages and handoffs |

### Limits of interaction
- They are **your** teammates, not a public swarm. They don't talk
  to other people's Bots.
- Shared computer means shared credentials. Sensitive work that
  must be isolated needs a **separate user account**, not a second
  Bot.
- A direct message from you beats background work and can redirect
  the current turn.

---

## What makes Grok Bot special

Most chat AIs (including regular Grok) get you to "pretty good
draft." Grok Bot is built for the last mile: **the work lands in
the real tool.**

xAI's own framing:

1. **A real computer, not a chat box.** Persistent VM + browser +
   filesystem means it can use tools that have no API. That is the
   difference between "here's an email draft" and "the draft is
   sitting in Gmail / Salesforce waiting for send."
2. **Teammates, not one mega-prompt.** Named Bots keep role
   memory. They coordinate without you routing every file.
3. **Learn by watching once.** Demonstrate a messy multi-app path;
   it becomes a skill, then a scheduled routine.
4. **Always on.** Cloud-side execution. Laptop closed, phone in
   your pocket, work continues.
5. **Human only at judgment points.** 2FA, money, customer send,
   publish — you take over the screen, then give it back.

The product quote that captures it best: there is a huge difference
between 90% done and 100% done; most AI stops at 90%, Grok Bot is
aimed at finishing in the place a human would finish.

### How that compares to "just Grok"
Regular Grok is still special in a different way: **live X + web**,
less hedged personality, Imagine media, voice, and a single sync'd
assistant across web/apps/X. Use Grok to think and create. Use
Grok Bot to operate your stack overnight.

---

## Access (as of the current docs)

**Grok:** free to start; SuperGrok / SuperGrok Heavy raise weekly
usage across chat, Imagine, voice, and connectors.

**Grok Bot (beta):** SuperGrok Plus, SuperGrok Heavy; Cursor Pro+,
Ultra; Cursor Teams Standard/Premium. Desktop + iOS/Android
companion. Usage is separate from ordinary Grok chat. If you have
both Cursor and SuperGrok, it uses whichever bucket is larger.
Enterprise is more locked down (network controls, action recording,
identity).

Official starting points:
- Grok overview: <https://docs.x.ai/grok/overview>
- Grok Bot overview: <https://docs.x.ai/grok-bot/overview>
- Launch post: <https://x.ai/news/introducing-grok-bot>
- Product page: <https://x.ai/bot>
