use gpui::{AppContext, IntoElement, Render};
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

            WebView::new(builder.build_as_child(window).unwrap(), window, cx)
        });

        Self { webview }
    }

    pub fn reload(&self, cx: &mut Context<Self>) {
        self.webview.read(cx).reload();
    }
}

impl Render for SimpleWebView {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        self.webview.clone()
    }
}
