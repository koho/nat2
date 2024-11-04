use crate::config::Metadata;
use crate::watcher::{dns, format_value, Watcher};
use anyhow::{anyhow, Result};
use async_trait::async_trait;
use reqwest::header::HeaderMap;
use serde::{Deserialize, Serialize};
use std::fmt::Debug;
use stun::xoraddr::XorMappedAddress;
use tracing::debug;

/// Base request url.
const URL: &str = "https://api.cloudflare.com/client/v4/zones";

/// All supported DNS record types.
const TYPES: [&str; 9] = [
    "A", "AAAA", "CNAME", "HTTPS", "MX", "SRV", "SVCB", "TXT", "URI",
];

/// [Cloudflare](https://www.cloudflare.com).
pub struct Cloudflare {
    /// Instance name.
    name: String,
    /// API token.
    /// See <https://developers.cloudflare.com/fundamentals/api/get-started/create-token>.
    token: String,
}

#[derive(Deserialize)]
struct Response<T> {
    errors: Vec<Error>,
    success: bool,
    result: Option<T>,
}

#[derive(Deserialize)]
struct Error {
    code: i64,
    message: String,
}

impl<T> Response<T> {
    fn success(&self) -> Result<()> {
        if self.success {
            Ok(())
        } else {
            Err(if let Some(err) = self.errors.get(0) {
                anyhow!("error {}: {}", err.code, err.message)
            } else {
                anyhow!("unknown error")
            })
        }
    }
}

#[derive(Deserialize)]
struct Id {
    id: String,
}

#[derive(Serialize, Debug)]
struct Record {
    name: String,
    proxied: bool,
    #[serde(rename = "type")]
    record_type: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    priority: Option<u16>,
    #[serde(skip_serializing_if = "Option::is_none")]
    ttl: Option<u32>,
}

#[derive(Serialize, Debug)]
struct PlainRecord {
    #[serde(flatten)]
    base: Record,
    content: String,
}

#[derive(Serialize, Debug)]
struct CustomRecord<T> {
    #[serde(flatten)]
    base: Record,
    data: T,
}

#[derive(Serialize, Debug)]
struct SVCB {
    priority: u16,
    target: String,
    value: String,
}

#[derive(Serialize, Debug)]
struct SRV {
    port: u16,
    priority: u16,
    target: String,
    weight: u16,
}

#[derive(Serialize, Debug)]
struct URI {
    target: String,
    weight: u16,
}

impl Cloudflare {
    pub fn new(name: String, token: String) -> Self {
        Self { name, token }
    }

    fn headers(&self, json: bool) -> HeaderMap {
        let mut headers = HeaderMap::new();
        if json {
            headers.insert(
                "Content-Type",
                "application/json; charset=utf-8".parse().unwrap(),
            );
        }
        headers.insert(
            "Authorization",
            format!("Bearer {}", self.token).parse().unwrap(),
        );
        headers
    }

    /// Returns the first zone id that matches the given domain name.
    /// See <https://developers.cloudflare.com/api/operations/zones-get>.
    async fn get_zone_id(&self, domain: &String) -> Result<String> {
        let client = reqwest::Client::new();
        let resp: Response<Vec<Id>> = client
            .get(URL)
            .headers(self.headers(false))
            .query(&[("name", domain)])
            .send()
            .await?
            .json()
            .await?;
        resp.success()?;
        if let Some(zones) = resp.result {
            if let Some(zone) = zones.get(0) {
                return Ok(zone.id.to_owned());
            }
        }
        Err(anyhow!("{domain} is not found in your account"))
    }

    /// Returns the first record id that matches the given domain and record type.
    /// See <https://developers.cloudflare.com/api/operations/dns-records-for-a-zone-list-dns-records>.
    async fn get_record_id(
        &self,
        zone_id: &String,
        domain: &String,
        record_type: &String,
    ) -> Result<Option<String>> {
        let client = reqwest::Client::new();
        let resp: Response<Vec<Id>> = client
            .get(format!("{URL}/{zone_id}/dns_records"))
            .headers(self.headers(false))
            .query(&[("name", domain), ("type", record_type)])
            .send()
            .await?
            .json()
            .await?;
        resp.success()?;
        Ok(if let Some(list) = resp.result {
            list.get(0).map(|v| v.id.to_owned())
        } else {
            None
        })
    }

    /// Create a new DNS record for a zone.
    /// See <https://developers.cloudflare.com/api/operations/dns-records-for-a-zone-create-dns-record>.
    async fn create_record<T: Serialize + Debug>(
        &self,
        zone_id: String,
        record: T,
    ) -> Result<String> {
        debug!(zone_id, name = self.name(), "create {:?}", record);
        let client = reqwest::Client::new();
        let bytes = serde_json::to_vec(&record)?;
        let resp: Response<Id> = client
            .post(format!("{URL}/{zone_id}/dns_records"))
            .headers(self.headers(true))
            .body(bytes)
            .send()
            .await?
            .json()
            .await?;
        resp.success()?;
        let record_id = resp
            .result
            .ok_or(anyhow!("record id not found in response"))?
            .id;
        debug!(
            zone_id,
            record_id,
            name = self.name(),
            "create record succeed"
        );
        Ok(record_id)
    }

    /// Update an existing DNS record.
    /// See <https://developers.cloudflare.com/api/operations/dns-records-for-a-zone-patch-dns-record>.
    async fn update_record<T: Serialize + Debug>(
        &self,
        zone_id: String,
        record_id: String,
        record: T,
    ) -> Result<String> {
        debug!(
            zone_id,
            record_id,
            name = self.name(),
            "update {:?}",
            record
        );
        let client = reqwest::Client::new();
        let bytes = serde_json::to_vec(&record)?;
        let resp: Response<Id> = client
            .patch(format!("{URL}/{zone_id}/dns_records/{record_id}"))
            .headers(self.headers(true))
            .body(bytes)
            .send()
            .await?
            .json()
            .await?;
        resp.success()?;
        let record_id = resp
            .result
            .ok_or(anyhow!("record id not found in response"))?
            .id;
        debug!(
            zone_id,
            record_id,
            name = self.name(),
            "update record succeed"
        );
        Ok(record_id)
    }
}

/// Send different DNS records depending on the record type.
macro_rules! send_record {
    ($self:expr, $zone_id:expr, $record_id:expr, $kind:expr, {$($s:pat => $record:expr),*}) => {
        match $kind {
            $(
                $s => {
                    let record = $record;
                    if let Some(rid) = $record_id {
                        $self.update_record($zone_id, rid, record).await?;
                    } else {
                        $self.create_record($zone_id, record).await?;
                    }
                }
            )*
        }
    };
}

#[async_trait]
impl Watcher for Cloudflare {
    fn kind(&self) -> &'static str {
        "cf"
    }

    fn name(&self) -> &str {
        self.name.as_str()
    }

    async fn new_address(&self, addr: &XorMappedAddress, md: &Metadata) -> Result<()> {
        let domain = md.domain.as_ref().unwrap();
        let record_type = md.kind.clone().unwrap();
        let (domain_name, subdomain) = dns::split_domain_name(domain).unwrap();
        let zone_id = self.get_zone_id(&domain_name).await?;
        let mut record_id: Option<String> = md.rid.clone();
        if record_id.is_none() {
            record_id = self.get_record_id(&zone_id, domain, &record_type).await?;
        }
        let base = Record {
            name: dns::subdomain(subdomain),
            proxied: md.proxied.unwrap_or(false),
            record_type,
            priority: md.priority,
            ttl: md.ttl,
        };
        let value = format_value(&md.value, addr);
        send_record!(self, zone_id, record_id, base.record_type.to_uppercase().as_str(), {
            "HTTPS" | "SVCB" => CustomRecord {
                base,
                data: SVCB::try_from((md.priority.unwrap(), value))?,
            },
            "SRV" => CustomRecord {
                base,
                data: SRV::try_from(value)?,
            },
            "URI" => CustomRecord {
                base,
                data: URI {
                    target: value,
                    weight: 0,
                },
            },
            _ => PlainRecord {
                base,
                content: value,
            }
        });
        Ok(())
    }

    fn validate(&self, md: &Metadata) -> Result<()> {
        dns::validate(md)?;
        let record_type = md.kind.as_ref().unwrap().to_uppercase();
        if !TYPES.contains(&record_type.as_str()) {
            return Err(anyhow!("unsupported record type `{record_type}`"));
        }
        if record_type == "URI" && md.priority.is_none() {
            return Err(anyhow!("missing field `priority`"));
        }
        let example_addr = XorMappedAddress {
            ip: "1.1.1.1".parse()?,
            port: 1111,
        };
        let example_value = format_value(&md.value, &example_addr);
        match record_type.as_str() {
            "SRV" => {
                SRV::try_from(example_value)?;
            }
            "HTTPS" | "SVCB" => {
                SVCB::try_from((0, example_value))?;
            }
            _ => {}
        }
        Ok(())
    }
}

/// Value format: `priority weight port target`.
/// For example: `0 5 5060 www.example.com`.
impl TryFrom<String> for SRV {
    type Error = anyhow::Error;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        let labels: Vec<&str> = value.split(" ").filter(|v| v.len() > 0).collect();
        if labels.len() != 4 {
            return Err(anyhow!(
                "invalid value format (e.g. `priority weight port target`)"
            ));
        }
        let priority: u16 = labels[0].parse()?;
        let weight: u16 = labels[1].parse()?;
        let port: u16 = labels[2].parse()?;
        Ok(SRV {
            port,
            priority,
            target: labels[3].to_string(),
            weight,
        })
    }
}

/// Value format: `target key-value-pairs`.
/// For example: `www.example.com alpn="h2" ipv4hint="XX.XX.XX.XX" port="443"`.
impl TryFrom<(u16, String)> for SVCB {
    type Error = anyhow::Error;

    fn try_from(value: (u16, String)) -> Result<Self, Self::Error> {
        let priority = value.0;
        let value = value.1.trim();
        let (target, pairs) = value.split_once(" ").ok_or(anyhow!(
            "invalid value format (e.g. `target key-value-pairs`)"
        ))?;
        Ok(SVCB {
            priority,
            target: target.to_string(),
            value: pairs.trim().to_string(),
        })
    }
}
