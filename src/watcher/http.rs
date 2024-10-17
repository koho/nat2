use crate::config::Metadata;
use crate::watcher::{format_value, Watcher};
use async_trait::async_trait;
use std::collections::HashMap;
use std::str::FromStr;
use stun::xoraddr::XorMappedAddress;
use anyhow::Result;
use reqwest::header::HeaderMap;
use reqwest::Method;
use url::Url;

pub struct Http {
    url: Url,
    method: Method,
    body: Option<String>,
    headers: HeaderMap,
}

impl Http {
    pub fn new(url: String, method: &str, body: Option<String>, headers: HashMap<String, String>) -> Result<Self> {
        let url = Url::parse(url.as_str())?;
        let method = Method::from_str(method)?;
        let headers = HeaderMap::try_from(&headers)?;
        Ok(Self {
            url,
            method,
            body,
            headers,
        })
    }
}

#[async_trait]
impl Watcher for Http {
    fn name(&self) -> &'static str {
        "http"
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
        let mut req = client.request(self.method.clone(), url).headers(self.headers.clone());
        if let Some(body) = body {
            req = req.body(format_value(&body, addr));
        }
        req.send().await?.error_for_status()?;
        Ok(())
    }

    fn validate(&self, _: &Metadata) -> Result<()> {
        Ok(())
    }
}
