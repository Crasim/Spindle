# Spindle beta 17 feature audit

## Browser and computer use

| Feature | Verification |
| --- | --- |
| Browser Link installation | Reviewed the setup path. Spindle stages and verifies the extension and opens its install guide. File Explorer opens through the explicit **Show extension folder** action. The browser owns its final extension approval. |
| Grouping and release | Disposable Edge and Chrome profiles exercised a real extension, test page and native bridge. The tab entered the Spindle group. Escape revoked access. A separate Inkling 2 Small run opened one new tab, read a synthetic cart and its product links, and released the tab after the reply. |
| Pointer and active indicator | Both browsers displayed the packaged ImageGen PNG. The integration check compared the actual background image with the asset bytes and measured the pointer position against the clicked button. The disconnected upper-left triangle marks the hotspot. The border and Escape badge indicate active access. The built-in Browser tab uses the same artwork while opening a page and on the document itself during a Windows panel read. Linux and Android show it while a model page snapshot is loading. |
| Built-in browser | A foreground Windows chat requesting the built-in browser can read a public HTTPS page in its own WebView2 tab. Tests cover the requesting chat, page address, single delivery, cancellation handling and multiple tabs. Linux shows an isolated Chromium snapshot. Android shows a public page text snapshot. |
| Restricted sites | In a Windows test, eBay served a verification page. Ordinary public-page access works. The panel waits for a return to the requested address and can offer permissioned Browser Link access if the site still withholds the page. Live listing prices remain unverified. |
| Browser settings and skills | Edge and Chrome have separate controls. Manage includes the access switch, site policy, domain lists and grant revocation. A mismatched extension version prompts an update before use. The bundled browser-use skill explains when to choose the panel or a regular browser tab. |
| Market selection | Regression coverage checks permission to use device location, an optional manual market, and continued browsing after location is declined. Models receive URL lookup guidance and the market the user shared. |

## Conversation, model and project controls

| Feature | Verification |
| --- | --- |
| Sidebar selection | The highlighted thread button has horizontal padding and truncates labels within its bounds. Both project and standalone threads use the shared thread control. Source reviewed. |
| Nerd stats | The default is off. A rendered layout test verifies that the context footer disappears and that the composer moves down. |
| Provider picker and reasoning | The provider filter has its own row, up to 340 logical pixels wide. Provider tests cover optional Ollama credentials scoped to their endpoint and the selected reasoning effort in a GPT-6 OAuth request. |
| Destructive changes | Project deletion has a confirmation flow. Sandbox changes that delete files require approval for the current deletion list. Tests also exercise undo and refusal to overwrite concurrent user edits. Rules for human-approved browser activity depend on the site's controls and scope of the grant. |
| Age guidance | **I'm over 18** defaults to unchecked. Reviewed the common model request builder, including council and swarm modes. It adds age-appropriate language guidance. The preference is self-reported and depends on the model following the instruction. |
| Projects and startup | Reviewed the individual project cards and management actions. Background operation and start-at-login controls live under Preferences. |

## Sync and remote work

| Feature | Verification |
| --- | --- |
| Devices | Each device card has an OS indicator, receive switch, removal and management controls. Incoming categories and projects can be restricted per peer. The Add Device workflow is separate from the configured sync transports. |
| Defaults and overrides | The sync page identifies global defaults. Individual projects can override conversation and file sync. OneDrive is available through Connectors. Existing folder transports remain supported. |
| Remote job permissions | The destination grants sandbox job categories separately from host capability inventory and public browser access. A public browser job requires both host and browser permission and carries only a URL. Tests cover the default denial, each partial grant, both grants, and a paused peer. Remote access to a signed-in personal browser or a general host shell is outside the available operations. |
| Hosted service | Local integration tests cover encrypted store-and-forward, restart recovery, live relay, membership and account enrollment. The release includes a static downloads page. Private sync deployment needs a host that permits application data storage. See [hosting](hosting.md). |

The regular Windows tests run separately from the real-browser fixtures. The Android release is suitable for sideloading. On-device touch, keyboard and installation checks require an Android device.
