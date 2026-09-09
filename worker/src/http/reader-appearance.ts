/** Serialized into the privileged wrapper, never run inside the opaque article frame. */
export function applyReaderTheme(source: string, mode: "dark" | "light"): string {
  const doc = new DOMParser().parseFromString(source, "text/html");
  const dark = mode === "dark";
  const paper = dark ? "#191a1d" : "#f8f7f3";
  const ink = dark ? "#deddda" : "#272727";
  const canvas = document.createElement("canvas");
  canvas.width = canvas.height = 1;
  const context = canvas.getContext("2d", { willReadFrequently: true });
  if (!context) throw new Error("Color conversion unavailable");
  const colors = new Map<string, string>();

  function color(value: string, role: "text" | "background" | "border"): string {
    if (!value || /^(inherit|initial|unset|revert|currentcolor|transparent)$/i.test(value)) return value;
    const key = role + ":" + value;
    const cached = colors.get(key);
    if (cached) return cached;
    // Browser color parsing handles named, hex, rgb and hsl colors without mounting author DOM.
    if (!CSS.supports("color", value)) return value;
    context!.clearRect(0, 0, 1, 1);
    context!.fillStyle = value;
    context!.fillRect(0, 0, 1, 1);
    const [red, green, blue, alpha] = context!.getImageData(0, 0, 1, 1).data;
    if (!alpha) return "transparent";
    const [r, g, b] = [red / 255, green / 255, blue / 255];
    const max = Math.max(r, g, b), min = Math.min(r, g, b), delta = max - min;
    const lightness = (max + min) / 2;
    const chroma = delta === 0 ? 0 : delta / (1 - Math.abs(2 * lightness - 1));
    let result: string;
    if (delta < .01 || chroma < .12) {
      result = role === "text" ? ink : role === "border" ? (dark ? "#414247" : "#d9d8d2") : (dark ? "#202125" : "#f0efe9");
      if (role === "background" && alpha < 255) result += alpha.toString(16).padStart(2, "0");
    } else {
      const hue = ((max === r ? (g - b) / delta : max === g ? (b - r) / delta + 2 : (r - g) / delta + 4) * 60 + 360) % 360;
      const saturation = Math.min(.38, Math.max(.20, chroma));
      const light = role === "text" ? (dark ? 72 : 32) : role === "border" ? (dark ? 30 : 78) : (dark ? 16 : 94);
      const sat = role === "text" ? saturation * 100 : 22;
      result = `hsl(${hue.toFixed(1)} ${sat.toFixed(1)}% ${light}% / ${role === "background" ? alpha / 255 : 1})`;
    }
    colors.set(key, result);
    return result;
  }

  function declarations(style: CSSStyleDeclaration): void {
    for (const [property, role] of [
      ["color", "text"], ["text-decoration-color", "text"], ["background-color", "background"],
      ["border-top-color", "border"], ["border-right-color", "border"],
      ["border-bottom-color", "border"], ["border-left-color", "border"],
    ] as const) {
      const value = style.getPropertyValue(property);
      if (value) style.setProperty(property, color(value, role), style.getPropertyPriority(property));
    }
    if (style.boxShadow) style.boxShadow = "none";
  }

  function rules(list: CSSRuleList): void {
    for (const rule of Array.from(list)) {
      if (rule instanceof CSSStyleRule) {
        // viewerParts separates selector branches, including mixed SVG/HTML rules.
        // Static SVG paint is preserved. Its internal styles are excluded below too.
        if (!/(^|[\s,>+~])(?:svg|image)(?=[\s.#:[>+~,]|$)/i.test(rule.selectorText)) declarations(rule.style);
      } else if ("cssRules" in rule) rules((rule as CSSGroupingRule).cssRules);
    }
  }
  for (const style of Array.from(doc.querySelectorAll("style"))) {
    if (style.closest("svg")) continue;
    const sheet = new CSSStyleSheet();
    // The sheet is never adopted: @import cannot fetch or execute in the parent document.
    sheet.replaceSync(style.textContent ?? "");
    rules(sheet.cssRules);
    style.textContent = Array.from(sheet.cssRules, rule => rule.cssText).join("\n");
  }
  for (const element of Array.from(doc.querySelectorAll<HTMLElement>("[style]"))) {
    if (element.namespaceURI === "http://www.w3.org/1999/xhtml" && !element.closest("svg,picture,img")) declarations(element.style);
  }
  for (const element of [doc.documentElement, doc.body]) {
    element.style.setProperty("background-color", paper, "important");
    element.style.setProperty("color", ink, "important");
    element.style.setProperty("color-scheme", mode, "important");
    element.style.setProperty("--lam-viewer-gradient", "none", "important");
  }
  const fallback = doc.createElement("style");
  fallback.textContent = `svg,img,picture{--lam-viewer-gradient:initial!important}@layer lamReaderDefaults{a{color:${dark ? "#a4b9d5" : "#49678f"}}svg{background-color:white;color:#202020}}`;
  doc.head.appendChild(fallback);
  return "<!doctype html>" + doc.documentElement.outerHTML;
}
