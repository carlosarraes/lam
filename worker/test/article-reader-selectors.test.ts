import * as css from "css-tree";
import { describe, expect, it } from "vitest";
import { viewerParts } from "../src/http/article-viewer";

describe("reader selector preparation", () => {
  it("separates mixed SVG and HTML selectors without moving declarations or sharing resource slots", () => {
    const parts = viewerParts(`<style>.addition{color:blue}@media screen{svg,.addition,.escaped\\,name{color:red!important;background-image:url(lam-asset:0)}}.addition{color:green}</style>`, [
      { path: "chart.png", media_type: "image/png", size: 1, sha256: "0".repeat(64), disposition: "inline" },
    ]);
    expect(parts.filter(part => typeof part !== "string")).toEqual([{ asset: 0 }, { asset: 0 }]);
    let resource = 0;
    const html = parts.map(part => typeof part === "string" ? part : `https://example.com/asset-${resource++}.png`).join("");
    const source = html.match(/<style>([\s\S]*?)<\/style>/)![1];
    const selectors: string[] = [];
    const mediaRules: string[] = [];
    css.walk(css.parse(source), function(node) {
      if (node.type === "Rule") {
        selectors.push(css.generate(node.prelude!));
        if (this.atrule?.name === "media") mediaRules.push(css.generate(node));
      }
    });
    expect(selectors).toEqual([".addition", "svg", ".addition,.escaped\\,name", ".addition"]);
    expect(mediaRules).toEqual([
      "svg{color:red!important;background-image:url(https://example.com/asset-0.png)}",
      ".addition,.escaped\\,name{color:red!important;background-image:url(https://example.com/asset-1.png)}",
    ]);
  });

  it("bounds dense selector lists to two declaration blocks and leaves unmixed groups alone", () => {
    const selectors = Array.from({ length: 100 }, (_, i) => `.item${i}`).join(",");
    const declarations = Array.from({ length: 100 }, () => "color:red").join(";");
    for (const group of [selectors, `svg,${selectors}`, "svg,image"]) {
      const source = `${group}{${declarations}}`;
      const html = viewerParts(`<style>${source}</style>`, []).join("");
      const prepared = html.match(/<style>([\s\S]*?)<\/style>/)![1];
      let blocks = 0;
      css.walk(css.parse(prepared), node => { if (node.type === "Rule") blocks++; });
      expect(blocks).toBe(group.startsWith("svg,.") ? 2 : 1);
      expect(prepared.length).toBeLessThanOrEqual(source.length * 2);
    }
  });

  it("leaves native selector grouping intact", () => {
    const html = viewerParts("<style>svg,.addition{color:red}</style>", [], "native").join("");
    expect(html).toContain("<style>svg,.addition{color:red}</style>");
  });
});
