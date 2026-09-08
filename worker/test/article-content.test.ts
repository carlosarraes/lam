/// <reference types="vite/client" />
import { describe, expect, it, vi } from "vitest";
import { parse, Token, type DefaultTreeAdapterMap } from "parse5";
import { prepareArticleHtml, validateArticleAsset } from "../src/articles/content";
import type { Asset } from "../src/domain/Article";
import staticHtml from "./fixtures/articles/static.html?raw";
import scriptHtml from "./fixtures/articles/script.html?raw";
import cssImport from "./fixtures/articles/css-import.html?raw";
import svgFile from "./fixtures/articles/svg-file.html?raw";

const assets: Asset[] = [
  { path: "index.html", media_type: "text/html", size: 100, sha256: "a".repeat(64), disposition: "inline" },
  { path: "images/chart.png", media_type: "image/png", size: 100, sha256: "b".repeat(64), disposition: "inline" },
  { path: "notes.txt", media_type: "text/plain", size: 10, sha256: "c".repeat(64), disposition: "attachment" },
];
function elements(html: string) {
  const result: DefaultTreeAdapterMap["element"][] = [];
  const walk = (node: DefaultTreeAdapterMap["node"]) => {
    if ("tagName" in node) result.push(node);
    if ("childNodes" in node) node.childNodes.forEach(walk);
  };
  walk(parse(html));
  return result;
}
const attr = (node: DefaultTreeAdapterMap["element"], name: string) => node.attrs.find(a => a.name === name)?.value;

describe("static article content", () => {
  it("preserves readable structure and rewrites resources to stable manifest indexes", () => {
    const output = prepareArticleHtml(staticHtml, assets);
    const nodes = elements(output);
    for (const tag of ["table", "pre", "code", "details", "summary", "svg", "path", "text"])
      expect(nodes.some(n => n.tagName === tag)).toBe(true);
    expect(output).toContain("if (ready) {\n  publish();\n}\n");
    expect(attr(nodes.find(n => n.tagName === "img")!, "src")).toBe("lam-asset:1");
    expect(output).toContain("url(lam-asset:1)");
    expect(nodes.filter(n => n.tagName === "a").map(n => attr(n, "href"))).toEqual(["#report", "https://example.org/research"]);
    expect(attr(nodes.find(n => attr(n, "data-lam-attachment") !== undefined)!, "data-lam-attachment")).toBe("2");
    expect(output).not.toContain("download=");
    expect(prepareArticleHtml(staticHtml, assets)).toBe(output);
  });

  it.each([
    scriptHtml, cssImport, svgFile,
    '<img src="https://attacker.invalid/a.png">',
    '<img src="data:image/png;base64,aA==">',
    '<img src="blob:https://example.org/id">',
    '<img src="//attacker.invalid/a.png">',
    '<img src="images/chart.png" onerror="alert(1)">',
    '<img srcset="images/chart.png 1x, https://attacker.invalid/a.png 2x">',
    '<a href="javascript:alert(1)">Go</a>',
    '<a href="java&#x09;script:alert(1)">Go</a>',
    '<a href="https://example.org" ping="https://attacker.invalid">Go</a>',
    '<base href="https://attacker.invalid">',
    '<meta http-equiv="refresh" content="0;url=https://attacker.invalid">',
    '<link rel="stylesheet" href="https://attacker.invalid/a.css">',
    '<iframe srcdoc="<script>alert(1)</script>"></iframe>',
    '<object data="notes.txt"></object>', '<embed src="notes.txt">',
    '<form action="https://attacker.invalid"><input name="password"></form>',
    '<svg><foreignObject><p>Confusion</p></foreignObject></svg>',
    '<svg><animate attributeName="href" values="https://attacker.invalid"/></svg>',
    '<svg><use href="https://attacker.invalid/x.svg#x"/></svg>',
    '<svg><path fill="url(https://attacker.invalid/x.svg#x)"/></svg>',
    '<svg><style>@import "https://attacker.invalid/a.css";</style></svg>',
    '<math><mtext><img src="images/chart.png"></mtext></math>',
    '<div style="background: url(https://attacker.invalid/a.png)">x</div>',
    '<div style="background: u\\72l(https://attacker.invalid/a.png)">x</div>',
    '<div style="background: image-set(\'https://attacker.invalid/a.png\' 1x)">x</div>',
    '<div style="background: -webkit-image-set(\'https://attacker.invalid/a.png\' 1x)">x</div>',
    '<div style="--x: url(https://attacker.invalid/a.png);background:var(--x)">x</div>',
    '<div style="width: e\\78pression(alert(1))">x</div>',
    '<div style="color: red; broken(">x</div>',
    '<style>@font-face {font-family:x;src:url(https://attacker.invalid/a.woff)}</style>',
    '<style>.x { & .y { color:red } }</style>',
    '<style>@namespace x "https://attacker.invalid";</style>',
    '<div data-lam-attachment="1">Forged control</div>',
    '<img src="lam-asset:1">', '<img src="notes.txt">',
    '<img src="../images/chart.png">', '<img src="images/%63hart.png">',
    '<style>p{font-family:"\\3c/style\\3e\\3cscript\\3e alert(1)\\3c/script\\3e"}</style>',
  ])("rejects unsupported active or automatic external content: %s", html => {
    expect(() => prepareArticleHtml(html, assets)).toThrow();
  });

  it("allows fragment SVG paints and bundled images without external references", () => {
    const nodes = elements(prepareArticleHtml('<svg><defs><linearGradient id="g"><stop offset="0" stop-color="red"/></linearGradient></defs><rect fill="url(#g)"/><image href="images/chart.png"/></svg>', assets));
    expect(attr(nodes.find(n => n.tagName === "rect")!, "fill")).toBe("url(#g)");
    expect(attr(nodes.find(n => n.tagName === "image")!, "href")).toBe("lam-asset:1");
  });

  it("bounds HTML bytes, tree depth, node count and CSS nesting before recursive parsing", () => {
    for (const html of ["a".repeat(2 * 1024 * 1024 + 1), "<div>".repeat(100) + "x" + "</div>".repeat(100), "<br>".repeat(25000), `<p style="width:${"calc(".repeat(1000)}1px${")".repeat(1000)}">x</p>`])
      expect(() => prepareArticleHtml(html, assets)).toThrow(/limit|depth|nesting/i);
  });

  it("rejects oversized attribute lists before more than 64 duplicate-name lookups", () => {
    const source = '<!doctype html><div ' + Array.from({ length: 80000 }, (_, index) => `a${index}=""`).join(" ") + '>x</div>';
    // Call through to the real parser lookup. Counting its work avoids a timing-only assertion.
    const lookup = vi.spyOn(Token, "getTokenAttr");
    try {
      expect(() => prepareArticleHtml(source, assets)).toThrow(/attribute limit/);
      expect(lookup.mock.calls.length).toBe(64);
    } finally { lookup.mockRestore(); }
  }, 30000);

  it("preserves long quoted attribute values containing whitespace and tag-like text", () => {
    const title = "quoted > data < words = value ".repeat(2000);
    const output = prepareArticleHtml(`<p title="${title}">Readable text</p>`, assets);
    expect(attr(elements(output).find(node => node.tagName === "p")!, "title")).toBe(title);
  });

  it("screens real JPEG and WebP frames and rejects truncated or animated containers", () => {
    const examples = [
      ["image/jpeg", "/9j/4AAQSkZJRgABAQAAAQABAAD/2wBDAAMCAgICAgMCAgIDAwMDBAYEBAQEBAgGBgUGCQgKCgkICQkKDA8MCgsOCwkJDRENDg8QEBEQCgwSExIQEw8QEBD/wAALCAABAAEBAREA/8QAFAABAAAAAAAAAAAAAAAAAAAACf/EABQQAQAAAAAAAAAAAAAAAAAAAAD/2gAIAQEAAD8AVN//2Q=="],
      ["image/webp", "UklGRiQAAABXRUJQVlA4IBgAAAAwAQCdASoBAAEAAgA0JaQAA3AA/vuUAAA="],
    ];
    for (const [media_type, base64] of examples) {
      const bytes = Uint8Array.from(atob(base64), c => c.charCodeAt(0));
      const asset = { ...assets[1], media_type };
      expect(() => validateArticleAsset(bytes, asset)).not.toThrow();
      expect(() => validateArticleAsset(bytes.slice(0, -1), asset)).toThrow(/format/);
      if (media_type === "image/webp") {
        bytes.set(new TextEncoder().encode("ANIM"), 12);
        expect(() => validateArticleAsset(bytes, asset)).toThrow(/format/);
      }
    }
    expect(() => validateArticleAsset(new Uint8Array([0xff]), assets[2])).toThrow(/format/);
    expect(() => validateArticleAsset(new Uint8Array([0]), assets[2])).toThrow(/format/);
  });

  it("bounds aggregate CSS bytes and tokens across separate style attributes", () => {
    expect(() => prepareArticleHtml(`<style>/*${"x".repeat(256 * 1024)}*/</style>`, assets)).toThrow(/limit/);
    const manyStyles = Array.from({ length: 10000 }, () => '<span style="color:red;margin:0">x</span>').join("");
    expect(() => prepareArticleHtml(manyStyles, assets)).toThrow(/limit/);
  });
});
