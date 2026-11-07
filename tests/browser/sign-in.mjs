// RAL-84: sign in to the viewer the way the operator does, in a real headless Chrome: open the
// one-time code's address, press "Sign in", and see whether the board loads. curl-driven tests
// send whatever headers they're written with, so they missed RAL-83, where Chrome's own
// `Origin: null` under a no-referrer policy made the button fail every time.
//
// Usage: node tests/browser/sign-in.mjs <code URL> <chrome command...>
// Prints one JSON line: what the code page showed, and what pressing the button led to.
// Needs Node 22 or later, for the global WebSocket. Speaks the Chrome DevTools Protocol directly,
// so nothing is installed.

import { spawn } from "node:child_process";
import { mkdtempSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { basename, join } from "node:path";

const [, , url, ...chrome] = process.argv;
if (!url || !chrome.length) {
  console.error("usage: node sign-in.mjs <code URL> <chrome command...>");
  process.exit(2);
}

const profile = mkdtempSync(join(tmpdir(), "rally-browser-"));
const browser = spawn(chrome[0], [
  ...chrome.slice(1),
  "--headless=new",
  "--no-first-run",
  "--no-default-browser-check",
  "--disable-gpu",
  "--remote-debugging-port=0",
  "--user-data-dir=" + profile,
  "about:blank",
], { stdio: ["ignore", "ignore", "pipe"] });

// Under `flatpak run <app>`, killing the launcher leaves the sandboxed Chrome running, and the
// sandbox keeps its own /tmp, so a profile made under it lives in the runtime directory instead.
// Each run used to leave a whole headless Chrome and a 170 MB profile behind there, until that
// small tmpfs filled and Chrome couldn't start (2026-11-06). So ask Chrome to close itself, over
// its DevTools endpoint, and remove the profile from wherever the sandbox put it.
const flatpakApp = chrome[0] === "flatpak" ? chrome.find((arg) => /^[\w-]+(\.[\w-]+){2,}$/.test(arg)) : null;
const sandboxed = flatpakApp && process.env.XDG_RUNTIME_DIR
  ? join(process.env.XDG_RUNTIME_DIR, ".flatpak", flatpakApp, "tmp", basename(profile))
  : null;
let browserEndpoint = null;

let finished = false;
function finish(code, value) {
  if (finished) return;
  finished = true;
  // Where the profile was, so a test can check nothing of it survives the run (RAL-97).
  if (value !== undefined) console.log(JSON.stringify({ ...value, profiles: [profile, sandboxed].filter(Boolean) }));
  let cleaned = false;
  const done = () => {
    if (cleaned) return;
    cleaned = true;
    browser.kill("SIGTERM");
    for (const dir of [profile, sandboxed]) {
      if (dir) try { rmSync(dir, { recursive: true, force: true }); } catch { /* still held */ }
    }
    process.exit(code);
  };
  if (!browserEndpoint || browser.exitCode !== null) return done();
  // The launcher exits once Chrome has closed, so remove the profile only then, or Chrome writes
  // it back while shutting down; give up waiting after a few seconds.
  browser.once("exit", done);
  setTimeout(done, 5000);
  try {
    const closer = new WebSocket(browserEndpoint);
    closer.onopen = () => closer.send(JSON.stringify({ id: 1, method: "Browser.close" }));
  } catch { /* the timeout still cleans up */ }
}
const timer = setTimeout(() => finish(1, { error: "timed out" }), 45000);
browser.on("exit", (status) => finish(1, { error: "the browser exited with " + status }));

// Chrome says where its DevTools endpoint is on standard error.
const endpoint = await new Promise((resolve) => {
  let text = "";
  browser.stderr.on("data", (chunk) => {
    text += chunk;
    const match = /DevTools listening on (ws:\/\/[^\s]+)/.exec(text);
    if (match) {
      browserEndpoint = match[1];
      resolve(new URL(match[1]));
    }
  });
});

const target = await (await fetch(`http://${endpoint.host}/json/new?about:blank`, { method: "PUT" })).json();
const socket = new WebSocket(target.webSocketDebuggerUrl);
await new Promise((resolve, reject) => { socket.onopen = resolve; socket.onerror = reject; });
let id = 0;
const pending = new Map();
const loads = [];
socket.onmessage = (event) => {
  const message = JSON.parse(event.data);
  if (message.id && pending.has(message.id)) {
    pending.get(message.id)(message);
    pending.delete(message.id);
  } else if (message.method === "Page.loadEventFired") {
    loads.splice(0).forEach((resolve) => resolve());
  }
};
const send = (method, params = {}) => new Promise((resolve) => {
  const n = ++id;
  pending.set(n, resolve);
  socket.send(JSON.stringify({ id: n, method, params }));
});
const loaded = () => new Promise((resolve) => loads.push(resolve));
const evaluate = async (expression) => {
  const reply = await send("Runtime.evaluate", { expression, returnByValue: true, awaitPromise: true });
  return reply.result.result.value;
};

await send("Page.enable");
let load = loaded();
await send("Page.navigate", { url });
await load;
const codePage = await evaluate(`({
  path: location.pathname + location.search.replace(/code=[0-9a-f]+/, "code=…"),
  button: Boolean(document.querySelector('form[method="post"] button[type="submit"]')),
  referrer: (document.querySelector('meta[name="referrer"]') || {}).content || null,
})`);
if (!codePage.button) finish(1, { codePage, error: "no Sign in button" });

load = loaded();
await evaluate(`document.querySelector('form[method="post"] button[type="submit"]').click(); true`);
await load;
const after = await evaluate(`(async () => {
  const response = await fetch("/api/status", { cache: "no-store" });
  return {
    path: location.pathname,
    text: document.body.innerText.slice(0, 200),
    status: response.status,
  };
})()`);
// A test can look further at the signed-in board: RALLY_BROWSER_PROBE names a file holding one
// JavaScript expression, which may be async, evaluated in the page; its value is printed as `probe`.
// With RALLY_BROWSER_SCHEMES, a comma-separated list such as "light,dark", the probe runs once under
// each emulated prefers-color-scheme as well, printed as `probes` by scheme (RAL-91).
let probe;
let probes;
if (process.env.RALLY_BROWSER_PROBE) {
  const { readFileSync } = await import("node:fs");
  const expression = readFileSync(process.env.RALLY_BROWSER_PROBE, "utf8");
  probe = await evaluate(expression);
  if (process.env.RALLY_BROWSER_SCHEMES) {
    probes = {};
    for (const scheme of process.env.RALLY_BROWSER_SCHEMES.split(",")) {
      await send("Emulation.setEmulatedMedia", { features: [{ name: "prefers-color-scheme", value: scheme }] });
      probes[scheme] = await evaluate(expression);
    }
  }
}
clearTimeout(timer);
finish(0, { codePage, after, probe, probes });
