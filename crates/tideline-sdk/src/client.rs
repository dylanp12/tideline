//! The TLR/1 client.

use crate::error::{Error, Result};
use crate::run::Run;
use bytes::Bytes;
use serde::de::DeserializeOwned;
use serde::Serialize;
use std::collections::BTreeMap;
use tideline_proto::{Agent, RunEnvelope};

#[derive(Clone)]
pub struct Tideline {
    pub(crate) base: String,
    pub(crate) key: Option<String>,
    pub(crate) http: reqwest::Client,
}

impl Tideline {
    /// Point at a server. `key` is sent as a bearer token when present.
    pub fn new(url: &str, key: Option<&str>) -> Self {
        Tideline {
            base: url.trim_end_matches('/').to_string(),
            key: key.map(str::to_string),
            http: reqwest::Client::new(),
        }
    }

    pub fn with_http(mut self, http: reqwest::Client) -> Self {
        self.http = http;
        self
    }

    pub(crate) fn req(&self, method: reqwest::Method, path: &str) -> reqwest::RequestBuilder {
        let rb = self.http.request(method, format!("{}{}", self.base, path));
        match &self.key {
            Some(k) => rb.bearer_auth(k),
            None => rb,
        }
    }

    /// Send a request and deserialize the response.
    ///
    /// Deserialization runs on the raw bytes, never through `serde_json::Value`:
    /// a generic value reorders metadata keys, which changes the digest and
    /// fails a record that is perfectly sound (`spec/tlr-1.md` §4.2).
    pub(crate) async fn send<T: DeserializeOwned>(&self, rb: reqwest::RequestBuilder) -> Result<T> {
        let bytes = self.send_raw(rb).await?;
        serde_json::from_slice(&bytes)
            .map_err(|e| Error::Protocol(format!("unreadable response: {e}")))
    }

    pub(crate) async fn send_raw(&self, rb: reqwest::RequestBuilder) -> Result<Bytes> {
        let res = rb.send().await?;
        let code = res.status().as_u16();
        let body = res.bytes().await?;
        if !(200..300).contains(&code) {
            return Err(Error::Status {
                code,
                body: String::from_utf8_lossy(&body).into_owned(),
            });
        }
        Ok(body)
    }

    /// Open a run. The envelope becomes the first link in its chain.
    pub fn start_run(&self, run_id: &str, agent: Agent) -> StartRun<'_> {
        StartRun {
            client: self,
            body: NewRunBody {
                run_id: run_id.to_string(),
                agent,
                subject_ref: None,
                labels: BTreeMap::new(),
            },
        }
    }

    /// A handle to a run that already exists. Does not contact the server.
    pub fn run(&self, run_id: &str) -> Run {
        Run::new(self.clone(), run_id.to_string())
    }

    pub async fn list_runs(&self, query: &RunQuery) -> Result<RunPage> {
        let mut rb = self.req(reqwest::Method::GET, "/v1/runs");
        rb = rb.query(query);
        self.send(rb).await
    }

    /// Capabilities, supported protocol versions, and checkpoint signing keys.
    pub async fn well_known(&self) -> Result<serde_json::Value> {
        self.send(self.req(reqwest::Method::GET, "/v1/.well-known/tideline"))
            .await
    }
}

#[derive(Serialize)]
pub(crate) struct NewRunBody {
    pub run_id: String,
    pub agent: Agent,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub subject_ref: Option<String>,
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub labels: BTreeMap<String, String>,
}

/// Builder for `start_run`.
pub struct StartRun<'a> {
    client: &'a Tideline,
    body: NewRunBody,
}

impl StartRun<'_> {
    /// An opaque reference to the subject of the decision. Never personal data:
    /// it is not redactable and it appears in list queries.
    pub fn subject_ref(mut self, r: &str) -> Self {
        self.body.subject_ref = Some(r.to_string());
        self
    }

    pub fn label(mut self, key: &str, value: &str) -> Self {
        self.body.labels.insert(key.to_string(), value.to_string());
        self
    }

    pub async fn send(self) -> Result<Run> {
        let rb = self
            .client
            .req(reqwest::Method::POST, "/v1/runs")
            .json(&self.body);
        let env: RunEnvelope = self.client.send(rb).await?;
        Ok(Run::new(self.client.clone(), env.run_id))
    }
}

#[derive(Debug, Default, Serialize)]
pub struct RunQuery {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub agent: Option<String>,
    /// `key=value`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub since: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cursor: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub limit: Option<u32>,
}

#[derive(Debug, serde::Deserialize)]
pub struct RunPage {
    pub runs: Vec<RunEnvelope>,
    #[serde(default)]
    pub next_cursor: Option<String>,
}
