const svg = (paths: string) => `<svg viewBox="0 0 24 24" aria-hidden="true">${paths}</svg>`;

export const themeIcons = {
  system: svg('<rect x="3" y="4" width="18" height="13" rx="2"/><path d="M8 21h8m-4-4v4"/>'),
  dark: svg('<path d="M20.5 13.2A8.6 8.6 0 0 1 10.8 3.5a8.6 8.6 0 1 0 9.7 9.7Z"/>'),
  light: svg('<circle cx="12" cy="12" r="4"/><path d="M12 2v2m0 16v2M2 12h2m16 0h2M5 5l1.5 1.5m11 11L19 19M5 19l1.5-1.5m11-11L19 5"/>'),
  original: svg('<path d="M4 6h16M9 6v14m6-14v14M6 20h12"/>'),
};

export const readerStyle = `
*{box-sizing:border-box}body{--paper:#f8f7f3;--ink:#272727;--muted:#686863;--line:#deddd6;--hover:#eeede8;margin:0;font:15px system-ui;background:var(--paper);color:var(--ink)}
html[data-theme=dark]{color-scheme:dark}html[data-theme=dark] body{--paper:#191a1d;--ink:#deddda;--muted:#a2a29d;--line:#343539;--hover:#25262a}
header{height:60px;padding:10px 20px;display:flex;align-items:center;justify-content:space-between;position:relative;z-index:2}#brand{color:var(--muted);font-size:12px;letter-spacing:.12em}#controls-dismiss{position:fixed;inset:0;z-index:1}
button{font:inherit;cursor:pointer;color:var(--muted);background:transparent;border:0;border-radius:6px;min-height:40px;padding:8px}button:hover,button[aria-expanded=true]{color:var(--ink);background:var(--hover)}button:focus-visible{outline:2px solid #a98c60;outline-offset:2px}button:disabled{opacity:.4;cursor:default}
svg{width:19px;height:19px;fill:none;stroke:currentColor;stroke-width:1.5;stroke-linecap:round;stroke-linejoin:round;vertical-align:middle}.icon{width:40px;padding:10px}
#controls{position:absolute;right:20px;top:56px;z-index:2;width:260px;padding:10px;border:1px solid var(--line);border-radius:10px;background:var(--paper);box-shadow:0 10px 28px #00000018}#tools{display:flex;align-items:center;justify-content:space-between}#zoom-level{min-width:38px;text-align:center;font-size:11px;color:var(--muted)}.divider{height:22px;width:1px;background:var(--line);margin:0 6px}
#secondary{border-top:1px solid var(--line);margin-top:8px;padding-top:8px}.secondary{display:flex;align-items:center;gap:10px;text-align:left;width:100%;font-size:12px}.secondary svg{width:16px;height:16px}#attachments{padding:4px 0}#attachments button{display:block;width:100%;text-align:left;font-size:12px;overflow-wrap:anywhere}
#status{margin:0 20px;color:var(--muted);white-space:pre-wrap;font-size:13px}#status:empty{display:none}#status[data-quiet=true]{position:absolute;width:1px;height:1px;overflow:hidden;clip-path:inset(50%)}#retry{margin:8px 20px}iframe{display:block;width:100%;height:calc(100dvh - 60px);border:0;background:var(--paper)}[hidden]{display:none!important}
@media(max-width:420px){header{padding:10px 12px}#controls{right:12px;max-width:calc(100vw - 24px)}}`;

export const readerControls = `<header><span id="brand">LAM</span><span id="title" hidden></span>
<button id="controls-toggle" class="icon" type="button" aria-label="Reading controls" title="Reading controls" aria-controls="controls" aria-expanded="false">${svg('<path d="M4 7h9m4 0h3M4 17h3m4 0h9"/><circle cx="15" cy="7" r="2"/><circle cx="9" cy="17" r="2"/>')}</button>
<div id="controls" hidden aria-label="Reading controls"><div id="tools"><button id="theme" class="icon" type="button"></button><span class="divider"></span><button id="smaller" class="icon" type="button" aria-label="Zoom out" title="Zoom out">${svg('<path d="M5 12h14"/>')}</button><output id="zoom-level" aria-live="polite">100%</output><button id="larger" class="icon" type="button" aria-label="Zoom in" title="Zoom in">${svg('<path d="M5 12h14M12 5v14"/>')}</button></div>
<div id="secondary"><button id="original" class="secondary" type="button">${themeIcons.original}Original styling</button><button id="attachments-toggle" class="secondary" type="button" aria-controls="attachments" aria-expanded="false" hidden>${svg('<path d="M14 3H5v18h14V8Zm0 0v5h5M8 12h8m-8 4h6"/>')}<span id="attachments-label">Attachments</span></button><div id="attachments" hidden></div></div></div></header><div id="controls-dismiss" hidden aria-hidden="true"></div><p id="status" role="status">Opening article…</p><button id="retry" type="button" hidden>Retry</button>`;
