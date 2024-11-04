use crate::config::Metadata;
use crate::watcher::{dns, format_value, Watcher};
use anyhow::{anyhow, Result};
use async_trait::async_trait;
use hex::ToHex;
use reqwest::header::HeaderMap;
use ring::{digest, hmac, rand};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use stun::xoraddr::XorMappedAddress;
use time::format_description::well_known::{iso8601, Iso8601};
use time::OffsetDateTime;
use tracing::debug;
use url::ParseError::EmptyHost;
use url::Url;

/// [AliDNS](https://www.alidns.com).
pub struct AliDns {
    /// Instance name.
    name: String,
    /// Request url.
    url: String,
    /// HTTP host header.
    host: String,
    /// Similar to username.
    secret_id: String,
    /// Similar to password.
    secret_key: String,
}

#[derive(Deserialize)]
struct BaseResponse {
    #[serde(rename = "RequestId")]
    request_id: String,
    #[serde(rename = "Code")]
    code: Option<String>,
    #[serde(rename = "Message")]
    message: Option<String>,
}

#[derive(Serialize)]
struct DescribeSubDomainRecordsRequest {
    #[serde(rename = "SubDomain")]
    subdomain: String,
    #[serde(rename = "Type")]
    record_type: String,
}

#[derive(Deserialize)]
struct DescribeSubDomainRecordsResponse {
    #[serde(flatten)]
    common: BaseResponse,
    #[serde(rename = "DomainRecords")]
    domain_records: Option<DomainRecords>,
}

#[derive(Deserialize)]
struct DomainRecords {
    #[serde(rename = "Record")]
    record: Vec<RecordId>,
}

#[derive(Deserialize)]
struct RecordId {
    #[serde(rename = "RecordId")]
    record_id: String,
}

#[derive(Serialize, Debug)]
struct Record {
    #[serde(rename = "DomainName")]
    domain_name: String,
    #[serde(rename = "RR")]
    rr: String,
    #[serde(rename = "Type")]
    record_type: String,
    #[serde(rename = "Value")]
    value: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(rename = "Priority")]
    priority: Option<u16>,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(rename = "TTL")]
    ttl: Option<u32>,
}

#[derive(Deserialize)]
struct RecordResponse {
    #[serde(flatten)]
    common: BaseResponse,
    #[serde(rename = "RecordId")]
    record_id: Option<String>,
}

#[derive(Serialize)]
struct UpdateDomainRecordRequest {
    #[serde(rename = "RecordId")]
    record_id: String,
    #[serde(flatten)]
    record: Record,
}

type AddDomainRecordRequest = Record;

impl BaseResponse {
    fn success(&self) -> Result<()> {
        self.code
            .as_ref()
            .map(|v| {
                Err::<(), anyhow::Error>(anyhow!(
                    "{}: {}",
                    v,
                    self.message
                        .as_ref()
                        .unwrap_or(&"Please refer to the API documentation.".to_string())
                ))
            })
            .transpose()?;
        Ok(())
    }
}

macro_rules! http_host {
    ($val:expr $(,)?) => {{
        let url = Url::parse($val)?;
        let mut host = url.host().ok_or(EmptyHost)?.to_string();
        host.push_str(
            &url.port()
                .map_or(String::new(), |v| format!(":{}", v.to_string())),
        );
        host
    }};
}

impl AliDns {
    pub fn new(
        name: String,
        secret_id: String,
        secret_key: String,
        url: Option<String>,
    ) -> Result<Self> {
        let url = url.unwrap_or("https://dns.aliyuncs.com".to_string());
        let host = http_host!(url.as_str());
        Ok(Self {
            name,
            url,
            host,
            secret_id,
            secret_key,
        })
    }

    /// Signature v3.
    /// See <https://help.aliyun.com/zh/sdk/product-overview/v3-request-structure-and-signature>.
    fn sign(&self, url: &mut Url, headers: &HeaderMap) -> String {
        let mut query: Vec<(String, String)> = url
            .query_pairs()
            .map(|(key, value)| (key.into_owned(), value.into_owned()))
            .collect();
        query.sort();
        url.query_pairs_mut().clear().extend_pairs(query);
        let mut acs_header_map = HashMap::new();
        let mut acs_names = Vec::new();
        for key in headers.keys() {
            let lowercase_key = key.as_str().to_lowercase();
            if lowercase_key.starts_with("x-acs-") {
                let mut values: Vec<String> = headers
                    .get_all(key)
                    .iter()
                    .map(|v| v.to_str().unwrap().trim().to_string())
                    .collect();
                values.sort();
                acs_header_map.insert(lowercase_key.clone(), values.join(","));
                acs_names.push(lowercase_key);
            }
        }
        acs_names.sort();
        let signed_acs_headers = acs_names.join(";");
        let canonical_acs_headers: Vec<String> = acs_names
            .iter()
            .map(|v| format!("{v}:{}", acs_header_map.get(v).unwrap()))
            .collect();
        let hashed_request_payload = acs_header_map
            .get(&"x-acs-content-sha256".to_string())
            .unwrap();
        let canonical_request = format!(
            "POST\n/\n{}\nhost:{}\n{}\n\nhost;{}\n{}",
            url.query().unwrap_or("").replace("+", "%20"),
            self.host,
            canonical_acs_headers.join("\n"),
            signed_acs_headers,
            hashed_request_payload,
        );
        let string_to_sign = format!(
            "ACS3-HMAC-SHA256\n{}",
            digest::digest(&digest::SHA256, canonical_request.as_ref())
                .as_ref()
                .encode_hex::<String>()
        );
        let signature = hmac::sign(
            &hmac::Key::new(hmac::HMAC_SHA256, self.secret_key.as_ref()),
            string_to_sign.as_ref(),
        );
        format!(
            "ACS3-HMAC-SHA256 Credential={}, SignedHeaders=host;{}, Signature={}",
            self.secret_id,
            signed_acs_headers,
            signature.encode_hex::<String>()
        )
    }

    fn headers(&self, action: &str, url: &mut Url) -> Result<HeaderMap> {
        let mut headers = HeaderMap::new();
        headers.insert("x-acs-action", action.parse()?);
        headers.insert("x-acs-version", "2015-01-09".parse()?);
        const FORMAT: iso8601::EncodedConfig = iso8601::Config::DEFAULT
            .set_time_precision(iso8601::TimePrecision::Second {
                decimal_digits: None,
            })
            .encode();
        headers.insert(
            "x-acs-date",
            OffsetDateTime::now_utc()
                .format(&Iso8601::<FORMAT>)?
                .parse()?,
        );
        headers.insert(
            "x-acs-content-sha256",
            digest::digest(&digest::SHA256, b"")
                .as_ref()
                .encode_hex::<String>()
                .parse()?,
        );
        let rng = rand::SystemRandom::new();
        let nonce: [u8; 16] = rand::generate(&rng)?.expose();
        headers.insert(
            "x-acs-signature-nonce",
            nonce.encode_hex::<String>().parse()?,
        );
        headers.insert("Authorization", self.sign(url, &headers).parse()?);
        Ok(headers)
    }

    /// Returns the first record id that matches the given domain and record type.
    /// See <https://help.aliyun.com/zh/dns/api-alidns-2015-01-09-describesubdomainrecords>.
    async fn get_record_id(&self, domain: String, record_type: String) -> Result<Option<String>> {
        let client = reqwest::Client::new();
        let payload = DescribeSubDomainRecordsRequest {
            subdomain: domain,
            record_type,
        };
        let mut req = client.post(&self.url).query(&payload).build()?;
        let resp: DescribeSubDomainRecordsResponse = client
            .post(req.url().to_owned())
            .headers(self.headers("DescribeSubDomainRecords", req.url_mut())?)
            .send()
            .await?
            .json()
            .await?;
        resp.common.success()?;
        Ok(if let Some(list) = resp.domain_records {
            list.record.get(0).map(|v| v.record_id.to_owned())
        } else {
            None
        })
    }

    /// Create a new record.
    /// See <https://help.aliyun.com/zh/dns/api-alidns-2015-01-09-adddomainrecord>.
    async fn create_record(&self, record: Record) -> Result<String> {
        debug!(name = self.name(), "create {:?}", record);
        let client = reqwest::Client::new();
        let payload: AddDomainRecordRequest = record;
        let mut req = client.post(&self.url).query(&payload).build()?;
        let resp: RecordResponse = client
            .post(req.url().to_owned())
            .headers(self.headers("AddDomainRecord", req.url_mut())?)
            .send()
            .await?
            .json()
            .await?;
        resp.common.success()?;
        let record_id = resp
            .record_id
            .ok_or(anyhow!("record id not found in response"))?;
        debug!(
            request_id = resp.common.request_id,
            record_id,
            name = self.name(),
            "create record succeed"
        );
        Ok(record_id)
    }

    /// Update the record with a specific id.
    /// See <https://help.aliyun.com/zh/dns/api-alidns-2015-01-09-updatedomainrecord>.
    async fn update_record(&self, record_id: String, record: Record) -> Result<String> {
        debug!(record_id, name = self.name(), "update {:?}", record);
        let client = reqwest::Client::new();
        let payload = UpdateDomainRecordRequest { record_id, record };
        let mut req = client.post(&self.url).query(&payload).build()?;
        let resp: RecordResponse = client
            .post(req.url().to_owned())
            .headers(self.headers("UpdateDomainRecord", req.url_mut())?)
            .send()
            .await?
            .json()
            .await?;
        resp.common.success()?;
        let record_id = resp
            .record_id
            .ok_or(anyhow!("record id not found in response"))?;
        debug!(
            request_id = resp.common.request_id,
            record_id,
            name = self.name(),
            "update record succeed"
        );
        Ok(record_id)
    }
}

#[async_trait]
impl Watcher for AliDns {
    fn kind(&self) -> &'static str {
        "alidns"
    }

    fn name(&self) -> &str {
        self.name.as_str()
    }

    async fn new_address(&self, addr: &XorMappedAddress, md: &Metadata) -> Result<()> {
        let domain = md.domain.as_ref().unwrap();
        let record_type = md.kind.clone().unwrap();
        let (domain_name, subdomain) = dns::split_domain_name(domain).unwrap();
        let mut record_id: Option<String> = md.rid.clone();
        if record_id.is_none() {
            record_id = self
                .get_record_id(domain.to_owned(), record_type.clone())
                .await?;
        }
        let record = Record {
            domain_name,
            rr: dns::subdomain(subdomain),
            record_type,
            value: format_value(&md.value, addr),
            priority: md.priority,
            ttl: md.ttl,
        };
        if let Some(rid) = record_id {
            self.update_record(rid, record).await?;
        } else {
            self.create_record(record).await?;
        }
        Ok(())
    }

    fn validate(&self, md: &Metadata) -> Result<()> {
        dns::validate(md)
    }
}
