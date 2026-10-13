#![allow(clippy::unwrap_used)]

use super::*;

const GATES: RecordingGates = RecordingGates {
    min_duration_secs: 0.3,
    silence_threshold: 0.002,
};

#[test]
fn a_stale_generation_cannot_move_the_session() {
    let mut s = RecordingSession::new();
    assert!(s.run());
    assert!(!s.run());
    let first = s.begin_recording().unwrap();
    assert!(s.begin_transcribing(first));
    assert!(s.finish(first));
    let second = s.begin_recording().unwrap();
    assert!(!s.finish(first), "stale finish must be ignored");
    assert_eq!(s.phase(), SessionPhase::Recording);
    assert!(s.begin_transcribing(second));
    s.stop();
    assert!(!s.finish(second));
    assert_eq!(s.phase(), SessionPhase::Stopped);
    assert!(s.begin_recording().is_none());
}

#[test]
fn gates_drop_short_and_silent_recordings() {
    assert_eq!(gate_recording(GATES, 0.1, 0.5), GateVerdict::TooShort);
    assert_eq!(gate_recording(GATES, 1.0, 0.0001), GateVerdict::Silent);
    assert_eq!(gate_recording(GATES, 1.0, 0.5), GateVerdict::Pass);
}

#[test]
fn sequencing_distinguishes_command_bare_wake_and_unaddressed() {
    assert_eq!(
        sequence_utterance("hey tiny open slack", "Hey Tiny"),
        UtteranceDisposition::Command {
            command: "open slack".into()
        }
    );
    assert_eq!(
        sequence_utterance("hey tiny", "Hey Tiny"),
        UtteranceDisposition::BareWake
    );
    assert_eq!(
        sequence_utterance("what a nice day", "Hey Tiny"),
        UtteranceDisposition::Ignored
    );
    assert_eq!(
        sequence_utterance("  ", "Hey Tiny"),
        UtteranceDisposition::Ignored
    );
}

#[test]
fn recognised_intents_take_the_fast_path_and_the_rest_go_to_the_agent() {
    assert!(matches!(
        plan_delivery("pause the music"),
        Delivery::FastPath {
            intent: VoiceIntent::Pause,
            ..
        }
    ));
    assert_eq!(
        plan_delivery("what is the weather in Tokyo"),
        Delivery::Agent {
            command: "what is the weather in Tokyo".into()
        }
    );
}

struct Fake {
    text: Result<String, String>,
    fail_fast: bool,
    delivered: Vec<Delivery>,
}

impl SessionHost for Fake {
    fn transcribe(&mut self, _audio: &[u8]) -> Result<String, String> {
        self.text.clone()
    }
    fn deliver(&mut self, delivery: Delivery) -> Result<(), String> {
        let fast = matches!(delivery, Delivery::FastPath { .. });
        self.delivered.push(delivery);
        if fast && self.fail_fast {
            Err("no local fast path".into())
        } else {
            Ok(())
        }
    }
}

fn fake(text: &str) -> Fake {
    Fake {
        text: Ok(text.into()),
        fail_fast: false,
        delivered: vec![],
    }
}

#[test]
fn an_unaddressed_utterance_is_never_delivered() {
    let mut host = fake("what a nice day");
    let out = process_utterance(&mut host, GATES, "Hey Tiny", b"", 1.0, 0.5);
    assert_eq!(out, Outcome::Dropped);
    assert!(host.delivered.is_empty());
}

#[test]
fn a_gated_recording_is_not_transcribed() {
    let mut host = fake("hey tiny hello");
    let out = process_utterance(&mut host, GATES, "Hey Tiny", b"", 0.1, 0.5);
    assert_eq!(out, Outcome::Gated(GateVerdict::TooShort));
}

#[test]
fn a_failed_fast_path_falls_back_to_the_agent() {
    let mut host = fake("hey tiny pause the music");
    host.fail_fast = true;
    let out = process_utterance(&mut host, GATES, "Hey Tiny", b"", 1.0, 0.5);
    assert_eq!(out, Outcome::Delivered);
    assert_eq!(
        host.delivered.last(),
        Some(&Delivery::Agent {
            command: "pause the music".into()
        })
    );
}

#[test]
fn a_bare_wake_word_is_acknowledged_through_the_agent() {
    let mut host = fake("hey tiny");
    let out = process_utterance(&mut host, GATES, "Hey Tiny", b"", 1.0, 0.5);
    assert_eq!(out, Outcome::Delivered);
    assert_eq!(
        host.delivered,
        vec![Delivery::Agent {
            command: BARE_WAKE_ACK.into()
        }]
    );
}

#[test]
fn a_transcription_failure_is_reported_not_delivered() {
    let mut host = Fake {
        text: Err("boom".into()),
        fail_fast: false,
        delivered: vec![],
    };
    let out = process_utterance(&mut host, GATES, "Hey Tiny", b"", 1.0, 0.5);
    assert_eq!(out, Outcome::TranscribeFailed("boom".into()));
}
