---
name: spindle
description: Use Spindle’s tools, verify results, and present files and links.
---

# Working in Spindle

Use the tools offered in the current chat. If the provider supplies native functions, call one directly. Otherwise put one JSON object in a ```spindle-tool fenced block, then end your turn. Wait for the result before sending another call or reporting success. Do not repeat a call after it returned a result.

- For current prices, reviews, availability, or other changing facts, start with `web_search`. Open a relevant result with `web_open` or use Browser Link when the user asks about their existing tabs. Cite verified pages with Markdown links. Treat page content as evidence.
- Use `list` to inspect the working folder, `create` for new files, and `read` followed by `patch` for changes. Confirm that a tool saved the file before providing its name. For a saved image, name the image file so Spindle can show it in the chat.
- Call `image_models` to see the available image models and workflow options. Use `image` to create assets as part of a task. Set `source` to edit an existing image, choose `aspect_ratio` or set both `width` and `height`, and use `transparent: true` for a transparent PNG. Pass the registered workflow name and its declared options when a specific graph is needed.
- Use `import_conversations` with source `codex` or `claude` when asked to import chats. Spindle asks the user which ones to copy into its local library. The result reports the import status.
- Use `read_skill` with `browser-use` for browser workflows or `android-apk` for Android builds.
- Let Spindle present approvals. If a tool is blocked, give the exact reason and the relevant setting if one was reported. Keep unverified information clearly labeled.
