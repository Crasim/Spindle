# CLI and MCP

[Spindle](../README.md)

## Using Spindle from other programs (CLI and MCP)

Whichever Spindle has a workspace open (the window, or a windowless one) also answers other
programs on the same computer. If none is running, the command starts a windowless Spindle,
which stops after five minutes with no requests and no work, and hands the workspace over
when you open the window. Chats started this way appear in the normal chat list.

Commands run in the workspace folder (or pass `--dir`); add `--json` for scripts:

```
spindle ask "Make form.html: a contact form" [--project DIR] [--model qwen3.5:9b] [--goal]
spindle send 12 "Add a phone field"
spindle status [12] [--wait 120]
spindle chats · spindle show 12 [--full] · spindle file 12 form.html [--save out.html]
spindle approve 34 [once|conversation|project] · spindle deny 34   (at a terminal only)
spindle projects · spindle models
```

Anything that needs approval waits for you in the Spindle window, or at a terminal with
`spindle approve`. Scripts and agents can't approve (the command refuses without a terminal,
and MCP has no approve tool).

### MCP (Claude Code, Codex, VS Code, other agents)

`spindle mcp --dir WORKSPACE` is an MCP server on stdio with eight tools: `spindle_ask`,
`spindle_send`, `spindle_status`, `spindle_read_chat`, `spindle_list_chats`,
`spindle_list_projects`, `spindle_list_models`, `spindle_get_file`. WORKSPACE is the folder
Spindle runs in (for the desktop shortcut, this folder).

Claude Code:
```
claude mcp add spindle -- "C:\Projects\Spindle\target\release\spindle.exe" mcp --dir "C:\Projects\Spindle"
```
Codex (`~/.codex/config.toml`):
```toml
[mcp_servers.spindle]
command = 'C:\Projects\Spindle\target\release\spindle.exe'
args = ["mcp", "--dir", 'C:\Projects\Spindle']
```
VS Code (`.vscode/mcp.json`):
```json
{ "servers": { "spindle": { "type": "stdio",
  "command": "C:\\Projects\\Spindle\\target\\release\\spindle.exe",
  "args": ["mcp", "--dir", "C:\\Projects\\Spindle"] } } }
```
If the build script put the newest build in `target\staged\release`, use that path, or copy
`spindle.exe` somewhere fixed and point at that.

How it works: the owning Spindle listens on 127.0.0.1 (a random port) and writes the port and
a random token to `%LOCALAPPDATA%\Spindle\control\<workspace>.json` (Linux:
`~/.local/share/Spindle/control/`, mode 0600). Requests must carry the token.
