use crate::config::Metadata;
use crate::watcher::{format_value, Watcher};
use anyhow::Result;
use async_trait::async_trait;
use reqwest::header::HeaderMap;
use reqwest::Method;
use std::collections::HashMap;
use std::str::FromStr;
use stun::xoraddr::XorMappedAddress;
use tracing::debug;
use url::Url;

/// HTTP API.
pub struct Http {
    /// Instance name.
    name: String,
    /// Request url could contain placeholder `{ip}` and `{port}` which
    /// will be replaced with real value before sending the request.
    url: Url,
    /// Request method.
    method: Method,
    /// Request body could be JSON string, plain text, etc...
    /// Placeholder `{ip}` and `{port}` are supported.
    /// Note that this value could be overridden by watcher metadata.
    body: Option<String>,
    /// Request headers.
    headers: HeaderMap,
}

impl Http {
    pub fn new(
        name: String,
        url: String,
        method: &str,
        body: Option<String>,
        headers: HashMap<String, String>,
    ) -> Result<Self> {
        let url = Url::parse(url.as_str())?;
        let method = Method::from_str(method)?;
        let headers = HeaderMap::try_from(&headers)?;
        Ok(Self {
            name,
            url,
            method,
            body,
            headers,
        })
    }
}

#[async_trait]
impl Watcher for Http {
    fn kind(&self) -> &'static str {
        "http"
    }

    fn name(&self) -> &str {
        self.name.as_str()
    }

    async fn new_address(&self, addr: &XorMappedAddress, md: &Metadata) -> Result<()> {
        let client = reqwest::Client::new();
        let mut body = Some(md.value.clone());
        if md.value.is_empty() {
            body = self.body.clone();
        }
        let mut url = self.url.clone();
        if let Some(query) = url.query() {
            url.set_query(Some(format_value(&query.to_string(), addr).as_str()));
        }
        let mut req = client
            .request(self.method.clone(), url)
            .headers(self.headers.clone());
        if let Some(body) = body {
            req = req.body(format_value(&body, addr));
        }
        let resp = req.send().await?.error_for_status()?;
        debug!(
            code = resp.status().as_str(),
            name = self.name(),
            "request completed successfully"
        );
        Ok(())
    }

    fn validate(&self, _: &Metadata) -> Result<()> {
        Ok(())
    }
}
