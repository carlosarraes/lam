# HTML Articles

Articles keep reports in a reading library separate from requests and FYIs. Publication never waits for a decision. An Article remains available after reading, and Carlos can mark it unread again.

## Prepare and publish

Use `lam article publish --file PATH --title TITLE --summary TEXT`, adding one `--asset RELATIVE_PATH` per file and `--silent` when requested. The entry is normalized to `index.html`; asset paths resolve from its directory. Supply a canonical Unix directory path without symlink components. This also applies to macOS system aliases such as `/tmp`; use the actual directory returned by `pwd -P`.

```sh
lam article publish --file /canonical/report/index.html --title "Report" --summary "Findings and next steps" --asset images/chart.png --asset notes.txt --silent
lam article list --read all --query "Report"
lam article open ARTICLE_ID
```

Only the entry and listed assets upload. Unlisted files stay local. Missing references fail Worker publication, even if the referenced file exists beside the entry. The CLI never discovers resources, recursively uploads directories, or downloads remote URLs. Limits are 2 MiB for HTML, 20 MiB per asset, 50 MiB combined, and fifty additional assets. PNG, JPEG and WebP are inline; PDF and UTF-8 text are download-only. Format screening is not full image decoding or PDF sanitization. Viewers also bound image dimensions and decode images before rendering them.

The server validates and canonicalizes static HTML. Its stored original entry digest differs from the canonical document digest. Resources become `lam-asset:<manifest index>` internally; attachment links become inert markers with native save controls. Author ordinary relative paths, leaving these internal markers to the server.

## Static show-me preparation

When LAM delivery is requested, make a publication copy of the local show-me artifact. Keep a readable narrow layout, selectable text, a title and a brief summary. Use HTML headings, paragraphs, lists, tables, details/summary and inline SVG. CSS supports flat rules, literal colors/sizes, common layout declarations, and flat `@media` rules. Use system fonts and explicitly bundled raster files. Scripts, CDN runtimes, remote media/fonts, custom properties, `var()`, animation and unsupported SVG are rejected.

Render Mermaid locally to SVG before embedding it. Request text labels with `htmlLabels: false`, replace any remaining `foreignObject` labels with SVG text, and resolve renderer styles to literal supported attributes. Remove renderer-only CSS and attributes after preserving their visible effect. SVG paths, groups, rectangles, text/tspan and fragment marker/paint references are supported. Treat the result as a candidate until the actual Worker accepts it and both readers display it correctly. A renderer's generic SVG export is not a compatibility guarantee.

The fixture under `worker/test/fixtures/articles/` contains a responsive report, rendered Mermaid diagram, a PNG and text attachment. Its integration command is `cd worker && node test/article-viewer.browser.mjs`. This runs a candidate CLI against isolated Worker/D1/R2 storage and Chromium. It uses local synthetic credentials and canaries. The accompanying Mermaid source and preparation script record the renderer conversion used for this fixture.

Ordinary show-me requests still produce local output and open it locally. LAM delivery is conditional on the user's request.

## Read state and recovery

`article publish` prints an ID only on confirmed success. After staging, failures identify the draft and exit nonzero. Interrupted operations retry within the same attempt with the same identity and bytes. Each new CLI invocation uses a fresh identity, so a whole-command rerun can create a duplicate. Retain an uncertain draft ID and inspect the library or authenticated server state before deciding to publish again. The current CLI has no resume-by-draft command.

Listing and preview fetching leave read state unchanged. Verified visible content triggers one versioned read attempt. Explicit `lam article read ID` and `lam article unread ID` first load the canonical version. A conflict fetches and reports the newer state without replaying the write. Leave these actions to the reader.

Desktop opening issues a short-lived Article-scoped viewer session. Its URL is a capability and should not appear in logs or messages. Reopen from LAM when it expires. Android fetches through its current native credential session and uses `lam://articles/UUID` for stable routing. That ID-only link requires a native handler; no desktop handler is installed by this feature.

## Local acceptance and rollout checkpoint

Local verification and production/device acceptance are separate. `just check` covers Worker and CLI checks; Android requires unit, lint, debug, minified-release, icon and disposable-emulator gates. Browser tests cover Chromium. Safari and Firefox require their own runs before compatibility can be claimed. Native Mac build/test evidence is staged independently without replacing installed binaries.

Foreground Android and TUI clients reconcile from `/v2/events` and fetch canonical state. Reconnect, resume and explicit refresh repair missed invalidations. Silent publication creates no notification job. Ordinary publication creates a durable quiet job; transport acknowledgement loss can produce duplicate notifications. The bounded scheduled runner exists locally, but no deployed Cron Trigger was added. Android background push remains paused with FCM unresolved; source completion is not background delivery acceptance.

Before rollout, obtain approval for the exact D1 migrations, private R2 bucket and binding, Worker deployment, scheduler configuration, tested CLI/APK installation, and proposed installed show-me guide. Back up metadata/config and retain rollback binaries/APKs. Apply additive migrations and private bindings before deploying; install CLIs atomically and Android through the approved Wi-Fi path. Verify credential metadata unchanged, debug/production icons distinct, and read-only health checks. Use one explicitly approved sample Article for phone acceptance. Rollback retains additive tables and R2 objects. Independent code/security review and a decision on paused push remain release prerequisites.
