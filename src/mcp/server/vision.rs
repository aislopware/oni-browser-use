//! `vision` handler: screenshot (+ optional Set-of-Marks overlay), inline or
//! written to a file, of the viewport, a sized viewport or the whole page.

use serde_json::{Value, json};
use std::sync::atomic::{AtomicU64, Ordering};

use crate::error::ObuError;
use crate::page::Page;

static MARK_SEQUENCE: AtomicU64 = AtomicU64::new(0);

/// Above this many base64 chars a capture is offloaded to a PNG artifact
/// instead of being inlined (see H2 below).
const INLINE_LIMIT: usize = 900_000;

/// Chrome draws a capture into one GPU texture; past this many device
/// pixels on a side it returns a blank or clipped image.
const MAX_CAPTURE_PIXELS: f64 = 16_384.0;

/// The most output pixels in one capture. A PNG is four bytes a pixel at
/// worst, and its reply in base64 must fit one CDP message
/// (`MAX_CDP_MESSAGE_BYTES`), or the browser connection closes on it.
const MAX_CAPTURE_AREA: f64 = 40_000_000.0;

static PARTIAL_SEQUENCE: AtomicU64 = AtomicU64::new(0);

/// A viewport to capture at, parsed from `size` (`WxH` or `WxH@scale`).
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct ShotSize {
    pub width: u32,
    pub height: u32,
    /// Output pixels per CSS pixel.
    pub scale: f64,
}

/// Parse `size`: `1440x900`, or `1440x900@2` for two output pixels per CSS
/// pixel.
pub(crate) fn parse_size(size: &str) -> std::result::Result<ShotSize, ObuError> {
    let bad = || {
        ObuError::Usage(format!(
            "vision size must be WxH or WxH@scale (e.g. 1440x900@2), got '{size}'"
        ))
    };
    let (dims, scale) = match size.split_once('@') {
        Some((dims, scale)) => (dims, scale.parse::<f64>().map_err(|_| bad())?),
        None => (size, 1.0),
    };
    let (width, height) = dims.split_once(['x', 'X']).ok_or_else(bad)?;
    let width = width.parse::<u32>().map_err(|_| bad())?;
    let height = height.parse::<u32>().map_err(|_| bad())?;
    if !(16..=8192).contains(&width) || !(16..=8192).contains(&height) {
        return Err(ObuError::Usage(
            "vision size: width and height must be 16 to 8192 CSS pixels".into(),
        ));
    }
    if !scale.is_finite() || !(0.25..=4.0).contains(&scale) {
        return Err(ObuError::Usage(
            "vision size: scale must be 0.25 to 4".into(),
        ));
    }
    let (out_width, out_height) = (f64::from(width) * scale, f64::from(height) * scale);
    if out_width > MAX_CAPTURE_PIXELS
        || out_height > MAX_CAPTURE_PIXELS
        || out_width * out_height > MAX_CAPTURE_AREA
    {
        return Err(ObuError::Usage(format!(
            "vision size: {size} is {out_width:.0}x{out_height:.0} output pixels; a capture is at most {MAX_CAPTURE_PIXELS:.0} on a side and {:.0} megapixels",
            MAX_CAPTURE_AREA / 1e6
        )));
    }
    Ok(ShotSize {
        width,
        height,
        scale,
    })
}

/// The absolute path a capture is written to. Only `.png`: the bytes are a
/// PNG whatever the name says.
pub(crate) fn output_path(path: &str) -> std::result::Result<std::path::PathBuf, ObuError> {
    if !path.to_ascii_lowercase().ends_with(".png") {
        return Err(ObuError::Usage(format!(
            "vision path must end in .png, got '{path}'"
        )));
    }
    std::path::absolute(path)
        .map_err(|e| ObuError::Usage(format!("vision path '{path}' is not usable: {e}")))
}

/// Pixel size of a PNG, read from its IHDR chunk.
fn png_size(bytes: &[u8]) -> Option<(u32, u32)> {
    let field = |at: usize| Some(u32::from_be_bytes(bytes.get(at..at + 4)?.try_into().ok()?));
    (bytes.get(..8)? == b"\x89PNG\r\n\x1a\n").then_some(())?;
    Some((field(16)?, field(20)?))
}

/// One vision capture, before it is shaped for a surface: MCP wraps it as
/// text + image content blocks, the CLI prints `image_path`.
pub struct VisionShot {
    pub text: String,
    /// Base64 PNG to inline.
    pub image: Option<String>,
    /// PNG the handler already wrote to an artifact file (oversized
    /// captures). Returned as a field, not only inside `text`: the CLI used
    /// to see no image and print no `image_path` for exactly these.
    pub image_path: Option<String>,
    pub is_error: bool,
}

impl VisionShot {
    fn error(text: String) -> Self {
        VisionShot {
            text,
            image: None,
            image_path: None,
            is_error: true,
        }
    }
}

/// Capture a screenshot of the current page and return it as an MCP image
/// content block (base64 PNG).
///
/// This is the `vision` tool (decision D5) — a rare fallback for canvas
/// content, exotic layouts, or when the structural model fails.
pub async fn handle_vision(
    id: Option<Value>,
    args: &Value,
    page: &mut Page,
) -> std::result::Result<Value, ObuError> {
    let shot = capture_vision(args, page).await?;
    Ok(vision_response(id, shot))
}

/// The MCP JSON-RPC result for a capture: a text block, the image block
/// when inlined, `isError` only on failure.
fn vision_response(id: Option<Value>, shot: VisionShot) -> Value {
    let mut content = vec![json!({ "type": "text", "text": shot.text })];
    if let Some(data) = shot.image {
        content.push(json!({
            "type": "image",
            "data": data,
            "mimeType": "image/png"
        }));
    }
    let mut result = json!({ "content": content });
    if shot.is_error {
        result["isError"] = json!(true);
    }
    json!({
        "jsonrpc": "2.0",
        "id": id,
        "result": result,
    })
}

/// Screenshot (+ optional Set-of-Marks overlay) as a [`VisionShot`]. Shared
/// by the MCP handler and the CLI.
pub async fn capture_vision(
    args: &Value,
    page: &mut Page,
) -> std::result::Result<VisionShot, ObuError> {
    super::validate::vision(args)?;
    let marks = args.get("marks").and_then(|m| m.as_bool()).unwrap_or(false);
    let full = args.get("full").and_then(|f| f.as_bool()).unwrap_or(false);
    let size = match args.get("size").and_then(|s| s.as_str()) {
        Some(size) if !size.is_empty() => Some(parse_size(size)?),
        _ => None,
    };
    let path = match args.get("path").and_then(|p| p.as_str()) {
        Some(path) if !path.is_empty() => Some(output_path(path)?),
        _ => None,
    };
    let url = args.get("url").and_then(|u| u.as_str()).unwrap_or("");
    // Same gate as act's pre-navigation: a person holding the page keeps
    // it, at its size as well as its address.
    if (!url.is_empty() || size.is_some()) && crate::realbrowser::input_paused() {
        return Err(crate::realbrowser::paused_error());
    }
    // The viewport is resized for this capture only and handed back after
    // it: a page left at an emulated size reads as a different device to
    // every later visit. Set before the page opens, since a page that sizes
    // itself as it loads never hears of a later resize, and before the
    // marks, which are placed by layout.
    if let Some(size) = size {
        let sized = page
            .cdp_ref()
            .send(
                "Emulation.setDeviceMetricsOverride",
                Some(json!({
                    "width": size.width,
                    "height": size.height,
                    "deviceScaleFactor": size.scale,
                    "mobile": false,
                })),
            )
            .await;
        if let Err(e) = sized {
            // An override that timed out may still have been applied.
            return give_up(page, true, e).await;
        }
    }
    if !url.is_empty() {
        // Navigating to the page it is on is skipped, so it would keep the
        // layout it loaded with at the old size.
        let reload = size.is_some() && page.is_at(url);
        let opened = match page.navigate(url).await {
            Ok(_) if reload => page.act(crate::action::Action::Reload).await.map(|_| ()),
            Ok(_) => Ok(()),
            Err(e) => Err(e),
        };
        if let Err(e) = opened {
            return give_up(page, size.is_some(), e).await;
        }
    }
    let mut note = String::new();
    let overlay_id = serde_json::to_string(&format!(
        "obu-marks-{}-{}",
        std::process::id(),
        MARK_SEQUENCE.fetch_add(1, Ordering::Relaxed)
    ))?;
    let mut overlay_installed = false;

    // A tab that is not the frontmost one can stall captureScreenshot —
    // Chromium throttles occluded/background renderers, so the call burns
    // its full CDP timeout and vision fails (reproduced live: vision right
    // after open-tab timed out at 30s, twice, on the new tab). Activate
    // only when the target is not already its window's active tab (#89:
    // raising the window on macOS steals focus); best-effort (fails
    // harmlessly when the target is mid-detach).
    let _ = page.activate_if_needed().await;

    let zoom = if size.is_some() {
        layout(page).await.zoom
    } else {
        1.0
    };
    if let Some(size) = size
        && !viewport_reached(page, size, zoom).await
    {
        note.push_str(" (the viewport did not reach the requested size)");
    }

    if marks {
        // V14: Set-of-Marks. Paint numbered ref badges on
        // visible elements — the refs match the structural
        // model exactly, so a vision-capable agent can say
        // "click e5" and the act tool just works.
        let items: Vec<(String, String)> = page
            .model()
            .elements()
            .iter()
            .map(|e| (e.ref_id.clone(), e.raw.sig.clone()))
            .collect();
        let items_js = serde_json::to_string(&items)?;
        let overlay = "((items)=>{".to_string()
            + "const d=document;if(!d||!d.body)return 0;"
            + &crate::page::perception::JS_PREAMBLE
            // Never remove a page-owned node merely because its ID matches.
            + &format!("const id={overlay_id};if(d.getElementById(id))throw new Error('marks ID collision');")
            + "const ov=d.createElement('div');ov.id=id;"
            + "ov.style.cssText='position:fixed;inset:0;pointer-events:none;z-index:2147483647;';"
            + "const vw=innerWidth,vh=innerHeight;"
            + "const all=deepAll(d,sel);"
            + "const sigs=new Map();const counts={};"
            // Sig = '' frame prefix (main doc) | role | shortName | rank,
            // rank counted over ALL matches (vis-failing included) — matches
            // the capture script (V25c). Only vis-passing elements are mapped
            // for marking, but the rank counts everything so sigs agree.
            + "for(let i=0;i<all.length;i++){const n=all[i];const r=role(n);if(r==='hidden')continue;"
            + "const nm=name(n,false);const key=r+'\\u0000'+nm;counts[key]=(counts[key]||0)+1;"
            + "if(!vis(n))continue;"
            + "sigs.set('|'+r+'|'+nm+'|'+counts[key],n);}"
            + "let marked=0;"
            + "for(const[ref,sig]of items){const el=sigs.get(sig);if(!el)continue;"
            + "const rect=el.getBoundingClientRect();"
            + "if(rect.bottom<0||rect.top>vh||rect.right<0||rect.left>vw)continue;"
            + "const b=d.createElement('div');b.textContent=ref;"
            + "b.style.cssText='position:fixed;left:'+Math.max(0,rect.x+rect.width/2-12)+'px;top:'+Math.max(0,rect.y+rect.height/2-8)+'px;background:rgba(220,0,110,0.92);color:#fff;font:bold 11px/14px monospace;padding:0 4px;border-radius:3px;border:1px solid #fff;';"
            + "ov.appendChild(b);marked++;}"
            + "d.body.appendChild(ov);return marked;})(" + &items_js + ")";
        let painted = page
            .cdp_ref()
            .send(
                "Runtime.evaluate",
                Some(json!({
                    "expression": overlay,
                    "returnByValue": true,
                })),
            )
            .await;
        let res = match painted {
            Ok(res) => res,
            Err(e) => return give_up(page, size.is_some(), e).await,
        };
        if let Some(exc) = res.get("exceptionDetails") {
            let msg = exc
                .get("exception")
                .and_then(|e| e.get("description"))
                .and_then(|d| d.as_str())
                .unwrap_or("overlay failed");
            note.push_str(&format!(
                " (marks overlay error: {})",
                crate::platform::truncate_utf8(msg, 120)
            ));
        } else {
            overlay_installed = true;
            let marked = res
                .get("result")
                .and_then(|r| r.get("value"))
                .and_then(|v| v.as_i64())
                .unwrap_or(0);
            note.push_str(&format!(
                " ({marked} elements marked; badge refs match the structural model)"
            ));
        }
    }

    let mut capture = json!({ "format": "png" });
    // The override sets the pixel ratio where the browser honors it. Where it
    // does not, the clip makes up the difference, so the PNG is the size
    // asked for on any display.
    let ratio = pixel_ratio(page).await;
    let measured = if full {
        layout(page).await
    } else {
        Layout {
            zoom,
            content: None,
            drawn: None,
        }
    };
    // A clip is sized in device-independent pixels. The page's ratio counts
    // its zoom as well, so the zoom is taken out of it.
    let per_dip = if ratio > 0.0 {
        ratio / measured.zoom
    } else {
        0.0
    };
    let scale = size.map_or(1.0, |size| clip_scale(size.scale, per_dip));
    if full {
        match measured.content {
            Some(css) => {
                let (width, height) = content_dips(css, measured.drawn, ratio, measured.zoom);
                // Output pixels per clip pixel as the browser will draw them,
                // or the scale asked for when the ratio is unreadable.
                let per_clip = if per_dip > 0.0 {
                    scale * per_dip
                } else {
                    size.map_or(1.0, |s| s.scale)
                };
                let (shown_width, shown_height) = full_bounds(width, height, per_clip);
                if (shown_width, shown_height) != (width, height) {
                    note.push_str(&format!(
                        " (the page is {width:.0}x{height:.0} px; captured the first {shown_width:.0}x{shown_height:.0})"
                    ));
                }
                capture["captureBeyondViewport"] = json!(true);
                capture["clip"] = json!({
                    "x": 0, "y": 0,
                    "width": shown_width,
                    "height": shown_height,
                    "scale": scale,
                });
            }
            None => note.push_str(" (page size unreadable; captured the viewport)"),
        }
    } else if let Some(size) = size {
        // Always a clip: a browser that shows the page in a smaller window
        // captures only that window without one.
        capture["clip"] = json!({
            "x": 0, "y": 0,
            "width": size.width,
            "height": size.height,
            "scale": scale,
        });
    }
    let result = page
        .cdp_ref()
        .send("Page.captureScreenshot", Some(capture))
        .await;
    let mut stuck = String::new();
    if size.is_some()
        && let Err(e) = hand_back(page).await
    {
        stuck = still_sized(&e);
        note.push_str(&stuck);
    }

    // Remove the overlay BEFORE processing the result —
    // the page must never keep our marks.
    if overlay_installed {
        let _ = page.cdp_ref().send("Runtime.evaluate", Some(json!({
            "expression": format!("(()=>{{const o=document.getElementById({overlay_id});if(o)o.remove();return true;}})()"),
            "returnByValue": true,
        }))).await;
    }

    match result {
        Ok(res) => {
            let data = res.get("data").and_then(|d| d.as_str()).unwrap_or("");
            if let Some(size) = size.filter(|_| !full) {
                note.push_str(&size_mismatch(size, data));
            }
            let mut shot = match path {
                Some(path) => shot_to_file(data, &note, &path),
                None => shot_from_capture(data, &note),
            };
            if shot.is_error {
                shot.text.push_str(&stuck);
            }
            Ok(shot)
        }
        // Propagate Closed so serve() can self-heal.
        Err(ObuError::Closed) => Err(ObuError::Closed),
        Err(e) => Ok(VisionShot::error(format!("\u{2717} error: {e}{stuck}"))),
    }
}

fn still_sized(e: &ObuError) -> String {
    format!(" (\u{26a0} the page is still at the capture's size: {e})")
}

/// Ends a capture that failed before its screenshot. The page gets its
/// viewport back first; when that fails too, the error says the page is
/// still sized for the capture.
async fn give_up(
    page: &Page,
    sized: bool,
    e: ObuError,
) -> std::result::Result<VisionShot, ObuError> {
    if !sized || matches!(e, ObuError::Closed) {
        return Err(e);
    }
    match hand_back(page).await {
        Ok(()) => Err(e),
        Err(ObuError::Closed) => Err(ObuError::Closed),
        Err(restore) => Ok(VisionShot::error(format!(
            "\u{2717} error: {e}{}",
            still_sized(&restore)
        ))),
    }
}

/// How much of a page of `width` by `height` one capture holds at `per_css`
/// output pixels to each of the page's.
fn full_bounds(width: f64, height: f64, per_css: f64) -> (f64, f64) {
    let side = (MAX_CAPTURE_PIXELS / per_css).floor().max(1.0);
    let width = width.min(side);
    let rows = (MAX_CAPTURE_AREA / (width * per_css * per_css)).floor();
    (width, height.min(side).min(rows).max(1.0))
}

/// Gives the page its own viewport back. Tried twice: a page left at an
/// emulated size reads as a different device to every later visit.
async fn hand_back(page: &Page) -> std::result::Result<(), ObuError> {
    let clear = || {
        page.cdp_ref()
            .send("Emulation.clearDeviceMetricsOverride", None)
    };
    match clear().await {
        Ok(_) => Ok(()),
        Err(ObuError::Closed) => Err(ObuError::Closed),
        Err(_) => clear().await.map(|_| ()),
    }
}

/// Wait for layout to take an emulated viewport: a capture taken before it
/// does shows the old size. Polls instead of waiting on a frame, which a
/// background tab may never paint. A read that fails ends the wait: a page
/// that answers nothing would hold every one of the polls to its timeout.
async fn viewport_reached(page: &Page, size: ShotSize, zoom: f64) -> bool {
    for _ in 0..40 {
        let Ok(read) = page
            .cdp_ref()
            .send(
                "Runtime.evaluate",
                Some(json!({ "expression": "innerWidth+'x'+innerHeight", "returnByValue": true })),
            )
            .await
        else {
            return false;
        };
        // A page whose own `innerWidth` throws answers the same every time.
        let Some(seen) = read["result"]["value"]
            .as_str()
            .filter(|_| read.get("exceptionDetails").is_none())
        else {
            return false;
        };
        if reached(seen, size, zoom) {
            return true;
        }
        tokio::time::sleep(std::time::Duration::from_millis(25)).await;
    }
    false
}

/// Whether a viewport read as `WxH` CSS pixels is the size asked for. A
/// zoomed page has fewer CSS pixels than the device-independent ones the
/// size is set in, to within one of them.
fn reached(seen: &str, size: ShotSize, zoom: f64) -> bool {
    let Some((width, height)) = seen.split_once('x') else {
        return false;
    };
    let near = |seen: &str, wanted: u32| {
        seen.parse::<f64>()
            .is_ok_and(|css| (css * zoom - f64::from(wanted)).abs() < zoom.max(1.0))
    };
    near(width, size.width) && near(height, size.height)
}

/// The page's device pixels per CSS pixel, or 0 when unreadable.
async fn pixel_ratio(page: &Page) -> f64 {
    page.cdp_ref()
        .send(
            "Runtime.evaluate",
            Some(json!({ "expression": "devicePixelRatio", "returnByValue": true })),
        )
        .await
        .ok()
        .and_then(|r| r["result"]["value"].as_f64())
        .unwrap_or(0.0)
}

/// The clip scale that turns a page at `ratio` into a capture at `scale`.
pub(crate) fn clip_scale(scale: f64, ratio: f64) -> f64 {
    if ratio <= 0.0 || (scale / ratio - 1.0).abs() < 0.001 {
        return 1.0;
    }
    scale / ratio
}

/// Says so when the browser returned another size than the one asked for,
/// so a caller never hands on a capture it believes is exact.
fn size_mismatch(size: ShotSize, data: &str) -> String {
    let head = data.get(..44).and_then(crate::cli::base64_decode);
    let Some((width, height)) = head.as_deref().and_then(png_size) else {
        return String::new();
    };
    let wanted = (
        (f64::from(size.width) * size.scale).round(),
        (f64::from(size.height) * size.scale).round(),
    );
    if (f64::from(width), f64::from(height)) == wanted {
        return String::new();
    }
    format!(
        " (the browser returned {width}x{height} px, not the {:.0}x{:.0} asked for)",
        wanted.0, wanted.1
    )
}

/// The page's zoom and its whole size.
struct Layout {
    /// Device-independent pixels per CSS pixel: 1.25 at 125%.
    zoom: f64,
    /// In CSS pixels, which the browser rounds down.
    content: Option<(f64, f64)>,
    /// In the pixels the page is drawn at.
    drawn: Option<(f64, f64)>,
}

async fn layout(page: &Page) -> Layout {
    let metrics = page
        .cdp_ref()
        .send("Page.getLayoutMetrics", None)
        .await
        .unwrap_or(Value::Null);
    let zoom = metrics["cssVisualViewport"]["zoom"]
        .as_f64()
        .filter(|zoom| zoom.is_finite() && *zoom > 0.0)
        .unwrap_or(1.0);
    let size = |of: &str| match (
        metrics[of]["width"].as_f64(),
        metrics[of]["height"].as_f64(),
    ) {
        (Some(width), Some(height)) if width >= 1.0 && height >= 1.0 => Some((width, height)),
        _ => None,
    };
    Layout {
        zoom,
        content: size("cssContentSize"),
        drawn: size("contentSize"),
    }
}

/// The page's whole size in the device-independent pixels a clip is sized in.
/// Its CSS size comes rounded down, which at a `ratio` other than 1 leaves
/// the last part pixel out. The size it is `drawn` at has it, and is used
/// where the ratio accounts for it: under an emulated ratio it does not.
fn content_dips(css: (f64, f64), drawn: Option<(f64, f64)>, ratio: f64, zoom: f64) -> (f64, f64) {
    let whole = |drawn: f64| (drawn / ratio + 1e-6).floor();
    match drawn {
        Some((width, height)) if ratio > 0.0 && whole(width) == css.0 && whole(height) == css.1 => {
            (width / ratio * zoom, height / ratio * zoom)
        }
        _ => (dips(css.0, zoom), dips(css.1, zoom)),
    }
}

/// CSS pixels as the device-independent pixels a clip is sized in, rounded
/// up so the last part pixel of the page is in the capture.
fn dips(css: f64, zoom: f64) -> f64 {
    (css * zoom - 1e-6).ceil().max(1.0)
}

/// Write a captured PNG where the caller asked for it. The image is not
/// inlined as well: a caller that names a file wants the file.
fn shot_to_file(data: &str, note: &str, path: &std::path::Path) -> VisionShot {
    let Some(bytes) = crate::cli::base64_decode(data).filter(|b| !b.is_empty()) else {
        return VisionShot::error("Screenshot returned no data.".into());
    };
    // Written beside the file and renamed over it: a failed write leaves the
    // old capture whole, and a symlink at the path is replaced, not followed.
    // The partial is a new file of this call's own, so nothing already there
    // is written through or removed.
    let partial = path.with_extension(format!(
        "png.{}-{}.part",
        std::process::id(),
        PARTIAL_SEQUENCE.fetch_add(1, Ordering::Relaxed)
    ));
    let written = path
        .parent()
        .map_or(Ok(()), std::fs::create_dir_all)
        .and_then(|()| {
            std::fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&partial)
        })
        .and_then(|mut file| {
            let moved = std::io::Write::write_all(&mut file, &bytes).and_then(|()| {
                drop(file);
                std::fs::rename(&partial, path)
            });
            if moved.is_err() {
                let _ = std::fs::remove_file(&partial);
            }
            moved
        });
    if let Err(e) = written {
        return VisionShot::error(format!(
            "\u{2717} error: screenshot not saved to {}: {e}",
            path.display()
        ));
    }
    let pixels = png_size(&bytes).map_or(String::new(), |(w, h)| format!("{w}x{h} px, "));
    VisionShot {
        text: format!(
            "screenshot{note} saved to {} ({pixels}{} KB)",
            path.display(),
            bytes.len().div_ceil(1024)
        ),
        image: None,
        image_path: Some(path.display().to_string()),
        is_error: false,
    }
}

/// Inline a captured PNG, or offload it when oversized.
fn shot_from_capture(data: &str, note: &str) -> VisionShot {
    if data.is_empty() {
        return VisionShot::error("Screenshot returned no data.".into());
    }
    // H2: multi-MB screenshots (tall pages, 4K full-page) inline
    // through the single stdout writer backpressure the whole
    // server (stdin unread, signals unhandled, idle timer frozen)
    // on a slow client. Offload big ones to a PNG artifact and
    // return the path; vision-capable agents read the file.
    if data.len() > INLINE_LIMIT
        && let Some(bytes) = crate::cli::base64_decode(data)
        && let Ok(path) = crate::artifacts::write_artifact_bytes(&bytes, "png")
    {
        return VisionShot {
            text: format!(
                "screenshot{note} ({} KB — too large to inline, saved to {path})",
                bytes.len() / 1024
            ),
            image: None,
            image_path: Some(path),
            is_error: false,
        };
    }
    VisionShot {
        text: format!("screenshot{note}"),
        image: Some(data.to_string()),
        image_path: None,
        is_error: false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// An oversized capture is saved by the handler; the path must come back
    /// as a field (the CLI maps it to `image_path`), while the MCP response
    /// keeps its exact text-only shape.
    #[test]
    fn oversized_capture_returns_its_saved_path() {
        // 1,000,000 base64 chars → 750,000 PNG-sized bytes, over the limit.
        let data = "AAAA".repeat(250_000);
        assert!(data.len() > INLINE_LIMIT);
        let shot = shot_from_capture(&data, "");
        let path = shot.image_path.clone().expect("saved path returned");
        assert!(shot.image.is_none() && !shot.is_error);
        assert_eq!(std::fs::metadata(&path).unwrap().len(), 750_000);
        assert!(shot.text.contains(&path), "{}", shot.text);

        let v = vision_response(Some(json!(7)), shot);
        assert_eq!(v["id"], 7);
        let content = v["result"]["content"].as_array().unwrap();
        assert_eq!(content.len(), 1, "no inline image for an offloaded capture");
        assert!(
            content[0]["text"]
                .as_str()
                .unwrap()
                .starts_with("screenshot (732 KB — too large to inline, saved to ")
        );
        assert!(v["result"].get("isError").is_none());
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn size_is_width_by_height_with_an_optional_scale() {
        let size = |w, h, scale| ShotSize {
            width: w,
            height: h,
            scale,
        };
        assert_eq!(parse_size("1440x900").unwrap(), size(1440, 900, 1.0));
        assert_eq!(parse_size("390X844@3").unwrap(), size(390, 844, 3.0));
        assert_eq!(parse_size("1920x1080@0.5").unwrap(), size(1920, 1080, 0.5));
        assert_eq!(parse_size("3840x2160@2").unwrap(), size(3840, 2160, 2.0));
        for bad in [
            "",
            "1440",
            "x900",
            "1440x",
            "1440x900@",
            "1440x900@x",
            "15x900",
            "1440x8193",
            "1440x900@0.1",
            "1440x900@5",
            "8192x8192@4",
            "4097x900@4",
            "8192x4096@2",
            "8192x8192",
            "4000x4000@2",
            "-1x900",
            "1440 x 900",
        ] {
            assert!(parse_size(bad).is_err(), "{bad}");
        }
    }

    /// The clip scales by what the browser's pixel ratio leaves undone, and a
    /// capture of another size than asked for says so.
    #[test]
    fn a_sized_capture_is_exact_or_says_what_it_is() {
        assert_eq!(clip_scale(2.0, 2.0), 1.0);
        assert_eq!(clip_scale(1.0, 2.0), 0.5);
        assert_eq!(clip_scale(2.0, 1.0), 2.0);
        assert_eq!(clip_scale(2.0, 0.0), 1.0);

        // A 1x1 PNG.
        let png = "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mNkYAAAAAYAAjCB0C8AAAAASUVORK5CYII=";
        let size = |width, height, scale| ShotSize {
            width,
            height,
            scale,
        };
        assert_eq!(size_mismatch(size(16, 16, 0.0625), png), "");
        assert_eq!(
            size_mismatch(size(16, 32, 0.0625), png),
            " (the browser returned 1x1 px, not the 1x2 asked for)"
        );
        assert_eq!(
            size_mismatch(size(1920, 1080, 2.0), png),
            " (the browser returned 1x1 px, not the 3840x2160 asked for)"
        );
        assert_eq!(size_mismatch(size(1920, 1080, 2.0), "not a png"), "");
    }

    /// A whole page is cut to what one capture holds, by side and by area,
    /// counted in the pixels the browser draws.
    #[test]
    fn a_full_page_is_cut_to_what_one_capture_holds() {
        assert_eq!(full_bounds(1440.0, 4200.0, 1.0), (1440.0, 4200.0));
        assert_eq!(full_bounds(1440.0, 100_000.0, 2.0), (1440.0, 6944.0));
        assert_eq!(full_bounds(1440.0, 100_000.0, 1.0), (1440.0, 16384.0));
        assert_eq!(full_bounds(8196.0, 8196.0, 2.0), (8192.0, 1220.0));
        assert_eq!(full_bounds(390.0, 60_000.0, 3.0), (390.0, 5461.0));
    }

    /// A zoomed page has more device-independent pixels than CSS ones, and
    /// a clip is sized in the first: the whole page is in the capture, and
    /// its viewport counts as reached.
    #[test]
    fn a_zoomed_page_is_measured_in_the_pixels_a_clip_uses() {
        assert_eq!(dips(4000.0, 1.25), 5000.0);
        assert_eq!(dips(1000.0, 1.1), 1100.0);
        assert_eq!(dips(1000.4, 1.0), 1001.0);
        let size = ShotSize {
            width: 1920,
            height: 1080,
            scale: 1.0,
        };
        assert!(reached("1920x1080", size, 1.0));
        assert!(!reached("1919x1080", size, 1.0));
        assert!(reached("1536x864", size, 1.25));
        assert!(reached("1745x982", size, 1.1));
        assert!(!reached("1280x800", size, 1.25));
        assert!(!reached("", size, 1.0));
        // Under an override the page's ratio is the scale times its zoom.
        assert_eq!(clip_scale(2.0, 2.5 / 1.25), 1.0);
    }

    /// Measured on Chromium 157: a 1000.5px page at a ratio of 2 reports
    /// 1000 CSS pixels and 2001 drawn, and a clip of 1000.5 captures 2001.
    #[test]
    fn a_full_page_keeps_the_part_pixel_its_css_size_rounds_away() {
        let css = (800.0, 1000.0);
        assert_eq!(
            content_dips(css, Some((1600.0, 2001.0)), 2.0, 1.0),
            (800.0, 1000.5)
        );
        // At 125% zoom the clip is in the zoomed pixels.
        let (width, height) = content_dips(css, Some((1000.0, 1251.0)), 1.25, 1.25);
        assert!((width - 1000.0).abs() < 1e-6 && (height - 1251.0).abs() < 1e-6);
        // An emulated ratio does not scale the drawn size, so the CSS size stands.
        assert_eq!(
            content_dips(css, Some((800.0, 1000.0)), 2.0, 1.0),
            (800.0, 1000.0)
        );
        assert_eq!(content_dips(css, None, 2.0, 1.25), (1000.0, 1250.0));
        assert_eq!(content_dips(css, Some((1600.0, 2001.0)), 0.0, 1.0), css);
    }

    /// A named file gets the PNG, its folder is made, and the result
    /// carries the path and pixel size but no inline image.
    #[test]
    fn a_named_file_gets_the_capture_instead_of_an_inline_image() {
        // A 1x1 PNG.
        let png = "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mNkYAAAAAYAAjCB0C8AAAAASUVORK5CYII=";
        let dir = std::env::temp_dir().join(format!("obu-vision-test-{}", std::process::id()));
        let path = output_path(dir.join("nested/shot.png").to_str().unwrap()).unwrap();
        let shot = shot_to_file(png, " (2 elements marked)", &path);
        assert!(!shot.is_error && shot.image.is_none(), "{}", shot.text);
        assert_eq!(shot.image_path.as_deref(), path.to_str());
        assert_eq!(
            shot.text,
            format!(
                "screenshot (2 elements marked) saved to {} (1x1 px, 1 KB)",
                path.display()
            )
        );
        assert_eq!(std::fs::read(&path).unwrap().len(), 68);

        let v = vision_response(Some(json!(1)), shot);
        assert_eq!(v["result"]["content"].as_array().unwrap().len(), 1);

        assert!(shot_to_file("", "", &path).is_error);
        assert!(output_path("shot.jpeg").is_err());
        assert!(output_path("relative/shot.PNG").unwrap().is_absolute());
        // A path that cannot be written is an error result, not a panic,
        // and leaves no partial file behind.
        let blocked = path.join("under-a-file.png");
        assert!(shot_to_file(png, "", &blocked).is_error);
        assert_eq!(std::fs::read_dir(dir.join("nested")).unwrap().count(), 1);
        // A symlink at the path is replaced; what it pointed at is untouched.
        #[cfg(unix)]
        {
            let target = dir.join("elsewhere.txt");
            std::fs::write(&target, "keep").unwrap();
            let link = dir.join("nested/link.png");
            std::os::unix::fs::symlink(&target, &link).unwrap();
            assert!(!shot_to_file(png, "", &link).is_error);
            assert_eq!(std::fs::read_to_string(&target).unwrap(), "keep");
            assert_eq!(std::fs::read(&link).unwrap().len(), 68);
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn small_and_failed_captures_keep_their_mcp_shape() {
        let v = vision_response(
            None,
            shot_from_capture("iVBORw0KGgo=", " (3 elements marked)"),
        );
        assert_eq!(
            v,
            json!({"jsonrpc":"2.0","id":null,"result":{"content":[
                {"type":"text","text":"screenshot (3 elements marked)"},
                {"type":"image","data":"iVBORw0KGgo=","mimeType":"image/png"}
            ]}})
        );
        let v = vision_response(None, shot_from_capture("", ""));
        assert_eq!(
            v,
            json!({"jsonrpc":"2.0","id":null,"result":{"content":[
                {"type":"text","text":"Screenshot returned no data."}
            ],"isError":true}})
        );
    }
}
