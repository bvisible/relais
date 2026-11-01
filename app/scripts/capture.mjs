// Regenerates the images in docs/assets from the panel's browser preview, in a
// throwaway Chrome profile. The accounts on them are the preview's made-up ones:
// never capture the installed app, it shows real accounts.
//
// With `pnpm dev --port 1420 --strictPort` running: node scripts/capture.mjs

import { spawn, spawnSync } from "node:child_process";
import { mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

const CHROME = "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome";
const PREVIEW = "http://localhost:1420/?state=showcase";
const BANNER = "http://localhost:1420/scripts/hero.html";
const PORT = 9333;
const OUT = join(dirname(fileURLToPath(import.meta.url)), "../../docs/assets");
const PAPER = { r: 250, g: 243, b: 234, a: 1 };

const sleep = (ms) => new Promise((resolve) => setTimeout(resolve, ms));

async function startChrome(profile) {
  const chrome = spawn(CHROME, ["--headless=new", `--remote-debugging-port=${PORT}`, `--user-data-dir=${profile}`, "--no-first-run", "--no-default-browser-check", "--disable-extensions", "--hide-scrollbars", "--lang=en-US", "about:blank"], { stdio: "ignore" });
  for (let i = 0; i < 100; i += 1) {
    try {
      await fetch(`http://127.0.0.1:${PORT}/json/version`);
      return chrome;
    } catch {
      await sleep(100);
    }
  }
  throw new Error("Chrome did not start");
}

async function openPage() {
  const target = await (await fetch(`http://127.0.0.1:${PORT}/json/new?about:blank`, { method: "PUT" })).json();
  const socket = new WebSocket(target.webSocketDebuggerUrl);
  await new Promise((resolve) => socket.addEventListener("open", resolve, { once: true }));
  let id = 0;
  const waiting = new Map();
  socket.addEventListener("message", (event) => {
    const message = JSON.parse(event.data);
    if (message.id && waiting.has(message.id)) {
      const { resolve, reject } = waiting.get(message.id);
      waiting.delete(message.id);
      if (message.error) reject(new Error(message.error.message));
      else resolve(message.result);
    }
  });
  const send = (method, params = {}) =>
    new Promise((resolve, reject) => {
      id += 1;
      waiting.set(id, { resolve, reject });
      socket.send(JSON.stringify({ id, method, params }));
    });
  const evaluate = async (expression) => (await send("Runtime.evaluate", { expression, awaitPromise: true, returnByValue: true })).result.value;
  await send("Page.enable");
  await send("Runtime.enable");
  await send("Page.addScriptToEvaluateOnNewDocument", { source: "Object.defineProperty(navigator, 'language', { get: () => 'en-US' });" });
  return { send, evaluate, close: () => socket.close() };
}

async function show(page, { url, width, height, scale, dark = false, background, full = false }) {
  await page.send("Emulation.setDeviceMetricsOverride", { width, height, deviceScaleFactor: scale, mobile: false });
  await page.send("Emulation.setEmulatedMedia", { features: [{ name: "prefers-color-scheme", value: dark ? "dark" : "light" }] });
  await page.send("Emulation.setDefaultBackgroundColorOverride", { color: background ?? { r: 0, g: 0, b: 0, a: 0 } });
  await page.send("Page.navigate", { url });
  await sleep(300);
  await page.evaluate(`localStorage.setItem("relais.fullView", "${full ? "on" : "off"}"); localStorage.setItem("relais.followTheme", "off"); location.reload()`);
  await sleep(1300);
  await page.evaluate("document.fonts.ready.then(() => true)");
}

const box = (page, selector) => page.evaluate(`(() => { const r = document.querySelector(${JSON.stringify(selector)}).getBoundingClientRect(); return { x: r.x, y: r.y, width: r.width, height: r.height, scale: 1 }; })()`);

async function shoot(page, file, clip) {
  const { data } = await page.send("Page.captureScreenshot", { format: "png", ...(clip ? { clip } : {}) });
  writeFileSync(join(OUT, file), Buffer.from(data, "base64"));
}

const clickButton = (page, text) => page.evaluate(`[...document.querySelectorAll("button")].find((b) => b.textContent.trim() === ${JSON.stringify(text)}).click()`);

// A cursor drawn on the page: headless Chrome shows none.
const CURSOR = `(() => {
  const cursor = document.createElement("div");
  cursor.id = "demo-cursor";
  cursor.innerHTML = '<svg width="22" height="22" viewBox="0 0 24 24"><path d="M5 3l14 9-6 1-3 6z" fill="#141414" stroke="#fffdf8" stroke-width="1.6" stroke-linejoin="round"/></svg>';
  cursor.style.cssText = "position:fixed;left:150px;top:500px;z-index:99;pointer-events:none;transition:left 620ms cubic-bezier(.2,0,0,1),top 620ms cubic-bezier(.2,0,0,1)";
  document.body.append(cursor);
})()`;

const moveCursor = (page, text) => page.evaluate(`(() => { const b = [...document.querySelectorAll("button")].find((b) => b.textContent.trim() === ${JSON.stringify(text)}); const r = b.getBoundingClientRect(); const c = document.getElementById("demo-cursor"); c.style.left = (r.x + r.width / 2) + "px"; c.style.top = (r.y + r.height / 2) + "px"; })()`);

/** Records the page for `step`, then turns the frames into a small GIF. */
async function record(page, file, step) {
  const frames = mkdtempSync(join(tmpdir(), "relais-frames-"));
  let recording = true;
  let count = 0;
  const started = Date.now();
  const FPS = 10;
  const loop = (async () => {
    while (recording) {
      const { data } = await page.send("Page.captureScreenshot", { format: "png" });
      const wanted = Math.floor(((Date.now() - started) / 1000) * FPS);
      while (count <= wanted) {
        writeFileSync(join(frames, `${String(count).padStart(5, "0")}.png`), Buffer.from(data, "base64"));
        count += 1;
      }
    }
  })();
  await step();
  recording = false;
  await loop;
  const filter = "split[a][b];[a]palettegen=max_colors=160:stats_mode=diff[p];[b][p]paletteuse=dither=bayer:bayer_scale=4:diff_mode=rectangle";
  spawnSync("ffmpeg", ["-y", "-loglevel", "error", "-framerate", String(FPS), "-i", join(frames, "%05d.png"), "-vf", filter, "-loop", "0", join(OUT, file)], { stdio: "inherit" });
  rmSync(frames, { recursive: true, force: true });
}

mkdirSync(OUT, { recursive: true });
const profile = mkdtempSync(join(tmpdir(), "relais-capture-"));
const chrome = await startChrome(profile);
try {
  const page = await openPage();

  await show(page, { url: PREVIEW, width: 400, height: 720, scale: 2 });
  const panel = await box(page, ".panel");
  await shoot(page, "panel.png", panel);
  await clickButton(page, "Switch here");
  await sleep(700);
  await shoot(page, "confirm.png", await box(page, ".panel"));

  await show(page, { url: PREVIEW, width: 400, height: 720, scale: 2, dark: true });
  await shoot(page, "panel-dark.png", await box(page, ".panel"));

  await show(page, { url: PREVIEW, width: 400, height: 720, scale: 2, full: true });
  await sleep(500);
  await shoot(page, "panel-full.png", await box(page, ".panel"));

  await show(page, { url: PREVIEW, width: 400, height: 720, scale: 1 });
  const { data: panelImage } = await page.send("Page.captureScreenshot", { format: "png", clip: await box(page, ".panel") });
  await show(page, { url: PREVIEW, width: 400, height: 720, scale: 2 });
  const { data: sharpPanel } = await page.send("Page.captureScreenshot", { format: "png", clip: await box(page, ".panel") });
  for (const [file, scale, image] of [
    ["hero.png", 2, sharpPanel],
    ["social-preview.png", 1, panelImage],
  ]) {
    await page.send("Emulation.setDeviceMetricsOverride", { width: 1280, height: 640, deviceScaleFactor: scale, mobile: false });
    await page.send("Emulation.setDefaultBackgroundColorOverride", { color: PAPER });
    await page.send("Page.navigate", { url: BANNER });
    await sleep(1200);
    await page.evaluate(`new Promise((resolve) => { const img = document.getElementById("panel"); img.onload = resolve; img.src = "data:image/png;base64,${image}"; })`);
    await page.evaluate("document.fonts.ready.then(() => true)");
    await sleep(200);
    await shoot(page, file);
  }

  await show(page, { url: PREVIEW, width: 440, height: 650, scale: 2, background: PAPER });
  await page.evaluate(`(() => { const style = document.createElement("style"); style.textContent = "html, body { background: #faf3ea; } .panel { margin: 20px auto; box-shadow: 0 6px 12px rgba(40, 28, 18, 0.06), 0 16px 36px rgba(40, 28, 18, 0.1); }"; document.head.append(style); })()`);
  await page.evaluate(CURSOR);
  await sleep(300);
  await record(page, "demo.gif", async () => {
    await sleep(1500);
    await moveCursor(page, "Switch here");
    await sleep(900);
    await clickButton(page, "Switch here");
    await sleep(1800);
    await moveCursor(page, "Switch to spare");
    await sleep(900);
    await clickButton(page, "Switch to spare");
    await sleep(4400);
  });
  page.close();
} finally {
  chrome.kill();
  await sleep(300);
  rmSync(profile, { recursive: true, force: true });
}
