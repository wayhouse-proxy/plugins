//! `selftest`: a plugin whose only purpose is to prove the plugin pipeline works.
//!
//! Every tick it reads a counter from its `state`, adds one, writes it back and logs
//! `selftest tick N`. Install it, enable it, and `GET /plugins/{id}/status` shows `ticks`
//! rising together with the log lines, and the counter surviving controller restarts.
//! `init` logs the config it was given (the `log` capability is the only one besides
//! `state` it uses). It uses nothing the host does not already provide.

// Re-export so the linker keeps `alloc` and the ABI section.
pub use wayhouse_plugin_abi::alloc;

/// State key holding the tick counter (decimal text, so it is readable in a dump).
pub const COUNTER_KEY: &str = "ticks";

/// Next counter value and its stored form, given the stored previous value. A missing or
/// unreadable value restarts at 1: a plugin must tolerate state it did not write.
pub fn next_tick(prev: Option<&[u8]>) -> (u64, Vec<u8>) {
    let n = prev
        .and_then(|b| std::str::from_utf8(b).ok())
        .and_then(|s| s.parse::<u64>().ok())
        .map_or(1, |p| p.saturating_add(1));
    (n, n.to_string().into_bytes())
}

/// Message logged on each tick.
pub fn tick_line(n: u64) -> String {
    format!("selftest tick {n}")
}

/// Message logged by `init`.
pub fn init_line(config: &[u8]) -> String {
    format!("selftest init, config is {} bytes", config.len())
}

#[cfg(target_arch = "wasm32")]
mod guest {
    use super::*;

    // The capability declaration the host reads without running the module.
    #[used]
    #[link_section = "wayhouse.plugin-caps"]
    static CAPS: [u8; include_bytes!("../caps.json").len()] = *include_bytes!("../caps.json");

    #[link(wasm_import_module = "wayhouse")]
    extern "C" {
        fn log(level: i32, ptr: i32, len: i32);
        fn state_get(kptr: i32, klen: i32, out_ptr: i32, out_cap: i32) -> i32;
        fn state_put(kptr: i32, klen: i32, vptr: i32, vlen: i32) -> i32;
    }

    fn info(msg: &str) {
        // SAFETY: pointer and length describe `msg`, valid for the call.
        unsafe { log(2, msg.as_ptr() as i32, msg.len() as i32) };
    }

    #[no_mangle]
    pub extern "C" fn init(config_ptr: i32, config_len: i32) {
        // SAFETY: the host wrote `config_len` bytes at `config_ptr` (from `alloc`) before the call.
        let config =
            unsafe { std::slice::from_raw_parts(config_ptr as *const u8, config_len as usize) };
        info(&init_line(config));
    }

    #[no_mangle]
    pub extern "C" fn on_timer() {
        let mut buf = [0u8; 32];
        // SAFETY: the key and `buf` are valid for the call; the host writes at most `buf.len()` bytes.
        let len = unsafe {
            state_get(
                COUNTER_KEY.as_ptr() as i32,
                COUNTER_KEY.len() as i32,
                buf.as_mut_ptr() as i32,
                buf.len() as i32,
            )
        };
        let prev = usize::try_from(len).ok().and_then(|l| buf.get(..l));
        let (n, stored) = next_tick(prev);
        // SAFETY: key and value are valid for the call.
        let rc = unsafe {
            state_put(
                COUNTER_KEY.as_ptr() as i32,
                COUNTER_KEY.len() as i32,
                stored.as_ptr() as i32,
                stored.len() as i32,
            )
        };
        if rc != 0 {
            info("selftest could not store the counter");
            return;
        }
        info(&tick_line(n));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn first_tick_is_one() {
        assert_eq!(next_tick(None), (1, b"1".to_vec()));
    }

    #[test]
    fn counts_up_from_stored_value() {
        assert_eq!(next_tick(Some(b"41")), (42, b"42".to_vec()));
    }

    #[test]
    fn garbage_state_restarts_at_one() {
        assert_eq!(next_tick(Some(b"abc")).0, 1);
        assert_eq!(next_tick(Some(&[0xff, 0xfe])).0, 1);
        assert_eq!(next_tick(Some(b"")).0, 1);
    }

    #[test]
    fn counter_saturates() {
        assert_eq!(next_tick(Some(u64::MAX.to_string().as_bytes())).0, u64::MAX);
    }

    #[test]
    fn log_lines_are_stable() {
        assert_eq!(tick_line(3), "selftest tick 3");
        assert_eq!(init_line(b"abcd"), "selftest init, config is 4 bytes");
    }

    #[test]
    fn caps_json_declares_what_the_plugin_uses() {
        let caps = include_str!("../caps.json");
        assert!(
            caps.contains("\"on_timer\":true")
                && caps.contains("\"log\":true")
                && caps.contains("\"state\"")
        );
    }
}
