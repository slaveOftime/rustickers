use gpui::{AppContext, IntoElement, Render};
use gpui::{Context, Entity, Window};
use gpui_component::webview::WebView;
use gpui_component::wry::raw_window_handle;
use gpui_component::wry::{self};
use raw_window_handle::HasWindowHandle;

pub struct SimpleWebView {
    webview: Entity<WebView>,
}

impl SimpleWebView {
    pub fn new(source: &str, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let webview = cx.new(|cx| {
            let window_handle = window.window_handle().expect("No window handle");
            let mut builder = wry::WebViewBuilder::new()
                .with_user_agent("Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/120.0.0.0 Safari/537.36")
                .with_transparent(true);
            
            builder = if crate::utils::url::is_url(source) {
                println!("Loading URL in webview: {}", source);
                builder.with_url(source)
            } else {
                println!("Loading HTML in webview."); 
                builder.with_html(source)
            };

            WebView::new(builder.build_as_child(&window_handle).unwrap(), window, cx)
        });

        Self { webview }
    }
}

impl Render for SimpleWebView {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        self.webview.clone()
    }
}
