//! Resumable byte ranges with bounded recovery and URL-free errors.
use bytes::Bytes;
use futures_util::{Stream, StreamExt};
use reqwest::header::{ACCEPT_ENCODING, CONTENT_ENCODING, CONTENT_RANGE, RANGE};
use std::{pin::Pin, time::Duration};
use tokio::time::{Instant, sleep, timeout};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TransferError {
    pub stage: &'static str,
    pub status: u16,
}
impl TransferError {
    pub const fn new(stage: &'static str, status: u16) -> Self {
        Self { stage, status }
    }
    pub fn diagnostic(self) -> String {
        format!(
            "NEXUS_TRANSFER_DIAGNOSTIC_V1 {} {}",
            self.stage, self.status
        )
    }
}
impl std::fmt::Display for TransferError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if self.status == 0 {
            write!(
                f,
                "Package transfer failed ({}). Download checkpoints are preserved.",
                self.stage
            )
        } else {
            write!(
                f,
                "Package transfer failed ({}; HTTP {}). Download checkpoints are preserved.",
                self.stage, self.status
            )
        }
    }
}
impl std::error::Error for TransferError {}

#[derive(Clone, Copy)]
pub(crate) struct Policy {
    pub timeout: Duration,
    pub budget: Duration,
    pub backoff: Duration,
    pub retries: u32,
}
impl Default for Policy {
    fn default() -> Self {
        Self {
            timeout: Duration::from_secs(30),
            budget: Duration::from_secs(180),
            backoff: Duration::from_millis(250),
            retries: 5,
        }
    }
}
type Body = Pin<Box<dyn Stream<Item = Result<Bytes, reqwest::Error>> + Send>>;
pub(crate) struct RangeReader {
    client: reqwest::Client,
    url: String,
    position: u64,
    end: u64,
    response_end: u64,
    total: Option<u64>,
    body: Option<Body>,
    pending: Bytes,
    policy: Policy,
    failures: u32,
    idle_spent: Duration,
}
impl RangeReader {
    pub fn new(client: &reqwest::Client, url: &str, start: u64, end: u64, policy: Policy) -> Self {
        Self {
            client: client.clone(),
            url: url.to_owned(),
            position: start,
            end,
            response_end: end,
            total: None,
            body: None,
            pending: Bytes::new(),
            policy,
            failures: 0,
            idle_spent: Duration::ZERO,
        }
    }
    pub(crate) fn from_response(
        client: &reqwest::Client,
        url: &str,
        start: u64,
        len: u64,
        response: reqwest::Response,
        policy: Policy,
    ) -> Self {
        let mut reader = Self::new(client, url, start, len.saturating_sub(1), policy);
        reader.total = Some(len);
        reader.body = Some(Box::pin(response.bytes_stream()));
        reader
    }
    pub fn disconnect(&mut self) {
        self.body = None;
        self.pending = Bytes::new();
        self.failures = 0;
        self.idle_spent = Duration::ZERO;
    }
    fn remaining(&self) -> Duration {
        self.policy.budget.saturating_sub(self.idle_spent)
    }
    async fn retry(&mut self, error: TransferError) -> Result<(), TransferError> {
        // Never poll an exhausted/broken old stream after a reconnect fails.
        self.body = None;
        if self.failures >= self.policy.retries || self.remaining().is_zero() {
            return Err(error);
        }
        let delay = self
            .policy
            .backoff
            .saturating_mul(1 << self.failures.min(4));
        self.failures += 1;
        if delay >= self.remaining() {
            return Err(error);
        }
        sleep(delay).await;
        self.idle_spent += delay;
        Ok(())
    }
    pub async fn next(&mut self) -> Result<Option<Bytes>, TransferError> {
        loop {
            if self.position > self.end {
                return Ok(None);
            }
            if !self.pending.is_empty() {
                let len = self.pending.len().min(16 * 1024);
                // Intentional throttling is outside the network timeout and
                // survives control cancellation without losing buffered bytes.
                crate::bandwidth::acquire(len).await;
                let bytes = self.pending.split_to(len);
                self.position += len as u64;
                self.failures = 0;
                self.idle_spent = Duration::ZERO;
                if self.position > self.response_end {
                    self.body = None;
                }
                return Ok(Some(bytes));
            }
            let limit = self.policy.timeout.min(self.remaining());
            if limit.is_zero() {
                return Err(TransferError::new("transfer-idle-timeout", 0));
            }
            if self.body.is_none() {
                let request = self
                    .client
                    .get(&self.url)
                    .header(ACCEPT_ENCODING, "identity")
                    .header(RANGE, format!("bytes={}-{}", self.position, self.end));
                let started = Instant::now();
                let response = timeout(limit, request.send()).await;
                self.idle_spent += started.elapsed();
                let response = match response {
                    Err(_) => {
                        self.retry(TransferError::new("transfer-headers-timeout", 0))
                            .await?;
                        continue;
                    }
                    Ok(Err(error)) => {
                        let stage = if error.is_timeout() {
                            "transfer-headers-timeout"
                        } else {
                            "transfer-request"
                        };
                        self.retry(TransferError::new(stage, 0)).await?;
                        continue;
                    }
                    Ok(Ok(response)) => response,
                };
                let status = response.status().as_u16();
                if status != 206 {
                    let error = TransferError::new("transfer-http", status);
                    // 429 requires a user-visible wait, never hammer a rate-limited CDN.
                    if matches!(status, 408 | 500 | 502 | 503 | 504) {
                        self.retry(error).await?;
                        continue;
                    }
                    return Err(error);
                }
                if response
                    .headers()
                    .get(CONTENT_ENCODING)
                    .is_some_and(|v| v != "identity")
                {
                    return Err(TransferError::new("transfer-encoding", status));
                }
                let (last, total) = valid_range(&response, self.position, self.end)
                    .ok_or(TransferError::new("transfer-range", status))?;
                if self.total.is_some_and(|old| old != total) {
                    return Err(TransferError::new("transfer-range", status));
                }
                self.total = Some(total);
                self.response_end = last;
                self.body = Some(Box::pin(response.bytes_stream()));
            }
            let started = Instant::now();
            let next = timeout(
                self.policy.timeout.min(self.remaining()),
                self.body.as_mut().unwrap().next(),
            )
            .await;
            self.idle_spent += started.elapsed();
            match next {
                Ok(Some(Ok(bytes))) if !bytes.is_empty() => {
                    if bytes.len() as u64 > self.response_end - self.position + 1 {
                        return Err(TransferError::new("transfer-size", 206));
                    }
                    self.pending = bytes;
                }
                Ok(Some(Ok(_))) => {
                    self.retry(TransferError::new("transfer-eof", 0)).await?;
                }
                Ok(None) => {
                    self.retry(TransferError::new("transfer-eof", 0)).await?;
                }
                Ok(Some(Err(error))) => {
                    self.retry(TransferError::new(
                        if error.is_timeout() {
                            "transfer-idle-timeout"
                        } else {
                            "transfer-read"
                        },
                        0,
                    ))
                    .await?;
                }
                Err(_) => {
                    self.retry(TransferError::new("transfer-idle-timeout", 0))
                        .await?;
                }
            }
        }
    }
}
fn valid_range(response: &reqwest::Response, start: u64, end: u64) -> Option<(u64, u64)> {
    let raw = response
        .headers()
        .get(CONTENT_RANGE)?
        .to_str()
        .ok()?
        .strip_prefix("bytes ")?;
    let (bounds, total) = raw.split_once('/')?;
    let (first, last) = bounds.split_once('-')?;
    let (first, last, total) = (
        first.parse::<u64>().ok()?,
        last.parse::<u64>().ok()?,
        total.parse::<u64>().ok()?,
    );
    (first == start && last >= first && last <= end && total > last).then_some((last, total))
}

#[cfg(test)]
#[path = "streaming_http_tests.rs"]
mod tests;
