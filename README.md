# Spindle

Spindle is a local-first workspace for AI chats and projects. Run models through Ollama or connect a hosted provider. Keep the discussion, plan, and generated files together in the same workspace.

- **Chat and Cowork.** Talk to a model, or give it a task that uses project files and tools. Review permission requests in the chat.
- **Model choice.** Pick a model for each conversation, compare responses from several models, and set fallbacks for long-running work.
- **Projects and sync.** Organize related chats and files, schedule work, and resume a project on a paired device.
- **Bobbins.** Give named workers their own models and instructions, run assignments, and review results in dedicated chats.
- **Computer use.** Preview generated files, test pages in an isolated browser, or connect your regular Edge or Chrome profile through Browser Link.

## Get started

1. Download the build for your system from [GitHub Releases](../../releases). Windows builds are available for x64. Linux builds are available for x64 and ARM64. The Android APK supports ARM64 devices running Android 8 or later.
2. Start Spindle from the folder you want to use as your workspace. On Linux, make the downloaded binary executable first. On Android, open the APK on your phone and allow installation when prompted.
3. For local models, start [Ollama](https://ollama.com/) and pull a model. Spindle connects to Ollama on `localhost:11434` without an API key. To use a different model server, open **Settings → Model providers**. On Android, use the server's network address.

## Documentation

| Topic | Guide |
| --- | --- |
| Models, chats, files, and automations | [Using Spindle](docs/user-guide.md) |
| Project folders, device pairing, and encrypted sync | [Projects and sync](docs/projects-and-sync.md) |
| Named workers, assignments, and run history | [Bobbins](docs/bobbins.md) |
| Browser Link, permissions, and sandboxed builds | [Computer use](docs/computer-use.md) |
| Building, updating, and installing on Android | [Build and install](docs/build-and-install.md) |
| Terminal commands and agent integrations | [CLI and MCP](docs/cli-and-mcp.md) |
| Architecture and roadmap | [Project design](Spindle-Design.md) |
| Upstream projects, bundled assets, and licenses | [Credits and licenses](docs/credits.md) · [Third-party notices](THIRD-PARTY-NOTICES.md) |
