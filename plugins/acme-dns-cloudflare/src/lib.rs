//! `acme-dns-cloudflare`: a DNS-provider plugin for the controller's ACME runner.
//!
//! The runner calls `on_acme_dns` with `present` or `cleanup` for one `_acme-challenge` TXT
//! record. The plugin finds the Cloudflare zone that contains the host, then adds or removes
//! the record through the Cloudflare API with the `CLOUDFLARE_API_TOKEN` secret slot (a token
//! with `Zone:Read` and `DNS:Edit`). It never sees an ACME account or certificate key.
//!
//! One guest call may make 8 HTTP requests, so the zone search tries at most
//! [`MAX_ZONE_TRIES`] names and a cleanup removes at most [`MAX_DELETES`] records.

use serde::Deserialize;
use serde_json::{json, Value};

// Re-export so the linker keeps `alloc` and the ABI section.
pub use wayhouse_plugin_abi::alloc;

const API: &str = "https://api.cloudflare.com/client/v4";
/// Zone names tried, most specific first (`a.b.example.com`, `b.example.com`, `example.com`).
pub const MAX_ZONE_TRIES: usize = 3;
/// Records one cleanup removes: 3 zone lookups, 1 listing and 4 deletes make the 8 calls.
pub const MAX_DELETES: usize = 4;
/// Longest message handed back to the controller.
pub const MAX_MESSAGE: usize = 480;
/// Cloudflare's code for a record that already exists.
const ALREADY_EXISTS: [i64; 2] = [81057, 81058];

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Op {
    Present,
    Cleanup,
}

/// The `on_acme_dns` request document.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct Request {
    pub op: Op,
    pub fqdn: String,
    pub value: String,
    pub zone_hint: String,
}

/// What Cloudflare answered.
#[derive(Debug, Clone, PartialEq)]
pub struct Reply {
    pub status: u16,
    pub body: Value,
}

/// The HTTP side, so the logic runs natively in tests.
pub trait Http {
    /// `body` is a JSON document for `POST`.
    fn send(&mut self, method: &str, url: &str, body: Option<&Value>) -> Result<Reply, String>;
}

/// Runs one request; `Ok` is the message for a success, `Err` the reason for a refusal.
pub fn run(req: &Request, http: &mut impl Http) -> Result<String, String> {
    check_name(&req.fqdn)?;
    check_name(&req.zone_hint)?;
    check_value(&req.value)?;
    if req.fqdn != format!("_acme-challenge.{}", req.zone_hint) {
        return Err("fqdn is not _acme-challenge.<zone_hint>".into());
    }
    let zone = find_zone(&req.zone_hint, http)?;
    match req.op {
        Op::Present => present(&zone, req, http),
        Op::Cleanup => cleanup(&zone, req, http),
    }
}

/// A DNS name the plugin puts in a URL: lowercase labels of `a-z`, `0-9`, `-` and `_`.
fn check_name(name: &str) -> Result<(), String> {
    let ok = !name.is_empty()
        && name.len() <= 253
        && name.split('.').all(|l| {
            !l.is_empty()
                && l.len() <= 63
                && l.bytes()
                    .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-' || b == b'_')
        });
    if ok {
        Ok(())
    } else {
        Err(format!("not a usable DNS name: {name:.80}"))
    }
}

fn check_value(value: &str) -> Result<(), String> {
    if !value.is_empty() && value.len() <= 255 && value.bytes().all(|b| b.is_ascii_graphic()) {
        Ok(())
    } else {
        Err("the TXT value is empty, too long or not printable ASCII".into())
    }
}

/// `a.b.example.com` gives `a.b.example.com`, `b.example.com`, `example.com`, at most
/// [`MAX_ZONE_TRIES`], never a bare top-level label.
pub fn zone_candidates(host: &str) -> Vec<&str> {
    let mut out = Vec::new();
    let mut rest = host;
    while rest.contains('.') && out.len() < MAX_ZONE_TRIES {
        out.push(rest);
        rest = &rest[rest.find('.').unwrap_or(0) + 1..];
    }
    out
}

/// Cloudflare ids are hex, but only the shape matters: the id goes into a URL.
fn valid_id(id: &str) -> bool {
    !id.is_empty() && id.len() <= 64 && id.bytes().all(|b| b.is_ascii_alphanumeric())
}

/// The first error message of a Cloudflare answer, or the status.
fn failure(reply: &Reply) -> String {
    let msg = reply.body["errors"]
        .get(0)
        .and_then(|e| e["message"].as_str())
        .unwrap_or("no error message");
    match reply.status {
        401 | 403 => format!(
            "Cloudflare refused the token ({}): {msg}; it needs Zone:Read and DNS:Edit",
            reply.status
        ),
        s => format!("Cloudflare answered {s}: {msg}"),
    }
}

fn succeeded(reply: &Reply) -> bool {
    (200..300).contains(&reply.status) && reply.body["success"].as_bool() == Some(true)
}

fn find_zone(host: &str, http: &mut impl Http) -> Result<String, String> {
    for candidate in zone_candidates(host) {
        let reply = http.send("GET", &format!("{API}/zones?name={candidate}"), None)?;
        if matches!(reply.status, 401 | 403) || !succeeded(&reply) {
            return Err(failure(&reply));
        }
        if let Some(id) = reply.body["result"]
            .get(0)
            .and_then(|z| z["id"].as_str())
            .filter(|id| valid_id(id))
        {
            return Ok(id.to_string());
        }
    }
    Err(format!(
        "no Cloudflare zone contains {host} (does the token have Zone:Read on it?)"
    ))
}

fn present(zone: &str, req: &Request, http: &mut impl Http) -> Result<String, String> {
    let body = json!({"type": "TXT", "name": req.fqdn, "content": req.value, "ttl": 60});
    let reply = http.send(
        "POST",
        &format!("{API}/zones/{zone}/dns_records"),
        Some(&body),
    )?;
    if succeeded(&reply) {
        return Ok("TXT record set".into());
    }
    let exists = reply.body["errors"].as_array().is_some_and(|e| {
        e.iter().any(|e| {
            e["code"]
                .as_i64()
                .is_some_and(|c| ALREADY_EXISTS.contains(&c))
        })
    });
    if exists {
        return Ok("TXT record already there".into());
    }
    Err(failure(&reply))
}

/// Cloudflare returns TXT content in quotes.
fn unquote(content: &str) -> &str {
    content
        .strip_prefix('"')
        .and_then(|c| c.strip_suffix('"'))
        .unwrap_or(content)
}

fn cleanup(zone: &str, req: &Request, http: &mut impl Http) -> Result<String, String> {
    let list = http.send(
        "GET",
        &format!(
            "{API}/zones/{zone}/dns_records?type=TXT&name={}&per_page=100",
            req.fqdn
        ),
        None,
    )?;
    if !succeeded(&list) {
        return Err(failure(&list));
    }
    let ids: Vec<&str> = list.body["result"]
        .as_array()
        .map(Vec::as_slice)
        .unwrap_or_default()
        .iter()
        .filter(|r| r["name"].as_str() == Some(req.fqdn.as_str()))
        .filter(|r| r["content"].as_str().map(unquote) == Some(req.value.as_str()))
        .filter_map(|r| r["id"].as_str())
        .filter(|id| valid_id(id))
        .take(MAX_DELETES)
        .collect();
    for id in &ids {
        let reply = http.send(
            "DELETE",
            &format!("{API}/zones/{zone}/dns_records/{id}"),
            None,
        )?;
        if !succeeded(&reply) {
            return Err(failure(&reply));
        }
    }
    Ok(match ids.len() {
        0 => "nothing to remove".into(),
        n => format!("removed {n} TXT record(s)"),
    })
}

/// The answer text, cut to what the controller keeps.
pub fn message(result: &Result<String, String>) -> String {
    let (Ok(m) | Err(m)) = result;
    let mut end = m.len().min(MAX_MESSAGE);
    while !m.is_char_boundary(end) {
        end -= 1;
    }
    m[..end].to_string()
}

#[cfg(target_arch = "wasm32")]
mod guest {
    use super::*;
    use base64::Engine;

    // The capability declaration the host reads without running the module.
    #[used]
    #[link_section = "wayhouse.plugin-caps"]
    static CAPS: [u8; include_bytes!("../caps.json").len()] = *include_bytes!("../caps.json");

    #[link(wasm_import_module = "wayhouse")]
    extern "C" {
        fn http_request(ptr: i32, len: i32) -> i32;
        fn http_read(out_ptr: i32, out_cap: i32) -> i32;
        fn acme_dns_result(ok: i32, ptr: i32, len: i32) -> i32;
    }

    struct Host;

    impl Http for Host {
        fn send(&mut self, method: &str, url: &str, body: Option<&Value>) -> Result<Reply, String> {
            let mut req = json!({
                "method": method,
                "url": url,
                "headers": {
                    "Authorization": "Bearer ${secret:CLOUDFLARE_API_TOKEN}",
                    "Content-Type": "application/json",
                },
            });
            if let Some(b) = body {
                let raw = serde_json::to_vec(b).map_err(|e| e.to_string())?;
                req["body"] = Value::String(base64::engine::general_purpose::STANDARD.encode(raw));
            }
            let doc = serde_json::to_vec(&req).map_err(|e| e.to_string())?;
            // SAFETY: pointer and length describe `doc`, valid for the call.
            let len = unsafe { http_request(doc.as_ptr() as i32, doc.len() as i32) };
            let len = usize::try_from(len).map_err(|_| "the request was too large".to_string())?;
            let mut out = vec![0u8; len];
            // SAFETY: `out` is valid for `len` bytes, the size the host announced.
            let got = unsafe { http_read(out.as_mut_ptr() as i32, out.len() as i32) };
            if got < 0 {
                return Err("could not read the response".into());
            }
            let v: Value = serde_json::from_slice(&out).map_err(|e| e.to_string())?;
            if let Some(e) = v["error"].as_str() {
                return Err(format!("the HTTP call failed: {e}"));
            }
            let status = v["status"].as_u64().unwrap_or(0) as u16;
            let raw = base64::engine::general_purpose::STANDARD
                .decode(v["body"].as_str().unwrap_or(""))
                .map_err(|e| e.to_string())?;
            Ok(Reply {
                status,
                body: serde_json::from_slice(&raw).unwrap_or(Value::Null),
            })
        }
    }

    #[no_mangle]
    pub extern "C" fn init(_config_ptr: i32, _config_len: i32) {}

    /// Required by the host; this plugin has nothing to do on a timer.
    #[no_mangle]
    pub extern "C" fn on_timer() {}

    #[no_mangle]
    pub extern "C" fn on_acme_dns(ptr: i32, len: i32) {
        // SAFETY: the host wrote `len` bytes at `ptr` (from `alloc`) before the call.
        let doc = unsafe { std::slice::from_raw_parts(ptr as *const u8, len as usize) };
        let result = match serde_json::from_slice::<Request>(doc) {
            Ok(req) => run(&req, &mut Host),
            Err(e) => Err(format!("unreadable request: {e}")),
        };
        let msg = message(&result);
        // SAFETY: pointer and length describe `msg`, valid for the call.
        unsafe {
            acme_dns_result(
                i32::from(result.is_ok()),
                msg.as_ptr() as i32,
                msg.len() as i32,
            )
        };
    }
}

#[cfg(test)]
mod tests;
