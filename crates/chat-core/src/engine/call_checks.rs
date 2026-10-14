//! Checks on call signals from the network, before the UI hands them to WebRTC.
//!
//! The SDP of an offer / answer goes straight into `setRemoteDescription`, so anything WebRTC
//! would accept is attack surface: data channels, extra media sections, odd transports,
//! mismatched DTLS fingerprints, huge descriptions. Only what our own clients produce passes:
//! at most one audio and one video section over UDP/TLS/RTP/SAVPF with a single SHA-256 DTLS
//! fingerprint. ICE candidates are capped and stripped down to the fields WebRTC needs.
//!
//! The same rules are mirrored in the UIs (`app/src/lib/callChecks.ts`,
//! `mobile/lib/state/call_checks.dart`) as a second line of defense.

use serde_json::{json, Map, Value};

pub const MAX_SDP_BYTES: usize = 16 * 1024;
const MAX_SDP_LINE: usize = 2048;
/// Candidates inside the SDP itself (we trickle, so normally none).
const MAX_SDP_CANDIDATES: usize = 50;
/// Remote ICE candidates accepted per call (the UIs count across signals; the core per signal).
pub const MAX_ICE_CANDIDATES: usize = 50;
const MAX_CANDIDATE_BYTES: usize = 512;
const MAX_SDP_MID: usize = 32;
const MAX_UFRAG: usize = 256;
const MAX_M_LINES: u64 = 2;
const PROTO: &str = "UDP/TLS/RTP/SAVPF";
const ICE_OPTIONS: &[&str] = &["trickle", "renomination", "ice2"];

/// Age of a signal by the server's receive time. The envelope's own timestamp is chosen by the
/// sender and must not be trusted (an old offer could ring again). `None`: no server time,
/// treated as too old.
pub fn signal_age(now_ms: i64, server_timestamp: i64) -> Option<i64> {
    (server_timestamp > 0).then(|| now_ms - server_timestamp)
}

#[derive(Default)]
struct Section {
    fingerprints: Vec<String>,
    setup: bool,
}

/// SHA-256 fingerprint value: 32 colon-separated hex bytes, normalized to upper case.
fn sha256_fingerprint(v: &str) -> Option<String> {
    let (alg, value) = v.split_once(' ')?;
    if !alg.eq_ignore_ascii_case("sha-256") {
        return None;
    }
    let value = value.trim();
    let bytes: Vec<&str> = value.split(':').collect();
    if bytes.len() != 32 || !bytes.iter().all(|b| b.len() == 2 && b.chars().all(|c| c.is_ascii_hexdigit())) {
        return None;
    }
    Some(value.to_ascii_uppercase())
}

/// Whether a remote session description is one our clients would produce.
pub fn valid_sdp(sdp: &str) -> bool {
    if sdp.is_empty() || sdp.len() > MAX_SDP_BYTES {
        return false;
    }
    let mut session = Section::default();
    let mut media: Vec<Section> = Vec::new();
    let (mut audio, mut video, mut candidates) = (0, 0, 0);
    let mut first = true;
    for raw in sdp.split('\n') {
        let line = raw.strip_suffix('\r').unwrap_or(raw);
        if line.is_empty() {
            continue;
        }
        let b = line.as_bytes();
        if line.len() > MAX_SDP_LINE || b.len() < 2 || !b[0].is_ascii_lowercase() || b[1] != b'=' || line.contains('\r') {
            return false;
        }
        if first {
            if line != "v=0" {
                return false;
            }
            first = false;
            continue;
        }
        let value = &line[2..];
        if b[0] == b'm' {
            let parts: Vec<&str> = value.split(' ').collect();
            if parts.len() < 4 || parts[1].parse::<u16>().is_err() || parts[2] != PROTO {
                return false;
            }
            match parts[0] {
                "audio" => audio += 1,
                "video" => video += 1,
                _ => return false,
            }
            if audio > 1 || video > 1 {
                return false;
            }
            media.push(Section::default());
            continue;
        }
        if b[0] != b'a' {
            continue;
        }
        let (attr, arg) = value.split_once(':').unwrap_or((value, ""));
        let section = media.last_mut().unwrap_or(&mut session);
        match attr {
            "fingerprint" => match sha256_fingerprint(arg) {
                Some(fp) => section.fingerprints.push(fp),
                None => return false,
            },
            "setup" => {
                if !matches!(arg, "actpass" | "active" | "passive") {
                    return false;
                }
                section.setup = true;
            }
            "ice-options" => {
                if !arg.split(' ').filter(|t| !t.is_empty()).all(|t| ICE_OPTIONS.contains(&t)) {
                    return false;
                }
            }
            "candidate" => {
                candidates += 1;
                if candidates > MAX_SDP_CANDIDATES {
                    return false;
                }
            }
            // Data channels.
            "sctp-port" | "sctpmap" => return false,
            _ => {}
        }
    }
    if first || media.is_empty() || media.len() as u64 > MAX_M_LINES || session.fingerprints.len() > 1 {
        return false;
    }
    // Exactly one fingerprint value, at session level or repeated in each media section.
    let mut fingerprint: Option<&String> = session.fingerprints.first();
    for m in &media {
        if m.fingerprints.len() > 1 || (!m.setup && !session.setup) {
            return false;
        }
        match (m.fingerprints.first(), fingerprint) {
            (None, None) => return false,
            (Some(fp), Some(known)) if fp != known => return false,
            (Some(fp), None) => fingerprint = Some(fp),
            _ => {}
        }
    }
    true
}

/// `data` of an offer / answer signal: carries a valid `sdp`.
pub fn valid_sdp_signal(data: &Value) -> bool {
    data.get("sdp").and_then(Value::as_str).is_some_and(valid_sdp)
}

/// One remote ICE candidate reduced to the RTCIceCandidateInit fields, or `None` if malformed.
pub fn clean_candidate(c: &Value) -> Option<Value> {
    let o = c.as_object()?;
    let candidate = o.get("candidate")?.as_str()?;
    if !candidate.starts_with("candidate:") || candidate.len() >= MAX_CANDIDATE_BYTES || candidate.contains(['\r', '\n']) {
        return None;
    }
    let mut out = Map::new();
    out.insert("candidate".into(), json!(candidate));
    match o.get("sdpMid") {
        None | Some(Value::Null) => {}
        Some(Value::String(mid)) if mid.len() <= MAX_SDP_MID && !mid.contains(['\r', '\n']) => {
            out.insert("sdpMid".into(), json!(mid));
        }
        _ => return None,
    }
    match o.get("sdpMLineIndex") {
        None | Some(Value::Null) => {}
        Some(v) => match v.as_u64() {
            Some(i) if i < MAX_M_LINES => {
                out.insert("sdpMLineIndex".into(), json!(i));
            }
            _ => return None,
        },
    }
    match o.get("usernameFragment") {
        None | Some(Value::Null) => {}
        Some(Value::String(u)) if u.len() <= MAX_UFRAG => {
            out.insert("usernameFragment".into(), json!(u));
        }
        _ => return None,
    }
    Some(Value::Object(out))
}

/// `data` of an "ice" signal with malformed candidates dropped and at most
/// `MAX_ICE_CANDIDATES` kept. `None` when nothing usable is left.
pub fn clean_ice_signal(data: &Value) -> Option<Value> {
    let list = data.get("candidates")?.as_array()?;
    let clean: Vec<Value> = list.iter().filter_map(clean_candidate).take(MAX_ICE_CANDIDATES).collect();
    (!clean.is_empty()).then(|| json!({ "candidates": clean }))
}

/// Candidate type of an ICE candidate line (`candidate:...` or `a=candidate:...`).
fn candidate_type(candidate: &str) -> Option<&str> {
    let mut it = candidate.split(' ');
    it.find(|t| *t == "typ")?;
    it.next()
}

/// Connection address of an ICE candidate line (5th field).
fn candidate_address(candidate: &str) -> Option<&str> {
    candidate.split(' ').nth(4)
}

/// A relay candidate we may send, with its related address (`raddr`, our public address as
/// the TURN server saw it) blanked; `None` for any other candidate type.
fn scrub_relay_candidate(candidate: &str) -> Option<String> {
    if candidate_type(candidate) != Some("relay") {
        return None;
    }
    let mut out: Vec<&str> = Vec::new();
    let mut blank: Option<&str> = None;
    for t in candidate.split(' ') {
        match blank.take() {
            Some(v) => out.push(v),
            None => {
                blank = match t {
                    "raddr" => Some("0.0.0.0"),
                    "rport" => Some("0"),
                    _ => None,
                };
                out.push(t);
            }
        }
    }
    Some(out.join(" "))
}

/// `data` of an "ice" signal we send with only relay candidates left (relay-only calls,
/// engine/call_settings.rs). `None` when nothing is left to send.
pub fn relay_only_ice_signal(data: &Value) -> Option<Value> {
    let list = data.get("candidates")?.as_array()?;
    let relay: Vec<Value> = list
        .iter()
        .filter_map(|c| {
            let scrubbed = scrub_relay_candidate(c.get("candidate")?.as_str()?)?;
            let mut c = c.clone();
            c["candidate"] = Value::String(scrubbed);
            Some(c)
        })
        .collect();
    if relay.is_empty() {
        return None;
    }
    let mut out = data.clone();
    out["candidates"] = Value::Array(relay);
    Some(out)
}

/// A session description we send with every non-relay candidate removed and connection
/// addresses that are not relay addresses blanked, and the related address of relay candidates
/// blanked too (relay-only calls). With iceTransportPolicy
/// "relay" WebRTC produces no other candidates; this makes sure our IP address never leaves.
pub fn relay_only_sdp(sdp: &str) -> String {
    let relay_addrs: Vec<&str> = sdp
        .split('\n')
        .map(|l| l.strip_suffix('\r').unwrap_or(l))
        .filter_map(|l| l.strip_prefix("a="))
        .filter(|l| l.starts_with("candidate:") && candidate_type(l) == Some("relay"))
        .filter_map(candidate_address)
        .collect();
    let mut out = String::with_capacity(sdp.len());
    for raw in sdp.split_inclusive('\n') {
        let line = raw.trim_end_matches(['\r', '\n']);
        let ending = &raw[line.len()..];
        if let Some(c) = line.strip_prefix("a=").filter(|l| l.starts_with("candidate:")) {
            if let Some(c) = scrub_relay_candidate(c) {
                out.push_str("a=");
                out.push_str(&c);
                out.push_str(ending);
            }
            continue;
        }
        // "c=IN IP4 <addr>" and "a=rtcp:<port> IN IP4 <addr>": the default candidate's address.
        let blanked = match line.rsplit_once(' ') {
            Some((head, addr)) if (line.starts_with("c=IN ") || line.starts_with("a=rtcp:")) && !relay_addrs.contains(&addr) => {
                match head.rsplit(' ').next() {
                    Some("IP4") if addr != "0.0.0.0" => Some(format!("{head} 0.0.0.0")),
                    Some("IP6") if addr != "::" => Some(format!("{head} ::")),
                    _ => None,
                }
            }
            _ => None,
        };
        match blanked {
            Some(l) => out.push_str(&l),
            None => out.push_str(line),
        }
        out.push_str(ending);
    }
    out
}

/// `data` of an offer / answer we send, made relay-only (see [`relay_only_sdp`]).
pub fn relay_only_sdp_signal(data: &Value) -> Value {
    let mut out = data.clone();
    if let Some(sdp) = data.get("sdp").and_then(Value::as_str) {
        out["sdp"] = Value::String(relay_only_sdp(sdp));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    const FP: &str = "sha-256 4A:AD:B9:B1:3F:82:18:3B:54:02:12:DF:3E:5D:49:6B:19:E5:7C:AB:3D:B4:0A:3E:4E:A9:66:9F:12:E5:8A:01";
    const FP2: &str = "sha-256 4A:AD:B9:B1:3F:82:18:3B:54:02:12:DF:3E:5D:49:6B:19:E5:7C:AB:3D:B4:0A:3E:4E:A9:66:9F:12:E5:8A:02";

    /// Chrome / libwebrtc (desktop webview on Windows/Linux, flutter_webrtc) video offer.
    fn chrome_video_offer() -> String {
        [
            "v=0",
            "o=- 4611731400430051336 2 IN IP4 127.0.0.1",
            "s=-",
            "t=0 0",
            "a=group:BUNDLE 0 1",
            "a=extmap-allow-mixed",
            "a=msid-semantic: WMS 5c3b1a",
            "m=audio 9 UDP/TLS/RTP/SAVPF 111 63 9 0 8 13 110 126",
            "c=IN IP4 0.0.0.0",
            "a=rtcp:9 IN IP4 0.0.0.0",
            "a=ice-ufrag:Kq3N",
            "a=ice-pwd:1kJ1iCZ0Wmm0bVjgnvGgZXdr",
            "a=ice-options:trickle",
            &format!("a=fingerprint:{FP}"),
            "a=setup:actpass",
            "a=mid:0",
            "a=extmap:1 urn:ietf:params:rtp-hdrext:ssrc-audio-level",
            "a=sendrecv",
            "a=msid:5c3b1a 7f0a52c1-3c1e-4f0e-9a52-6d1a0b0c9e11",
            "a=rtcp-mux",
            "a=rtpmap:111 opus/48000/2",
            "a=rtcp-fb:111 transport-cc",
            "a=fmtp:111 minptime=10;useinbandfec=1",
            "a=ssrc:3570614608 cname:4TOk42mSjXCkVIa6",
            "m=video 9 UDP/TLS/RTP/SAVPF 96 97 102 103",
            "c=IN IP4 0.0.0.0",
            "a=rtcp:9 IN IP4 0.0.0.0",
            "a=ice-ufrag:Kq3N",
            "a=ice-pwd:1kJ1iCZ0Wmm0bVjgnvGgZXdr",
            "a=ice-options:trickle",
            &format!("a=fingerprint:{FP}"),
            "a=setup:actpass",
            "a=mid:1",
            "a=sendrecv",
            "a=rtcp-mux",
            "a=rtcp-rsize",
            "a=rtpmap:96 VP8/90000",
            "a=rtpmap:97 rtx/90000",
            "a=fmtp:97 apt=96",
            "a=ssrc-group:FID 2231627014 632943048",
            "",
        ]
        .join("\r\n")
    }

    /// Safari / WebKit (desktop webview on macOS) audio answer.
    fn webkit_audio_answer() -> String {
        [
            "v=0",
            "o=- 2971428351937846404 2 IN IP4 127.0.0.1",
            "s=-",
            "t=0 0",
            "a=group:BUNDLE 0",
            "a=msid-semantic: WMS",
            "m=audio 9 UDP/TLS/RTP/SAVPF 111 63 103 9 0 8 105 13 110 113 126",
            "c=IN IP4 0.0.0.0",
            "a=rtcp:9 IN IP4 0.0.0.0",
            "a=ice-ufrag:yOeX",
            "a=ice-pwd:OGWyhrfvyQbmWDV0oUm/BJul",
            "a=ice-options:trickle",
            &format!("a=fingerprint:{FP}"),
            "a=setup:active",
            "a=mid:0",
            "a=sendrecv",
            "a=msid:- 0b0a4f5e-6a31-4c8e-a2b4-7b9d63d3f1aa",
            "a=rtcp-mux",
            "a=rtpmap:111 opus/48000/2",
            "a=fmtp:111 minptime=10;useinbandfec=1",
            "a=ssrc:1093475926 cname:Eh1oM0bQqNWv1XQE",
            "",
        ]
        .join("\r\n")
    }

    /// Firefox style: fingerprint at session level only.
    fn session_fingerprint_offer() -> String {
        [
            "v=0",
            "o=mozilla...THIS_IS_SDPARTA-99.0 1 0 IN IP4 0.0.0.0",
            "s=-",
            "t=0 0",
            &format!("a=fingerprint:{FP}"),
            "a=group:BUNDLE 0",
            "a=ice-options:trickle",
            "m=audio 9 UDP/TLS/RTP/SAVPF 109 0",
            "c=IN IP4 0.0.0.0",
            "a=sendrecv",
            "a=ice-pwd:a0a1b8d2fd1a6c0f05f3a52b2b2a2c51",
            "a=ice-ufrag:3c1e1b2b",
            "a=mid:0",
            "a=setup:actpass",
            "a=rtpmap:109 opus/48000/2",
            "",
        ]
        .join("\r\n")
    }

    #[test]
    fn accepts_real_descriptions() {
        assert!(valid_sdp(&chrome_video_offer()));
        assert!(valid_sdp(&webkit_audio_answer()));
        assert!(valid_sdp(&session_fingerprint_offer()));
        assert!(valid_sdp(&chrome_video_offer().replace("\r\n", "\n")));
        assert!(valid_sdp_signal(&json!({ "sdp": chrome_video_offer(), "video": true })));
        assert!(!valid_sdp_signal(&json!({ "sdp": 5 })));
        assert!(!valid_sdp_signal(&json!(null)));
    }

    #[test]
    fn rejects_data_channels_and_extra_media() {
        let dc = format!(
            "{}m=application 9 UDP/DTLS/SCTP webrtc-datachannel\r\na=mid:2\r\na=sctp-port:5000\r\n",
            chrome_video_offer()
        );
        assert!(!valid_sdp(&dc));
        let two_audio = format!(
            "{}m=audio 9 UDP/TLS/RTP/SAVPF 111\r\na=mid:2\r\na=fingerprint:{FP}\r\na=setup:actpass\r\n",
            webkit_audio_answer()
        );
        assert!(!valid_sdp(&two_audio));
        assert!(!valid_sdp(&webkit_audio_answer().replace("UDP/TLS/RTP/SAVPF", "RTP/AVP")));
        assert!(!valid_sdp(&webkit_audio_answer().replace("UDP/TLS/RTP/SAVPF", "TCP/RTP/AVP")));
        assert!(!valid_sdp(&webkit_audio_answer().replace("a=mid:0", "a=mid:0\r\na=sctp-port:5000")));
        assert!(!valid_sdp(&webkit_audio_answer().replace("m=audio", "m=text")));
    }

    #[test]
    fn rejects_fingerprint_problems() {
        // Different fingerprints in the two sections.
        let offer = chrome_video_offer();
        let i = offer.rfind(FP).unwrap();
        let mixed = format!("{}{FP2}{}", &offer[..i], &offer[i + FP.len()..]);
        assert!(!valid_sdp(&mixed));
        // None at all.
        assert!(!valid_sdp(&webkit_audio_answer().replace(&format!("a=fingerprint:{FP}\r\n"), "")));
        // Two in one section.
        assert!(!valid_sdp(&webkit_audio_answer().replace("a=mid:0", &format!("a=mid:0\r\na=fingerprint:{FP}"))));
        // Weak hash.
        assert!(!valid_sdp(&webkit_audio_answer().replace("sha-256", "sha-1")));
        // Session level and media level disagree.
        let both = session_fingerprint_offer().replace("a=mid:0", &format!("a=mid:0\r\na=fingerprint:{FP2}"));
        assert!(!valid_sdp(&both));
        // Missing setup.
        assert!(!valid_sdp(&webkit_audio_answer().replace("a=setup:active\r\n", "")));
        assert!(!valid_sdp(&webkit_audio_answer().replace("a=setup:active", "a=setup:holdconn")));
    }

    #[test]
    fn rejects_malformed_or_oversized() {
        assert!(!valid_sdp(""));
        assert!(!valid_sdp("garbage"));
        assert!(!valid_sdp(&webkit_audio_answer().replace("v=0", "v=1")));
        assert!(!valid_sdp(&webkit_audio_answer().replace("a=ice-options:trickle", "a=ice-options:trickle bogus")));
        let big = webkit_audio_answer().replace("a=mid:0", &format!("a=mid:0\r\n{}", "a=x-pad:0\r\n".repeat(2000)));
        assert!(big.len() > MAX_SDP_BYTES && !valid_sdp(&big));
        let long_line = webkit_audio_answer().replace("a=mid:0", &format!("a=mid:0\r\na=x:{}", "a".repeat(3000)));
        assert!(!valid_sdp(&long_line));
        let cands = (0..51).map(|i| format!("a=candidate:{i} 1 udp 1 10.0.0.1 {} typ host\r\n", 1000 + i)).collect::<String>();
        assert!(!valid_sdp(&webkit_audio_answer().replace("a=mid:0\r\n", &format!("a=mid:0\r\n{cands}"))));
        assert!(!valid_sdp(&webkit_audio_answer().replace("a=mid:0", "a=mid:0\r\nbad line")));
    }

    #[test]
    fn cleans_ice_candidates() {
        let good = json!({
            "candidate": "candidate:842163049 1 udp 1677729535 203.0.113.7 51234 typ srflx raddr 0.0.0.0 rport 0 generation 0 ufrag Kq3N network-cost 999",
            "sdpMid": "0",
            "sdpMLineIndex": 0,
            "usernameFragment": "Kq3N",
            "extra": "dropped",
        });
        let cleaned = clean_candidate(&good).unwrap();
        assert!(cleaned.get("extra").is_none());
        assert_eq!(cleaned["sdpMid"], "0");
        assert!(clean_candidate(&json!({ "candidate": "a=candidate:1 1 udp 1 1.2.3.4 1 typ host" })).is_none());
        assert!(clean_candidate(&json!({ "candidate": format!("candidate:{}", "1".repeat(600)) })).is_none());
        assert!(clean_candidate(&json!({ "candidate": "candidate:1", "sdpMid": "x".repeat(33) })).is_none());
        assert!(clean_candidate(&json!({ "candidate": "candidate:1", "sdpMLineIndex": 7 })).is_none());
        assert!(clean_candidate(&json!({ "candidate": "candidate:1\r\na=evil" })).is_none());
        assert!(clean_candidate(&json!("candidate:1")).is_none());

        let many: Vec<Value> = (0..80).map(|_| good.clone()).chain([json!({ "candidate": "" })]).collect();
        let data = clean_ice_signal(&json!({ "candidates": many })).unwrap();
        assert_eq!(data["candidates"].as_array().unwrap().len(), MAX_ICE_CANDIDATES);
        assert!(clean_ice_signal(&json!({ "candidates": [{ "candidate": "" }] })).is_none());
        assert!(clean_ice_signal(&json!({ "candidates": "x" })).is_none());
    }

    #[test]
    fn relay_only_filters_what_we_send() {
        let host = json!({ "candidate": "candidate:1 1 udp 2122260223 192.168.1.20 50000 typ host generation 0", "sdpMid": "0" });
        let srflx = json!({ "candidate": "candidate:2 1 udp 1686052607 203.0.113.7 51234 typ srflx raddr 192.168.1.20 rport 50000", "sdpMid": "0" });
        let relay = json!({ "candidate": "candidate:3 1 udp 41885439 198.51.100.9 3478 typ relay raddr 203.0.113.7 rport 51234 generation 0", "sdpMid": "0" });
        let data = relay_only_ice_signal(&json!({ "candidates": [host, srflx, relay] })).unwrap();
        let scrubbed = json!({ "candidate": "candidate:3 1 udp 41885439 198.51.100.9 3478 typ relay raddr 0.0.0.0 rport 0 generation 0", "sdpMid": "0" });
        assert_eq!(data["candidates"], json!([scrubbed]), "our public address in raddr is blanked");
        assert!(relay_only_ice_signal(&json!({ "candidates": [host, srflx] })).is_none(), "nothing left to send");

        let sdp = webkit_audio_answer().replace(
            "a=mid:0\r\n",
            "a=mid:0\r\na=candidate:1 1 udp 2122260223 192.168.1.20 50000 typ host\r\n\
             a=candidate:2 1 udp 1686052607 203.0.113.7 51234 typ srflx raddr 192.168.1.20 rport 50000\r\n\
             a=candidate:3 1 udp 41885439 198.51.100.9 3478 typ relay raddr 203.0.113.7 rport 51234\r\n\
             a=rtcp:9 IN IP4 192.168.1.20\r\nc=IN IP4 203.0.113.7\r\n",
        );
        assert!(valid_sdp(&sdp));
        let out = relay_only_sdp(&sdp);
        assert!(valid_sdp(&out));
        assert!(!out.contains("192.168.1.20") && !out.contains("203.0.113.7"), "{out}");
        assert!(out.contains("typ relay") && out.contains("c=IN IP4 0.0.0.0") && out.contains("a=rtcp:9 IN IP4 0.0.0.0"));
        assert!(!out.contains("\n\n") && out.matches('\n').count() == sdp.matches('\n').count() - 2);
        // A description without candidates (trickle, relay policy) is left alone.
        assert_eq!(relay_only_sdp(&webkit_audio_answer()), webkit_audio_answer());
        let sig = relay_only_sdp_signal(&json!({ "sdp": sdp, "video": true }));
        assert_eq!(sig["video"], true);
        assert!(!sig["sdp"].as_str().unwrap().contains("typ host"));
    }

    #[test]
    fn age_uses_server_time_only() {
        assert_eq!(signal_age(100_000, 40_000), Some(60_000));
        // No server time: treated as stale, never as fresh.
        assert_eq!(signal_age(100_000, 0), None);
    }
}
