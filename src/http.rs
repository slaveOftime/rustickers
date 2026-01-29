// https://github.com/wilfreddenton/writ/blob/main/src/http.rs
use std::sync::Arc;

use async_compat::CompatExt;
use futures::{FutureExt, future::BoxFuture};
use gpui::http_client::{
    AsyncBody, HttpClient, Request, Response, Url,
    http::{HeaderValue, StatusCode},
};

const USER_AGENT: &str = "Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/120.0.0.0 Safari/537.36";

pub struct ReqwestClient {
    client: reqwest::Client,
    user_agent: HeaderValue,
}

impl ReqwestClient {
    pub fn new() -> Arc<Self> {
        let client = reqwest::Client::builder()
            .user_agent(USER_AGENT)
            .build()
            .unwrap_or_else(|_| reqwest::Client::new());

        Arc::new(Self {
            client,
            user_agent: HeaderValue::from_static(USER_AGENT),
        })
    }
}

impl HttpClient for ReqwestClient {
    fn user_agent(&self) -> Option<&HeaderValue> {
        Some(&self.user_agent)
    }

    fn proxy(&self) -> Option<&Url> {
        None
    }

    fn send(
        &self,
        req: Request<AsyncBody>,
    ) -> BoxFuture<'static, anyhow::Result<Response<AsyncBody>>> {
        let (parts, _body) = req.into_parts();
        let uri = parts.uri.to_string();
        let client = self.client.clone();

        async move {
            let response = client.get(&uri).send().compat().await?;
            let status = response.status().as_u16();
            let bytes = response.bytes().compat().await?;

            let async_body = AsyncBody::from_bytes(bytes);
            let mut http_response = Response::new(async_body);
            *http_response.status_mut() = StatusCode::from_u16(status).unwrap_or(StatusCode::OK);

            Ok(http_response)
        }
        .boxed()
    }
}
