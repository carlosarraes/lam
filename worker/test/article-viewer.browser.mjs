// Local real-Worker/Chromium acceptance test. No deployment, credentials in URLs, or URL logging.
import { Miniflare, Log, LogLevel, convertV4MiniflareOptions } from "miniflare";
import { build } from "esbuild";
import { createServer } from "node:http";
import { readdir, readFile, mkdtemp, writeFile, copyFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join, resolve } from "node:path";
import { execFile } from "node:child_process";
import { promisify } from "node:util";
import assert from "node:assert/strict";
import { createHash } from "node:crypto";

const exec = promisify(execFile);
await exec("cargo", ["build", "--locked"], { cwd: "../cli" });
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
  return { frame, call, close: () => socket.close() };
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
  const png = await readFile("test/fixtures/articles/chart.png");
  const source = (await readFile("test/fixtures/articles/show-me.html", "utf8")).replace("https://example.com/explicit", `${canaryOrigin}/explicit`);
  const notes = await readFile("test/fixtures/articles/notes.txt");
  await writeFile(join(folder, "index.html"), source);
  await copyFile("test/fixtures/articles/chart.png", join(folder, "chart.png"));
  await copyFile("test/fixtures/articles/notes.txt", join(folder, "notes.txt"));
  await writeFile(join(folder, "credentials.env"), "DO-NOT-UPLOAD-THIS-SECRET\n");
  const uploads = [];
  let interrupted = false;
  const proxy = createServer(async (req, res) => {
    const chunks = []; for await (const chunk of req) chunks.push(chunk);
    const body = Buffer.concat(chunks);
    uploads.push({ path: req.url, method: req.method, body });
    const response = await fetch(origin + req.url, { method: req.method, headers: { Authorization: req.headers.authorization, "Idempotency-Key": req.headers["idempotency-key"] ?? "" }, body: body.length ? body : undefined });
    const bytes = Buffer.from(await response.arrayBuffer());
    if (!interrupted && req.method === "PUT") { interrupted = true; req.socket.destroy(); return; }
    res.writeHead(response.status, Object.fromEntries(response.headers)); res.end(bytes);
  });
  await new Promise(resolve => proxy.listen(0, "127.0.0.1", resolve));
  await writeFile(join(folder, "config.toml"), `server = "http://127.0.0.1:${proxy.address().port}"\ntoken = "browser-test-token"\ntopic = "test"\n`);
  const cli = async assets => exec(resolve("../cli/target/debug/lam"), ["article", "publish", "--file", join(folder, "index.html"), "--title", "Show-me delivery report", "--summary", "Static Mermaid and explicit assets", ...assets.flatMap(path => ["--asset", path]), "--silent"], { env: { ...process.env, LAM_CONFIG: join(folder, "config.toml"), LAM_NAME: "test:browser" } });
  let id;
  try {
    await assert.rejects(cli(["notes.txt"]), error => {
      assert.equal(error.code, 1);
      assert.match(error.stderr, /article draft [a-f0-9-]{36} was not published/);
      assert.match(error.stderr, /chart.png must be supplied in the manifest/);
      return true;
    });
    assert.equal(hits.length, 0, "omitted resource rejection never fetches the network canary");
    const rejected = uploads.find(row => row.path === "/v2/articles/uploads");
    assert.deepEqual(JSON.parse(rejected.body).assets.map(asset => asset.path), ["index.html", "notes.txt"]);
    assert.ok(uploads.every(row => !row.body.includes("DO-NOT-UPLOAD-THIS-SECRET")));
    id = (await cli(["chart.png", "notes.txt"])).stdout.trim();
    assert.match(id, /^[a-f0-9-]{36}$/);
    const attempts = uploads.filter(row => row.method === "PUT" && row.path === uploads.find(row => row.method === "PUT").path);
    assert.equal(attempts.length, 2); assert.deepEqual(attempts[0].body, attempts[1].body);
    assert.ok(uploads.every(row => !row.body.includes("DO-NOT-UPLOAD-THIS-SECRET")));
  } finally { await new Promise(resolve => proxy.close(resolve)); }
  assert.equal((await db.prepare("SELECT count(*) AS n FROM article_notification_jobs WHERE article_id = ?").bind(id).first()).n, 0);
  console.log("PASS: real candidate CLI, omitted-reference rejection/draft ID, explicit manifest/secret exclusion, interrupted upload byte-identical retry, silent publication");
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
  assert.equal(await frame.evaluate("document.querySelector('img').naturalWidth"), 240);
  assert.equal(await frame.evaluate("document.querySelector('svg').textContent.includes('Prepare report')"), true);
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
  await browser("download", "#attachments button", join(folder, "downloaded-notes.txt"));
  assert.deepEqual(await readFile(join(folder, "downloaded-notes.txt")), notes);
  await browser("find", "text", "Zoom in", "click");
  assert.match(await browser("eval", "document.querySelector('iframe').style.zoom"), /1.1/);
  await frame.evaluate("scrollTo(0,0)");
  await browser("eval", "scrollTo(0,0); void 0");
  await browser("screenshot", join(folder, "article.png"));

  const bucket = await mf.getR2Bucket("ARTICLE_BUCKET");
  const canonicalKey = `articles/${id}/sanitized/index.html`;
  const canonical = await (await bucket.get(canonicalKey)).text();
  assert.notEqual(createHash("sha256").update(canonical).digest("hex"), createHash("sha256").update(source).digest("hex"));
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
  assert.match(await browser("eval", "document.querySelector('#title').textContent"), /Show-me delivery report/);
  assert.match(await browser("eval", "document.querySelector('iframe').contentDocument === null"), /true/);
  const newerUnread = await unread();
  await browser("tab", "t2");
  await browser("tab", "t1");
  await new Promise(resolve => setTimeout(resolve, 100));
  assert.equal((await readState()).version, newerUnread, "visibility changes never repeat a successful read update");
  assert.equal((await readState()).read_at, null);
  await bucket.put(canonicalKey, canonical);

  // Load the first document while Chromium reports the tab hidden, then expose it.
  const issuedBackground = await (await api(`/v2/articles/${id}/view-session`, "POST")).json();
  const background = await debug.call("Target.createTarget", { url: "about:blank", background: true });
  const attached = await debug.call("Target.attachToTarget", { targetId: background.targetId, flatten: true });
  const evaluateBackground = async expression => (await debug.call("Runtime.evaluate", { expression, returnByValue: true }, attached.sessionId)).result.value;
  await debug.call("Page.navigate", { url: issuedBackground.url }, attached.sessionId);
  for (let attempt = 0; attempt < 100 && !await evaluateBackground("document.querySelector('iframe')?.hidden === false"); attempt++) await new Promise(resolve => setTimeout(resolve, 50));
  assert.equal(await evaluateBackground("document.querySelector('iframe')?.hidden"), false);
  assert.equal(await evaluateBackground("document.visibilityState"), "hidden");
  assert.equal((await readState()).version, newerUnread);
  assert.equal((await readState()).read_at, null, "first load in a background tab stays unread");
  await debug.call("Target.activateTarget", { targetId: background.targetId });
  for (let attempt = 0; attempt < 100 && (await readState()).read_at === null; attempt++) await new Promise(resolve => setTimeout(resolve, 50));
  assert.equal((await readState()).version, newerUnread + 1);
  await debug.call("Target.closeTarget", { targetId: background.targetId });
  await browser("tab", "t1");

  const offscreenVersion = await unread();
  await browser("eval", "window.task7Target=true; void 0");
  const { targetInfos } = await debug.call("Target.getTargets");
  let parentSession;
  for (const target of targetInfos.filter(target => target.type === "page")) {
    const session = await debug.call("Target.attachToTarget", { targetId: target.targetId, flatten: true });
    const result = await debug.call("Runtime.evaluate", { expression: "window.task7Target === true", returnByValue: true }, session.sessionId);
    if (result.result.value) { parentSession = session; break; }
  }
  assert.ok(parentSession, "inject into the browser automation's actual active page");
  await debug.call("Page.enable", {}, parentSession.sessionId);
  const injection = await debug.call("Page.addScriptToEvaluateOnNewDocument", { source: "document.addEventListener('DOMContentLoaded', () => {const frame=document.querySelector('iframe'); if(frame){frame.style.position='fixed';frame.style.top='2000px';}})" }, parentSession.sessionId);
  const offscreenSession = await (await api(`/v2/articles/${id}/view-session`, "POST")).json();
  await browser("open", "about:blank");
  await browser("eval", `location.replace(${JSON.stringify(offscreenSession.url)}); void 0`);
  await browser("wait", "--fn", "document.querySelector('iframe')?.hidden === false");
  assert.equal((await browser("eval", "document.querySelector('iframe').getBoundingClientRect().top >= innerHeight")).trim(), "true");
  assert.equal((await readState()).version, offscreenVersion, "first offscreen load stays unread");
  await browser("eval", "const frame=document.querySelector('iframe');frame.style.position='static';frame.style.top='';frame.scrollIntoView(); void 0");
  await browser("wait", "--text", "Opened · read");
  await debug.call("Page.removeScriptToEvaluateOnNewDocument", { identifier: injection.identifier }, parentSession.sessionId);

  const transportVersion = await unread();
  await browser("network", "route", `${origin}/view/articles/${id}/read`, "--abort");
  const failedRead = await (await api(`/v2/articles/${id}/view-session`, "POST")).json();
  await browser("open", "about:blank");
  await browser("eval", `location.replace(${JSON.stringify(failedRead.url)}); void 0`);
  await browser("wait", "--text", "read state could not be saved");
  assert.equal((await readState()).version, transportVersion);
  assert.equal((await api(`/v2/articles/${id}/read`, "PUT", { read: true, version: transportVersion })).status, 200);
  const unreadAfterFailure = await unread();
  await browser("network", "unroute", `${origin}/view/articles/${id}/read`);
  await browser("click", "#retry");
  await browser("wait", "--text", "the newer state was kept");
  assert.equal((await readState()).version, unreadAfterFailure);
  assert.equal((await readState()).read_at, null);

  // Expire the real D1 viewer sessions and exercise the visible download control.
  await db.prepare("UPDATE article_view_sessions SET expires_at = '2000-01-01T00:00:00.000Z' WHERE article_id = ?").bind(id).run();
  await browser("click", "#attachments button");
  await browser("wait", "--text", "Download failed. Try the download button again.");
  assert.equal((await readState()).version, unreadAfterFailure);
  const fresh = await (await api(`/v2/articles/${id}/view-session`, "POST")).json();
  const exchange = await fetch(`${origin}/view/articles/${id}/exchange`, { method: "POST", headers: { Authorization: `Bearer ${new URL(fresh.url).hash.slice(1)}` } });
  const capability = (await exchange.json()).token;
  assert.equal((await fetch(`${origin}/view/articles/${id}/images/1`, { headers: { Authorization: `Bearer ${capability}` } })).status, 200);
  const otherDraft = JSON.parse(uploads.filter(row => row.path === "/v2/articles/uploads").at(-1).body);
  const otherId = (await (await api("/v2/articles/uploads", "POST", otherDraft)).json()).id;
  for (const [index, bytes] of [Buffer.from(source), png, notes].entries()) {
    assert.equal((await fetch(`${origin}/v2/articles/${otherId}/assets/${index}`, { method: "PUT", headers: { Authorization: "Bearer browser-test-token" }, body: bytes })).status, 204);
  }
  assert.equal((await api(`/v2/articles/${otherId}/publish`, "POST")).status, 200);
  const cross = await fetch(`${origin}/view/articles/${otherId}/images/1`, { headers: { Authorization: `Bearer ${capability}` } });
  assert.equal(cross.status, 401);
  assert.equal(hits.length, 1);
  if (process.env.LAM_ANDROID_FIXTURE === "1") {
    assert.ok(process.env.LAM_ADB, "emulator-only adb wrapper is required");
    const result = await exec(process.env.LAM_ADB, ["shell", "am", "instrument", "-w", "-r", "-e", "class", "dev.carraes.lam.articles.ArticleLocalWorkerTest", "-e", "articleOrigin", origin.replace("127.0.0.1", "10.0.2.2").replace("localhost", "10.0.2.2"), "-e", "articleId", id, "dev.carraes.lam.debug.test/androidx.test.runner.AndroidJUnitRunner"], { maxBuffer: 1024 * 1024 });
    assert.match(result.stdout, /OK \(1 test\)/);
    assert.doesNotMatch(result.stdout, /FAILURES|INSTRUMENTATION_FAILED/);
    assert.notEqual((await readState()).read_at, null, "actual emulator reader committed canonical read state");
    console.log("PASS: same real CLI-published fixture read and marked read by disposable Android emulator");
  }
  console.log("PASS: authenticated render, preview inert, opaque sandbox, image decode, details, fragments, selection, zoom, explicit noreferrer/noopener link, download bytes");
  console.log("PASS: missing-image feedback, failed-content unread preservation, explicit retry, blocked script/network/frame canaries, no repeated read after newer unread");
  console.log("PASS: initial background/offscreen unread, failed read plus newer unread/manual retry, expired viewer, cross-article resource denial");
  console.log(`Screenshot: ${join(folder, "article.png")}`);
} finally {
  debug?.close();
  await browser("close").catch(() => {});
  await mf.dispose();
  await new Promise(resolve => canary.close(resolve));
}
