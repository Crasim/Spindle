# Spindle sync: design

Status: phase 1 built overnight 2026-11-23 (see "Phase 1 as built" at the end). Windows-only parts not yet compiled.

## Goal

Keep a person's Spindle the same on all their devices: conversations, projects, settings, skills, memories, schedules, and later plugins. It should work in three ways, and you can choose:

1. **Peer to peer.** Devices talk to each other directly. You pair them by scanning a QR code, or by typing a peer address and a code.
2. **Hosted, store.** A server keeps an encrypted copy, so a device can catch up even when no other device is on.
3. **Hosted, passthrough.** A server only relays traffic between devices that are online. It keeps nothing, or only a short queue.

None of these lets the server read the data. Every mode carries the same encrypted records, so a server or a relay only ever sees opaque blobs.

## What Spindle already has

`sync.rs` already does project thread sync through a shared folder:
- **Records:** each revision is encrypted with AES-256-GCM, using a key derived from a passphrase (PBKDF2).
- **History:** revision IDs are authenticated, and each revision points to its parent.
- **Deletes:** tombstones remove threads on other devices.
- **Conflicts:** a divergent edit becomes a separate thread.
- **Running work:** work stays owned by the device that started it.

That is the right core. The plan is to generalize it, not replace it:
- one record format for every kind of item;
- device keys instead of a typed passphrase;
- pluggable transports, with the folder kept as one of them.

## Model

### Sync controls

- Installed skills sync by default. Each device can opt out of sending and receiving skill records.
- Credential-free providers require opt-in on both devices. Each sender publishes its own connections. A loopback address becomes the sender's network hostname while retaining the scheme, port, and path. Existing authenticated connection records remain separate.
- A project folder carries an immutable ID under `.spindle`. Devices reuse an existing folder with the same ID, including a renamed copy. Imported project folders with IDs from earlier sessions keep their sync bindings.
- Stop sync and Resume sync publish separate project sharing state. Deletion publishes a separate permanent record so delayed project updates cannot recreate it. File deletion requires an explicit checkbox and a matching local project identity.
- Removed devices leave the active list, their LAN links are pruned, and hosted revocations remain queued until the server acknowledges them.

### Space

A **space** is one person's (or team's) synced Spindle. It has:
- **A space key:** 32 random bytes. Every record is encrypted with keys derived from it.
- **A space ID:** a hash of the space key, so servers can sort blobs without being able to read them.
- **Recovery without another device:** standard account security, no word phrase. On a hosted store you sign in with an account password (and optionally a passkey or TOTP). The space key is kept on the server only in a wrapped form: encrypted on your device with a key derived from your password (Argon2id, or PBKDF2 as Spindle uses today), the way password managers do it. The server never sees the password or the space key. With no hosted store, a new device joins by pairing with an existing one.

### Device

Each device has:
- an Ed25519 key pair for signing what it publishes;
- an X25519 key pair for pairing;
- a name and a list of capabilities (desktop, phone, headless).

The space keeps a signed **device list**. A device that is removed from it gets no new keys.

### Items and records

Everything that syncs is an **item** with a kind and an ID:

| Kind | Contents | How conflicts merge |
|---|---|---|
| `chat` | history, title, model, goal, plan, pending-work note | Appends merge. A divergent edit becomes a **branch** of the same chat (see below). |
| `project` | name, description, instructions, memory flag, chat list | Per field, the latest write wins (hybrid logical clock). |
| `project-files` | when the project's **sync files** switch (or the global default) is on: file contents in chunks, addressed by content, minus `.spindleignore` matches | Per file, the latest write wins. The losing version is kept as `name (from device).ext`. |
| `settings` | preferences, model tuning, fallbacks, budgets, appearance | Per key, the latest write wins. |
| `skill` | the SKILL.md and its files | Per skill, the latest write wins; the earlier version is kept. |
| `memory` | personal and project memories | Set union, with tombstones. |
| `schedule` | schedules and automations | Per field, the latest write wins. The device that runs a schedule is chosen per schedule. |
| `provider` | connection name, base URL, protocol, model list. **No secrets.** | Per field, the latest write wins. |
| `secret` | API keys for providers and services (OpenAI, Anthropic, OpenRouter, Fish Audio, speech servers and the like). **On by default.** **OAuth sign-ins never sync** (for example ChatGPT plan sign-in): their tokens and client registrations belong to one device, so each device signs in itself. | Latest write wins, with an extra layer of encryption. |
| `plugin` | later: manifest and files | Per plugin. |

A **record** is one revision of one item. It contains:

```
{ v, space, kind, id, parent[], clock (HLC), device, body }
```

The record is serialized, then encrypted with AES-256-GCM. Each kind gets its own key, derived from the space key with HKDF. The encrypted record is signed by the device. A record's ID is the SHA-256 of the encrypted bytes, so records are immutable and addressed by content. Transports only move records they don't already have.

A **head index** maps each item to its latest record IDs. It is just another encrypted record, so a new device can learn the current state without downloading the whole history.

Large files are split into chunks of about 1 MB that are deduplicated by content (like restic). A file record lists its chunk hashes.

### What syncs, per project

Two switches decide what a project shares: **sync conversations** and **sync files**. Each exists in two places:

- **Global (Settings › Sync):** the default for every project. The defaults are conversations **on** and files **off**.
- **Per project (the project's settings):** **Use global setting**, **On** or **Off**. The project's choice always wins over the global one.

What each switch covers:

- **Sync files off:** none of the project's files leave this device. The project's name, instructions and chats can still sync, depending on the other switch.
- **Sync conversations off:** the project's chats stay on this device. The project itself (name, instructions, memory switch) still appears on other devices, with a note that its chats are kept on another device.
- **Both off:** the project is listed on other devices only by name, marked "not synced". It can also be hidden from them completely with **Don't show on other devices**.

**Excluding files:** a `.spindleignore` file in the project (same syntax as `.gitignore`) keeps matching files from syncing even when file sync is on. Spindle always leaves out secrets-looking files (`.env`, `*.pem`, `*.key`, `id_*`), `.git`, `target`, `node_modules` and the sandbox copies.

**Per device, to limit bloat:** each device can choose, for each project with file sync on, between **download files** (the default on desktops) and **only when opened**. With "only when opened", the file list syncs, and the content is fetched when you open a file or a model reads it. A phone defaults to "only when opened".

**When the switches change:**
- **Turning a switch off** stops new records. It never deletes copies other devices already have. A **Remove from other devices** button sends tombstones; their sandbox copies and anything they exported aren't touched.
- **Turning one on** publishes the current state.

The switches themselves are project settings, so they sync too. Two devices therefore agree on what is shared, while what each device downloads stays its own choice.

### Conversation branches

A chat edited on two devices at once doesn't split into two chats. It becomes one chat with **branches**:

- **Main branch:** the line of the device where the conversation started. It stays main even when another device's edits arrive later.
- **Other branches:** when another device adds turns after a point the main branch has moved past, those turns become a branch named for that device ("from Laptop"), starting at the last turn both share.
- **In the chat:** a small marker at the fork shows "2 branches", with a switcher like the one for edited messages. Reading a branch shows the shared turns and then that branch's turns.
- **Continuing a branch:** typing in a branch continues that branch. **Make main** promotes it; the old main becomes a branch, so nothing is lost. **Delete branch** removes it everywhere (a tombstone).
- **Running work:** a task belongs to the branch and device where it started, so another device's branch never resumes it.
- **Fewer branches:** two devices that add the *same* turns (for example both imported the same revision) don't make a branch. Turns are compared by content hash.

Records carry each turn's branch ID and parent turn, so merging is a tree merge: no edit is ever dropped, and every device ends up with the same tree.

### When sync happens

The aim is to keep both devices' copies current before anyone types, so branches stay rare.

- **In the background:** changes go out shortly after they are saved, and arrive live where the transport can push them (hosted server or LAN peer). Otherwise Spindle checks every few minutes.
- **On demand:** **Sync now** in the chat header, in Settings › Sync, and as `/sync` or `spindle sync now`.
- **When a conversation opens:** Spindle fetches that chat's latest records first, so you see the other device's turns before you add one. The chat shows right away and updates when the fetch arrives (within about 2 seconds).
- **Before sending in a synced chat:** a quick fetch of that chat. If new turns arrived, your message goes after them on the same branch, so nothing forks. If the fetch can't reach any transport within a short wait (about 3 seconds), the message is sent anyway; a branch can form later, and the indicator says so.
- **Settings:** each of these can be turned off: sync on open, sync before sending, and background sync. With all three off, sync happens only when you ask.

### Sync indicator

- **Where:** in the chat header, next to the title, and in the sidebar for each synced chat.
- **States:**
  - **Synced:** a quiet check with the time of the last sync, shown on hover.
  - **Syncing:** a small spinner.
  - **Couldn't sync:** a warning mark, for example "Couldn't sync: server not reachable · last synced 14 min ago". Clicking it shows the reason (offline, sign-in expired, server error, device removed) and a **Retry**.
- **Couldn't sync before sending:** the turn carries the same warning mark, so you know it may branch.
- **Not synced:** chats or kinds you've turned off show nothing.

### Sync protocol (the same over every transport)

1. **Have/want:** each side sends a summary of its heads (one hash per kind); where they differ, they swap record IDs.
2. **Fetch:** each side asks for the records it's missing, which arrive in batches.
3. **Apply:** check the signature, decrypt, merge by kind, then save.

Every transport has to provide only three things: `list(since)`, `get(ids)` and `put(records)`, plus `watch` for live updates where it can.

## Sync and remote work

**Sync** copies state: chats, projects, settings and so on. **Remote** asks a particular device to do work: run tools, read files that live only there, or use its models and GPU. They share devices, keys and transports, but they answer different questions. Sync answers "what does everyone know?"; remote answers "who does the work?".

### Where work runs: the "Run on" setting

Projects (and chats outside projects) **sync as usual**. A separate **Run on** setting says which device does the work:

- **Run on: This device** (the default) works as now. To run anything, this device needs the project's files: its own copy, or file sync.
- **Run on: <host>** (for example "Desktop") sends every message in the project's chats to that host. It runs the tools, models and scripts with its own files, sandbox and permissions. You watch progress live, and the finished turns arrive by sync like any other change.

How it's set:
- **Per project**, in the project's settings. All of its chats inherit it.
- **Per chat**, an override in the chat header, for example one chat in a project that runs locally.
- **Per device:** each device has its own Run on choice, so a project can run here on the desktop and on Desktop from the laptop. A new device that has the project but not its files defaults to the project's **home device** (the one that created it, or the one marked as home).

**Why sync and remote fit together:** with Run on set to a host, this device needs only the conversation, not the files. Its file sync for that project drops to **only when opened** automatically, so it can still look at a file but never downloads the project to stay current. When you switch Run on back to This device, Spindle offers to download the files.

**Example:**
- **The project** "VRT reports" lives on Desktop, with file sync off.
- **On the laptop**, the project appears with its chats, set to Run on: Desktop.
- **You type** a request; Desktop runs it against its own files and models, and the reply shows up on both machines.
- **The files** never leave Desktop.

**Moving work:** you can change Run on for a project or chat at any time. Work already running finishes where it started.

### Remote requests

These are the same operations the local control channel already has for the CLI and MCP (`ask`, `send`, `status`, `approve`, `deny`, `cancel`), sent over the sync transports and signed by the sending device:

- **Live updates:** progress streams back to every device showing the chat (steps, tool notes and the reply as it is written) over the live channel, which is the hosted relay's WebSocket or a direct LAN connection. Spindle doesn't wait for the next sync to show it.
- **Target offline:** the request waits in the hosted mailbox (or on the sending device) and shows "Waiting for Desktop to come online". You can cancel it, or **Run here instead** when this device can do it. Waking a sleeping PC (Wake-on-LAN) could come later.
- **Order:** requests carry the chat's branch and the last turn the sender saw, so the target runs them in order and on the right branch. If the target has moved on, the request is answered on that branch, with no fork.

### Permissions for remote work

- **Accepting remote work:** each device has **Accept remote work** (off by default), with a list of the devices allowed to send it. A phone can be allowed to send without being allowed to receive.
- **The target decides:** sandbox, trust levels and approval levels (once, conversation, project) work exactly as when you sit at that device. Remote work never gets more than local work.
- **Approvals from another device:** an approval card appears on the device that sent the request, with the summary Spindle wrote. You can approve it there when the target allows **Approve from paired devices**. For actions outside the sandbox (running host commands, writing outside the project), the target can require you to be at that machine. That setting is on by default.
- **Secrets stay put:** remote work uses the target's own credentials, so a request never carries keys.

### Commands

- `spindle remote list` shows devices and whether they accept work.
- `spindle remote ask --on Desktop "…"` runs a prompt on another device.
- `spindle remote status` shows what is running where.
- MCP gets matching tools (`spindle_ask` with an optional `device`).

## Background service

Sync and remote work need Spindle running when its window is closed. The service is the same binary in a headless mode, built on the windowless `spindle serve` that already backs the CLI and MCP.

### What it does

- **`spindle service`:** runs the engine without a window and never exits on its own (unlike `spindle serve`, which stops after 5 idle minutes). It:
  - holds the workspace;
  - runs background sync and keeps the live connection to the hosted relay or LAN peers;
  - runs schedules that are due (replacing the Task Scheduler entry, which only checked once a minute);
  - and, when **Accept remote work** is on, takes remote requests.
- **The window becomes a client.** When the service is running, the desktop window connects to it over the local control channel (the same token-protected loopback connection the CLI and MCP use), instead of opening the workspace itself. Closing the window leaves the work running. Opening it shows whatever is in progress, including remote work.
- **Without the service** Spindle works as now: the window owns the workspace. If you start the service later, the window hands over (the existing `stop`/hand-over path, which waits until it's idle).
- **Resources:** the service doesn't load models itself. It talks to Ollama and other servers the way the window does. When idle it just waits on its connections.

### Starting it

- **Settings › Sync › Run Spindle in the background**, or `spindle service install` and `spindle service uninstall`, plus `spindle service status`.
- **Windows:** a per-user background process started at sign-in (a Task Scheduler "At log on" task, hidden, restart on failure), not a Windows service under SYSTEM. It has to run as you, because the sandbox (AppContainer) and stored keys (DPAPI) are per user. See the tray icon, below.
- **Linux:** a `systemd --user` unit (`spindle.service`, restart on failure). For a headless host, `loginctl enable-linger` keeps it running without a signed-in session; Spindle prints that command, it doesn't run it.
- **Headless hosts** (a server with no desktop) run only the service. You manage them from another device through remote work, or with the CLI on the host.

### Tray icon

The service shows a tray icon (the Windows notification area; on Linux, a StatusNotifierItem through the `ksni` crate, so no GTK). Its icon reflects the state: idle, syncing, working, or "couldn't sync". The menu has:

- **Open Spindle** (also on double-click): opens the window, or brings it forward if it's already open.
- **Status lines:** "Synced 2 min ago" or the reason it couldn't sync, and "Working: <chat>" while something runs.
- **Pause sync and remote work** / **Resume**.
- **Exit Spindle:** a full exit. It stops the service *and* closes any open window, and frees the workspace.
  - **If work is running,** it asks first: **Exit anyway** (running work is held and resumes or can be reviewed next time, like a cancelled run) or **Cancel**.
  - **Not permanent:** after Exit, the service starts again at the next sign-in. To stop that, use Settings › Sync › Run Spindle in the background, or `spindle service uninstall`.

Closing the window never exits; only the tray's Exit or `spindle service stop` does. Without the service, there's no tray icon and closing the window exits as it does today.

### Safety

- **Network access:** the service listens on the network only for what you turned on. Peer-to-peer listens on the LAN; remote requests arrive through the relay or a LAN connection, signed by a paired device. The control channel stays on loopback with its token.
- **Same rules as the window:** sandbox, trust levels and approvals behave exactly as they do in the window. An approval waits until you answer it, in the window or from a paired device where that's allowed. Nothing is approved because no one is watching.
- **Pause:** Settings and the tray can pause sync and remote work without stopping the service.

## Transports

### 1. Folder (exists)

A shared folder, for example OneDrive, Syncthing or an SMB share. Its layout moves to `spindle-sync/<space>/records/ab/abcdef…`. The current project sync folders keep working.

### 2. Peer to peer

- **On the same network:** devices announce themselves over mDNS (`_spindle._tcp`) and connect directly over TLS, using their device keys (QUIC or TCP).
- **Across networks:** by default the connection goes through a relay (option 3, below). Making connections through NAT routers directly is phase 3.

**Pairing by QR code (easiest, especially for a phone):**
1. The existing device shows a QR code containing `spindle://pair?v=1&dev=<pubkey>&addr=<ip:port,…>&relay=<url>&code=<128-bit one-time secret>&exp=<time>`.
2. The new device scans it, connects to `addr` (or through `relay`), and runs a handshake keyed with the one-time secret. The new device then shows a number from 1 to 100, worked out from the handshake key and never sent; you type it on the existing device. Only then are the keys sent, so someone who guessed the code still needs to see the new device's screen. Three wrong numbers end the pairing.
3. The existing device sends the space key, encrypted to the new device's key, and the new device downloads the head index and then the records.

**Pairing by typing (desktop to desktop, or no camera):** enter a short code such as `ABCD-EFGH-JKLM` together with the address `192.168.1.42:43117` or a relay pairing ID. Because the code is short, the handshake uses a PAKE (SPAKE2), so the code can't be guessed offline. The code expires after 10 minutes, and 5 wrong tries cancel it.

**Joining without another device:** sign in to the hosted store with your account. The device downloads the wrapped space key, unwraps it locally with your password, and is added to the device list. The server requires the sign-in before handing out the wrapped key, and rate-limits attempts.

### 3. Hosted

The server is one small program in the same binary: `spindle sync-server --data DIR --listen 0.0.0.0:8443`. It also comes as a Docker image, so it can run on your home server.

**Modes (set per server, and per space when you register it):**
- **Store:** keeps every encrypted record and the head index. Devices sync whenever they connect. Old records can be trimmed once every device has seen them.
- **Passthrough (relay):** keeps nothing. Devices that are online are connected to each other, and the server forwards encrypted frames. It can also act as a mailbox: a queue held for N hours (24 by default) for a device that is offline, so that it catches up. With the mailbox time set to 0 it's a pure relay.
- **Both:** store, and also relay live changes so they arrive within seconds.

**Authentication:** a device proves it is on the space's device list by signing a challenge. Registering a space requires an admin token for the server. The server never sees the space key.

**API:** HTTPS with JSON for control, and a WebSocket for live updates and relaying:
- `PUT /v1/spaces/{space}/records` (batch)
- `GET /v1/spaces/{space}/records?since=cursor`
- `POST /v1/spaces/{space}/get` (by ID)
- `GET /v1/spaces/{space}/live` (WebSocket: change notices, relay frames and pairing)

## Security

- **End to end:** records are encrypted and signed on the device. Servers, relays and shared folders hold only ciphertext and metadata: the space ID, record sizes and timestamps.
- **Removing a device:** sign a new device list and rotate the space key. New records use the new key. As today, the removed device keeps what it already received.
- **Secrets:**
  - API keys sync by default; you can turn that off per device.
  - They are encrypted a second time with a key held only by devices allowed to have secrets (desktops by default; a phone companion gets none).
  - After they arrive, each device stores them the way it does now (DPAPI on Windows).
  - OAuth sign-ins (tokens, refresh tokens, client registrations, PKCE state) are never synced. A synced provider that uses OAuth shows "Sign in on this device".
  - Anything else bound to one device (lock files, sandbox state, local paths that don't exist elsewhere) stays local.
- **Pairing codes:** they expire, are single-use and are rate-limited, and pairing ends with you typing the new device's 1–100 number on the existing device.
- **Running work:** execution stays with the device that started it (as today). Synced pending work shows as "running on <device>". Approvals for sandboxed or risky actions are never synced as granted.

## UI

**Settings › Sync** contains:
- **This device:** its name, plus an on/off switch for each kind of item.
- **Devices:** a list with a "last seen" time for each, and **Remove**.
- **Add a device:** shows the QR code and a typed code with the address, or a field to paste a `spindle://pair` link.
- **Transports:**
  - Folder: pick a folder.
  - Hosted: server URL, mode, and a status line.
  - Peer to peer: on or off, plus the port.
- **Account (hosted store):** sign in, change the password, add a passkey or TOTP, and sign out of this device.
- **Activity:** the latest syncs and any conflicts, each conflict with a link to both versions.

There are also commands:
- `spindle sync status`
- `spindle sync pair` (prints the QR code to the terminal)
- `spindle sync join <link|code>`
- `spindle sync-server …`

## Phases

**Phase 1 (tonight's build):**
- The core:
  - space and device keys, the record format, and the hosted account with a password-wrapped space key;
  - per-kind merges for `chat`, `project`, `settings`, `skill`, `memory`, `schedule`, `provider` and `secret` (API keys only; OAuth excluded);
  - the head index and the have/want protocol over an in-memory transport, with tests.
- Transports:
  - the folder transport (the existing project sync migrated alongside it);
  - the hosted server in store mode and passthrough mode, with the mailbox;
  - the client for the hosted server.
- Pairing:
  - pairing links and QR codes (the `qrcode` crate draws them in egui and in the terminal);
  - typed codes with SPAKE2.
- The Settings › Sync page.
- The background service: `spindle service` (headless, no idle exit, background sync and schedules), with `spindle service install|uninstall|status` (Windows sign-in task, Linux systemd user unit) and the window connecting to it as a client, plus the tray icon (Open Spindle, status, Pause/Resume, and Exit Spindle, which stops everything).
- Sync now, sync on open, sync before sending (each can be turned off), and the sync indicator with Retry.
- Records already carry each project's home device and "Run on" choices, so remote work can be added in phase 2 without changing the format.
- The global and per-project switches for conversations and files (the project wins). The file transfer itself is phase 2, but the switches and `.spindleignore` are honoured from the start.

**Phase 2:**
- remote work: the per-project and per-chat "Run on" setting (per device), remote requests with live progress, the mailbox for offline devices, remote approvals, and `spindle remote …`;
- peer to peer on the same network (mDNS and direct TLS) with live updates;
- project files in chunks;
- the Docker image;
- a pairing page for phones (the browser companion).

**Phase 3:**
- connections across networks without a relay;
- an Android app;
- plugin sync once plugins exist.

## Decisions made without asking (change any of them)

- Hosted means **you run the server** (your home server or a VPS). Spindle does not operate a hosted service.
- API keys **sync**, encrypted a second time. **OAuth sign-ins never sync**: each device signs in itself.
- Project **files** don't sync by default, and chats do. Both are global defaults that each project can override (the project wins).
- Peer to peer across networks goes **through the relay** for now.
- **No recovery phrase.** Without a hosted store, a new device must pair with one you already have; with one, you sign in to your account.

## Open questions

1. Will the server run on your home server (`model-host`, or another box)? Should it be reachable from outside your network (through a reverse proxy with TLS), or stay on the LAN or a VPN only?
2. Should the global default for project files stay off, with projects turning it on where needed?
3. Which devices may hold API keys: every desktop, or only ones you mark?
4. Should a phone be a full device (an Android app later) or only a companion that reads chats and sends prompts to a desktop?
5. How long should the passthrough mailbox keep messages for an offline device: 24 hours, 7 days, or none?
6. Remote approvals: is approving from your phone fine for sandboxed actions only (the default), or also for host commands?
7. Teams: should more than one person ever share a space (with permissions per project), or is it always one person?

## Phase 1 as built

**Built:**
- the core (keys, signed records, the chat branch tree, the device list with key rotation);
- folder and hosted transports, and peer to peer on the local network (UDP announcements with a space tag, a signed X25519 handshake checked against the device list, encrypted frames; no server or folder needed);
- `spindle sync-server` (store, relay, mailbox, live channel, account sign-in, pairing rendezvous);
- pairing by link/QR or typed code (SPAKE2, then the new device shows a 1–100 number that is typed on the existing device; three wrong numbers end it), over the server or the LAN;
- the adapter for chats, settings, skills, memories, providers, API keys, schedules and projects, with the global and per-project switches;
- Sync now, on open and before sending;
- `spindle service` with install/uninstall/status/stop and the tray (Open, status, Pause/Resume, Exit holding work);
- Settings › Sync, the chat-header indicator and the branch bar;
- live wake-ups from the server.

- an Android app (`android/Spindle.apk`): the same program as a full device, with a one-column phone layout.

**Different from the design, for now:**
- Peer discovery uses UDP broadcast rather than mDNS, and connections are TCP with Spindle's own handshake rather than TLS or QUIC. Devices on other subnets are added by address.
- The phone syncs only while the app is open, and runs no commands (no sandbox on Android).
- The window doesn't attach to a running service. One owner at a time hands the workspace over, and closing the window hides it to the tray when background running is on.
- The per-project switches are on Settings › Sync, not on the project's page.
- A sent turn isn't marked when the quick pre-send sync failed; the header indicator shows the failure instead.
- There is no head-hash summary in have/want.
- The account has no passkey or TOTP yet.
- Linux can't store synced API keys.
- File transfer, `.spindleignore` enforcement and download-on-open come in phase 2, with remote work.

