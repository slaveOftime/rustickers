use gpui::{AppContext, IntoElement, Render, Rgba};
use gpui::{Context, Entity, Window};
use gpui_wry::WebView;
use std::fs;
use wry::http::Response;

pub struct SimpleWebView {
    webview: Entity<WebView>,
}

impl SimpleWebView {
    pub fn new(source: &str, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let webview = cx.new(|cx| {
            let mut builder = wry::WebViewBuilder::new()
                .with_user_agent("Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/120.0.0.0 Safari/537.36")
                .with_transparent(true).with_new_window_req_handler(|url, _| {
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
                    let mimetype = if file_path.ends_with(".html") || file_path.ends_with(".htm") { "text/html" } 
                        else if file_path.ends_with(".js") { "text/javascript" } 
                        else if file_path.ends_with(".pdf") { "application/pdf" } 
                        else if file_path.ends_with(".mp4") { "video/mp4" }
                        else if file_path.ends_with(".mkv") { "video/x-matroska" }
                        else if file_path.ends_with(".avi") { "video/x-msvideo" }
                        else if file_path.ends_with(".mov") { "video/quicktime" }
                        else if file_path.ends_with(".wmv") { "video/x-ms-wmv" }
                        else if file_path.ends_with(".flv") { "video/x-flv" }
                        else if file_path.ends_with(".mpg") { "video/mpeg" }
                        else if file_path.ends_with(".3gp") { "video/3gpp" }
                        else if file_path.ends_with(".webm") { "video/webm" }
                        else if file_path.ends_with(".mpeg") { "video/mpeg" }
                        else { "application/octet-stream" };

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

            let webview = WebView::new(builder.build_as_child(window).unwrap(), window, cx);
            let _ =  webview.focus_parent();
            webview
        });

        Self { webview }
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
