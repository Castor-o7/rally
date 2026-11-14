// RAL-84: sign in to the viewer the way the operator does, in a real headless Chrome: open the
// one-time code's address, press "Sign in", and see whether the board loads. curl-driven tests
// send whatever headers they're written with, so they missed RAL-83, where Chrome's own
// `Origin: null` under a no-referrer policy made the button fail every time.
//
// Usage: node tests/browser/sign-in.mjs <code URL> <chrome command...>
// Prints one JSON line: what the code page showed, and what pressing the button led to.
// Needs Node 22 or later, for the global WebSocket. Speaks the Chrome DevTools Protocol directly,
// so nothing is installed.

import { spawn, spawnSync } from "node:child_process";
import { mkdtempSync, readFileSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { basename, join } from "node:path";

const [, , url, ...chrome] = process.argv;
if (!url || !chrome.length) {
  console.error("usage: node sign-in.mjs <code URL> <chrome command...>");
  process.exit(2);
}

const windowSize = process.env.RALLY_BROWSER_WINDOW || "1500,1000";
if (!/^[0-9]{2,5},[0-9]{2,5}$/.test(windowSize)) {
  console.error("RALLY_BROWSER_WINDOW is width,height in pixels, such as 800,1000");
  process.exit(2);
}
const profile = mkdtempSync(join(tmpdir(), "rally-browser-"));
// The app is found wherever `flatpak` appears, so a wrapper in front of it (as RAL-160's tests use)
// still has its sandboxed profile cleaned up.
const flatpakAt = chrome.findIndex((arg) => basename(arg) === "flatpak");
const flatpakApp = flatpakAt >= 0
  ? chrome.slice(flatpakAt).find((arg) => /^[\w-]+(\.[\w-]+){2,}$/.test(arg))
  : null;
const sandboxed = flatpakApp && process.env.XDG_RUNTIME_DIR
  ? join(process.env.XDG_RUNTIME_DIR, ".flatpak", flatpakApp, "tmp", basename(profile))
  : null;
const chromeArgs = [
  "--headless=new",
  "--no-first-run",
  "--no-default-browser-check",
  "--disable-gpu",
  // No component downloads: each run's updater left a .crx and a fetcher folder in /tmp, which
  // under flatpak is the sandbox's, in the runtime directory, and isn't the profile, so nothing
  // removed them (1,971 entries, 188 MB, by 2026-11-13; RAL-150). Flatpak's wrapper sets TMPDIR
  // to /tmp, so they can't be sent to the profile instead.
  "--disable-component-update",
  "--disable-default-apps",
  // A desktop window, as the operator views it (RAL-92), rather than headless's 800x600; a test
  // can ask for another with RALLY_BROWSER_WINDOW, as "width,height" (RAL-111).
  "--window-size=" + windowSize,
  "--remote-debugging-port=0",
  "--user-data-dir=" + profile,
  "about:blank",
];

// RAL-160: Chrome's standard error is kept, the last 4 KB of it, and given with every failure, so
// a browser that dies says why. Under load on Murph headless Chrome twice aborted at start inside
// flatpak's zypak spawn helper (an assertion in libsystemd's bus code), and its words were lost.
const STDERR_KEPT = 4096;
let browser = null;
let stderrTail = "";
// An abort at start that was tried once more: its status and what Chrome said.
let firstAbort = null;

function launch() {
  stderrTail = "";
  const child = spawn(chrome[0], [...chrome.slice(1), ...chromeArgs], {
    stdio: ["ignore", "ignore", "pipe"],
  });
  child.stderr.on("data", (chunk) => {
    stderrTail = (stderrTail + chunk).slice(-STDERR_KEPT);
  });
  return child;
}

// Under `flatpak run <app>`, killing the launcher leaves the sandboxed Chrome running, and the
// sandbox keeps its own /tmp, so a profile made under it lives in the runtime directory instead.
// Each run used to leave a whole headless Chrome and a 170 MB profile behind there, until that
// small tmpfs filled and Chrome couldn't start (2026-11-06). So ask Chrome to close itself, over
// its DevTools endpoint, and remove the profile from wherever the sandbox put it.
let browserEndpoint = null;

// Every process of this run's Chrome, found by its profile's name, which no other run shares.
const profileName = basename(profile);
const running = () => spawnSync("pgrep", ["-f", "--", profileName]).stdout.toString().trim() !== "";
const signal = (name) => spawnSync("pkill", ["-" + name, "-f", "--", profileName]);
const pause = (ms) => Atomics.wait(new Int32Array(new SharedArrayBuffer(4)), 0, 0, ms);

// RAL-150: whatever happened, no Chrome of this run is left. The launcher's exit doesn't say the
// sandboxed Chrome has gone, and Browser.close can take longer than any wait under load, so each
// process is found by its profile and asked to stop, then killed, and checked gone.
function reap() {
  if (browser) browser.kill("SIGTERM");
  for (const [name, wait] of [["TERM", 3000], ["KILL", 3000]]) {
    if (!running()) return;
    signal(name);
    for (let waited = 0; waited < wait && running(); waited += 100) pause(100);
  }
}

let finished = false;
function finish(code, value) {
  if (finished) return;
  finished = true;
  // Where the profile was, so a test can check nothing of it survives the run (RAL-97).
  if (value !== undefined) {
    const said = { ...value, profiles: [profile, sandboxed].filter(Boolean) };
    // What Chrome said, with any failure (RAL-160); and an abort at start that was retried, always.
    if (code !== 0 && said.chrome_stderr === undefined) said.chrome_stderr = stderrTail;
    if (firstAbort) said.retried_after_abort = firstAbort;
    if (code !== 0 && exitSeen && said.exit === undefined) said.exit = exitSeen;
    console.log(JSON.stringify(said));
  }
  let cleaned = false;
  const done = () => {
    if (cleaned) return;
    cleaned = true;
    reap();
    for (const dir of [profile, sandboxed]) {
      if (dir) try { rmSync(dir, { recursive: true, force: true }); } catch { /* still held */ }
    }
    process.exit(code);
  };
  if (!browser || !browserEndpoint || browser.exitCode !== null) return done();
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
// A step that throws, such as a fetch refused or a reply without a result, still ends in finish,
// so its cleanup runs (RAL-150); before, node exited at once and left Chrome running.
process.on("uncaughtException", (error) => finish(1, { error: "the script failed: " + error }));
process.on("unhandledRejection", (error) => finish(1, { error: "the script failed: " + error }));
for (const name of ["SIGINT", "SIGTERM", "SIGHUP"]) {
  process.on(name, () => finish(1, { error: "stopped by " + name }));
}
// A failure that ends the run here: finish cleans up and exits, and nothing after it runs.
const fail = (value) => {
  finish(1, value);
  return new Promise(() => {});
};
const isAbort = (code, signal) => code === 134 || signal === "SIGABRT";

// What the machine looked like when Chrome exited, so a failure can be tied to its own records
// (the lamplighter, RAL-160): the time to the second, which coredumpctl lists a crash by; the pid
// this script held, which under flatpak is the launcher's, not Chrome's; the load; and whether the
// document portal's mount was there, whose loss broke the browser tests on 2026-11-13.
function exitFacts(child) {
  const read = (path) => { try { return readFileSync(path, "utf8"); } catch { return ""; } };
  const uid = typeof process.getuid === "function" ? process.getuid() : null;
  const docMount = uid === null ? null
    : read("/proc/self/mountinfo").split("\n").filter((line) => line.includes(` /run/user/${uid}/doc `)).length;
  return {
    at: new Date().toISOString().replace(/\.\d+Z$/, "Z"),
    pid: child ? child.pid : null,
    pid_is: flatpakApp ? "the flatpak launcher; Chrome's own pid is inside the sandbox" : "Chrome",
    loadavg: read("/proc/loadavg").split(" ").slice(0, 3).join(" ") || null,
    doc_portal_mounted: docMount === null ? null : docMount > 0,
  };
}
// The facts of the exit that ended the run, if Chrome's exit did.
let exitSeen = null;

// One start: Chrome launched, its DevTools endpoint found, a page connected, and the code page
// loaded. If Chrome exits first, it throws that exit, so an abort at start can be told apart.
async function startOnce() {
  browserEndpoint = null;
  const child = launch();
  browser = child;
  const died = new Promise((_, reject) => {
    child.once("exit", (code, signal) => {
      exitSeen = exitFacts(child);
      reject(Object.assign(new Error("exited"), { exit: { code, signal } }));
    });
  });
  died.catch(() => {});
  const guard = (promise) => Promise.race([promise, died]);

  // Chrome says where its DevTools endpoint is on standard error.
  const endpoint = await guard(new Promise((resolve) => {
    let text = "";
    child.stderr.on("data", (chunk) => {
      text += chunk;
      const match = /DevTools listening on (ws:\/\/[^\s]+)/.exec(text);
      if (match) {
        browserEndpoint = match[1];
        resolve(new URL(match[1]));
      }
    });
  }));

  const target = await guard((async () =>
    (await fetch(`http://${endpoint.host}/json/new?about:blank`, { method: "PUT" })).json())());
  const socket = new WebSocket(target.webSocketDebuggerUrl);
  await guard(new Promise((resolve, reject) => { socket.onopen = resolve; socket.onerror = reject; }));
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

  await guard(send("Page.enable"));
  // Chrome won't make a window narrower than 500 pixels, so a phone's width, such as 390, is
  // emulated as a mobile viewport instead (RAL-125).
  const [windowWidth, windowHeight] = windowSize.split(",").map(Number);
  if (windowWidth < 500) {
    await guard(send("Emulation.setDeviceMetricsOverride",
      { width: windowWidth, height: windowHeight, deviceScaleFactor: 1, mobile: true }));
  }
  const load = loaded();
  await guard(send("Page.navigate", { url }));
  await guard(load);
  return { send, loaded, evaluate };
}

// RAL-160: an abort at start (exit 134, or SIGABRT, before the code page has loaded) is tried once
// more, and said: on standard error now, and as `retried_after_abort` in what's printed. A second
// abort, or any other exit, fails with what Chrome said each time. Nothing more is papered over.
const exitStatus = ({ code, signal }) => signal || code;
let page;
try {
  page = await startOnce();
} catch (error) {
  if (!error.exit) throw error;
  const first = { status: exitStatus(error.exit), ...exitSeen, chrome_stderr: stderrTail };
  if (!isAbort(error.exit.code, error.exit.signal)) {
    await fail({ error: `the browser exited with ${first.status} before the page loaded` });
  }
  reap();
  for (const dir of [profile, sandboxed]) {
    if (dir) try { rmSync(dir, { recursive: true, force: true }); } catch { /* still held */ }
  }
  firstAbort = first;
  process.stderr.write(`sign-in.mjs: Chrome aborted at start (${first.status}) at ${first.at}, pid ${first.pid} `
    + `(${first.pid_is}), load ${first.loadavg}, document portal mounted: ${first.doc_portal_mounted}; `
    + `trying once more (RAL-160). What it said:\n${first.chrome_stderr}\n`);
  try {
    page = await startOnce();
  } catch (again) {
    if (!again.exit) throw again;
    const status = exitStatus(again.exit);
    await fail({
      error: isAbort(again.exit.code, again.exit.signal)
        ? `Chrome aborted at start twice (${first.status}, then ${status})`
        : `the browser exited with ${status} before the page loaded, after an abort at start`,
    });
  }
}
const { send, loaded, evaluate } = page;
// From here, the browser exiting is a failure, with what it said.
browser.on("exit", (code, signal) => {
  exitSeen = exitFacts(browser);
  finish(1, { error: "the browser exited with " + (signal || code) });
});
if (browser.exitCode !== null) await fail({ error: "the browser exited with " + browser.exitCode });

const codePage = await evaluate(`({
  path: location.pathname + location.search.replace(/code=[0-9a-f]+/, "code=…"),
  button: Boolean(document.querySelector('form[method="post"] button[type="submit"]')),
  referrer: (document.querySelector('meta[name="referrer"]') || {}).content || null,
})`);
if (!codePage.button) await fail({ codePage, error: "no Sign in button" });

let load = loaded();
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
