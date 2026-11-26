const disconnect = document.querySelector("#disconnect");
const allowTab = document.querySelector("#allow-tab");
const status = document.querySelector("#status");
const mode = document.querySelector("#mode");
const request = document.querySelector("#request");
const block = document.querySelector("#block");
const allowlist = document.querySelector("#allowlist");
const rules = document.querySelector("#rules");

function render(result) {
  disconnect.hidden = !result.connected;
  allowTab.hidden = !result.revoked;
  request.hidden = !result.connected || Boolean(result.rule) || result.mode !== "allowlist";
  request.disabled = Boolean(result.requested);
  block.hidden = !result.connected || !result.rule || result.mode !== "blocklist";
  allowlist.hidden = !result.connected || !["allowlist", "blocklist"].includes(result.mode);
  document.querySelector("#access").hidden = !result.connected;
  if (result.mode) mode.value = result.mode;
  document.querySelector("#rule-heading").textContent = result.mode === "blocklist" ? "Blocked URLs" : "Allowed URLs";
  status.textContent = result.error || (result.revoked ? "You disconnected this tab from Spindle."
    : result.rule ? `Access allowed · ${result.rule}`
    : result.requested ? "Access requested. Review it in your Spindle chat."
    : result.connected ? `Blocked · ${result.origin}. Check your Spindle chat for an access request.`
    : result.enabled ? "Ready. Return to your Spindle chat." : "Check this extension’s permission in your browser.");
  rules.replaceChildren();
  for (const rule of result.rules || []) {
    const item = document.createElement("li");
    const remove = document.createElement("button");
    remove.type = "button";
    remove.textContent = "Remove";
    remove.setAttribute("aria-label", `Remove ${rule}`);
    remove.addEventListener("click", async () => {
      await chrome.runtime.sendMessage({ kind: "remove-rule", rule });
    });
    item.append(document.createTextNode(rule), remove);
    rules.append(item);
  }
}

disconnect.addEventListener("click", async () => render(await chrome.runtime.sendMessage({ kind: "disconnect" })));
allowTab.addEventListener("click", async () => render(await chrome.runtime.sendMessage({ kind: "allow-tab" })));
request.addEventListener("click", async () => render(await chrome.runtime.sendMessage({ kind: "request-site" })));
block.addEventListener("click", async () => render(await chrome.runtime.sendMessage({ kind: "block-site" })));
document.querySelector("#save-mode").addEventListener("click", async () => {
  render(await chrome.runtime.sendMessage({ kind: "set-mode", mode: mode.value }));
});
chrome.runtime.onMessage.addListener(message => {
  if (message.kind === "update") render(message.status);
});
chrome.runtime.sendMessage({ kind: "status" }).then(render);
