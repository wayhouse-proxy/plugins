//! `pelican`: routes hostnames to the game servers of a [Pelican](https://pelican.dev) panel.
//!
//! Every tick it lists the panel's servers with their allocations (Application API,
//! read-only key in the secret slot `PELICAN_TOKEN`), turns each server into a
//! `hostname -> node ip:port` entry and publishes the whole set with `routes_set`. Point a
//! wildcard DNS record at the one wayhouse entry node: adding, moving or deleting a Pelican
//! server then needs no DNS or firewall edit anywhere, the next tick moves the route.
//!
//! The panel host, the claimable hostnames and the backend networks live in the module's
//! capability declaration, which the host reads without running the module, so they are fixed
//! when it is built (see the README). `init` only takes the non-secret settings in
//! [`panel::Config`].

pub mod b64;
pub mod caps;
pub mod panel;

/// What the plugin answers a webhook call with. The call is only a hint to reconcile now:
/// the request (headers, body) is never looked at, so a repeated, late or forged-but-authorised
/// call can at worst cause one more read of the panel. `202` when the reconcile ran and
/// published, `502` when it did not, so Pelican's delivery log shows the failure.
pub mod webhook {
    pub fn answer(reconciled: &Result<usize, String>) -> String {
        let status = if reconciled.is_ok() { 202 } else { 502 };
        serde_json::json!({ "status": status }).to_string()
    }
}

// Re-export so the linker keeps `alloc` and the ABI section.
pub use wayhouse_plugin_abi::alloc;

/// The capability declaration baked in by `build.rs`.
pub const CAPS_JSON: &str = include_str!(concat!(env!("OUT_DIR"), "/caps.json"));

#[cfg(target_arch = "wasm32")]
mod guest {
    use std::sync::Mutex;

    use super::*;

    #[used]
    #[link_section = "wayhouse.plugin-caps"]
    static CAPS: [u8; CAPS_JSON.len()] = *CAPS_JSON.as_bytes().first_chunk().unwrap();

    #[link(wasm_import_module = "wayhouse")]
    extern "C" {
        fn log(level: i32, ptr: i32, len: i32);
        fn routes_set(ptr: i32, len: i32) -> i32;
        fn http_request(ptr: i32, len: i32) -> i32;
        fn http_read(out_ptr: i32, out_cap: i32) -> i32;
        fn webhook_respond(ptr: i32, len: i32) -> i32;
    }

    static CONFIG: Mutex<Option<panel::Config>> = Mutex::new(None);

    fn say(level: i32, msg: &str) {
        // SAFETY: pointer and length describe `msg`, valid for the call.
        unsafe { log(level, msg.as_ptr() as i32, msg.len() as i32) };
    }

    fn fetch(url: &str) -> Result<Vec<u8>, String> {
        let req = panel::request_doc(url);
        // SAFETY: pointer and length describe `req`, valid for the call.
        let n = unsafe { http_request(req.as_ptr() as i32, req.len() as i32) };
        let n =
            usize::try_from(n).map_err(|_| "request refused: over the size limit".to_string())?;
        let mut buf = vec![0u8; n];
        // SAFETY: `buf` is `n` writable bytes, the length `http_request` announced.
        let got = unsafe { http_read(buf.as_mut_ptr() as i32, buf.len() as i32) };
        if usize::try_from(got) != Ok(n) {
            return Err("could not read the http response".to_string());
        }
        panel::response_body(&buf)
    }

    #[no_mangle]
    pub extern "C" fn init(config_ptr: i32, config_len: i32) {
        // SAFETY: the host wrote `config_len` bytes at `config_ptr` (from `alloc`) before the call.
        let bytes =
            unsafe { std::slice::from_raw_parts(config_ptr as *const u8, config_len as usize) };
        match panel::Config::parse(bytes) {
            Ok(c) => {
                say(2, &format!("pelican init, panel {}", c.panel_url));
                *CONFIG.lock().unwrap() = Some(c);
            }
            Err(e) => say(0, &format!("pelican init: {e}")),
        }
    }

    /// Lists the panel and publishes the whole route set; the count published, or why not.
    /// On an error the previous routes stay.
    fn reconcile() -> Result<usize, String> {
        let cfg = CONFIG
            .lock()
            .unwrap()
            .clone()
            .ok_or("no valid config, nothing published")?;
        let limits = caps::RouteLimits::from_caps_json(CAPS_JSON)?;
        let (entries, warnings) = panel::plan(fetch, &cfg, &limits)
            .map_err(|e| format!("{e}; routes left as they were"))?;
        for w in &warnings {
            say(1, &format!("pelican: {w}"));
        }
        let doc = serde_json::to_string(&entries).expect("entries serialize");
        // SAFETY: pointer and length describe `doc`, valid for the call.
        if unsafe { routes_set(doc.as_ptr() as i32, doc.len() as i32) } != 0 {
            return Err("the host refused the route set".into());
        }
        Ok(entries.len())
    }

    fn report(r: &Result<usize, String>) {
        match r {
            Ok(n) => say(2, &format!("pelican: {n} routes published")),
            Err(e) => say(0, &format!("pelican: {e}")),
        }
    }

    #[no_mangle]
    pub extern "C" fn on_timer() {
        report(&reconcile());
    }

    /// A webhook is a hint to reconcile now; the request is not read (see [`webhook`]).
    /// Calls are serialized by the host and at most 4 wait, so a burst costs at most a few
    /// reads of the panel, and an unchanged route set is not committed again.
    #[no_mangle]
    pub extern "C" fn on_webhook(_request_ptr: i32, _request_len: i32) {
        let r = reconcile();
        report(&r);
        let doc = webhook::answer(&r);
        // SAFETY: pointer and length describe `doc`, valid for the call.
        unsafe { webhook_respond(doc.as_ptr() as i32, doc.len() as i32) };
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn embedded_caps_declare_what_the_plugin_uses() {
        let v: serde_json::Value = serde_json::from_str(CAPS_JSON).unwrap();
        assert_eq!(v["triggers"]["on_timer"], true);
        assert_eq!(v["triggers"]["on_webhook"], true);
        assert_eq!(v["log"], true);
        assert!(v["http"]["hosts"].as_array().is_some_and(|h| !h.is_empty()));
        assert_eq!(v["secrets"][0]["name"], "PELICAN_TOKEN");
        assert!(v.get("state").is_none(), "pelican keeps no state");
        // The same document must parse as the route limits `on_timer` filters with.
        caps::RouteLimits::from_caps_json(CAPS_JSON).unwrap();
    }

    #[test]
    fn webhook_answers_202_after_a_reconcile_and_502_when_it_failed() {
        let ok: serde_json::Value = serde_json::from_str(&webhook::answer(&Ok(3))).unwrap();
        assert_eq!(ok["status"], 202);
        let err: serde_json::Value =
            serde_json::from_str(&webhook::answer(&Err("boom".into()))).unwrap();
        assert_eq!(err["status"], 502);
    }

    #[test]
    fn secret_slot_is_bound_to_a_declared_http_host() {
        let v: serde_json::Value = serde_json::from_str(CAPS_JSON).unwrap();
        let hosts: Vec<_> = v["http"]["hosts"]
            .as_array()
            .unwrap()
            .iter()
            .map(|h| h["host"].as_str().unwrap().to_string())
            .collect();
        for bound in v["secrets"][0]["hosts"].as_array().unwrap() {
            let bound = bound.as_str().unwrap().split(':').next().unwrap();
            assert!(
                hosts.iter().any(|h| h == bound),
                "{bound} not in http.hosts"
            );
        }
    }
}
