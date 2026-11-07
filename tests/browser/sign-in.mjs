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
import { join } from "node:path";

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

let finished = false;
function finish(code, value) {
  if (finished) return;
  finished = true;
  if (value !== undefined) console.log(JSON.stringify(value));
  browser.kill("SIGTERM");
  try { rmSync(profile, { recursive: true, force: true }); } catch { /* the browser may still hold it */ }
  process.exit(code);
}
const timer = setTimeout(() => finish(1, { error: "timed out" }), 45000);
browser.on("exit", (status) => finish(1, { error: "the browser exited with " + status }));

// Chrome says where its DevTools endpoint is on standard error.
const endpoint = await new Promise((resolve) => {
  let text = "";
  browser.stderr.on("data", (chunk) => {
    text += chunk;
    const match = /DevTools listening on (ws:\/\/[^\s]+)/.exec(text);
    if (match) resolve(new URL(match[1]));
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
clearTimeout(timer);
finish(0, { codePage, after });
