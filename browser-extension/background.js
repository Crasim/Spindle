let current;
let nativePort;
let pollTimer;
let enabled = false;
let located = null;
const clientId = crypto.randomUUID().replaceAll("-", "");
const browser = /Edg\//.test(navigator.userAgent) ? "edge" : "chrome";
const browserPermissions = { permissions: ["tabs"] };

async function activeTab() {
  const window = await chrome.windows.getLastFocused({ windowTypes: ["normal"] }).catch(() => null);
  if (!window) return null;
  const [tab] = await chrome.tabs.query({ active: true, windowId: window.id });
  return tab?.id && /^https?:\/\//i.test(tab.url || "") ? tab : null;
}

function matchesPage(tab, page) {
    try {
      const url = new URL(tab.url);
      return url.origin === page.origin && url.pathname === page.pathname && (!page.search || url.search === page.search)
        && !url.username && !url.password;
    } catch { return false; }
}

async function findPage(page) {
  const tabs = await chrome.tabs.query({windowType: "normal"});
  return tabs.filter(tab => matchesPage(tab, page)).sort((a, b) => (b.lastAccessed || 0) - (a.lastAccessed || 0))[0];
}

async function requestedTab(address) {
  if (!address) return activeTab();
  const page = new URL(address);
  if (!["http:", "https:"].includes(page.protocol) || page.username || page.password) {
    throw Error("This address cannot be opened by Browser Link.");
  }
  const existing = await findPage(page);
  const tab = existing || await chrome.tabs.create({url: page.href, active: true});
  // A newly created tab starts at about:blank. Bind only after the requested
  // document commits, and don't read an account sign-in or redirect by accident.
  for (let attempt = 0; attempt < 100; attempt++) {
    const loaded = await chrome.tabs.get(tab.id);
    if (loaded.status === "complete" && matchesPage(loaded, page)) return loaded;
    if (loaded.status === "complete" && /^https?:/.test(loaded.url || "") && !matchesPage(loaded, page)) {
      throw Error("The website redirected to a different page. Finish any sign-in in the browser, then ask Spindle to try again.");
    }
    await new Promise(resolve => setTimeout(resolve, 200));
  }
  throw Error("The requested page did not finish opening. Check the browser window, then retry in this chat.");
}

async function heartbeat() {
  const port = nativePort;
  if (!port) return;
  enabled = await chrome.permissions.contains(browserPermissions);
  const tab = enabled ? await activeTab() : null;
  if (port === nativePort) {
    port.postMessage({ browser: { id: clientId, browser, ready: enabled, has_tab: Boolean(tab), guided: true,
      managed: Boolean(globalThis.spindleManagedProfile), located } });
  }
}

function startNative() {
  if (nativePort) return;
  const port = chrome.runtime.connectNative("org.spindleapp.browser_link");
  nativePort = port;
  port.onMessage.addListener(async message => {
    if (port !== nativePort) return;
    if (message.locate && enabled) {
      const match = await findPage(new URL(message.locate)).catch(() => null);
      located = {key: message.key, last_visit: match ? Math.max(1, Math.floor(match.lastAccessed || 1)) : 0};
      await heartbeat();
    } else if (message.disconnected) {
      clearTab();
    } else if (message.connect) {
      clearTab();
      let tab;
      let error;
      try {
        if (enabled) tab = await requestedTab(message.page);
      } catch (problem) { error = problem.message; }
      if (port !== nativePort) return;
      if (!tab) {
        port.postMessage({ tab: null, error });
        return;
      }
      const page = new URL(tab.url);
      current = { tabId: tab.id, origin: page.origin, ready: false };
      port.postMessage({ tab: page.origin + page.pathname });
    } else if (message.packet && current) {
      await receivePacket(current, message.packet);
    }
  });
  port.onDisconnect.addListener(() => {
    void chrome.runtime.lastError;
    if (port !== nativePort) return;
    nativePort = undefined;
    clearInterval(pollTimer);
    clearTab();
    setTimeout(startNative, 2500);
  });
  const pulse = () => heartbeat().catch(() => {});
  pulse();
  pollTimer = setInterval(pulse, 1500);
}

chrome.runtime.onStartup.addListener(startNative);
chrome.runtime.onInstalled.addListener(startNative);
chrome.permissions.onAdded.addListener(() => {
  startNative();
  heartbeat().catch(() => {});
});
chrome.permissions.onRemoved.addListener(() => {
  stop();
  heartbeat().catch(() => {});
});
startNative();

chrome.runtime.onMessage.addListener((message, sender, respond) => {
  if (sender.url !== chrome.runtime.getURL("popup.html")) return;
  if (message.kind === "status") {
    startNative();
    chrome.permissions.contains(browserPermissions).then(value => {
      enabled = value;
      respond(status());
    });
    return true;
  }
  if (message.kind === "disconnect") stop();
  if (message.kind === "request-site" && current?.ready && !current.rule) {
    sendPacket({ request_site: true });
    current.requested = true;
  }
  if (message.kind === "remove-rule" && current?.ready && current.rules.includes(message.rule)) {
    if (current.mode === "allowlist" && current.rule === message.rule) current.rule = null;
    sendPacket({ remove_rule: message.rule });
  }
  if (message.kind === "block-site" && current?.ready && current.mode === "blocklist") {
    current.rule = null;
    sendPacket({ block_site: true });
  }
  if (message.kind === "set-mode" && current?.ready && message.mode !== current.mode) {
    current.rule = null;
    sendPacket({ mode: message.mode });
  }
  respond(status());
});

chrome.tabs.onRemoved.addListener(tabId => {
  if (current?.tabId === tabId) stop();
});

chrome.tabs.onUpdated.addListener((tabId, change) => {
  if (current?.tabId !== tabId || !change.url) return;
  if (!sameOrigin(change.url, current.origin)) {
    stop();
  } else if (current.ready) {
    const location = new URL(change.url);
    current.rule = null;
    sendPacket({ location: location.origin + location.pathname });
  }
});

function sameOrigin(url, origin) {
  try { return new URL(url).origin === origin; } catch { return false; }
}

function allowed(address, rule) {
  try {
    const page = new URL(address);
    const site = new URL(rule);
    const prefix = site.pathname.replace(/\/+$/, "");
    return page.origin === site.origin && !page.username && !page.password &&
      (page.pathname === prefix || page.pathname.startsWith(prefix + "/"));
  } catch {
    return false;
  }
}

function status() {
  return { enabled, connected: Boolean(current?.ready), origin: current?.origin, rule: current?.rule,
    rules: current?.rules || [], requested: current?.requested, mode: current?.mode };
}

function clearTab() {
  const old = current;
  current = undefined;
  if (old) {
    chrome.action.setBadgeText({ tabId: old.tabId, text: "" }).catch(() => {});
    chrome.runtime.sendMessage({ kind: "update", status: status() }).catch(() => {});
  }
}

function stop() {
  clearTab();
  nativePort?.postMessage({ disconnect: true });
}

function sendPacket(packet) {
  nativePort?.postMessage({ packet });
}

async function receivePacket(session, message) {
  if (Array.isArray(message.rules)) {
    session.rule = message.rule;
    session.rules = message.rules;
    session.mode = message.mode;
    session.ready = true;
    if (session.rule) session.requested = false;
    chrome.action.setBadgeText({ tabId: session.tabId, text: session.rule ? "ON" : "OFF" }).catch(() => {});
    chrome.runtime.sendMessage({ kind: "update", status: status() }).catch(() => {});
    return;
  }
  let reply;
  try {
    const tab = await chrome.tabs.get(session.tabId);
    const requested = message.operation?.action === "read" && message.operation.url ? new URL(message.operation.url) : null;
    if (current !== session || !allowed(tab.url, session.rule) ||
        (requested && (new URL(tab.url).origin !== requested.origin || new URL(tab.url).pathname !== requested.pathname
          || (requested.search && new URL(tab.url).search !== requested.search))) ||
        (session.mode === "blocklist" && session.rules.some(rule => allowed(tab.url, rule)))) {
      throw Error("This URL is blocked in Spindle.");
    }
    const [response] = await chrome.scripting.executeScript({
      target: { tabId: session.tabId },
      func: operate,
      args: [message.operation, session.rule, session.mode === "blocklist" ? session.rules : []]
    });
    reply = response?.result || { error: "The tab did not return a result." };
  } catch {
    reply = { error: "The browser could not access this page. If your browser restricts the extension on this site, open its extension menu and allow Browser Link on this site, then retry here." };
  }
  if (current === session) sendPacket({ id: message.id, ...reply });
}

function operate(operation, rule, blocked) {
  const site = new URL(rule);
  const prefix = site.pathname.replace(/\/+$/, "");
  const matches = (url, site, prefix) => url.origin === site.origin && !url.username && !url.password &&
    (url.pathname === prefix || url.pathname.startsWith(prefix + "/"));
  const allowedPath = url => matches(url, site, prefix) && !blocked.some(rule => {
    const denied = new URL(rule);
    return matches(url, denied, denied.pathname.replace(/\/+$/, ""));
  });
  if (!allowedPath(location)) return { error: "The tab left the allowlisted URL." };
  if (operation.action === "read" && operation.url) {
    const requested = new URL(operation.url);
    if (location.origin !== requested.origin || location.pathname !== requested.pathname ||
        (requested.search && location.search !== requested.search)) {
      return { error: "The tab left the requested page." };
    }
  }
  // This state lives in the extension's isolated world for this document.
  const cache = globalThis.spindleTabTargets ||= new Map();
  const visible = element => Boolean(element.getClientRects().length) &&
    getComputedStyle(element).visibility === "visible" && !element.closest("[hidden],[inert]");
  const field = element => element instanceof HTMLTextAreaElement ||
    (element instanceof HTMLInputElement && ["text", "search", "email", "url", "tel", "number"].includes(element.type));
  const privateField = element => Boolean(element.closest('input[type="password"], input[type="file"]')) ||
    /(?:\bcc-|password|one-time-code|secret|token|otp)/i.test(
      [element.name, element.autocomplete, element.id, element.getAttribute("aria-label"), element.placeholder].join(" ")
    );
  const label = element => (element.getAttribute("aria-label") || element.labels?.[0]?.innerText ||
      element.innerText || element.getAttribute("placeholder") || element.getAttribute("title") ||
      element.getAttribute("name") || element.tagName.toLowerCase()).replace(/\s+/g, " ").trim().slice(0, 90);
  const href = element => {
    if (!(element instanceof HTMLAnchorElement)) return undefined;
    const url = new URL(element.href, location.href);
    return ["http:", "https:"].includes(url.protocol) && !url.username && !url.password ? url : undefined;
  };
  if (operation.action === "read") {
    cache.clear();
    const nonce = Array.from(crypto.getRandomValues(new Uint32Array(2)), n => n.toString(16)).join("-");
    const controls = [];
    const elements = document.querySelectorAll("a[href],button,input,textarea,[role='button']");
    for (const element of elements) {
      if (controls.length >= 40) break;
      if (!visible(element) || element.matches(":disabled") || privateField(element)) continue;
      const url = href(element);
      if (element instanceof HTMLAnchorElement && (!url || !allowedPath(url) || element.target === "_blank" || element.hasAttribute("download"))) continue;
      const name = label(element);
      const target = `${nonce}/${controls.length + 1} ${name}`;
      const action = field(element) ? "fill" : "click";
      if (action === "fill" && element.readOnly) continue;
      cache.set(target, { element, name, href: url?.href, action, page: location.href });
      controls.push({ target, action });
    }
    const page = new URL(location.href);
    const main = document.querySelector("main, [role='main']") || document.body;
    const snapshot = { title: document.title.slice(0, 240), page: page.origin + page.pathname.slice(0, 512),
      text: main?.innerText.slice(0, 8500) || "", controls, links: [] };
    const encoder = new TextEncoder();
    let remaining = 50 * 1024 - encoder.encode(JSON.stringify(snapshot)).length;
    const seen = new Set();
    for (const link of main?.querySelectorAll("a[href]") || []) {
      if (snapshot.links.length >= 60 || remaining <= 0) break;
      if (!visible(link) || link.hasAttribute("download")) continue;
      const url = href(link);
      if (!url || url.origin !== page.origin) continue;
      url.search = "";
      url.hash = "";
      if (url.href === snapshot.page || seen.has(url.href) || url.href.length > 1024) continue;
      const text = (link.getAttribute("aria-label") || link.innerText || link.title ||
        link.querySelector("img[alt]")?.alt || "").replace(/\s+/g, " ").trim().slice(0, 240);
      if (!text) continue;
      const reference = { text, url: url.href };
      const size = encoder.encode(JSON.stringify(reference)).length + 1;
      if (size > remaining) continue;
      remaining -= size;
      seen.add(url.href);
      snapshot.links.push(reference);
    }
    return { result: snapshot };
  }
  const target = cache.get(operation.target);
  if (!target || target.page !== location.href || target.action !== operation.action ||
      !target.element.isConnected || !visible(target.element) || target.element.matches(":disabled") ||
      privateField(target.element) || target.name !== label(target.element) ||
      target.href !== href(target.element)?.href) {
    return { error: "The control changed. Read the tab again." };
  }
  cache.clear();
  const element = target.element;
  if (operation.action === "fill") {
    if (!field(element) || element.readOnly || typeof operation.text !== "string" || operation.text.length > 4000) {
      return { error: "This field cannot be filled." };
    }
    const prototype = element instanceof HTMLTextAreaElement ? HTMLTextAreaElement.prototype : HTMLInputElement.prototype;
    element.focus();
    Object.getOwnPropertyDescriptor(prototype, "value").set.call(element, operation.text);
    element.dispatchEvent(new Event("input", { bubbles: true }));
    element.dispatchEvent(new Event("change", { bubbles: true }));
  } else if (operation.action === "click") {
    if (element instanceof HTMLAnchorElement && (element.target === "_blank" || element.hasAttribute("download"))) {
      return { error: "Read the tab again." };
    }
    element.click();
  }
  return { result: { action: operation.action, control: target.name } };
}
