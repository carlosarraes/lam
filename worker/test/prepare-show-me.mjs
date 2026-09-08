// Regenerate this specific fixture from Mermaid 11.17.0. No publication or remote assets.
import { execFile } from "node:child_process";
import { promisify } from "node:util";
import { readFile, writeFile, mkdtemp } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import assert from "node:assert/strict";

const exec = promisify(execFile);
const root = "test/fixtures/articles";
const folder = await mkdtemp(join(tmpdir(), "lam-mermaid-"));
await exec("npm", ["exec", "--yes", "--package=@mermaid-js/mermaid-cli@11.17.0", "--", "mmdc", "-i", `${root}/show-me.mmd`, "-o", join(folder, "raw.svg"), "-c", `${root}/mermaid.json`, "-p", `${root}/puppeteer.json`]);
const raw = await readFile(join(folder, "raw.svg"), "utf8");
const session = `lam-mermaid-${process.pid}`;
const browser = async (...args) => (await exec("agent-browser", ["--session", session, "--executable-path", "/usr/sbin/chromium", ...args], { maxBuffer: 4 * 1024 * 1024 })).stdout;
try {
  await browser("open", "about:blank");
  await browser("eval", `document.body.innerHTML = ${JSON.stringify(raw)}; void 0`);
  const svg = JSON.parse(await browser("eval", `(() => {
    const source = document.querySelector('svg');
    // Decorative filter shadows have no equivalent in the accepted SVG subset.
    source.querySelectorAll('filter').forEach(n => n.remove());
    // This renderer still emits HTML node labels with flowchart.htmlLabels=false.
    for (const label of source.querySelectorAll('foreignObject')) {
      const text = document.createElementNS('http://www.w3.org/2000/svg', 'text');
      text.textContent = label.textContent;
      text.setAttribute('x', String(Number(label.getAttribute('width')) / 2));
      text.setAttribute('y', String(Number(label.getAttribute('height')) / 2));
      text.style.textAnchor = 'middle'; text.style.dominantBaseline = 'middle';
      text.style.fill = getComputedStyle(label.firstElementChild).color;
      label.replaceWith(text);
    }
    const properties = 'fill fill-opacity fill-rule stroke stroke-width stroke-opacity stroke-linecap stroke-linejoin stroke-miterlimit stroke-dasharray stroke-dashoffset opacity font-family font-size font-weight font-style text-anchor dominant-baseline marker-start marker-mid marker-end'.split(' ');
    const attributes = new Set('viewBox preserveAspectRatio x y x1 y1 x2 y2 cx cy r rx ry width height d points transform dx dy textLength lengthAdjust gradientUnits gradientTransform spreadMethod offset fx fy fr clipPathUnits refX refY markerWidth markerHeight markerUnits orient id'.split(' '));
    const nodes = [source, ...source.querySelectorAll('*')].filter(n => n.tagName !== 'style');
    const styles = nodes.map(n => properties.map(p => [p, getComputedStyle(n).getPropertyValue(p).replace(/url\\(["']?[^#)]*#([^"')]+)["']?\\)/g, 'url(#$1)')]));
    source.querySelectorAll('style').forEach(n => n.remove());
    nodes.forEach((node, i) => {
      for (const attr of [...node.attributes]) if (!attributes.has(attr.name)) node.removeAttribute(attr.name);
      for (const [property, value] of styles[i]) if (value) node.setAttribute(property, value);
    });
    source.setAttribute('width', '100%'); source.removeAttribute('height');
    return source.outerHTML;
  })()`));
  assert.match(svg, /Prepare report/);
  assert.doesNotMatch(svg, /foreignObject|<style|var\(/);
  const html = `<!doctype html><html lang="en"><head><meta charset="utf-8"><meta name="viewport" content="width=device-width,initial-scale=1"><title>Show-me delivery report</title><style>
body{margin:0;background:#f5f7fa;color:#17293a;font:18px system-ui;line-height:1.55}main{max-width:820px;margin:auto;padding:24px}h1{line-height:1.15}figure{margin:24px 0;padding:16px;border:1px solid #ccd5df;border-radius:12px;background:white}img{max-width:100%;height:auto}svg{max-width:100%}.background{background-image:url(chart.png);height:32px}a{color:#165d92}details{padding:12px;background:white}figcaption{font-size:14px;color:#46566b}@media(max-width:560px){main{padding:14px}body{font-size:16px}}
</style></head><body><main><h1 id="top">Show-me delivery report</h1><p>Selectable article text</p><p>Prepare a static report, publish its explicit bundle, and read it in LAM.</p><figure>${svg}<figcaption>Mermaid 11.17.0, SVG text labels and resolved static paint.</figcaption></figure><figure><img src="chart.png" width="240" height="96" alt="Bundled chart"><figcaption>A bundled PNG stays inside the Article.</figcaption></figure><div class="background">Background image</div><details><summary>Expand details</summary><p>Details revealed</p></details><p><a href="#end">Jump to end</a> <a href="__CANARY_ORIGIN__/explicit">Visit external canary</a> <a href="notes.txt">Notes attachment</a></p><div style="height:900px"></div><h2 id="end">End of article</h2></main></body></html>\n`;
  // The committed example uses a deliberate external link, never an automatic resource.
  await writeFile(`${root}/show-me.html`, html.replace('__CANARY_ORIGIN__', 'https://example.com'));
  await browser("eval", "document.body.innerHTML='<canvas width=240 height=96></canvas>'; const c=document.querySelector('canvas').getContext('2d');c.fillStyle='#e6eef7';c.fillRect(0,0,240,96);c.fillStyle='#176f8b';c.fillRect(24,50,40,30);c.fillRect(100,30,40,50);c.fillRect(176,12,40,68); void 0");
  const png = JSON.parse(await browser("eval", "document.querySelector('canvas').toDataURL('image/png')"));
  await writeFile(`${root}/chart.png`, Buffer.from(png.split(',')[1], 'base64'));
  console.log(`Generated show-me.html and chart.png; original Mermaid output: ${join(folder, 'raw.svg')}`);
} finally { await browser("close"); }
