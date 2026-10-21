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

    #[no_mangle]
    pub extern "C" fn on_timer() {
        let Some(cfg) = CONFIG.lock().unwrap().clone() else {
            say(0, "pelican: no valid config, nothing published");
            return;
        };
        let limits = match caps::RouteLimits::from_caps_json(CAPS_JSON) {
            Ok(l) => l,
            Err(e) => return say(0, &format!("pelican: {e}")),
        };
        let (entries, warnings) = match panel::plan(fetch, &cfg, &limits) {
            Ok(r) => r,
            Err(e) => return say(0, &format!("pelican: {e}; routes left as they were")),
        };
        for w in &warnings {
            say(1, &format!("pelican: {w}"));
        }
        let doc = serde_json::to_string(&entries).expect("entries serialize");
        // SAFETY: pointer and length describe `doc`, valid for the call.
        if unsafe { routes_set(doc.as_ptr() as i32, doc.len() as i32) } != 0 {
            say(0, "pelican: the host refused the route set");
            return;
        }
        say(2, &format!("pelican: {} routes published", entries.len()));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn embedded_caps_declare_what_the_plugin_uses() {
        let v: serde_json::Value = serde_json::from_str(CAPS_JSON).unwrap();
        assert_eq!(v["triggers"]["on_timer"], true);
        assert_eq!(v["log"], true);
        assert!(v["http"]["hosts"].as_array().is_some_and(|h| !h.is_empty()));
        assert_eq!(v["secrets"][0]["name"], "PELICAN_TOKEN");
        assert!(v.get("state").is_none(), "pelican keeps no state");
        // The same document must parse as the route limits `on_timer` filters with.
        caps::RouteLimits::from_caps_json(CAPS_JSON).unwrap();
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
