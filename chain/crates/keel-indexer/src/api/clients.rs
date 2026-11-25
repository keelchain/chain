//! The directory of clients (exchanges): the node's list, with whether
//! each client's website is proved. A client proves a website by serving
//! its address at `https://<website>/.well-known/keel.txt`; the check is
//! this indexer's, repeated every few minutes, and is not part of
//! consensus (the name's uniqueness is).

use super::*;
use crate::types::field_str;
use axum::extract::State;
use std::collections::HashMap;
use std::net::IpAddr;
use std::sync::Mutex;
use std::time::{Duration, Instant};

/// How long a result is served before the website is fetched again.
const RECHECK: Duration = Duration::from_secs(600);
const FETCH_TIMEOUT: Duration = Duration::from_secs(8);
/// The proof file is one address; nothing past this is read.
const MAX_BODY: usize = 4096;

#[derive(Clone, Copy)]
struct Check {
    /// `None` until the first fetch has finished.
    verified: Option<bool>,
    started: Instant,
    running: bool,
}

/// Results by `address@website`.
#[derive(Default)]
pub struct WebsiteChecks(Mutex<HashMap<String, Check>>);

impl WebsiteChecks {
    /// The last result for `key`, and whether a fetch is due.
    fn poll(&self, key: &str) -> (Option<bool>, bool) {
        let mut map = self.0.lock().expect("lock");
        let now = Instant::now();
        let entry = map.entry(key.to_string()).or_insert(Check {
            verified: None,
            started: now,
            running: false,
        });
        let due = !entry.running
            && (entry.verified.is_none() || now.duration_since(entry.started) >= RECHECK);
        if due {
            entry.running = true;
            entry.started = now;
        }
        (entry.verified, due)
    }

    fn finish(&self, key: &str, verified: bool) {
        if let Some(entry) = self.0.lock().expect("lock").get_mut(key) {
            entry.verified = Some(verified);
            entry.running = false;
        }
    }
}

/// Whether `ip` is on the public internet. The website is a name a client
/// chose, so it must not lead this server to itself or its own network.
fn is_public(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => {
            let o = v4.octets();
            !(v4.is_private()
                || v4.is_loopback()
                || v4.is_link_local()
                || v4.is_unspecified()
                || v4.is_broadcast()
                || v4.is_multicast()
                || v4.is_documentation()
                || o[0] == 0
                || (o[0] == 100 && (o[1] & 0xc0) == 64)
                || (o[0] == 198 && (o[1] & 0xfe) == 18)
                || o[0] >= 240)
        }
        IpAddr::V6(v6) => {
            if let Some(v4) = v6.to_ipv4_mapped() {
                return is_public(IpAddr::V4(v4));
            }
            let first = v6.segments()[0];
            !(v6.is_loopback()
                || v6.is_unspecified()
                || v6.is_multicast()
                || (first & 0xfe00) == 0xfc00
                || (first & 0xffc0) == 0xfe80)
        }
    }
}

/// Whether the proof file names `address` (64 hex characters, any case,
/// with or without `0x`) as a whole word.
fn proves(body: &str, address: &str) -> bool {
    body.split(|c: char| !c.is_ascii_alphanumeric())
        .any(|word| word.trim_start_matches("0x").eq_ignore_ascii_case(address))
}

async fn fetch_proof(host: &str, address: &str) -> bool {
    let Ok(resolved) = tokio::net::lookup_host((host, 443)).await else {
        return false;
    };
    let resolved: Vec<_> = resolved.collect();
    // Every address the name resolves to must be public, and the request
    // goes to the one checked here, not to a second lookup.
    let Some(target) = resolved.first().copied() else {
        return false;
    };
    if !resolved.iter().all(|a| is_public(a.ip())) {
        return false;
    }
    let Ok(http) = reqwest::Client::builder()
        .timeout(FETCH_TIMEOUT)
        .redirect(reqwest::redirect::Policy::none())
        .resolve(host, target)
        .build()
    else {
        return false;
    };
    let Ok(mut resp) = http
        .get(format!("https://{host}/.well-known/keel.txt"))
        .send()
        .await
    else {
        return false;
    };
    if resp.status() != reqwest::StatusCode::OK {
        return false;
    }
    let mut body = Vec::new();
    while let Ok(Some(chunk)) = resp.chunk().await {
        body.extend_from_slice(&chunk);
        if body.len() >= MAX_BODY {
            body.truncate(MAX_BODY);
            break;
        }
    }
    proves(&String::from_utf8_lossy(&body), address)
}

/// `GET /v1/clients`: every client, named ones first, each with
/// `website_verified`: true, false, or null while the first check runs
/// (and always null for a client with no website).
pub async fn clients(State(app): State<App>) -> ApiResult {
    let mut list = app
        .node
        .get("/v1/clients")
        .await?
        .and_then(|v| v.get("clients").and_then(Value::as_array).cloned())
        .unwrap_or_default();
    for row in &mut list {
        let address = field_str(row, "address").unwrap_or_default();
        let website = row
            .get("profile")
            .and_then(|p| field_str(p, "website"))
            .unwrap_or_default();
        if address.is_empty() || website.is_empty() {
            row["website_verified"] = Value::Null;
            continue;
        }
        let key = format!("{address}@{website}");
        let (verified, due) = app.website_checks.poll(&key);
        if due {
            let app = app.clone();
            tokio::spawn(async move {
                let ok = fetch_proof(&website, &address).await;
                app.website_checks.finish(&key, ok);
            });
        }
        row["website_verified"] = json!(verified);
    }
    Ok(Json(json!({ "clients": list })))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_public_addresses_are_fetched() {
        for ip in [
            "127.0.0.1",
            "10.1.2.3",
            "172.16.0.1",
            "192.168.1.1",
            "169.254.169.254",
            "100.64.0.1",
            "0.0.0.0",
            "198.18.0.1",
            "224.0.0.1",
            "255.255.255.255",
            "::1",
            "::",
            "fc00::1",
            "fd12::1",
            "fe80::1",
            "::ffff:127.0.0.1",
            "::ffff:10.0.0.1",
        ] {
            assert!(!is_public(ip.parse().unwrap()), "{ip}");
        }
        for ip in [
            "1.1.1.1",
            "93.184.216.34",
            "2606:4700::1111",
            "::ffff:1.1.1.1",
        ] {
            assert!(is_public(ip.parse().unwrap()), "{ip}");
        }
    }

    #[test]
    fn the_proof_is_the_whole_address() {
        let a = "75f234d8e6a57167a61a48dc3d7b6ad56d90fef0decbfdec8a72133f7c85b45e";
        assert!(proves(a, a));
        assert!(proves(&format!("keel-client={}\n", a.to_uppercase()), a));
        assert!(proves(&format!("0x{a}"), a));
        assert!(proves(&format!("first\n{a}\nlast"), a));
        assert!(!proves(&a[..63], a));
        assert!(!proves(&format!("{a}0"), a));
        assert!(!proves("", a));
    }

    #[test]
    fn a_result_is_served_until_the_recheck_is_due() {
        let checks = WebsiteChecks::default();
        assert_eq!(checks.poll("a@x.example"), (None, true));
        // Running: no second fetch, still unknown.
        assert_eq!(checks.poll("a@x.example"), (None, false));
        checks.finish("a@x.example", true);
        assert_eq!(checks.poll("a@x.example"), (Some(true), false));
        // Another website for the same client starts unknown.
        assert_eq!(checks.poll("a@y.example"), (None, true));
    }
}
