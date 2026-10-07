---
name: gpui-visual-check
description: Use when iterating on a GPUI visual feature (dialog layout, sticker chrome, button sizing, file-sticker HTML preview, blank-region regressions) and a check against the rendered output is needed. Captures an OS-level PNG of a running window so the agent (or a developer) can confirm visually that the layout actually looked right at runtime. The recipe is opinionated and opt-in; it does not run as part of `cargo test`, CI, or any gated path. Treat it as the dev-loop equivalent of a "look at the screen" loop. Apply when the user explicitly asks for a visual / screenshot / picture check during iteration, **or** when the change is one a known visual bug was hiding (e.g. embedded `wry::WebView` showing blank after a fix).
---

# Visual check for GPUI windows

A small workflow + a tiny capture helper for cases where the code reads right but the rendered window might not look right. Optional. Never runs under `cargo test` or CI.

## When to use

- A UI layout change has landed and the diff doesn't tell you whether the
  runtime frame will look right.
- A "should be invisible" element is suspected of leaking. The work that
  spawned this skill was a `wry::WebView` rendering blank inside the
  file-sticker — code was correct, the OS-level paint was the only way
  to verify visually.
- You want to compare a pre-fix frame against a post-fix candidate.

## When NOT to use

- A pure data/control-flow change. `cargo test --features cli` is faster
  and checks the same thing semantically.
- Refactors where the visible output cannot differ. Reasoning about the
  diff is enough.
- macOS / Linux CI automation. The host for this skill is Windows.
  Windows-only section at the bottom summarises why.

## Recipe (Windows)

```powershell
$env:GPUI_DISABLE_DIRECT_COMPOSITION = "true"
.\target\debug\<binary>.exe            # in one shell, leave it running
# in another shell:

powershell -ExecutionPolicy Bypass -File scripts\screenshot.ps1 `
            -ProcessName rustickers `
            -OutPath path\to\iter.png `
            -WaitMs 1500
# then open the PNG (the `read` tool exposes files inside the
# conversation, so the agent can see the frame too).
# and kill the binary:
taskkill /IM rustickers.exe /F
```

Three things to know before invoking:

1. **`GPUI_DISABLE_DIRECT_COMPOSITION=true` must be set in the launching
   shell.** Without it, any GPUI window containing an embedded
   `wry::WebView` (file sticker HTML / SVG / PDF, command-result web
   view) paints blank. The same guard lives in
   `src/native/mod.rs::run_native` for the production app. Setting it
   for the smoke too means the captured PNG actually contains the
   WebView surface, so the visual diff is meaningful.

2. **First-paint latency is roughly 800–1200 ms on Windows.** Wait by
   1500 ms to be safe; bump to 2500 if the binary has heavy startup
   (e.g. opening the SQLite store on a cold spindle). The default
   `-WaitMs 1500` is enough for the chooser and a sticker re-render but
   not for a cold `tokio::spawn` chain.

3. **Capture the largest visible top-level window owned by the process.**
   Title-matching is brittle because every rust sticker uses
   `WindowBackgroundAppearance::Transparent` and an unset title (no
   `GetWindowText` filter possible). The script picks by area:
   `Get-Process $name | sort area desc | first -0`. If multiple windows
   are visible (chooser + sticker + file preview + settings), the
   largest one is usually the right one, but for an absolute match pass
   the right one manually with `-ProcessId <pid>`.

## The script

`scripts/screenshot.ps1` is a 50-line P/Invoke wrapper around
`PrintWindow` with `PW_RENDERFULLCONTENT` (= 2). That flag is the bit
that convinces WebView2 / Edge-composited layers to paint into our
bitmap — without it, the captured frame is missing exactly the kind
of compositor surface the WebView bug hides in. PNG output goes via
`System.Drawing.Common`'s `Image::FromHbitmap`.

Don't add new features to it. If a new OS needs the same workflow,
shells out to a platform-native tool (macOS: `screencapture -x`,
Linux: `grim`/`maim`/`gnome-screenshot`). The script deliberately
does NOT attempt a polyglot abstraction.

## Known not-yet-possible

These would expand the skill but each is blocked by an external gap:

- **Headless screenshot** (`HeadlessAppContext::capture_screenshot`).
  Pinning `gpui-kit` 0.7.1 means cross-platform headless can't run
  on Windows because the Windows platform crate does not provide a
  `PlatformHeadlessRenderer`. macOS only. The kit's own
  `crates/kit/tests/rendering.rs` is `#[cfg(target_os = "macos")]`
  for the same reason; see
  `https://github.com/longbridge/gpui-kit/.../website/docs/test.md`
  for the canonical writeup. Until we move to a kit release whose
  Windows platform adds a headless renderer, this skill is a
  desktop-presence-required loop, not a CI step.

- **UI-integration tests** via `#[gpui_kit::test]`. The kit's
  `test-support` feature pulls `gpui_web` (wasm), which conflicts with
  the lockfile's `unicode-properties = 0.1.4` (the wasm crate asks
  for `0.1.3`). Lockfile has been stable around that conflict since
  the migration; resolving it is upstream.

## Failure modes the agent should anticipate

- **The PNG shows transparency where content should be.** WebView or
  D3D11 surface didn't paint. Check the
  `GPUI_DISABLE_DIRECT_COMPOSITION` env var. The fix is identical to
  `04c5636` and its docstring in `src/native/mod.rs`.

- **The PNG is all black / all white.** The window is genuinely
  empty, or `PrintWindow` ran before the first paint completed. Bump
  `-WaitMs`. If that doesn't help, the GPUI window is failing to
  render at all — `cargo run` with `RUST_LOG=gpui=debug` to see the
  paint loop.

- **The PNG is the wrong window.** Multi-window scenarios. Use
  `-WindowTitle <partial>` or `-ProcessId <pid>` instead of
  `-ProcessName`. Or close the noisy windows first.

- **WebView2 reports "could not initialise".** That's an upstream
  wry/build issue, not a visual-check issue; ignore — the PNG will
  be missing only that one surface and you can still diff the rest.
