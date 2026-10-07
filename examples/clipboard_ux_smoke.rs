//! Visual smoke harness for the GPUI frontend.
//!
//! Two purposes:
//!
//! 1. **Sandbox mode** (the original): boot the chooser with deterministic
//!    fixtures so the keyboard flow can be exercised without the full app
//!    or a running clipboard watcher.
//! 2. **Visual inspect mode**: pick what UI to show, optionally attach an OS
//!    screenshot of the visible window to a PNG. Useful for verifying
//!    visual fixes on this developer's host (e.g. did the file-sticker's
//!    HTML preview actually paint after the direct-composition fix).
//!
//! ## CLI
//!
//! Pick what to render:
//!
//! ```text
//!   (no flag)                  existing chooser smoke (8 ordinary items)
//!   --configured               chooser smoke with a pre-built vault
//!   --file <PATH>              open a file sticker previewing <PATH>
//!   --chooser-with-file <PATH> chooser with one newest item whose content
//!                              is `local://localhost/<PATH>`, so pressing
//!                              Enter on that row exercises the same code
//!                              path as `rst sticker file --preview`.
//! ```
//!
//! Inspect (opt-in; default = interactive sandbox):
//!
//! ```text
//!   --screenshot <OUT.png>     dump the visible window bitmap to OUT.png
//!   --exit-after-ms <N>        quietly close the app N ms after the first
//!                              frame. Always used with --screenshot so the
//!                              smoke is non-interactive.
//! ```
//!
//! Screenshotting is genuinely opt-in. Without a flag, the smoke still
//! behaves as it did before -- interactive, window stays up until the user
//! closes it. `--screenshot` deliberately does not run on macOS/Linux yet
//! because the host this was built on is Windows; a stub prints a clear
//! error explaining why.
//!
//! ## GPUI_DISABLE_DIRECT_COMPOSITION
//!
//! Set before `application()` is called, on Windows, the same way
//! `src/native/mod.rs::run_native` does it. Required so the embedded wry
//! WebView actually paints; without it the file-sticker HTML preview
//! shows the same transparent region the blank-screen bug presented.

use gpui_kit::component::{Theme, ThemeMode};
use gpui_kit::{App, application};
use rustickers::clipboard::storage::{ClipboardHistoryStore, SqliteClipboardStore};
use rustickers::clipboard::vault_controller::VaultController;
use rustickers::native::windows::StickerWindowEvent;
use rustickers::native::{
    clipboard::CaptureFilter,
    components::Assets,
    windows::{clipboard, sticker},
};
use rustickers::storage::{ArcStickerStore, open_sqlite};
use std::sync::{Arc, mpsc};
use std::time::Duration;

const FIXTURE_ITEMS: &[&str] = &[
    "A short clipboard item",
    "https://example.test/docs/clipboard-history?view=keyboard",
    "日本語 · 中文 · emoji ✨",
    "A multiline copy\nwith a second line",
    "Another recent copy",
    "Code: let greeting = \"Hello, world!\";",
    "A useful saved command",
    "The oldest sample",
];

#[derive(Default)]
struct Args {
    configured: bool,
    file: Option<String>,
    chooser_with_file: Option<String>,
    screenshot: Option<String>,
    exit_after_ms: Option<u64>,
}

fn parse_args() -> Args {
    let mut args = Args::default();
    let mut iter = std::env::args().skip(1);
    while let Some(arg) = iter.next() {
        match arg.as_str() {
            "--configured" => args.configured = true,
            "--file" => args.file = iter.next(),
            "--chooser-with-file" => args.chooser_with_file = iter.next(),
            "--screenshot" => args.screenshot = iter.next(),
            "--exit-after-ms" => {
                args.exit_after_ms = iter.next().and_then(|s| s.parse().ok());
            }
            "--help" | "-h" => {
                eprintln!(
                    "Usage: clipboard_ux_smoke [--configured]\n\
                     \n                              [--file PATH]\n\
                     \n                              [--chooser-with-file PATH]\n\
                     \n                              [--screenshot OUT.png]\n\
                     \n                              [--exit-after-ms N]\n"
                );
                std::process::exit(0);
            }
            other => {
                eprintln!("unrecognised argument: {other}; pass --help");
                std::process::exit(2);
            }
        }
    }
    args
}

fn main() -> anyhow::Result<()> {
    // Direct composition must be off before `application()` constructs the
    // platform. See module docstring and the matching block in
    // `src/native/mod.rs::run_native` for the rationale.
    #[cfg(target_os = "windows")]
    if std::env::var_os("GPUI_DISABLE_DIRECT_COMPOSITION").is_none() {
        unsafe {
            std::env::set_var("GPUI_DISABLE_DIRECT_COMPOSITION", "true");
        }
    }

    let args = parse_args();

    let fixtures = tempfile::tempdir()?;
    let clip_dir = fixtures.path().join("clipboard");
    let stickers_dir = fixtures.path().join("stickers");
    std::fs::create_dir_all(&clip_dir)?;
    std::fs::create_dir_all(&stickers_dir)?;

    let clip_store: Arc<dyn ClipboardHistoryStore> = Arc::new(futures::executor::block_on(
        SqliteClipboardStore::open(clip_dir.join("clipboard.db")),
    )?);

    let sticker_store: ArcStickerStore =
        futures::executor::block_on(open_sqlite(stickers_dir.join("stickers.db")))?;

    seed_chooser_fixtures(&clip_store, args.chooser_with_file.as_deref())?;

    let vault = Arc::new(VaultController::new(clip_store.clone()));
    if args.configured {
        futures::executor::block_on(vault.create("fixture-password"))?;
    }

    let app = application().with_assets(Assets);
    let screenshot = args.screenshot.clone();
    let exit_after_ms = args.exit_after_ms;
    let file_to_open = args.file.clone();

    app.run(move |cx: &mut App| {
        gpui_kit::init(cx);
        Theme::change(ThemeMode::Dark, None, cx);
        clipboard::set_global_capture_filter(CaptureFilter::new());
        clipboard::set_global_store(clip_store.clone());
        clipboard::set_global_vault_controller(vault);

        let (tx, _rx) = mpsc::channel::<StickerWindowEvent>();

        let interactive = screenshot.is_none() && exit_after_ms.is_none();

        if let Some(path) = file_to_open.as_deref() {
            // Open file sticker for the given path. This branch is the one used
            // to verify the HTML-preview fix: pass --file <something.html>
            // and (without --exit-after-ms) the live window shows whether the
            // file sticker renders the page or paints a blank region.
            sticker::StickerWindow::open_file_preview_with_sources(
                cx,
                tx.clone(),
                sticker_store.clone(),
                vec![path.to_string()],
                sticker::PreviewOptions::default(),
            )
            .expect("open file preview");
        } else {
            clipboard::ClipboardChooserWindow::toggle(cx, tx, clip_store, None)
                .expect("toggle chooser");
        }

        if !interactive {
            schedule_inspect(cx, screenshot, exit_after_ms);
        }
    });
    drop(fixtures);
    Ok(())
}

fn seed_chooser_fixtures(
    store: &Arc<dyn ClipboardHistoryStore>,
    chooser_with_file: Option<&str>,
) -> anyhow::Result<()> {
    for (index, text) in FIXTURE_ITEMS.iter().enumerate() {
        futures::executor::block_on(store.upsert_ordinary(
            text,
            text,
            &SqliteClipboardStore::ordinary_digest(text),
            text.len() as i64,
            100 - index as i64,
        ))?;
    }

    // A dedicated "newest" item whose content is the local:// URL of the
    // chosen file. Bumping `last_used_at` past the existing fixtures
    // surfaces it at the top of the chooser; pressing Enter on that row
    // exercises the same code path as the host's file-sticker preview,
    // but from inside the chooser flow.
    if let Some(path) = chooser_with_file {
        let url = format!("local://localhost/{}", path);
        futures::executor::block_on(store.upsert_ordinary(
            &url,
            &url,
            &SqliteClipboardStore::ordinary_digest(&url),
            url.len() as i64,
            200,
        ))?;
    }
    Ok(())
}

fn schedule_inspect(cx: &mut App, screenshot: Option<String>, exit_after_ms: Option<u64>) {
    cx.spawn(async move |cx| {
        let wait_ms = exit_after_ms.unwrap_or(800);
        cx.background_executor()
            .timer(Duration::from_millis(wait_ms))
            .await;
        if let Some(path) = screenshot {
            capture_visible_window(&path);
        }
        if exit_after_ms.is_some() {
            cx.update(|cx| cx.quit());
        }
    })
    .detach();
}

/// Pull the largest visible top-level window owned by our process and save
/// it as a PNG. Windows-only because the host is Windows.
fn capture_visible_window(out_path: &str) {
    let path = std::path::Path::new(out_path);
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let report = capture_diagnostics(out_path);
    match report {
        Ok(report) => {
            eprintln!(
                "screenshot saved to {out_path} ({} bytes, {}x{})",
                report.bytes, report.width, report.height
            );
        }
        Err(err) => {
            eprintln!("screenshot failed: {err}");
        }
    }
}

#[derive(Debug)]
struct CaptureReport {
    bytes: usize,
    width: u32,
    height: u32,
}

#[cfg(target_os = "windows")]
fn capture_diagnostics(out_path: &str) -> Result<CaptureReport, String> {
    use std::io::Cursor;
    use windows::Win32::Foundation::{HWND, LPARAM, RECT};
    use windows::Win32::Graphics::Gdi::{
        BITMAPINFO, BITMAPINFOHEADER, CreateCompatibleBitmap, CreateCompatibleDC, DIB_RGB_COLORS,
        DeleteDC, DeleteObject, GetDC, GetDIBits, RGBQUAD, ReleaseDC,
    };
    use windows::Win32::Storage::Xps::PrintWindow;
    use windows::Win32::System::Threading::GetCurrentProcessId;
    use windows::Win32::UI::WindowsAndMessaging::{
        EnumWindows, GetWindowRect, GetWindowThreadProcessId, IsWindowVisible,
    };
    use windows::core::BOOL;

    struct EnumerateCtx {
        my_pid: u32,
        best_hwnd: HWND,
        best_area: i64,
    }

    unsafe extern "system" fn enum_proc(hwnd: HWND, lparam: LPARAM) -> BOOL {
        let ctx = unsafe { &mut *(lparam.0 as *mut EnumerateCtx) };
        let mut pid = 0u32;
        unsafe { GetWindowThreadProcessId(hwnd, Some(&mut pid)) };
        if pid != ctx.my_pid {
            return BOOL(1);
        }
        if !unsafe { IsWindowVisible(hwnd) }.as_bool() {
            return BOOL(1);
        }
        let mut rect = RECT::default();
        if unsafe { GetWindowRect(hwnd, &mut rect) }.is_err() {
            return BOOL(1);
        }
        let width = (rect.right - rect.left) as i64;
        let height = (rect.bottom - rect.top) as i64;
        let area = width * height;
        if area <= 0 {
            return BOOL(1);
        }
        if area > ctx.best_area {
            ctx.best_area = area;
            ctx.best_hwnd = hwnd;
        }
        BOOL(1)
    }

    let null_hwnd = HWND(std::ptr::null_mut());
    let my_pid = unsafe { GetCurrentProcessId() };
    let mut ctx = EnumerateCtx {
        my_pid,
        best_hwnd: null_hwnd,
        best_area: 0,
    };
    unsafe {
        EnumWindows(Some(enum_proc), LPARAM(&mut ctx as *mut _ as isize))
            .map_err(|e| format!("EnumWindows: {e}"))?;
    }
    if ctx.best_hwnd.0.is_null() {
        return Err("no visible window owned by this process".into());
    }
    let hwnd = ctx.best_hwnd;

    let mut rect = RECT::default();
    unsafe { GetWindowRect(hwnd, &mut rect) }.map_err(|e| format!("GetWindowRect: {e}"))?;
    let width = (rect.right - rect.left).max(1);
    let height = (rect.bottom - rect.top).max(1);

    // PrintWindow with PW_RENDERFULLCONTENT (2) is needed to make Edge /
    // WebView2 layers actually paint into our bitmap. Without it the
    // captured frame is missing exactly the file-sticker's HTML preview.
    let flags = windows::Win32::Storage::Xps::PRINT_WINDOW_FLAGS(0x00000002);

    let screen_dc = unsafe { GetDC(Some(null_hwnd)) };
    if screen_dc.is_invalid() {
        return Err("GetDC returned NULL HDC".into());
    }
    let mem_dc = unsafe { CreateCompatibleDC(Some(screen_dc)) };
    if mem_dc.is_invalid() {
        let _: i32 = unsafe { ReleaseDC(Some(null_hwnd), screen_dc) };
        return Err("CreateCompatibleDC returned NULL HDC".into());
    }
    let bitmap = unsafe { CreateCompatibleBitmap(screen_dc, width, height) };
    if bitmap.is_invalid() {
        let _: BOOL = unsafe { DeleteDC(mem_dc) };
        let _: i32 = unsafe { ReleaseDC(Some(null_hwnd), screen_dc) };
        return Err("CreateCompatibleBitmap returned NULL HBITMAP".into());
    }
    let prev = unsafe { windows::Win32::Graphics::Gdi::SelectObject(mem_dc, bitmap.into()) };

    let ok = unsafe { PrintWindow(hwnd, mem_dc, flags) };
    if !ok.as_bool() {
        let _ = unsafe { windows::Win32::Graphics::Gdi::SelectObject(mem_dc, prev) };
        let _: BOOL = unsafe { DeleteObject(bitmap.into()) };
        let _: BOOL = unsafe { DeleteDC(mem_dc) };
        let _: i32 = unsafe { ReleaseDC(Some(null_hwnd), screen_dc) };
        return Err("PrintWindow returned 0".into());
    }

    // Read the bitmap as 32-bit top-down RGBA via GetDIBits.
    // `biHeight = -height` flips the rows so y=0 is the top edge.
    let mut info = BITMAPINFO {
        bmiHeader: BITMAPINFOHEADER {
            biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
            biWidth: width,
            biHeight: -height,
            biPlanes: 1,
            biBitCount: 32,
            biCompression: 0, // BI_RGB
            biSizeImage: 0,
            biXPelsPerMeter: 0,
            biYPelsPerMeter: 0,
            biClrUsed: 0,
            biClrImportant: 0,
        },
        bmiColors: [RGBQUAD::default(); 1],
    };
    let mut pixels = vec![0u8; (width as usize) * (height as usize) * 4];
    let scanlines = unsafe {
        GetDIBits(
            mem_dc,
            bitmap,
            0,
            height as u32,
            Some(pixels.as_mut_ptr() as *mut _),
            &mut info,
            DIB_RGB_COLORS,
        )
    };
    if scanlines == 0 {
        let _ = unsafe { windows::Win32::Graphics::Gdi::SelectObject(mem_dc, prev) };
        let _: BOOL = unsafe { DeleteObject(bitmap.into()) };
        let _: BOOL = unsafe { DeleteDC(mem_dc) };
        let _: i32 = unsafe { ReleaseDC(Some(null_hwnd), screen_dc) };
        return Err("GetDIBits returned 0".into());
    }

    // Push to image::RgbaImage.
    let mut img = image::RgbaImage::new(width as u32, height as u32);
    for y in 0..height as u32 {
        for x in 0..width as u32 {
            let idx = ((y as usize) * (width as usize) + (x as usize)) * 4;
            let b = pixels[idx];
            let g = pixels[idx + 1];
            let r = pixels[idx + 2];
            img.put_pixel(x, y, image::Rgba([r, g, b, 0xff]));
        }
    }

    let mut out = Vec::new();
    {
        let mut cursor = Cursor::new(&mut out);
        img.write_to(&mut cursor, image::ImageFormat::Png)
            .map_err(|e| format!("PNG encode: {e}"))?;
    }

    std::fs::write(out_path, &out).map_err(|e| format!("write {out_path}: {e}"))?;

    let _ = unsafe { windows::Win32::Graphics::Gdi::SelectObject(mem_dc, prev) };
    let _: BOOL = unsafe { DeleteObject(bitmap.into()) };
    let _: BOOL = unsafe { DeleteDC(mem_dc) };
    let _: i32 = unsafe { ReleaseDC(Some(null_hwnd), screen_dc) };

    Ok(CaptureReport {
        bytes: out.len(),
        width: width as u32,
        height: height as u32,
    })
}

#[cfg(not(target_os = "windows"))]
fn capture_diagnostics(out_path: &str) -> Result<CaptureReport, String> {
    let _ = out_path;
    Err(
        "--screenshot is implemented for Windows only; this smoke was written for a Windows host."
            .into(),
    )
}
