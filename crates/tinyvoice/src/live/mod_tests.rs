use super::*;

#[test]
fn only_a_start_text_frame_can_open_the_live_protocol() -> Result<()> {
    for frame in [
        TransportFrame::Audio { pcm16: vec![0, 1] },
        TransportFrame::Closed,
        TransportFrame::Other,
        TransportFrame::Text {
            text: r#"{"type":"stop"}"#.into(),
        },
    ] {
        assert_eq!(
            prepare_input(PrepareLiveInputRequest {
                awaiting_start: true,
                frame
            }),
            Err(Error::LiveProtocol(
                "the first frame must be a start frame".into()
            ))
        );
    }
    let malformed = prepare_input(PrepareLiveInputRequest {
        awaiting_start: true,
        frame: TransportFrame::Text {
            text: "invalid".into(),
        },
    });
    assert!(
        matches!(malformed, Err(Error::LiveProtocol(message)) if message.starts_with("bad start frame: "))
    );
    let started = prepare_input(PrepareLiveInputRequest {
        awaiting_start: true,
        frame: TransportFrame::Text {
            text: r#"{"type":"start","provider":"custom","input_sample_rate":0}"#.into(),
        },
    })?;
    assert!(
        matches!(started, PreparedLiveInput::Control(ClientFrame::Start {
        provider: Some(provider), input_sample_rate: 0, ..
    }) if provider == "custom")
    );
    Ok(())
}

#[test]
fn later_control_frames_keep_text_and_ignore_duplicate_start_or_bad_json() -> Result<()> {
    for (text, expected) in [
        (
            r#"{"type":"text","text":"  hello 世界  "}"#,
            ClientFrame::Text {
                text: "  hello 世界  ".into(),
            },
        ),
        (r#"{"type":"interrupt"}"#, ClientFrame::Interrupt),
        (r#"{"type":"stop"}"#, ClientFrame::Stop),
    ] {
        assert_eq!(
            prepare_input(PrepareLiveInputRequest {
                awaiting_start: false,
                frame: TransportFrame::Text { text: text.into() },
            })?,
            PreparedLiveInput::Control(expected)
        );
    }
    for text in [r#"{"type":"start"}"#, "invalid", r#"{"type":"unknown"}"#] {
        assert_eq!(
            prepare_input(PrepareLiveInputRequest {
                awaiting_start: false,
                frame: TransportFrame::Text { text: text.into() },
            })?,
            PreparedLiveInput::Transport(LiveTransportAction::Ignore)
        );
    }
    Ok(())
}

#[test]
fn audio_disconnect_and_non_command_frames_preserve_transport_actions() -> Result<()> {
    for (frame, expected) in [
        (
            TransportFrame::Audio {
                pcm16: vec![0, 255],
            },
            LiveTransportAction::Audio {
                pcm16: vec![0, 255],
            },
        ),
        (TransportFrame::Closed, LiveTransportAction::Closed),
        (TransportFrame::Other, LiveTransportAction::Ignore),
    ] {
        assert_eq!(
            prepare_input(PrepareLiveInputRequest {
                awaiting_start: false,
                frame
            })?,
            PreparedLiveInput::Transport(expected)
        );
    }
    Ok(())
}
