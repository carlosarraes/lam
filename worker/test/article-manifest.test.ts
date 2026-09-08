import { describe, expect, it } from "vitest";
import { normalizeAssetPath, validateManifest } from "../src/articles/manifest";

const html = { path: "index.html", media_type: "text/html", size: 12, sha256: "a".repeat(64), disposition: "inline" as const };
const draft = () => ({ title: "Report", summary: "", name: "", source_host: "host", source_project: "lam", silent: false, assets: [html] });

describe("article manifest", () => {
  it("keeps relative asset paths and rejects ambiguous or traversing paths", () => {
    expect(normalizeAssetPath("images/chart.png")).toBe("images/chart.png");
    expect(normalizeAssetPath("images/chart 1.png")).toBe("images/chart 1.png");
    for (const path of ["../config.toml", "/index.html", "a/../index.html", "a\\b", "%2e%2e/a", "%252e%252e/a", "a//b", "./a", "https://host/a", "a?b", "a#b", "a\u0000b", "bad\ud800.png"])
      expect(() => normalizeAssetPath(path), path).toThrow();
  });

  it("requires one HTML entry and rejects duplicates after Unicode normalization", () => {
    expect(validateManifest(draft())).toEqual(draft());
    for (const assets of [[], [html, html], [{ ...html, path: "other.html" }], [html, { ...html, path: "evil.svg", media_type: "image/svg+xml" }]])
      expect(() => validateManifest({ ...draft(), assets })).toThrow();
    const image = { ...html, path: "é.png", media_type: "image/png" };
    expect(() => validateManifest({ ...draft(), assets: [html, image, { ...image, path: "e\u0301.png" }] })).toThrow();
  });

  it("enforces byte, count, hash, metadata and download policy limits", () => {
    for (const asset of [{ ...html, size: 2 * 1024 * 1024 + 1 }, { ...html, size: -1 }, { ...html, size: 1.1 }, { ...html, sha256: "A".repeat(64) }, { ...html, disposition: "attachment" }])
      expect(() => validateManifest({ ...draft(), assets: [asset] })).toThrow();
    for (const media_type of ["application/zip", "application/octet-stream", "text/html", "image/svg+xml", "text/javascript"])
      expect(() => validateManifest({ ...draft(), assets: [html, { ...html, path: "attachment.bin", media_type, disposition: "attachment" }] })).toThrow();
    expect(() => validateManifest({ ...draft(), title: "😀".repeat(201) })).toThrow();
    expect(validateManifest({ ...draft(), title: "😀".repeat(200) }).title).toHaveLength(400);
    expect(() => validateManifest({ ...draft(), summary: "😀".repeat(2001) })).toThrow();
    const attachments = Array.from({ length: 50 }, (_, i) => ({ ...html, path: `${i}.txt`, media_type: "text/plain", disposition: "attachment" as const }));
    expect(validateManifest({ ...draft(), assets: [html, ...attachments] }).assets).toHaveLength(51);
    expect(() => validateManifest({ ...draft(), assets: [html, ...attachments, { ...attachments[0], path: "extra.txt" }] })).toThrow();
    expect(() => validateManifest({ ...draft(), assets: [html, ...attachments.slice(0, 3).map(a => ({ ...a, size: 20 * 1024 * 1024 }))] })).toThrow();
    expect(() => validateManifest({ ...draft(), assets: [html, { ...attachments[0], size: 20 * 1024 * 1024 + 1 }] })).toThrow();
    expect(() => validateManifest({ ...draft(), assets: [html, { ...attachments[0], disposition: "inline" }] })).toThrow();
  });
});
