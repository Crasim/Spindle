# Spindle CLI and MCP · plan

**Status (2026-11-22): built.** Decisions taken as proposed: approvals only at the keyboard (no trusted caller), windowless owner stops after 5 idle minutes, external chats in the normal list, Spindle-as-MCP-client later. Transport changed from named pipe / Unix socket to loopback TCP + per-user token file, so Windows and Linux work the same. See README › Using Spindle from other programs.

How other programs (a terminal, scripts, Claude Code, Codex, VS Code, other agents) will use
Spindle: a command-line interface, and an MCP server that exposes Spindle as tools.

## Where things stand

- One engine (`engine.rs`) driven by text commands (`/new`, `/model`, `/approve 4 once`, …),
  returning a `View` snapshot. The desktop is one front end; `--cli` is a bare stdin loop.
- Headless pieces already exist: `--ask MODEL PROMPT`, `--models`, `--eval` (one chat end to
  end, approvals automated), `--run-due` (scheduled work), `--setup-python`, `--probe`.
- One Spindle owns a workspace at a time: `Store::open` takes an exclusive lock on the
  encrypted state. A second process can't open the same workspace while the desktop runs.

That lock is the main design constraint. Two programs writing the same encrypted state would
corrupt it, so the CLI and MCP must go **through** whichever Spindle owns the workspace.

## Architecture: one owner, a local control channel

```
 desktop window ─┐
                 ├─ engine (owns the workspace) ── control server (local only)
 headless owner ─┘                                      ▲          ▲
                                                        │          │
                                             spindle CLI      spindle mcp (stdio)
                                                               ▲
                                                Claude Code / Codex / VS Code / agents
```

1. **Control server inside the engine.** A local JSON-RPC endpoint the owning Spindle opens:
   a named pipe on Windows (`\\.\pipe\spindle-<user>-<workspace hash>`, ACL'd to the user), a
   Unix socket on Linux (`$XDG_RUNTIME_DIR/spindle/<hash>.sock`, mode 0600). Requests map onto
   the existing engine commands; responses are `View` slices, never the whole encrypted state.
2. **Thin clients.** `spindle <command>` and `spindle mcp` connect to that endpoint. If no
   Spindle owns the workspace, they start a **headless owner** (the engine without a window,
   like `--eval`), use it, and leave it running briefly so the next call is fast.
3. **Same engine, same rules.** Prompts, tools, sandbox, approvals, memory, plans and
   history are identical whichever front end started the work. A chat started from the CLI
   shows up in the desktop, and the reverse.

## Approvals (the important rule)

A caller must not be able to approve its own risky actions, or the sandbox and approval
design stops meaning anything. Proposal:

- Contained work (reads, edits in the sandbox copy or a chat's own folder, sandboxed runs,
  browser checks of its own pages) needs no approval, as today.
- Anything that asks (writes outside the sandbox, installs, setup, host commands later) waits
  for approval **in the desktop or an interactive CLI prompt** at the keyboard. MCP callers
  get `waiting_for_approval` with what is being asked, and can poll or be notified.
- Grants the user has already given (conversation/project) apply as usual.
- An opt-in "trusted caller" setting per workspace can let a named MCP client approve
  `once`-level requests, off by default, shown in Settings with a warning.

## CLI

Human-friendly by default, `--json` on every command for scripts. Workspace from `--dir` or
the current folder.

| Command | Does |
|---|---|
| `spindle ask "prompt" [--project P] [--model M] [--goal] [--wait]` | New chat with a prompt; `--wait` streams progress and prints the final answer and files. |
| `spindle send CHAT "message" [--wait]` | Continue a chat. |
| `spindle chats [--project P]` · `spindle show CHAT [--full]` | List chats; print a transcript (steps folded unless `--full`). |
| `spindle status [CHAT]` | What's running, queued, or waiting for approval. |
| `spindle approve ID [once\|conversation\|project]` · `spindle deny ID` | Interactive only (refuses without a terminal). |
| `spindle files CHAT [--save DIR]` | List or copy the files a chat made. |
| `spindle projects` · `spindle project new NAME PATH` | Projects. |
| `spindle models` · `spindle model M` | Models (exists in part). |
| `spindle eval …` | The headless test runner (exists). |
| `spindle mcp` | Run as an MCP server on stdio (below). |
| `spindle serve` | Start a headless owner explicitly (for a server or a scheduled job). |

## MCP server

`spindle mcp` speaks MCP over stdio, the transport Claude Desktop, Claude Code, Codex and VS
Code all support. Streamable HTTP can come later for remote use (localhost by default, with a
token).

**Tools** (small set, plain names, results short and textual):

| Tool | Input | Returns |
|---|---|---|
| `spindle_ask` | prompt, project?, model?, goal?, wait_seconds? | chat id, status, final answer if done, files made |
| `spindle_send` | chat, message, wait_seconds? | same |
| `spindle_status` | chat? | running / queued / waiting_for_approval (with what) / done |
| `spindle_read_chat` | chat, full? | transcript (steps folded) |
| `spindle_list_chats` | project?, limit? | id, title, model, updated |
| `spindle_list_projects` | | name, path, description |
| `spindle_list_models` | | models with size and abilities (tools, vision) |
| `spindle_get_file` | chat, name | text, or a resource link for binary files |

**Resources:** `spindle://chat/<id>` (transcript), `spindle://chat/<id>/file/<name>` (files a
chat made), so clients can attach them.

**Long work:** `wait_seconds` (default ~60) returns early with `status: running` and the chat
id; the caller polls `spindle_status`. MCP progress notifications stream step lines
("Edit snake.html · Done") where the client supports them.

**What MCP callers can't do:** approve risky actions (unless trusted, above), read memories
or settings, or touch other workspaces.

## Phases

1. **Control server + headless owner** (engine side), with the `View`-slice API and tests.
   Move `--eval` onto it.
2. **CLI** commands above, `--json` everywhere, interactive approvals.
3. **MCP over stdio**: tools, resources, progress; a config snippet for Claude Code, Codex
   and VS Code in the README.
4. **Polish:** desktop shows "started from CLI/MCP" on chats; Settings › Integrations lists
   connected clients and the trusted-caller switch; streamable HTTP if wanted.

Each phase is testable headless the way the evals are: start a headless owner, drive it from
the client, check the transcript.

## Open design decisions

1. **Approvals from MCP:** keep them at the keyboard (proposed), or allow a trusted-caller
   opt-in at all?
2. **Headless owner lifetime:** stay up a few minutes after the last call (proposed), or
   until stopped with `spindle serve --stop`?
3. **Where MCP chats live:** in the workspace's normal chat list (proposed), or a separate
   "external" section?
4. **Spindle as an MCP *client* too** (using other MCP servers' tools inside Spindle chats) —
   separate plan, later?
