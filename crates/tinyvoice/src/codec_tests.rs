use super::*;

#[test]
fn m4a_repair_rejects_invalid_muxer_output_without_panicking() {
    assert!(repair_m4a(Vec::new(), 1).is_err());
    assert!(repair_m4a(b"not an MP4".to_vec(), 1).is_err());
}

#[test]
fn normalized_waveform_peak_uses_the_full_byte_range() {
    assert_eq!(normalized_peak_byte(0.0), 0);
    assert_eq!(normalized_peak_byte(0.5), 128);
    assert_eq!(normalized_peak_byte(1.0), 255);
    assert_eq!(normalized_peak_byte(2.0), 255);
}
