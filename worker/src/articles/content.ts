import { defaultTreeAdapter, html as namespaces, parse, serialize, type DefaultTreeAdapterTypes as Html, type TreeAdapter } from "parse5";
import * as css from "css-tree";
import type { Asset } from "../domain/Article";
import { BadRequest } from "../domain/Item";
import { HTML_BYTES, normalizeAssetPath } from "./manifest";

const unsupported = (feature: string): never => { throw new BadRequest({ message: `unsupported article content: ${feature}` }); };
const words = (value: string) => new Set(value.split(/\s+/));
const htmlElements = words("html head body title meta style main article section header footer nav aside div span p h1 h2 h3 h4 h5 h6 br hr strong em b i u s small sub sup mark blockquote q cite abbr time address pre code kbd samp var ul ol li dl dt dd table caption colgroup col thead tbody tfoot tr th td figure figcaption img a details summary");
const svgElements = words("svg g defs title desc path rect circle ellipse line polyline polygon text tspan linearGradient radialGradient stop clipPath marker image");
const commonAttributes = words("id class title lang dir role aria-label aria-labelledby aria-describedby aria-hidden");
const tagAttributes: Record<string, Set<string>> = {
  img: words("src alt width height loading decoding"), a: words("href download"), details: words("open"),
  ol: words("start reversed type"), li: words("value"), th: words("colspan rowspan scope headers"), td: words("colspan rowspan headers"),
  col: words("span width"), colgroup: words("span width"), time: words("datetime"),
};
const svgAttributes = words("viewBox preserveAspectRatio x y x1 y1 x2 y2 cx cy r rx ry width height d points transform dx dy textLength lengthAdjust text-anchor dominant-baseline gradientUnits gradientTransform spreadMethod offset fx fy fr clipPathUnits refX refY markerWidth markerHeight markerUnits orient");
const paintProperties = words("fill stroke clip-path marker-start marker-mid marker-end");
const svgStyleProperties = words("fill fill-opacity fill-rule stroke stroke-width stroke-opacity stroke-linecap stroke-linejoin stroke-miterlimit stroke-dasharray stroke-dashoffset clip-path clip-rule marker-start marker-mid marker-end opacity stop-color stop-opacity font-family font-size font-weight font-style text-anchor dominant-baseline");
const properties = words(`color background background-color background-image background-size background-position background-repeat background-origin background-clip
  display box-sizing width height min-width max-width min-height max-height aspect-ratio
  margin margin-top margin-right margin-bottom margin-left margin-inline margin-block margin-inline-start margin-inline-end margin-block-start margin-block-end
  padding padding-top padding-right padding-bottom padding-left padding-inline padding-block padding-inline-start padding-inline-end padding-block-start padding-block-end
  border border-width border-style border-color border-top border-right border-bottom border-left border-radius border-collapse border-spacing box-shadow
  font font-family font-size font-weight font-style font-variant line-height letter-spacing word-spacing text-align text-decoration text-transform text-indent text-overflow text-shadow
  white-space overflow-wrap word-break tab-size vertical-align list-style list-style-type list-style-position
  overflow overflow-x overflow-y opacity object-fit object-position
  position top right bottom left z-index float clear
  flex flex-direction flex-wrap flex-grow flex-shrink flex-basis align-items align-content align-self justify-content justify-items justify-self order gap row-gap column-gap
  grid grid-template-columns grid-template-rows grid-column grid-row grid-auto-flow grid-auto-columns grid-auto-rows
  transform transform-origin table-layout caption-side empty-cells`);
for (const property of svgStyleProperties) properties.add(property);
const functions = words("rgb rgba hsl hsla hwb lab lch oklab oklch calc min max clamp repeat minmax fit-content linear-gradient radial-gradient conic-gradient repeating-linear-gradient repeating-radial-gradient translate translateX translateY scale scaleX scaleY rotate skew skewX skewY matrix");
const cssNodes = words("StyleSheet Atrule AtrulePrelude MediaQueryList MediaQuery Condition Feature MediaFeature Ratio Block Rule SelectorList Selector TypeSelector ClassSelector IdSelector Combinator PseudoClassSelector Declaration DeclarationList Value Identifier Dimension Number Percentage Hash String Operator Function Url WhiteSpace Parentheses");
const fragment = (value: string) => /^#[A-Za-z_][A-Za-z0-9_.:-]*$/.test(value);
const raster = new Set(["image/png", "image/jpeg", "image/webp"]);

/** Format screening, not a media decoder or PDF sanitizer. Downloads stay opaque. */
export function validateArticleAsset(bytes: Uint8Array<ArrayBuffer>, asset: Asset): void {
  const fail = (): never => unsupported(`asset ${asset.path} bytes do not match the declared ${asset.media_type} format`);
  const text = (start: number, end: number) => new TextDecoder().decode(bytes.subarray(start, end));
  const view = new DataView(bytes.buffer, bytes.byteOffset, bytes.byteLength);
  if (asset.media_type === "text/plain") {
    try {
      const value = new TextDecoder("utf-8", { fatal: true }).decode(bytes);
      if (/[\u0000-\u0008\u000b\u000c\u000e-\u001f\u007f]/u.test(value)) fail();
    } catch { fail(); }
  } else if (asset.media_type === "application/pdf") {
    if (!/^%PDF-(1\.[0-7]|2\.0)[\r\n]/.test(text(0, 10)) || !/%%EOF\s*$/.test(text(Math.max(0, bytes.length - 1024), bytes.length))) fail();
  } else if (asset.media_type === "image/png") {
    if (bytes.length < 45 || ![137, 80, 78, 71, 13, 10, 26, 10].every((value, index) => bytes[index] === value)) fail();
    let offset = 8;
    let data = false;
    let chunks = 0;
    while (offset + 12 <= bytes.length) {
      if (++chunks > 4096) fail();
      const length = view.getUint32(offset);
      const type = text(offset + 4, offset + 8);
      if (offset + length + 12 > bytes.length || type === "acTL" || type === "fcTL" || type === "fdAT") fail();
      if (offset === 8 && (type !== "IHDR" || length !== 13 || view.getUint32(16) === 0 || view.getUint32(20) === 0)) fail();
      if (type === "IDAT" && length > 0) data = true;
      offset += length + 12;
      if (type === "IEND") { if (!data || length !== 0 || offset !== bytes.length) fail(); return; }
    }
    fail();
  } else if (asset.media_type === "image/jpeg") {
    if (bytes.length < 4 || bytes[0] !== 0xff || bytes[1] !== 0xd8 || bytes.at(-2) !== 0xff || bytes.at(-1) !== 0xd9) fail();
    let offset = 2;
    let frame = false;
    while (offset + 4 < bytes.length) {
      if (bytes[offset++] !== 0xff) fail();
      while (bytes[offset] === 0xff) offset++;
      const marker = bytes[offset++];
      if (offset + 2 > bytes.length) fail();
      const length = view.getUint16(offset);
      if (length < 2 || offset + length > bytes.length) fail();
      if (marker >= 0xc0 && marker <= 0xcf && ![0xc4, 0xc8, 0xcc].includes(marker)) {
        if (length < 8 || view.getUint16(offset + 3) === 0 || view.getUint16(offset + 5) === 0) fail();
        frame = true;
      }
      if (marker === 0xda) { if (!frame || length < 6 || offset + length >= bytes.length - 2) fail(); return; }
      offset += length;
    }
    fail();
  } else if (asset.media_type === "image/webp") {
    if (bytes.length < 26 || text(0, 4) !== "RIFF" || text(8, 12) !== "WEBP" || view.getUint32(4, true) !== bytes.length - 8) fail();
    let offset = 12;
    let data = false;
    while (offset + 8 <= bytes.length) {
      const type = text(offset, offset + 4);
      const length = view.getUint32(offset + 4, true);
      if (offset + 8 + length > bytes.length || type === "ANIM" || type === "ANMF") fail();
      if (type === "VP8X" && (length !== 10 || (bytes[offset + 8] & 2) !== 0)) fail();
      if ((type === "VP8 " && length >= 10) || (type === "VP8L" && length >= 5)) data = true;
      offset += 8 + length + length % 2;
    }
    if (!data || offset !== bytes.length) fail();
  } else fail();
}

/** Stable manifest indexes, never credentials. Viewers replace these at delivery. */
export function prepareArticleHtml(source: string, assets: readonly Asset[]): string {
  if (source.length > HTML_BYTES || new TextEncoder().encode(source).byteLength > HTML_BYTES) unsupported("HTML byte limit exceeded");
  const byPath = new Map(assets.map((asset, index) => [asset.path, { asset, index }]));
  const bundled = (value: string) => {
    let path: string;
    try { path = normalizeAssetPath(value.startsWith("./") ? value.slice(2) : value); }
    catch { return unsupported("resource URLs must be supplied relative asset paths"); }
    const match = byPath.get(path);
    if (!match || path === "index.html") return unsupported(`resource ${path} must be supplied in the manifest`);
    return match;
  };
  const media = (value: string) => {
    const { asset, index } = bundled(value);
    if (asset.disposition !== "inline" || !raster.has(asset.media_type)) unsupported("media requires an inline PNG, JPEG or WebP asset; use viewer controls for downloads");
    return `lam-asset:${index}`;
  };

  let cssBytes = 0;
  let cssTokens = 0;
  const prepareCss = (source: string, context: "stylesheet" | "declarationList" | "value", property?: string): string => {
    cssBytes += new TextEncoder().encode(source).byteLength;
    if (cssBytes > 256 * 1024) unsupported("CSS byte limit exceeded");
    // Tokenization is iterative. Bound recursive parser work before constructing an AST.
    let depth = 0;
    css.tokenize(source, type => {
      if (++cssTokens > 40000) unsupported("CSS token limit exceeded");
      if ([css.tokenTypes.Function, css.tokenTypes.LeftParenthesis, css.tokenTypes.LeftSquareBracket, css.tokenTypes.LeftCurlyBracket].includes(type)) {
        if (++depth > 32) unsupported("CSS nesting limit exceeded");
      } else if ([css.tokenTypes.RightParenthesis, css.tokenTypes.RightSquareBracket, css.tokenTypes.RightCurlyBracket].includes(type)) {
        if (--depth < 0) unsupported("malformed CSS delimiters");
      }
    });
    if (depth !== 0) unsupported("malformed CSS delimiters");
    let ast: css.CssNode;
    try { ast = css.parse(source, { context, onParseError: () => unsupported("malformed CSS; remove unsupported syntax") }); }
    catch (error) { if (error instanceof BadRequest) throw error; return unsupported("malformed CSS"); }
    css.walk(ast, function(node) {
      if (!cssNodes.has(node.type)) unsupported(`CSS ${node.type}; use static layout declarations`);
      if (node.type === "Atrule") {
        if (node.name !== "media" || !node.prelude || !node.block || node.block.children.some(child => child.type !== "Rule")) unsupported("only @media with flat style rules is allowed");
      }
      if (node.type === "Rule" && node.block.children.some(child => child.type !== "Declaration")) unsupported("CSS nesting; use flat style rules");
      if (node.type === "Declaration" && !properties.has(node.property)) unsupported(`CSS property ${node.property}; custom properties and dynamic styles are not supported`);
      if (node.type === "Function" && !functions.has(node.name)) unsupported(`CSS function ${node.name}`);
      if (node.type === "PseudoClassSelector" && (node.children !== null || !words("root first-child last-child only-child empty hover focus focus-visible").has(node.name))) unsupported("CSS pseudo selector");
      if (node.type === "TypeSelector" && node.name.includes("|")) unsupported("CSS namespaces");
      if (node.type === "Url") {
        const targetProperty = this.declaration?.property ?? property;
        if (targetProperty && paintProperties.has(targetProperty)) {
          if (!fragment(node.value)) unsupported("SVG references must be same-article fragments");
        } else {
          if (targetProperty !== "background" && targetProperty !== "background-image") unsupported("CSS URLs only allowed for bundled backgrounds or SVG fragments");
          node.value = media(node.value);
        }
      }
    });
    const result = css.generate(ast);
    // CSS string decoding can introduce an HTML raw-text closing delimiter on serialization.
    if (result.includes("<")) unsupported("CSS containing HTML delimiters");
    return result;
  };

  let nodes = 0;
  const count = () => { if (++nodes > 20000) unsupported("HTML node limit exceeded"); };
  const checkDepth = (parent: Html.ParentNode) => {
    let node: Html.ParentNode | null = parent;
    let depth = 0;
    while (node) {
      if (++depth > 64) unsupported("HTML depth limit exceeded");
      node = "parentNode" in node ? node.parentNode : null;
    }
  };
  const adapter: TreeAdapter<Html.DefaultTreeAdapterMap> = {
    ...defaultTreeAdapter,
    createElement(tag, namespace, attrs) {
      count();
      const allowed = namespace === namespaces.NS.HTML ? htmlElements : namespace === namespaces.NS.SVG ? svgElements : null;
      if (!allowed?.has(tag)) unsupported(`element <${tag}>; only static HTML and SVG are supported`);
      return defaultTreeAdapter.createElement(tag, namespace, attrs);
    },
    createCommentNode(data) { count(); return defaultTreeAdapter.createCommentNode(data); },
    appendChild(parent, child) { checkDepth(parent); defaultTreeAdapter.appendChild(parent, child); },
    insertBefore(parent, child, reference) { checkDepth(parent); defaultTreeAdapter.insertBefore(parent, child, reference); },
  };
  const document = parse(source, { treeAdapter: adapter, onParseError(error) {
    if (error.code !== "missing-doctype") unsupported(`malformed HTML (${error.code})`);
  } });
  const visit = (parent: Html.ParentNode) => {
    parent.childNodes = parent.childNodes.filter(node => node.nodeName !== "#comment" && node.nodeName !== "#documentType");
    for (const node of parent.childNodes) {
      if (!("tagName" in node)) continue;
      const svg = node.namespaceURI === namespaces.NS.SVG;
      if (node.tagName === "meta") {
        const values = new Map(node.attrs.map(attr => [attr.name, attr.value]));
        if (node.attrs.some(attr => attr.namespace)) unsupported("meta namespace");
        if (node.attrs.length === 1 && values.get("charset")?.toLowerCase() === "utf-8") node.attrs = [{ name: "charset", value: "utf-8" }];
        else if (node.attrs.length === 2 && values.get("name") === "viewport" && values.has("content")) node.attrs = [{ name: "name", value: "viewport" }, { name: "content", value: "width=device-width, initial-scale=1" }];
        else unsupported("meta directives; only UTF-8 charset and viewport are supported");
        continue;
      }
      let attachment: number | undefined;
      let outbound = false;
      const hasDownload = node.attrs.some(attr => attr.name === "download");
      node.attrs = node.attrs.filter(attr => {
        if (svg && attr.name === "xmlns" && attr.value === namespaces.NS.SVG) return false;
        if (svg && node.tagName === "image" && attr.name === "href" && (!attr.namespace || attr.namespace === namespaces.NS.XLINK)) {
          attr.value = media(attr.value); delete attr.namespace; delete attr.prefix; return true;
        }
        if (attr.namespace || attr.prefix) unsupported(`namespaced attribute ${attr.name}`);
        if (attr.name === "style") { attr.value = prepareCss(attr.value, "declarationList"); return true; }
        if (commonAttributes.has(attr.name)) return true;
        if (svg && svgStyleProperties.has(attr.name)) { attr.value = prepareCss(attr.value, "value", attr.name); return true; }
        if (svg && svgAttributes.has(attr.name)) return true;
        if (!svg && tagAttributes[node.tagName]?.has(attr.name)) {
          if (node.tagName === "img" && attr.name === "src") attr.value = media(attr.value);
          if (node.tagName === "a") {
            if (attr.name === "download") return false;
            if (fragment(attr.value)) { if (hasDownload) unsupported("fragment downloads; use bundled attachments"); }
            else if (/^https?:\/\//i.test(attr.value)) {
              let url: URL;
              try { url = new URL(attr.value); } catch { return unsupported("invalid outbound link"); }
              if (hasDownload || url.username || url.password || /[\u0000-\u0020\u007f\\]/u.test(attr.value)) unsupported("invalid outbound link; use explicit HTTP(S) navigation");
              attr.value = url.href; outbound = true;
            } else {
              const match = bundled(attr.value);
              if (match.asset.disposition !== "attachment") unsupported("download links require an attachment disposition");
              attachment = match.index;
              return false;
            }
          }
          return true;
        }
        return unsupported(`attribute ${attr.name} on <${node.tagName}>`);
      });
      if (attachment !== undefined) {
        node.tagName = node.nodeName = "span";
        node.attrs.push({ name: "data-lam-attachment", value: String(attachment) });
      } else if (outbound) node.attrs.push({ name: "rel", value: "noopener noreferrer" }, { name: "target", value: "_blank" });
      if (node.tagName === "style") {
        const source = node.childNodes.map(child => "value" in child ? child.value : "").join("");
        node.childNodes = [{ nodeName: "#text", value: prepareCss(source, "stylesheet"), parentNode: node }];
      } else visit(node);
    }
  };
  visit(document);
  const result = "<!DOCTYPE html>" + serialize(document);
  if (new TextEncoder().encode(result).byteLength > HTML_BYTES) unsupported("canonical HTML byte limit exceeded");
  return result;
}
