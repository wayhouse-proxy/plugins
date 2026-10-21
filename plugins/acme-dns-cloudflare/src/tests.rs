use super::*;

/// Answers from a script and records every call as `METHOD url`.
struct Fake {
    script: Vec<Reply>,
    calls: Vec<String>,
    bodies: Vec<Option<Value>>,
}

impl Fake {
    fn new(script: Vec<Reply>) -> Self {
        Self {
            script,
            calls: vec![],
            bodies: vec![],
        }
    }
}

impl Http for Fake {
    fn send(&mut self, method: &str, url: &str, body: Option<&Value>) -> Result<Reply, String> {
        self.calls.push(format!("{method} {url}"));
        self.bodies.push(body.cloned());
        assert!(!self.script.is_empty(), "unexpected call: {method} {url}");
        Ok(self.script.remove(0))
    }
}

fn ok(result: Value) -> Reply {
    Reply {
        status: 200,
        body: json!({"success": true, "result": result}),
    }
}

fn err(status: u16, code: i64, message: &str) -> Reply {
    Reply {
        status,
        body: json!({"success": false, "errors": [{"code": code, "message": message}]}),
    }
}

fn zone(id: &str) -> Reply {
    ok(json!([{"id": id, "name": "example.com"}]))
}

fn req(op: Op) -> Request {
    Request {
        op,
        fqdn: "_acme-challenge.panel.example.com".into(),
        value: "tok-en_1".into(),
        zone_hint: "panel.example.com".into(),
    }
}

#[test]
fn the_zone_search_goes_from_the_host_up_and_stops_at_the_first_zone() {
    let mut http = Fake::new(vec![ok(json!([])), zone("z1"), ok(json!({}))]);
    assert_eq!(run(&req(Op::Present), &mut http).unwrap(), "TXT record set");
    assert_eq!(
        http.calls,
        [
            "GET https://api.cloudflare.com/client/v4/zones?name=panel.example.com",
            "GET https://api.cloudflare.com/client/v4/zones?name=example.com",
            "POST https://api.cloudflare.com/client/v4/zones/z1/dns_records",
        ]
    );
}

#[test]
fn present_posts_a_txt_record_with_a_short_ttl() {
    let mut http = Fake::new(vec![zone("z1"), ok(json!({}))]);
    run(&req(Op::Present), &mut http).unwrap();
    assert_eq!(
        http.bodies[1],
        Some(
            json!({"type": "TXT", "name": "_acme-challenge.panel.example.com", "content": "tok-en_1", "ttl": 60})
        )
    );
}

#[test]
fn present_is_idempotent_when_the_record_exists() {
    let mut http = Fake::new(vec![zone("z1"), err(400, 81058, "Record already exists.")]);
    assert_eq!(
        run(&req(Op::Present), &mut http).unwrap(),
        "TXT record already there"
    );
}

#[test]
fn present_reports_another_refusal() {
    let mut http = Fake::new(vec![zone("z1"), err(400, 9999, "Invalid content")]);
    let e = run(&req(Op::Present), &mut http).unwrap_err();
    assert_eq!(e, "Cloudflare answered 400: Invalid content");
}

#[test]
fn a_rejected_token_stops_the_zone_search_at_once() {
    let mut http = Fake::new(vec![err(403, 9109, "Invalid access token")]);
    let e = run(&req(Op::Present), &mut http).unwrap_err();
    assert!(
        e.contains("refused the token") && e.contains("Zone:Read and DNS:Edit"),
        "{e}"
    );
    assert_eq!(http.calls.len(), 1);
}

#[test]
fn no_zone_gives_a_clear_error_after_trying_every_parent() {
    let mut r = req(Op::Present);
    r.zone_hint = "a.b.c.example.com".into();
    r.fqdn = "_acme-challenge.a.b.c.example.com".into();
    let mut http = Fake::new(vec![
        ok(json!([])),
        ok(json!([])),
        ok(json!([])),
        ok(json!([])),
    ]);
    let e = run(&r, &mut http).unwrap_err();
    assert!(
        e.contains("no Cloudflare zone contains a.b.c.example.com"),
        "{e}"
    );
    assert_eq!(http.calls.len(), 4);
    assert!(http.calls[3].ends_with("name=example.com"));
}

#[test]
fn a_host_five_labels_deep_reaches_the_apex_zone_and_stays_in_budget() {
    let mut r = req(Op::Cleanup);
    r.zone_hint = "a.b.c.d.example.com".into();
    r.fqdn = "_acme-challenge.a.b.c.d.example.com".into();
    let record =
        json!({"id": "r1", "name": "_acme-challenge.a.b.c.d.example.com", "content": "tok-en_1"});
    let mut script: Vec<Reply> = (0..4).map(|_| ok(json!([]))).collect();
    script.push(zone("z1"));
    script.push(ok(json!([record])));
    script.push(ok(json!({})));
    let mut http = Fake::new(script);
    assert_eq!(run(&r, &mut http).unwrap(), "removed 1 TXT record(s)");
    assert!(http.calls.len() <= 8);
}

#[test]
fn cleanup_removes_only_the_record_with_this_value() {
    let list = ok(json!([
        {"id": "r1", "name": "_acme-challenge.panel.example.com", "content": "\"tok-en_1\""},
        {"id": "r2", "name": "_acme-challenge.panel.example.com", "content": "\"other\""},
        {"id": "r3", "name": "_acme-challenge.elsewhere.example.com", "content": "\"tok-en_1\""},
    ]));
    let mut http = Fake::new(vec![zone("z1"), list, ok(json!({"id": "r1"}))]);
    assert_eq!(
        run(&req(Op::Cleanup), &mut http).unwrap(),
        "removed 1 TXT record(s)"
    );
    assert_eq!(
        http.calls[1..],
        [
            "GET https://api.cloudflare.com/client/v4/zones/z1/dns_records?type=TXT&name=_acme-challenge.panel.example.com&per_page=100",
            "DELETE https://api.cloudflare.com/client/v4/zones/z1/dns_records/r1",
        ]
    );
}

#[test]
fn cleanup_with_nothing_to_remove_succeeds() {
    let mut http = Fake::new(vec![zone("z1"), ok(json!([]))]);
    assert_eq!(
        run(&req(Op::Cleanup), &mut http).unwrap(),
        "nothing to remove"
    );
}

#[test]
fn cleanup_stays_inside_the_call_budget() {
    let records: Vec<Value> = (0..10)
        .map(|i| json!({"id": format!("r{i}"), "name": "_acme-challenge.panel.example.com", "content": "tok-en_1"}))
        .collect();
    let mut script = vec![zone("z1"), ok(json!(records))];
    script.extend((0..MAX_DELETES).map(|_| ok(json!({}))));
    let mut http = Fake::new(script);
    run(&req(Op::Cleanup), &mut http).unwrap();
    assert_eq!(http.calls.len(), 2 + MAX_DELETES);
    assert!(http.calls.len() <= 8);
}

#[test]
fn a_failed_delete_is_reported() {
    let list = ok(
        json!([{"id": "r1", "name": "_acme-challenge.panel.example.com", "content": "tok-en_1"}]),
    );
    let mut http = Fake::new(vec![zone("z1"), list, err(500, 1, "boom")]);
    assert_eq!(
        run(&req(Op::Cleanup), &mut http).unwrap_err(),
        "Cloudflare answered 500: boom"
    );
}

#[test]
fn names_that_could_change_a_url_are_refused_before_any_call() {
    for bad in ["a.com/../x", "a.com?x=1", "A.com", "a..com", "", "a.com\n"] {
        let mut r = req(Op::Present);
        r.zone_hint = bad.into();
        r.fqdn = format!("_acme-challenge.{bad}");
        let mut http = Fake::new(vec![]);
        assert!(run(&r, &mut http).is_err(), "{bad:?}");
        assert!(http.calls.is_empty());
    }
}

#[test]
fn a_record_id_from_the_answer_cannot_escape_the_path() {
    let list = ok(
        json!([{"id": "../x", "name": "_acme-challenge.panel.example.com", "content": "tok-en_1"}]),
    );
    let mut http = Fake::new(vec![zone("z1"), list]);
    assert_eq!(
        run(&req(Op::Cleanup), &mut http).unwrap(),
        "nothing to remove"
    );
    let bad_zone = || ok(json!([{"id": "../x"}]));
    let mut http = Fake::new(vec![bad_zone(), bad_zone()]);
    assert!(run(&req(Op::Present), &mut http).is_err());
}

#[test]
fn fqdn_must_belong_to_the_hint() {
    let mut r = req(Op::Present);
    r.fqdn = "_acme-challenge.other.example.com".into();
    assert!(run(&r, &mut Fake::new(vec![])).is_err());
}

#[test]
fn the_request_document_parses() {
    let doc = r#"{"op":"cleanup","fqdn":"_acme-challenge.a.example.com","value":"v","zone_hint":"a.example.com"}"#;
    let r: Request = serde_json::from_str(doc).unwrap();
    assert_eq!(r.op, Op::Cleanup);
}

#[test]
fn the_message_is_cut_on_a_char_boundary() {
    let long = "é".repeat(400);
    let m = message(&Err(long));
    assert!(m.len() <= MAX_MESSAGE && m.chars().all(|c| c == 'é'));
}

#[test]
fn caps_json_declares_what_the_plugin_uses() {
    let caps = include_str!("../caps.json");
    assert!(
        caps.contains("\"on_acme_dns\":true")
            && caps.contains("api.cloudflare.com")
            && caps.contains("CLOUDFLARE_API_TOKEN")
            && !caps.contains("\"state\"")
            && !caps.contains("\"routes\"")
    );
}
