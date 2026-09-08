// Local real-Worker/Chromium acceptance test. No deployment, credentials in URLs, or URL logging.
import { Miniflare, Log, LogLevel, convertV4MiniflareOptions } from "miniflare";
import { build } from "esbuild";
import { createServer } from "node:http";
import { readdir, readFile, mkdtemp } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { execFile } from "node:child_process";
import { promisify } from "node:util";
import assert from "node:assert/strict";
import { createHash } from "node:crypto";
import { deflateSync } from "node:zlib";

const exec = promisify(execFile);
const folder = await mkdtemp(join(tmpdir(), "lam-viewer-browser-"));
const browserSession = `lam-articles-${process.pid}`;
let debug;
async function browser(...args) {
  try {
    return (await exec("agent-browser", ["--session", browserSession, "--executable-path", process.env.CHROMIUM ?? "/usr/sbin/chromium", ...args], { maxBuffer: 1024 * 1024 })).stdout;
  } catch (error) {
    if (args[0] !== "eval") console.log(String(error.stderr).replace(/https?:\/\/[^\s]+/g, "[URL]"));
    throw new Error(`browser ${args[0]} failed; details suppressed to protect session credentials`);
  }
}

async function inspector() {
  const endpoint = (await browser("get", "cdp-url")).trim().replace(/^"|"$/g, "");
  const socket = new WebSocket(endpoint);
  await new Promise((resolve, reject) => { socket.onopen = resolve; socket.onerror = reject; });
  let serial = 0;
  const pending = new Map();
  socket.onmessage = event => {
    const response = JSON.parse(event.data);
    if (response.method === "Log.entryAdded") console.log("Frame log:", response.params.entry.text.replace(/(?:blob:)?https?:\/\/[^\s"']+/g, "[URL]"));
    if (!response.id) return;
    const operation = pending.get(response.id); pending.delete(response.id);
    if (response.error) operation.reject(new Error("CDP operation failed")); else operation.resolve(response.result);
  };
  const call = (method, params = {}, sessionId) => new Promise((resolve, reject) => {
    const id = ++serial; pending.set(id, { resolve, reject }); socket.send(JSON.stringify({ id, method, params, sessionId }));
  });
  const frame = async () => {
    const { targetInfos } = await call("Target.getTargets");
    const target = targetInfos.find(target => target.type === "iframe" && target.url.startsWith("about:srcdoc"));
    assert.ok(target, "opaque iframe has a DevTools target");
    const { sessionId } = await call("Target.attachToTarget", { targetId: target.targetId, flatten: true });
    let root;
    for (const parent of targetInfos.filter(target => target.type === "page" && target.url.includes("/view/articles/"))) {
      const attached = await call("Target.attachToTarget", { targetId: parent.targetId, flatten: true });
      await call("Page.enable", {}, attached.sessionId);
      const tree = await call("Page.getFrameTree", {}, attached.sessionId);
      if (!tree.frameTree.childFrames) root = attached;
      if (tree.frameTree.childFrames?.some(child => child.frame.id === target.targetId)) { root = attached; break; }
    }
    assert.ok(root, "find the opaque frame's actual parent target");
    await call("Log.enable", {}, sessionId);
    const evaluate = async expression => {
      const result = await call("Runtime.evaluate", { expression, returnByValue: true, awaitPromise: true }, sessionId);
      assert.ok(!result.exceptionDetails, "frame inspection completed");
      return result.result.value;
    };
    const activate = async selector => {
      await evaluate(`document.querySelector(${JSON.stringify(selector)}).scrollIntoView({block:'center'})`);
      const box = await evaluate(`(() => {const r=document.querySelector(${JSON.stringify(selector)}).getBoundingClientRect(); return {x:r.x+r.width/2,y:r.y+r.height/2};})()`);
      await call("Runtime.evaluate", { expression: "document.querySelector('iframe').scrollIntoView({block:'start'})" }, root.sessionId);
      const offset = await call("Runtime.evaluate", { expression: "(() => {const r=document.querySelector('iframe').getBoundingClientRect(); return {x:r.x,y:r.y};})()", returnByValue: true }, root.sessionId);
      await call("Page.bringToFront", {}, root.sessionId);
      await browser("mouse", "move", String(Math.round(box.x + offset.result.value.x)), String(Math.round(box.y + offset.result.value.y)));
      await browser("mouse", "down", "left");
      await browser("mouse", "up", "left");
      await new Promise(resolve => setTimeout(resolve, 50));
    };
    return { evaluate, activate };
  };
  return { frame, close: () => socket.close() };
}
const hits = [];
const canary = createServer((req, res) => {
  if (req.url === "/favicon.ico") { res.writeHead(204); res.end(); return; }
  hits.push({ url: req.url, referrer: req.headers.referer, authorization: req.headers.authorization, cookie: req.headers.cookie });
  res.setHeader("Content-Type", "text/html");
  res.end('<!doctype html><title>External canary</title><p id="opener"></p><script>document.querySelector("#opener").textContent = window.opener === null ? "no opener" : "opener exposed";</script>');
});
await new Promise(resolve => canary.listen(0, "127.0.0.1", resolve));
const canaryOrigin = `http://127.0.0.1:${canary.address().port}`;
const bundle = await build({ entryPoints: ["src/index.ts"], bundle: true, write: false, format: "esm", platform: "browser", target: "es2022", external: ["cloudflare:workers"] });
const mf = new Miniflare(convertV4MiniflareOptions({ log: new Log(LogLevel.NONE), port: 0, workers: [{ name: "lam-test", script: bundle.outputFiles[0].text, modules: true, compatibilityDate: "2026-08-01",
  bindings: { LAM_TOKEN: "browser-test-token", LAM_HMAC_SECRET: "browser-test-secret", NTFY_TOPIC: "browser-test-topic" },
  d1Databases: ["DB"], r2Buckets: ["ARTICLE_BUCKET"], durableObjects: { TOPIC: { className: "Topic", useSQLite: true }, EVENTS: { className: "EventStream", useSQLite: true } } }] }));
try {
  const origin = (await mf.ready).origin;
  const db = await mf.getD1Database("DB");
  for (const name of (await readdir("migrations")).filter(name => name.endsWith(".sql")).sort()) {
    for (const statement of (await readFile(join("migrations", name), "utf8")).split(";").map(sql => sql.trim()).filter(Boolean)) await db.prepare(statement).run();
  }
  const api = (path, method = "GET", body) => fetch(origin + path, { method, headers: { Authorization: "Bearer browser-test-token", "Idempotency-Key": crypto.randomUUID() }, body: body === undefined ? undefined : JSON.stringify(body) });
  const chunk = (type, data) => {
    const payload = Buffer.concat([Buffer.from(type), data]); let crc = 0xffffffff;
    for (const byte of payload) { crc ^= byte; for (let bit = 0; bit < 8; bit++) crc = (crc >>> 1) ^ ((crc & 1) ? 0xedb88320 : 0); }
    const length = Buffer.alloc(4); length.writeUInt32BE(data.length);
    const checksum = Buffer.alloc(4); checksum.writeUInt32BE((crc ^ 0xffffffff) >>> 0);
    return Buffer.concat([length, payload, checksum]);
  };
  const ihdr = Buffer.from([0,0,0,1,0,0,0,1,8,6,0,0,0]);
  const png = Buffer.concat([Buffer.from([137,80,78,71,13,10,26,10]), chunk("IHDR", ihdr), chunk("IDAT", deflateSync(Buffer.from([0,255,0,0,255]))), chunk("IEND", Buffer.alloc(0))]);
  const source = `<h1 id="top">Browser report</h1><p>Selectable article text</p><img src="pixel.png" alt="Bundled pixel" width="80" height="80"><style>.background{background-image:url(pixel.png);width:180px;height:32px}</style><div class="background">Background image</div><svg viewBox="0 0 10 10" width="80" height="80"><image href="pixel.png" width="10" height="10"></image></svg><details><summary>Expand details</summary><p>Details revealed</p></details><a href="#end">Jump to end</a> <a href="${canaryOrigin}/explicit">Visit external canary</a> <a href="notes.txt">Notes attachment</a><div style="height:1600px"></div><h2 id="end">End of article</h2>`;
  const sources = [{ path: "index.html", media_type: "text/html", disposition: "inline", bytes: Buffer.from(source) }, { path: "pixel.png", media_type: "image/png", disposition: "inline", bytes: png }, { path: "notes.txt", media_type: "text/plain", disposition: "attachment", bytes: Buffer.from("download-only notes\n") }];
  const draft = { title: "Browser report", summary: "Browser acceptance", name: "Test", source_host: "local", source_project: "lam", silent: true, assets: sources.map(({ bytes, ...asset }) => ({ ...asset, size: bytes.length, sha256: createHash("sha256").update(bytes).digest("hex") })) };
  const staged = await api("/v2/articles/uploads", "POST", draft);
  assert.equal(staged.status, 201);
  const { id } = await staged.json();
  for (const [index, source] of sources.entries()) assert.equal((await fetch(`${origin}/v2/articles/${id}/assets/${index}`, { method: "PUT", headers: { Authorization: "Bearer browser-test-token" }, body: source.bytes })).status, 204);
  assert.equal((await api(`/v2/articles/${id}/publish`, "POST")).status, 200);
  const readState = async () => (await api(`/v2/articles/${id}`)).json();
  await browser("open", `${origin}/view/articles/${id}`);
  await browser("wait", "--text", "Could not open the article");
  assert.equal((await readState()).read_at, null);
  const openSession = async () => {
    await browser("open", "about:blank");
    const issued = await api(`/v2/articles/${id}/view-session`, "POST");
    const session = await issued.json();
    await browser("eval", `location.replace(${JSON.stringify(session.url)}); void 0`);
    try { await browser("wait", "--text", "Opened · read"); }
    catch { console.log("Wrapper status:", await browser("eval", "document.querySelector('#status').textContent")); throw new Error("viewer did not reach opened state"); }
  };
  await openSession();
  assert.equal((await readState()).version, 1);
  assert.match(await browser("eval", "JSON.stringify({fragment:location.hash, isolated:document.querySelector('iframe').contentDocument === null})"), /isolated.*true/);
  assert.equal(hits.length, 0, "no automatic canary request");
  debug = await inspector();
  const frame = await debug.frame();
  assert.equal(await frame.evaluate("document.querySelector('img').naturalWidth"), 1);
  await frame.activate("summary");
  assert.equal(await frame.evaluate("document.querySelector('details').open"), true);
  await frame.activate('a[href="about:srcdoc#end"]');
  assert.equal(await frame.evaluate("location.hash"), "#end");
  assert.equal((await readState()).version, 1, "fragment navigation does not repeat read updates");
  await frame.evaluate("getSelection().selectAllChildren(document.querySelector('p'))");
  assert.equal(await frame.evaluate("getSelection().toString()"), "Selectable article text");
  await frame.activate(`a[href="${canaryOrigin}/explicit"]`);
  await browser("tab", "t2");
  await browser("wait", "--text", "no opener");
  assert.equal(hits.length, 1);
  assert.deepEqual(hits[0], { url: "/explicit", referrer: undefined, authorization: undefined, cookie: undefined });
  await browser("tab", "t1");
  await browser("download", "#attachments button", join(folder, "notes.txt"));
  assert.equal(await readFile(join(folder, "notes.txt"), "utf8"), "download-only notes\n");
  await browser("find", "text", "Zoom in", "click");
  assert.match(await browser("eval", "document.querySelector('iframe').style.zoom"), /1.1/);
  await frame.evaluate("scrollTo(0,0)");
  await browser("eval", "scrollTo(0,0); void 0");
  await browser("screenshot", join(folder, "article.png"));

  const bucket = await mf.getR2Bucket("ARTICLE_BUCKET");
  const canonicalKey = `articles/${id}/sanitized/index.html`;
  const canonical = await (await bucket.get(canonicalKey)).text();
  const unread = async () => {
    const before = await readState();
    const response = await api(`/v2/articles/${id}/read`, "PUT", { read: false, version: before.version });
    assert.equal(response.status, 200);
    return (await response.json()).version;
  };
  await unread();
  await bucket.delete(`articles/${id}/original/1`);
  await openSession();
  assert.match(await browser("eval", "document.querySelector('#status').textContent"), /images or placements unavailable/);
  assert.equal(await (await debug.frame()).evaluate("document.body.textContent.includes('Selectable article text')"), true);
  await bucket.put(`articles/${id}/original/1`, png);

  const failedVersion = await unread();
  await bucket.delete(canonicalKey);
  await browser("open", "about:blank");
  const failureSession = await (await api(`/v2/articles/${id}/view-session`, "POST")).json();
  await browser("eval", `location.replace(${JSON.stringify(failureSession.url)}); void 0`);
  await browser("wait", "--text", "Could not open the article");
  assert.equal((await readState()).version, failedVersion);
  assert.equal((await readState()).read_at, null);
  await bucket.put(canonicalKey, canonical);
  await browser("click", "#retry");
  await browser("wait", "--text", "Opened · read");
  assert.equal((await readState()).version, failedVersion + 1);

  // Simulate a compromised canonical object to prove the independent browser boundary.
  await bucket.put(canonicalKey, canonical.replace("</body>", `<script>window.attackExecuted=true;fetch('${canaryOrigin}/script');parent.document.body.textContent='escaped';</script><img src="${canaryOrigin}/automatic-image"><style>@import url('${canaryOrigin}/automatic-css');</style><iframe src="${canaryOrigin}/automatic-frame"></iframe></body>`));
  await unread();
  await openSession();
  const attacked = await debug.frame();
  assert.equal(await attacked.evaluate("window.attackExecuted === undefined"), true);
  assert.equal(hits.length, 1, "script, image, CSS and iframe canaries remain blocked");
  assert.match(await browser("eval", "document.querySelector('#title').textContent"), /Browser report/);
  assert.match(await browser("eval", "document.querySelector('iframe').contentDocument === null"), /true/);
  const newerUnread = await unread();
  await browser("tab", "t2");
  await browser("tab", "t1");
  await new Promise(resolve => setTimeout(resolve, 100));
  assert.equal((await readState()).version, newerUnread, "visibility changes never repeat a successful read update");
  assert.equal((await readState()).read_at, null);
  console.log("PASS: authenticated render, preview inert, opaque sandbox, image decode, details, fragments, selection, zoom, explicit noreferrer/noopener link, download bytes");
  console.log("PASS: missing-image feedback, failed-content unread preservation, explicit retry, blocked script/network/frame canaries, no repeated read after newer unread");
  console.log(`Screenshot: ${join(folder, "article.png")}`);
} finally {
  debug?.close();
  await browser("close").catch(() => {});
  await mf.dispose();
  await new Promise(resolve => canary.close(resolve));
}
