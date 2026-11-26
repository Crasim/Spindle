"""Run with: python tools/test-browser-link.py BROWSER TEST_BINARY HOST_BINARY [MODEL].

Requires the websocket-client package and a release test build of Spindle.
Uses local Ollama by default. For Maya, set SPINDLE_BROWSER_MODEL_ENDPOINT
to its API base URL and pass @maya/inkling-2-mini for MODEL.
Set SPINDLE_BROWSER_FIXTURE_COWORK to test Cowork instead of Chat.
"""
import contextlib
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
import json
import os
import secrets
import shutil
from pathlib import Path
import subprocess
import sys
import tempfile
import threading
import time

import websocket

ROOT = Path(__file__).resolve().parents[1]
QA = ROOT / "target" / "full-beta-qa"
PROOF = f"CART-{secrets.token_hex(4).upper()}"


class Fixture(BaseHTTPRequestHandler):
    def do_GET(self):
        page = """<!doctype html><title>Spindle browser fixture</title>
            <main><h1>Shopping cart</h1><p>Cart reference: __PROOF__</p>
            <ul>
              <li><a href="/products/citrus-comet-x4?tracking=fixture#details">Citrus Comet X4</a> &middot; Quantity 1</li>
              <li><a href="/products/night-otter-n7?tracking=fixture">Night Otter N7</a> &middot; Quantity 2</li>
            </ul>
            <section aria-label="Optional protection offer for Citrus Comet X4">
              <h2>Optional protection offer for Citrus Comet X4</h2>
              <a href="/products/random-card-insurance">Random Card</a>
              <label><input type="checkbox"> Add a two-year insurance plan for $4.99</label>
            </section>
            <label>Name <input id="name"></label>
            <label>Password <input type="password"></label>
            <button id="apply">Apply name</button><p id="result"></p>
            <script>
            document.querySelector('#apply').onclick = () => {
              document.querySelector('#result').textContent =
                'Hello ' + document.querySelector('#name').value;
            };
            </script></main>""".replace("__PROOF__", PROOF).encode()
        self.send_response(200)
        self.send_header("Content-Type", "text/html")
        self.end_headers()
        self.wfile.write(page)

    def log_message(self, *_):
        pass


def wait_for(check, seconds=15):
    end = time.monotonic() + seconds
    while time.monotonic() < end:
        value = check()
        if value:
            return value
        time.sleep(.15)
    raise RuntimeError("Timed out waiting for browser test")


class Cdp:
    def __init__(self, socket):
        self.socket = socket
        self.id = 0

    def call(self, method, params=None, session=None):
        self.id += 1
        msg = {"id": self.id, "method": method, "params": params or {}}
        if session:
            msg["sessionId"] = session
        self.socket.send(json.dumps(msg))
        while True:
            result = json.loads(self.socket.recv())
            if result.get("id") == self.id:
                if "error" in result:
                    raise RuntimeError(f"{method} {result['error']}")
                return result.get("result", {})


@contextlib.contextmanager
def native_fixture(folder, profile, exe, host_binary):
    name = f"org.spindleapp.browser_test_{secrets.token_hex(8)}"
    extension = folder / "extension"
    shutil.copytree(ROOT / "browser-extension", extension)
    worker = extension / "background.js"
    worker.write_text(worker.read_text(encoding="utf-8").replace("org.spindleapp.browser_link", name), encoding="utf-8")
    host = {
        "name": name,
        "description": "Disposable Spindle browser fixture",
        "type": "stdio",
        "path": str(Path(host_binary).resolve(strict=True)),
        "allowed_origins": ["chrome-extension://hlnagoiglillplcmaefcddjemiefpnoa/"],
    }
    env = {**os.environ, "SPINDLE_DATA_DIR": str(folder / "bridge-data")}
    edge = "edge" in Path(exe).name.lower()
    if os.name == "nt":
        vendor = r"Microsoft\Edge" if edge else r"Google\Chrome"
        key = rf"HKCU\Software\{vendor}\NativeMessagingHosts\{name}"
        hostfile = folder / "host.json"
        hostfile.write_text(json.dumps(host), encoding="utf-8")
        subprocess.run(["reg.exe", "add", key, "/ve", "/t", "REG_SZ", "/d", str(hostfile), "/f"], check=True, capture_output=True)
        try:
            yield extension, env
        finally:
            subprocess.run(["reg.exe", "delete", key, "/f"], check=True, capture_output=True)
    else:
        config = folder / "config"
        env["XDG_CONFIG_HOME"] = str(config)
        for parent in (profile, config / ("microsoft-edge" if edge else "google-chrome")):
            manifests = parent / "NativeMessagingHosts"
            manifests.mkdir(parents=True)
            (manifests / f"{name}.json").write_text(json.dumps(host), encoding="utf-8")
        yield extension, env


def check(exe, binary, host_binary, url, model=None):
    with tempfile.TemporaryDirectory(prefix="browser-qa-", dir=QA, ignore_cleanup_errors=True) as temp:
        folder = Path(temp)
        with native_fixture(folder, folder / "profile", exe, host_binary) as (extension_folder, env):
            run_browser(exe, binary, folder, extension_folder, env, url, model)


@contextlib.contextmanager
def connected_profile(exe, profile, extension_folder, env):
    browser_output = profile.with_suffix(".log").open("w")
    browser = subprocess.Popen([
        exe, "--headless=new", "--no-first-run", "--no-default-browser-check",
        "--disable-background-networking", "--remote-debugging-port=0",
        "--enable-unsafe-extension-debugging", f"--user-data-dir={profile}", "about:blank",
    ], stdout=browser_output, stderr=subprocess.STDOUT, env=env)
    try:
        port_file = profile / "DevToolsActivePort"
        port, endpoint = wait_for(lambda: port_file.exists() and port_file.read_text().splitlines()[:2])
        conn = websocket.create_connection(f"ws://127.0.0.1:{port}{endpoint}", timeout=8, suppress_origin=True)
        cdp = Cdp(conn)
        extension = cdp.call("Extensions.loadUnpacked", {"path": str(extension_folder)})["id"]
        assert extension == "hlnagoiglillplcmaefcddjemiefpnoa", extension
        print("Loaded Browser Link in", Path(exe).name, flush=True)
        try:
            yield cdp, extension
        finally:
            with contextlib.suppress(Exception):
                cdp.call("Browser.close")
            conn.close()
    finally:
        if browser.poll() is None:
            browser.terminate()
        browser.wait(timeout=10)
        browser_output.close()


def run_browser(exe, binary, folder, extension_folder, env, url, model):
    with contextlib.ExitStack() as profiles:
        cdp, extension = profiles.enter_context(connected_profile(exe, folder / "profile", extension_folder, env))
        guided = model and os.environ.get("SPINDLE_BROWSER_FIXTURE_GUIDED")
        existing = None
        owner = cdp
        if guided == "profiles":
            owner, _ = profiles.enter_context(connected_profile(exe, folder / "second-profile", extension_folder, env))
        if guided in ("background", "profiles"):
            existing = owner.call("Target.createTarget", {"url": url, "background": True})["targetId"]
        target = cdp.call("Target.createTarget", {"url": "about:blank" if guided else url})["targetId"]
        cdp.call("Target.activateTarget", {"targetId": target})
        page = cdp.call("Target.attachToTarget", {"targetId": target, "flatten": True})["sessionId"]
        if not guided:
            wait_for(lambda: cdp.call("Runtime.evaluate", {"expression": "document.title"}, page)["result"].get("value") == "Spindle browser fixture")
        popup = None
        if not model:
            popup_url = f"chrome-extension://{extension}/popup.html"
            popup_target = cdp.call("Target.createTarget", {"url": popup_url, "background": True})["targetId"]
            popup = cdp.call("Target.attachToTarget", {"targetId": popup_target, "flatten": True})["sessionId"]
            cdp.call("Target.activateTarget", {"targetId": target})
        test = "browser_fixture_round_trip"
        env = {**env, "SPINDLE_BROWSER_FIXTURE_ROOT": str(folder), "SPINDLE_BROWSER_FIXTURE_URL": url}
        if model:
            test = "chat_model_reads_browser_and_returns_product_links"
            env.update(SPINDLE_BROWSER_MODEL=model, SPINDLE_BROWSER_FIXTURE_PROOF=PROOF)
        tester = subprocess.Popen([binary, "--ignored", f"browser_link::tests::{test}", "--nocapture"], env=env)
        try:
            deadline = time.monotonic() + (155 if model else 65)
            requested = False
            text = "Waiting for the native browser connection"
            while tester.poll() is None and time.monotonic() < deadline:
                if popup:
                    status = cdp.call("Runtime.evaluate", {"expression": "document.querySelector('#status')?.textContent"}, popup)
                    text = status["result"].get("value", "")
                    if text.startswith(("Could not connect", "Open an HTTP", "Cannot access")):
                        raise RuntimeError(text)
                    if text.startswith("Blocked") and not requested:
                        cdp.call("Runtime.evaluate", {"expression": "document.querySelector('#request').click()"}, popup)
                        requested = True
                time.sleep(.25)
            if tester.poll() != 0:
                raise RuntimeError("Browser Link status: " + text)
            if model:
                print("Verified an approved Spindle model read of the current cart and its product links.", flush=True)
                if guided:
                    if owner is not cdp:
                        assert all(p["url"] != url for p in cdp.call("Target.getTargets")["targetInfos"])
                    pages = owner.call("Target.getTargets")["targetInfos"]
                    assert sum(p["type"] == "page" and p["url"] == url for p in pages) == 1
                    if existing:
                        assert any(p["targetId"] == existing and p["url"] == url for p in pages)
                        print("Verified Spindle reused the existing background cart tab.", flush=True)
                    else:
                        print("Verified Spindle opened exactly one requested cart tab from a blank window.", flush=True)
            else:
                print("Verified automatic connection, site approval, visible text, links, password exclusion, text entry, click and revocation in", Path(exe).name, flush=True)
        finally:
            if tester.poll() is None:
                tester.terminate()


if __name__ == "__main__":
    QA.mkdir(parents=True, exist_ok=True)
    server = ThreadingHTTPServer(("127.0.0.1", 0), Fixture)
    threading.Thread(target=server.serve_forever, daemon=True).start()
    try:
        model = sys.argv[4] if len(sys.argv) > 4 else None
        check(sys.argv[1], sys.argv[2], sys.argv[3], f"http://127.0.0.1:{server.server_port}/cart", model)
    finally:
        server.shutdown()
