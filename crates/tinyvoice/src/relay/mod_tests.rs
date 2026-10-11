//! Regressions for the text semantics migrated from the host voice relay.
use super::*;
use serde_json::json;

fn prepare(messages: Vec<Value>, prefix: &str) -> PreparedRelayTurn {
    prepare_turn(&RelayTurnRequest {
        messages,
        readback_prefix: prefix.into(),
    })
}

#[test]
fn last_user_content_is_authoritative_even_when_empty_or_malformed() {
    for content in [
        Value::Null,
        json!({"text": "not a content part"}),
        json!(5),
        json!(""),
    ] {
        let result = prepare(
            vec![
                json!({"role":"user", "content":"earlier"}),
                json!({"role":"user", "content":content}),
            ],
            "Read:",
        );
        assert_eq!(result.prompt, "");
        assert!(result.content_free);
        assert!(!result.answerable);
        assert!(result.arm_speak_back);
    }
    assert_eq!(prepare(vec![], "Read:").prompt, "");
    assert_eq!(
        prepare(
            vec![json!({"role":"assistant", "content":"answer"})],
            "Read:"
        )
        .prompt,
        ""
    );
}

#[test]
fn multimodal_text_and_history_preserve_order_roles_and_whitespace() {
    let result = prepare(
        vec![
            json!(null),
            json!({"content":"missing role"}),
            json!({"role":"system", "content":"private"}),
            json!({"role":"tool", "content":"private"}),
            json!({"role":"user", "content":"   "}),
            json!({"role":"assistant", "content":"answer"}),
            json!({"role":"agent", "content":[{"text":"hello"},{"image":"ignored"},{"text":"世界"},{"text":4}]}),
            json!({"role":"user", "content":"question"}),
        ],
        "Read:",
    );
    assert_eq!(
        result.history,
        vec![
            ("assistant".into(), "answer".into()),
            ("agent".into(), "hello 世界".into()),
            ("user".into(), "question".into())
        ]
    );
    assert_eq!(result.prompt, "question");
    assert!(result.answerable);
}

#[test]
fn unicode_letters_and_digits_are_content_but_recognition_artifacts_are_not() {
    for text in ["...", "…", " ? ", "-", ""] {
        assert!(prepare(vec![json!({"role":"user", "content":text})], "Read:").content_free);
    }
    for text in ["summarize my emails", "3", "मेरे ईमेल पढ़ो", "总结我的邮件"]
    {
        let result = prepare(vec![json!({"role":"user", "content":text})], "Read:");
        assert!(!result.content_free);
        assert!(result.answerable);
        assert!(result.arm_speak_back);
    }
}

#[test]
fn host_prefix_suppresses_recursive_readback_and_preserves_empty_payload() {
    for (text, payload) in [("Read:", ""), ("  Read: \n hello \n", "hello")] {
        let result = prepare(vec![json!({"role":"user", "content":text})], "Read:");
        assert_eq!(result.readback_payload.as_deref(), Some(payload));
        assert!(!result.answerable);
        assert!(!result.arm_speak_back);
    }
    assert_eq!(
        prepare(
            vec![json!({"role":"user", "content":"read: hello"})],
            "Read:"
        )
        .readback_payload,
        None
    );
    let result = prepare(vec![json!({"role":"user", "content":"hello"})], "");
    assert_eq!(result.readback_payload.as_deref(), Some("hello"));
    assert!(!result.arm_speak_back);
}
