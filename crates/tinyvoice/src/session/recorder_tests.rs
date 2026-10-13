use super::*;

fn press(r: &mut HotkeyRecorder) -> SessionAdvance {
    r.advance(SessionInput::Pressed)
}

fn running() -> HotkeyRecorder {
    let mut r = HotkeyRecorder::new();
    assert!(r.advance(SessionInput::Run).accepted);
    r
}

fn ready(r: &mut HotkeyRecorder, stop_buffered: bool) -> SessionAdvance {
    r.advance(SessionInput::SetupReady { stop_buffered })
}

#[test]
fn a_stopped_recorder_ignores_the_hotkey() {
    let mut r = HotkeyRecorder::new();
    let reply = press(&mut r);
    assert!(!reply.accepted);
    assert_eq!(reply.action, SessionAction::None);
    assert_eq!(reply.phase, SessionPhase::Stopped);
    assert!(!r.advance(SessionInput::Released).accepted);
}

#[test]
fn run_is_refused_while_already_running() {
    let mut r = running();
    assert!(!r.advance(SessionInput::Run).accepted);
}

#[test]
fn press_starts_a_recording_and_release_stops_it() {
    let mut r = running();
    let started = press(&mut r);
    assert_eq!(
        started.action,
        SessionAction::StartRecording { generation: 1 }
    );
    assert_eq!(started.phase, SessionPhase::Recording);
    assert_eq!(
        ready(&mut r, false).action,
        SessionAction::RecordingLive { generation: 1 }
    );
    let stopped = r.advance(SessionInput::Released);
    assert_eq!(
        stopped.action,
        SessionAction::StopRecording { generation: 1 }
    );
}

#[test]
fn press_while_recording_toggles_it_off() {
    let mut r = running();
    press(&mut r);
    ready(&mut r, false);
    assert_eq!(
        press(&mut r).action,
        SessionAction::StopRecording { generation: 1 }
    );
}

#[test]
fn release_with_no_recording_is_a_no_op() {
    let mut r = running();
    let reply = r.advance(SessionInput::Released);
    assert!(reply.accepted);
    assert_eq!(reply.action, SessionAction::None);
    assert_eq!(reply.phase, SessionPhase::Idle);
}

#[test]
fn a_stop_during_setup_is_deferred_until_the_minimum_after_setup() {
    let mut r = running();
    press(&mut r);
    assert_eq!(
        r.advance(SessionInput::Released).action,
        SessionAction::None
    );
    assert_eq!(
        ready(&mut r, false).action,
        SessionAction::DeferStop {
            generation: 1,
            after_ms: DEFERRED_STOP_MS
        }
    );
    assert_eq!(
        r.advance(SessionInput::DeferredStopElapsed).action,
        SessionAction::StopRecording { generation: 1 }
    );
    assert!(!r.advance(SessionInput::DeferredStopElapsed).accepted);
}

#[test]
fn a_second_press_during_setup_counts_as_stop_intent() {
    let mut r = running();
    press(&mut r);
    assert_eq!(press(&mut r).action, SessionAction::None);
    assert!(matches!(
        ready(&mut r, false).action,
        SessionAction::DeferStop { .. }
    ));
}

#[test]
fn a_stop_event_found_queued_behind_setup_defers_the_stop() {
    let mut r = running();
    press(&mut r);
    assert!(matches!(
        ready(&mut r, true).action,
        SessionAction::DeferStop { generation: 1, .. }
    ));
}

#[test]
fn a_deferred_stop_can_be_preempted_by_an_explicit_press() {
    let mut r = running();
    press(&mut r);
    r.advance(SessionInput::Released);
    ready(&mut r, false);
    assert_eq!(
        press(&mut r).action,
        SessionAction::StopRecording { generation: 1 }
    );
    assert!(!r.advance(SessionInput::DeferredStopElapsed).accepted);
}

#[test]
fn setup_failure_returns_to_idle_and_clears_buffered_intent() {
    let mut r = running();
    press(&mut r);
    r.advance(SessionInput::Released);
    let failed = r.advance(SessionInput::SetupFailed);
    assert!(failed.accepted);
    assert_eq!(failed.phase, SessionPhase::Idle);
    // The buffered stop must not leak into the next recording.
    press(&mut r);
    assert_eq!(
        ready(&mut r, false).action,
        SessionAction::RecordingLive { generation: 2 }
    );
    assert!(!r.advance(SessionInput::SetupFailed).accepted);
}

#[test]
fn setup_results_without_a_pending_setup_are_refused() {
    let mut r = running();
    assert!(!ready(&mut r, false).accepted);
}

#[test]
fn a_press_during_transcription_starts_the_next_recording() {
    let mut r = running();
    press(&mut r);
    ready(&mut r, false);
    r.advance(SessionInput::Released);
    assert!(
        r.advance(SessionInput::Transcribing { generation: 1 })
            .accepted
    );
    assert_eq!(r.status().phase, SessionPhase::Transcribing);
    let next = press(&mut r);
    assert_eq!(next.action, SessionAction::StartRecording { generation: 2 });
    assert_eq!(next.phase, SessionPhase::Recording);
}

#[test]
fn stale_processing_cannot_reset_newer_recording_state() {
    let mut r = running();
    press(&mut r);
    ready(&mut r, false);
    r.advance(SessionInput::Released);
    press(&mut r);
    let stale = r.advance(SessionInput::Finished { generation: 1 });
    assert!(!stale.accepted);
    assert_eq!(stale.phase, SessionPhase::Recording);
    assert!(
        !r.advance(SessionInput::Transcribing { generation: 1 })
            .accepted
    );
}

#[test]
fn current_processing_can_update_state() {
    let mut r = running();
    press(&mut r);
    ready(&mut r, false);
    r.advance(SessionInput::Released);
    assert!(
        r.advance(SessionInput::Transcribing { generation: 1 })
            .accepted
    );
    let done = r.advance(SessionInput::Finished { generation: 1 });
    assert!(done.accepted);
    assert_eq!(done.phase, SessionPhase::Idle);
}

#[test]
fn stop_invalidates_in_flight_work_and_allows_a_fresh_run() {
    let mut r = running();
    press(&mut r);
    ready(&mut r, false);
    r.advance(SessionInput::Released);
    let stopped = r.advance(SessionInput::Stop);
    assert_eq!(stopped.phase, SessionPhase::Stopped);
    assert!(!r.advance(SessionInput::Finished { generation: 1 }).accepted);
    assert_eq!(r.status().phase, SessionPhase::Stopped);
    assert!(r.advance(SessionInput::Run).accepted);
    assert_eq!(
        press(&mut r).action,
        SessionAction::StartRecording { generation: 3 }
    );
}
