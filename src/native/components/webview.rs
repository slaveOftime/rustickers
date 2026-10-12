use gpui::{AppContext, IntoElement, Render, Rgba};
use gpui::{Context, Entity, Window};
use gpui_wry::WebView;
use std::fs;
use wry::http::Response;

pub struct SimpleWebView {
    webview: Entity<WebView>,
}

impl SimpleWebView {
    /// Construct a `SimpleWebView` from either an HTTP(S) URL or inline HTML.
    /// Returns an error if the underlying wry backend rejects the
    /// construction (for example on platforms where the WebView runtime
    /// is unavailable, or when a sandboxed environment denies
    /// window-creation). Callers must handle the error path -- the
    /// previous infallible constructor panicked on failure, taking the
    /// whole window process down with it.
    ///
    /// The \`cx\` parameter accepts any GPUI app context; both
    /// \`&mut App\` and \`&mut Context<T>\` satisfy \`AppContext\`. The
    /// caller does not have to be inside an existing
    /// \`Entity<SimpleWebView>\`; this function allocates both the
    /// outer entity and the inner \`gpui_wry::WebView\` entity itself.
    pub fn try_new(
        source: &str,
        window: &mut Window,
        cx: &mut impl AppContext,
    ) -> Result<Entity<Self>, String> {
        let mut builder =
            wry::WebViewBuilder::new()
                .with_user_agent(
                    "Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/120.0.0.0 Safari/537.36",
                )
                .with_transparent(true)
                .with_new_window_req_handler(|url, _| {
                    tracing::info!(%url, "Opening URL in external browser");
                    if let Err(e) = webbrowser::open(&url) {
                        tracing::error!(%e, "Failed to open URL in browser");
                    }
                    wry::NewWindowResponse::Deny
                })
                .with_custom_protocol("local".into(), move |_, request| {
                    let mut file_path = request.uri().to_string();

                    file_path = file_path.replacen("local://localhost///?", "", 1);
                    file_path = file_path.replacen("local://localhost/", "", 1);

                    #[cfg(target_os = "windows")]
                    {
                        if file_path.starts_with('\\') {
                            file_path = file_path[1..].to_string();
                        }
                    }

                    tracing::debug!("Request file: {}", file_path);

                    // decode url
                    let file_path = match urlencoding::decode(&file_path) {
                        Ok(decoded) => decoded.into_owned(),
                        Err(e) => {
                            tracing::error!("Failed to decode: {:?}", e);
                            return Response::builder().status(400).body(Vec::new().into()).unwrap();
                        }
                    };

                    // 3. Read file
                    let content = match fs::read(&file_path) {
                        Ok(data) => data,
                        Err(e) => {
                            tracing::error!("Failed to read {}: {:?}", file_path, e);
                            return Response::builder().status(404).body(Vec::new().into()).unwrap();
                        }
                    };

                    // 4. Mime types (Same as before...)
                    let mimetype = if file_path.ends_with(".html") || file_path.ends_with(".htm") {
                        "text/html"
                    } else if file_path.ends_with(".js") {
                        "text/javascript"
                    } else if file_path.ends_with(".pdf") {
                        "application/pdf"
                    } else if file_path.ends_with(".mp4") {
                        "video/mp4"
                    } else if file_path.ends_with(".mkv") {
                        "video/x-matroska"
                    } else if file_path.ends_with(".avi") {
                        "video/x-msvideo"
                    } else if file_path.ends_with(".mov") {
                        "video/quicktime"
                    } else if file_path.ends_with(".wmv") {
                        "video/x-ms-wmv"
                    } else if file_path.ends_with(".flv") {
                        "video/x-flv"
                    } else if file_path.ends_with(".mpg") {
                        "video/mpeg"
                    } else if file_path.ends_with(".3gp") {
                        "video/3gpp"
                    } else if file_path.ends_with(".webm") {
                        "video/webm"
                    } else if file_path.ends_with(".mpeg") {
                        "video/mpeg"
                    } else {
                        "application/octet-stream"
                    };

                    Response::builder()
                        .header("Content-Type", mimetype)
                        .header("Access-Control-Allow-Origin", "*")
                        .body(content.into())
                        .unwrap()
                });

        builder = if crate::utils::url::is_url(source) {
            tracing::debug!(url = %source, "Loading URL in webview");
            builder.with_url(source)
        } else {
            tracing::debug!("Loading HTML in webview");
            builder.with_html(source)
        };

        // Build outside `cx.new` so a wry error can be returned through
        // the fallible signature instead of unwrapping in the entity
        // closure (which would panic the whole app). Both entities --
        // the outer \`Entity<SimpleWebView>\` and the inner
        // \`Entity<gpui_wry::WebView>\` -- are then created with the
        // already-trained builder.
        let inline_webview = builder
            .build_as_child(window)
            .map_err(|err| format!("Could not construct WebView for `{source}`: {err}"))?;

        let entity = cx.new(|cx| Self {
            webview: cx.new(|cx| {
                let wrapper = WebView::new(inline_webview, window, cx);
                // focus_parent returns a wry Err when the parent handle
                // no longer resolves; the original code ignored it.
                // Preserved: a transient focus failure must not abort
                // construction.
                let _ = wrapper.focus_parent();
                wrapper
            }),
        });

        Ok(entity)
    }

    pub fn reload(&self, cx: &mut Context<Self>) {
        let _ = self.webview.read(cx).reload();
    }

    pub fn set_bg(&mut self, color: Rgba, cx: &mut Context<Self>) {
        let color = (
            (color.r * 255.0) as u8,
            (color.g * 255.0) as u8,
            (color.b * 255.0) as u8,
            0,
        );
        let _ = self
            .webview
            .update(cx, |this, _| this.set_background_color(color));
    }
}

impl Render for SimpleWebView {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        self.webview.clone()
    }
}
