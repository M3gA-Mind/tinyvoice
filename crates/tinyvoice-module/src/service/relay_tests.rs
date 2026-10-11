use super::*;

#[test]
fn serialization_counts_escapes_and_accepts_the_exact_boundary() {
    let value = "\n世界";
    let size = serde_json::to_vec(value).unwrap().len();
    assert!(fits(&value, size));
    assert!(!fits(&value, size - 1));
}

#[test]
fn empty_write_and_flush_do_not_consume_capacity() {
    let mut writer = SizeLimit { remaining: 0 };
    assert_eq!(writer.write(b"").unwrap(), 0);
    writer.flush().unwrap();
    assert!(writer.write(b"x").is_err());
}

#[test]
fn many_messages_share_one_aggregate_limit() {
    let request = RelayTurnRequest {
        messages: (0..5)
            .map(|_| serde_json::json!({"role":"assistant","content":"x".repeat(1024 * 1024)}))
            .collect(),
        readback_prefix: String::new(),
    };
    assert!(prepare(&request).is_err());
}

#[test]
fn near_limit_text_can_be_copied_into_all_three_reply_fields() {
    let mut request = RelayTurnRequest {
        messages: vec![serde_json::json!({"role":"user","content":""})],
        readback_prefix: String::new(),
    };
    let overhead = serde_json::to_vec(&request).unwrap().len();
    let text = "x".repeat(MAX_RELAY_REQUEST_BYTES - overhead);
    request.messages[0]["content"] = serde_json::Value::String(text.clone());
    let reply = prepare(&request).unwrap();
    assert_eq!(reply.prompt, text);
    assert_eq!(reply.history[0].1, text);
    assert_eq!(reply.readback_payload.as_deref(), Some(text.as_str()));
    request.readback_prefix.push('x');
    assert!(prepare(&request).is_err());
}
