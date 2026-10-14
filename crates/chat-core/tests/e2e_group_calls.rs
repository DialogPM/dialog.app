//! Group calls end to end against a real server (see engine/group_calls.rs):
//! the ring reaches group members only (and not a muted member), signals from non-members are
//! dropped, the pair offer rule (smaller (account, device) offers) holds on both ends, and the
//! call ends with a history entry.
//!
//! `CHATAPP_E2E_SERVER=http://127.0.0.1:8080 cargo test -p chat-core --test e2e_group_calls -- --nocapture`

use std::sync::{Arc, Mutex};
use std::time::Duration;

use chat_core::content::{CallSignal, Content};
use chat_core::engine::{GroupCallJoinRequest, GroupCallSignalRequest, GroupCallStartRequest};
use chat_core::model::*;
use chat_core::{Core, CoreConfig, EventSink};
use serde_json::{json, Value};

#[derive(Default)]
struct Events(Mutex<Vec<(String, Value)>>);

impl EventSink for Events {
    fn emit(&self, event: &str, payload: Value) {
        self.0.lock().unwrap().push((event.to_string(), payload));
    }
}

struct Client {
    name: &'static str,
    core: Core,
    events: Arc<Events>,
    _dir: tempfile::TempDir,
}

impl Client {
    fn new(name: &'static str) -> Self {
        let dir = tempfile::tempdir().unwrap();
        let events = Arc::new(Events::default());
        let core = Core::start(CoreConfig {
            data_dir: dir.path().to_path_buf(),
            db_key: [7u8; 32],
            default_server_url: server(),
            platform: "cli".into(),
            events: events.clone(),
        })
        .unwrap();
        Self { name, core, events, _dir: dir }
    }

    async fn wait_event(&self, what: &str, pred: impl Fn(&str, &Value) -> bool) -> Value {
        for _ in 0..300 {
            if let Some((_, v)) = self.events.0.lock().unwrap().iter().find(|(n, v)| pred(n, v)) {
                return v.clone();
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
        panic!("{}: timed out waiting for {what}", self.name);
    }

    fn saw(&self, pred: impl Fn(&str, &Value) -> bool) -> bool {
        self.events.0.lock().unwrap().iter().any(|(n, v)| pred(n, v))
    }
}

fn server() -> String {
    std::env::var("CHATAPP_E2E_SERVER").unwrap_or_default()
}

/// A WebRTC offer as our clients produce it (passes call_checks).
fn offer_sdp() -> String {
    let fp = "sha-256 4A:AD:B9:B1:3F:82:18:3B:54:02:12:DF:3E:5D:49:6B:19:E5:7C:AB:3D:B4:0A:3E:4E:A9:66:9F:12:E5:8A:01";
    [
        "v=0",
        "o=- 4611731400430051336 2 IN IP4 127.0.0.1",
        "s=-",
        "t=0 0",
        "a=group:BUNDLE 0",
        "a=msid-semantic: WMS 5c3b1a",
        "m=audio 9 UDP/TLS/RTP/SAVPF 111",
        "c=IN IP4 0.0.0.0",
        "a=rtcp:9 IN IP4 0.0.0.0",
        "a=ice-ufrag:Kq3N",
        "a=ice-pwd:1kJ1iCZ0Wmm0bVjgnvGgZXdr",
        "a=ice-options:trickle",
        &format!("a=fingerprint:{fp}"),
        "a=setup:actpass",
        "a=mid:0",
        "a=sendrecv",
        "a=rtcp-mux",
        "a=rtpmap:111 opus/48000/2",
        "",
    ]
    .join("\r\n")
}

fn is_call(n: &str, v: &Value, call_id: &str) -> bool {
    n == "group_call" && v["call_id"].as_str() == Some(call_id)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn group_calls() {
    if server().is_empty() {
        eprintln!("CHATAPP_E2E_SERVER not set, skipping");
        return;
    }
    let _ = env_logger::builder().try_init();
    let url = server();
    let (ana, bob, cris, dan) = (Client::new("ana"), Client::new("bob"), Client::new("cris"), Client::new("dan"));
    let mut acc: Vec<AccountInfo> = Vec::new();
    for c in [&ana, &bob, &cris, &dan] {
        let u = url.clone();
        let r = c.core.run(move |i| async move { i.register(&u, "correct horse battery", "test device").await }).await.unwrap();
        acc.push(r.account);
        c.wait_event("online", |n, v| n == "connection" && v.as_str() == Some("online")).await;
    }
    let (a, b, c, d) = (acc[0].clone(), acc[1].clone(), acc[2].clone(), acc[3].clone());

    // Ana's group with Bob and Cris; Dan is not in it.
    for number in [b.number.clone(), c.number.clone()] {
        ana.core.run(move |i| async move { i.add_contact_by_number(&number, None).await }).await.unwrap();
    }
    let members = vec![b.account_id.clone(), c.account_id.clone()];
    let req: CreateGroupRequest = serde_json::from_value(json!({ "name": "Call team", "member_account_ids": members })).unwrap();
    let group = ana.core.run(move |i| async move { i.create_group(req).await }).await.unwrap();
    let gid = group.group_id.clone().unwrap();
    let mut convs = Vec::new();
    for cl in [&bob, &cris] {
        let conv = cl.wait_event("group conv", |n, v| n == "conversation_updated" && v["title"] == "Call team").await;
        let id = conv["id"].as_str().unwrap().to_string();
        let id2 = id.clone();
        cl.core.run(move |i| async move { i.accept_conversation(&id2).await }).await.unwrap();
        convs.push(id);
    }
    let (bob_conv, cris_conv) = (convs[0].clone(), convs[1].clone());
    // The members' sender keys reach everyone (group calls are announced with them).
    let bid = bob_conv.clone();
    let m: SendMessageRequest = serde_json::from_value(json!({ "conversation_id": bid, "text": "hi from bob" })).unwrap();
    bob.core.run(move |i| async move { i.send_message(m).await }).await.unwrap();
    ana.wait_event("bob's message", |n, v| n == "message" && v["message"]["text"] == "hi from bob").await;
    // Cris muted the group: the call does not ring there, it only shows as in progress.
    let patch: ConversationSettingsPatch = serde_json::from_value(json!({ "conversation_id": cris_conv, "muted_until": -1 })).unwrap();
    cris.core.run(move |i| async move { i.set_conversation_settings(patch).await }).await.unwrap();

    // --- ring: members only --------------------------------------------------------------
    let conv = group.id.clone();
    let info = ana
        .core
        .run(move |i| async move { i.group_call_start(GroupCallStartRequest { conversation_id: conv, video: false }).await })
        .await
        .unwrap();
    let call_id = info.call_id.clone();
    assert!(info.joined);
    assert_eq!(info.participants.len(), 1);
    let cid = call_id.clone();
    let rung = bob.wait_event("ring", move |n, v| is_call(n, v, &cid) && v["ringing"] == true).await;
    assert_eq!(rung["started_by"], a.account_id.as_str());
    assert_eq!(rung["participants"].as_array().unwrap().len(), 1);
    let cid = call_id.clone();
    let muted = cris.wait_event("call in progress", move |n, v| is_call(n, v, &cid)).await;
    assert_eq!(muted["ringing"], false, "a muted group must not ring");
    tokio::time::sleep(Duration::from_secs(2)).await;
    assert!(!dan.saw(|n, _| n == "group_call"), "a non-member must not hear of the call");
    // The state can be asked for (banner when the conversation is opened).
    let bc = bob_conv.clone();
    let st = bob.core.run(move |i| async move { i.group_call_state(&bc) }).await.unwrap().expect("call in progress");
    assert_eq!(st.call_id, call_id);

    // --- non-members are dropped ---------------------------------------------------------
    // Dan forges a join (and a pairwise offer) for the group straight to Ana.
    let forged = |kind: &str, data: Value, target: Option<u32>| {
        Content::Call(CallSignal { call_id: call_id.clone(), kind: kind.into(), data, target_device: target, group_id: Some(gid.clone()) })
    };
    let (join, offer) = (
        forged("group_join", json!({ "ring": true }), None),
        forged("group_offer", json!({ "sdp": offer_sdp() }), Some(a.device_id)),
    );
    let to = a.account_id.clone();
    dan.core
        .run(move |i| async move {
            i.send_to_account(&to, &join, false).await?;
            i.send_to_account(&to, &offer, false).await
        })
        .await
        .unwrap();
    // A non-member cannot use the group call commands either.
    let dan_conv = dan
        .core
        .run({
            let n = a.number.clone();
            move |i| async move {
                let ct = i.add_contact_by_number(&n, None).await?;
                i.open_direct_conversation(&ct.account_id).await
            }
        })
        .await
        .unwrap();
    let dc = dan_conv.id.clone();
    let err = dan
        .core
        .run(move |i| async move { i.group_call_start(GroupCallStartRequest { conversation_id: dc, video: false }).await })
        .await;
    assert!(err.is_err(), "a direct conversation has no group call");

    // --- Bob joins; the pair offer rule ---------------------------------------------------
    let (bc, cid) = (bob_conv.clone(), call_id.clone());
    let joined = bob
        .core
        .run(move |i| async move { i.group_call_join(GroupCallJoinRequest { conversation_id: bc, call_id: cid, video: false }).await })
        .await
        .unwrap();
    assert_eq!(joined.participants.len(), 2);
    let cid = call_id.clone();
    let bob_acc = b.account_id.clone();
    ana.wait_event("bob in the call", move |n, v| {
        is_call(n, v, &cid) && v["participants"].as_array().unwrap().iter().any(|p| p["account_id"] == bob_acc.as_str())
    })
    .await;
    // Dan's forged signals left no trace at Ana.
    let dan_acc = d.account_id.clone();
    assert!(
        !ana.saw(|n, v| (n == "group_call" && v["participants"].to_string().contains(&dan_acc))
            || (n == "group_call_signal" && v["account_id"] == dan_acc.as_str())),
        "signals from a non-member must be dropped"
    );

    // Who offers: the smaller (account, device).
    let ana_key = (a.account_id.clone(), a.device_id);
    let bob_key = (b.account_id.clone(), b.device_id);
    let ((small, small_conv, small_key), (large, large_conv, large_key)) = if ana_key < bob_key {
        ((&ana, group.id.clone(), ana_key), (&bob, bob_conv.clone(), bob_key))
    } else {
        ((&bob, bob_conv.clone(), bob_key), (&ana, group.id.clone(), ana_key))
    };
    let signal_req = |conv: &str, to: &(String, u32), kind: &str| GroupCallSignalRequest {
        conversation_id: conv.to_string(),
        call_id: call_id.clone(),
        account_id: to.0.clone(),
        device_id: to.1,
        kind: kind.into(),
        data: json!({ "sdp": offer_sdp() }),
    };
    // The larger side may not offer (refused locally)...
    let req = signal_req(&large_conv, &small_key, "offer");
    assert!(large.core.run(move |i| async move { i.group_call_signal(req).await }).await.is_err());
    // ...and an offer it sends anyway is dropped by the receiver.
    let rogue = Content::Call(CallSignal {
        call_id: call_id.clone(),
        kind: "group_offer".into(),
        data: json!({ "sdp": offer_sdp() }),
        target_device: Some(small_key.1),
        group_id: Some(gid.clone()),
    });
    let to = small_key.0.clone();
    large.core.run(move |i| async move { i.send_to_account(&to, &rogue, false).await }).await.unwrap();
    // The smaller side offers; the larger one gets it.
    let req = signal_req(&small_conv, &large_key, "offer");
    small.core.run(move |i| async move { i.group_call_signal(req).await }).await.unwrap();
    let small_acc = small_key.0.clone();
    let got = large
        .wait_event("pair offer", move |n, v| n == "group_call_signal" && v["kind"] == "offer" && v["account_id"] == small_acc.as_str())
        .await;
    assert_eq!(got["call_id"], call_id.as_str());
    tokio::time::sleep(Duration::from_secs(2)).await;
    let large_acc = large_key.0.clone();
    assert!(
        !small.saw(|n, v| n == "group_call_signal" && v["kind"] == "offer" && v["account_id"] == large_acc.as_str()),
        "an offer from the larger device must be dropped"
    );
    // A muted member that did not join never got a pairwise signal either.
    assert!(!cris.saw(|n, _| n == "group_call_signal"));

    // --- leaving ends the call, with a history entry -----------------------------------------
    let bc = bob_conv.clone();
    bob.core.run(move |i| async move { i.group_call_leave(&bc) }).await.unwrap();
    let ac = group.id.clone();
    ana.core.run(move |i| async move { i.group_call_leave(&ac) }).await.unwrap();
    for cl in [&ana, &bob, &cris] {
        let cid = call_id.clone();
        cl.wait_event("call over", move |n, v| is_call(n, v, &cid) && v["ended"] == true).await;
        cl.wait_event("history entry", |n, v| n == "message" && v["message"]["system"]["type"] == "group_call").await;
    }
    let entry = bob.wait_event("history entry", |n, v| n == "message" && v["message"]["system"]["type"] == "group_call").await;
    let value: Value = serde_json::from_str(entry["message"]["system"]["value"].as_str().unwrap()).unwrap();
    assert_eq!(value["joined"], true);
    let entry = cris.wait_event("history entry", |n, v| n == "message" && v["message"]["system"]["type"] == "group_call").await;
    let value: Value = serde_json::from_str(entry["message"]["system"]["value"].as_str().unwrap()).unwrap();
    assert_eq!(value["joined"], false);
}
