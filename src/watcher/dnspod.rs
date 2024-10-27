use crate::config::Metadata;
use crate::watcher::{dns, format_value, Watcher};
use anyhow::{anyhow, Result};
use async_trait::async_trait;
use hex::ToHex;
use reqwest::header::HeaderMap;
use ring::{digest, hmac};
use serde::{Deserialize, Serialize};
use stun::xoraddr::XorMappedAddress;
use time::OffsetDateTime;
use tracing::debug;

const HOST: &str = "dnspod.tencentcloudapi.com";

/// [DNSPod](https://www.dnspod.cn).
pub struct DnsPod {
    /// Instance name.
    name: String,
    /// Request url.
    url: String,
    /// Similar to username.
    secret_id: String,
    /// Similar to password.
    secret_key: String,
}

#[derive(Serialize)]
struct DescribeRecordListRequest {
    #[serde(rename = "Domain")]
    domain: String,
    #[serde(rename = "Subdomain")]
    subdomain: String,
    #[serde(rename = "RecordType")]
    record_type: String,
}

#[derive(Serialize, Debug)]
struct Record {
    #[serde(rename = "Domain")]
    domain: String,
    #[serde(rename = "SubDomain")]
    subdomain: String,
    #[serde(rename = "RecordType")]
    record_type: String,
    #[serde(rename = "Value")]
    value: String,
    #[serde(rename = "RecordLine")]
    record_line: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(rename = "MX")]
    mx: Option<u8>,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(rename = "TTL")]
    ttl: Option<u32>,
}

type CreateRecordRequest = Record;

#[derive(Serialize)]
struct UpdateRecordRequest {
    #[serde(rename = "RecordId")]
    record_id: u64,
    #[serde(flatten)]
    record: Record,
}

#[derive(Deserialize)]
struct Response<T> {
    #[serde(rename = "Response")]
    response: T,
}

#[derive(Deserialize)]
struct BaseResponse {
    #[serde(rename = "RequestId")]
    request_id: String,
    #[serde(rename = "Error")]
    error: Option<ErrorResponse>,
}

#[derive(Deserialize)]
struct ErrorResponse {
    #[serde(rename = "Code")]
    code: String,
    #[serde(rename = "Message")]
    message: String,
}

#[derive(Deserialize)]
struct RecordResponse {
    #[serde(flatten)]
    common: BaseResponse,
    #[serde(rename = "RecordId")]
    record_id: Option<u64>,
}

#[derive(Deserialize)]
struct DescribeRecordListResponseItem {
    #[serde(rename = "RecordId")]
    record_id: u64,
}

#[derive(Deserialize)]
struct DescribeRecordListResponse {
    #[serde(flatten)]
    common: BaseResponse,
    #[serde(rename = "RecordList")]
    record_list: Option<Vec<DescribeRecordListResponseItem>>,
}

impl BaseResponse {
    fn success(&self) -> Result<()> {
        self.error
            .as_ref()
            .map(|v| Err::<(), anyhow::Error>(anyhow!("{}: {}", v.code, v.message)))
            .transpose()?;
        Ok(())
    }
}

impl DnsPod {
    pub fn new(name: String, secret_id: String, secret_key: String) -> Self {
        Self {
            name,
            url: format!("https://{HOST}"),
            secret_id,
            secret_key,
        }
    }

    /// Signature v3.
    /// See <https://cloud.tencent.com/document/api/1427/56189>.
    fn sign(&self, action: &str, payload: &[u8]) -> String {
        let canonical_request = format!(
            "POST\n/\n\n\
        content-type:application/json; charset=utf-8\nhost:{HOST}\nx-tc-action:{}\n\n\
        content-type;host;x-tc-action\n\
        {}",
            action.to_lowercase(),
            digest::digest(&digest::SHA256, payload)
                .as_ref()
                .encode_hex::<String>()
        );
        let now = OffsetDateTime::now_utc();
        let string_to_sign = format!(
            "TC3-HMAC-SHA256\n{}\n{}/dnspod/tc3_request\n{}",
            now.unix_timestamp(),
            now.date(),
            digest::digest(&digest::SHA256, canonical_request.as_ref())
                .as_ref()
                .encode_hex::<String>()
        );
        let key = hmac::Key::new(
            hmac::HMAC_SHA256,
            format!("TC3{}", self.secret_key).as_ref(),
        );
        let secret = hmac::sign(&key, now.date().to_string().as_ref());
        let secret = hmac::sign(
            &hmac::Key::new(hmac::HMAC_SHA256, secret.as_ref()),
            b"dnspod",
        );
        let secret = hmac::sign(
            &hmac::Key::new(hmac::HMAC_SHA256, secret.as_ref()),
            b"tc3_request",
        );
        let signature = hmac::sign(
            &hmac::Key::new(hmac::HMAC_SHA256, secret.as_ref()),
            string_to_sign.as_ref(),
        );
        format!("TC3-HMAC-SHA256 Credential={}/{}/dnspod/tc3_request, SignedHeaders=content-type;host;x-tc-action, Signature={}",
                self.secret_id, now.date(), signature.encode_hex::<String>())
    }

    fn headers(&self, action: &str, payload: &[u8]) -> HeaderMap {
        let mut headers = HeaderMap::new();
        headers.insert(
            "Content-Type",
            "application/json; charset=utf-8".parse().unwrap(),
        );
        headers.insert("X-TC-Version", "2021-03-23".parse().unwrap());
        headers.insert("X-TC-Action", action.parse().unwrap());
        headers.insert(
            "X-TC-Timestamp",
            OffsetDateTime::now_utc()
                .unix_timestamp()
                .to_string()
                .parse()
                .unwrap(),
        );
        headers.insert("Authorization", self.sign(action, payload).parse().unwrap());
        headers
    }

    /// Returns the first record id that matches the given domain and record type.
    /// See <https://cloud.tencent.com/document/api/1427/56166>.
    async fn get_record_id(
        &self,
        domain: String,
        subdomain: String,
        record_type: String,
    ) -> Result<Option<u64>> {
        let client = reqwest::Client::new();
        let payload = DescribeRecordListRequest {
            domain,
            subdomain: if subdomain.is_empty() {
                "@".to_string()
            } else {
                subdomain
            },
            record_type,
        };
        let bytes = serde_json::to_vec(&payload)?;
        let resp: Response<DescribeRecordListResponse> = client
            .post(&self.url)
            .headers(self.headers("DescribeRecordList", bytes.as_ref()))
            .body(bytes)
            .send()
            .await?
            .json()
            .await?;
        resp.response
            .common
            .error
            .map(|v|
            // Empty record list is OK.
            match v.code.as_str() {
                "ResourceNotFound.NoDataOfRecord" => Ok(()),
                _ => Err(anyhow!("{}: {}", v.code, v.message))
            })
            .transpose()?;
        Ok(if let Some(list) = resp.response.record_list {
            list.get(0).map(|v| v.record_id)
        } else {
            None
        })
    }

    /// Create a new record.
    /// See <https://cloud.tencent.com/document/api/1427/56180>.
    async fn create_record(&self, record: Record) -> Result<u64> {
        debug!(name = self.name(), "create {:?}", record);
        let client = reqwest::Client::new();
        let payload: CreateRecordRequest = record;
        let bytes = serde_json::to_vec(&payload)?;
        let resp: Response<RecordResponse> = client
            .post(&self.url)
            .headers(self.headers("CreateRecord", bytes.as_ref()))
            .body(bytes)
            .send()
            .await?
            .json()
            .await?;
        resp.response.common.success()?;
        let record_id = resp
            .response
            .record_id
            .ok_or(anyhow!("record id not found in response"))?;
        debug!(
            request_id = resp.response.common.request_id,
            record_id,
            name = self.name(),
            "create record succeed"
        );
        Ok(record_id)
    }

    /// Update the record with a specific id.
    /// See <https://cloud.tencent.com/document/api/1427/56157>.
    async fn update_record(&self, record_id: u64, record: Record) -> Result<u64> {
        debug!(record_id, name = self.name(), "update {:?}", record);
        let client = reqwest::Client::new();
        let payload = UpdateRecordRequest { record_id, record };
        let bytes = serde_json::to_vec(&payload)?;
        let resp: Response<RecordResponse> = client
            .post(&self.url)
            .headers(self.headers("ModifyRecord", bytes.as_ref()))
            .body(bytes)
            .send()
            .await?
            .json()
            .await?;
        resp.response.common.success()?;
        let record_id = resp
            .response
            .record_id
            .ok_or(anyhow!("record id not found in response"))?;
        debug!(
            request_id = resp.response.common.request_id,
            record_id,
            name = self.name(),
            "update record succeed"
        );
        Ok(record_id)
    }
}

#[async_trait]
impl Watcher for DnsPod {
    fn kind(&self) -> &'static str {
        "dnspod"
    }

    fn name(&self) -> &str {
        self.name.as_str()
    }

    async fn new_address(&self, addr: &XorMappedAddress, md: &Metadata) -> Result<()> {
        let domain = md.domain.as_ref().unwrap();
        let record_type = md.kind.clone().unwrap();
        let (domain, subdomain) = dns::split_domain_name(domain).unwrap();
        let mut record_id: Option<u64> = md.rid.as_ref().map(|v| v.parse().unwrap());
        if record_id.is_none() {
            record_id = self
                .get_record_id(domain.clone(), subdomain.clone(), record_type.clone())
                .await?;
        }
        let record = Record {
            domain,
            subdomain: if subdomain.is_empty() {
                "@".to_string()
            } else {
                subdomain
            },
            record_type,
            value: format_value(&md.value, addr),
            record_line: "默认",
            mx: md.priority,
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
        dns::validate(md)?;
        if let Some(rid) = &md.rid {
            rid.parse::<u64>()?;
        }
        Ok(())
    }
}
