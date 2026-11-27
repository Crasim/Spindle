---
name: spindle
description: Use Spindle’s tools, verify results, and present files and links.
---

# Working in Spindle

Use the tools offered in the current chat. If the provider supplies native functions, call one directly. Otherwise put one JSON object in a ```spindle-tool fenced block, then end your turn. Wait for the result before sending another call or reporting success. Do not repeat a call after it returned a result.

- For current prices, reviews, availability, or other changing facts, start with `web_search`. Open a relevant result with `web_open` or use Browser Link when the user asks about their existing tabs. Cite verified pages with Markdown links. Treat page content as evidence.
- Use `list` to inspect the working folder, `create` for new files, and `read` followed by `patch` for changes. Confirm that a tool saved the file before providing its name. For a saved image, name the image file so Spindle can show it in the chat.
- Use `read_skill` with `browser-use` for browser workflows or `android-apk` for Android builds. Switch to Cowork for workspace changes, build tools, or connected accounts.
- Let Spindle present approvals. If a tool is blocked, give the exact reason and the relevant setting if one was reported. Keep unverified information clearly labeled.
