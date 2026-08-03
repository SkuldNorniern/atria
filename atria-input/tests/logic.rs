use atria_input::{
    EpochChange, InputBatch, InputError, InputPipeline, KeyAction, KeyCode, KeyNormalizer,
    PipelineStatus, RawInputEvent, RawReport, ReportAccumulator, ReportStatus, capabilities,
};
use atria_protocol::capability::Capability;

const EV_SYN: u16 = 0;
const EV_KEY: u16 = 1;
const SYN_REPORT: u16 = 0;

fn raw(event_type: u16, code: u16, value: i32) -> RawInputEvent {
    RawInputEvent::new(4, 250_000, event_type, code, value)
}

fn report(events: &[RawInputEvent]) -> RawReport {
    let mut accumulator = ReportAccumulator::new();
    for event in events {
        assert!(matches!(
            accumulator.push(*event),
            Ok(ReportStatus::Pending)
        ));
    }
    match accumulator.push(raw(EV_SYN, SYN_REPORT, 0)) {
        Ok(ReportStatus::Complete(report)) => report,
        Ok(ReportStatus::Pending | ReportStatus::Discontinuity) | Err(_) => {
            panic!("SYN_REPORT did not complete the report")
        }
    }
}

fn push_report(pipeline: &mut InputPipeline, events: &[RawInputEvent]) -> InputBatch {
    for event in events {
        assert!(matches!(pipeline.push(*event), Ok(PipelineStatus::Pending)));
    }
    match pipeline.push(raw(EV_SYN, SYN_REPORT, 0)) {
        Ok(PipelineStatus::Batch(batch)) => batch,
        Ok(PipelineStatus::Pending | PipelineStatus::ReacquireKeyState) | Err(_) => {
            panic!("report did not produce a key batch")
        }
    }
}

#[test]
fn partial_reports_are_not_visible() {
    let mut accumulator = ReportAccumulator::new();
    assert!(matches!(
        accumulator.push(raw(EV_KEY, 103, 1)),
        Ok(ReportStatus::Pending)
    ));
    let complete = match accumulator.push(raw(EV_SYN, SYN_REPORT, 0)) {
        Ok(ReportStatus::Complete(report)) => report,
        Ok(ReportStatus::Pending | ReportStatus::Discontinuity) | Err(_) => {
            panic!("SYN_REPORT did not complete the report")
        }
    };
    assert_eq!(complete.events(), &[raw(EV_KEY, 103, 1)]);
}

#[test]
fn an_oversized_report_is_discarded_through_its_boundary() {
    let mut accumulator = ReportAccumulator::new();
    for _ in 0..256 {
        assert!(matches!(
            accumulator.push(raw(EV_KEY, 103, 1)),
            Ok(ReportStatus::Pending)
        ));
    }
    assert!(matches!(
        accumulator.push(raw(EV_KEY, 106, 1)),
        Err(InputError::ReportTooLarge)
    ));
    assert!(matches!(
        accumulator.push(raw(EV_KEY, 108, 1)),
        Ok(ReportStatus::Pending)
    ));
    assert!(matches!(
        accumulator.push(raw(EV_SYN, SYN_REPORT, 0)),
        Ok(ReportStatus::Discontinuity)
    ));
}

#[test]
fn key_press_repeat_and_release_are_typed_transitions() {
    let complete = report(&[
        raw(EV_KEY, 103, 1),
        raw(EV_KEY, 103, 2),
        raw(EV_KEY, 103, 0),
    ]);
    let mut normalizer = KeyNormalizer::new();
    let epoch = InputPipeline::new().epoch();
    let events = match normalizer.normalize(&complete, epoch) {
        Ok(events) => events,
        Err(error) => panic!("normalization failed: {error}"),
    };
    assert_eq!(events.len(), 3);
    assert_eq!(events[0].code, KeyCode::Up);
    assert_eq!(events[0].action, KeyAction::Pressed);
    assert_eq!(events[1].action, KeyAction::Repeated);
    assert_eq!(events[2].action, KeyAction::Released);
    assert_eq!(events[0].timestamp_ns, 4_250_000_000);
    assert_eq!(events[0].epoch, epoch);
    assert!(!normalizer.is_down(KeyCode::Up));
}

#[test]
fn invalid_key_values_are_rejected() {
    let complete = report(&[raw(EV_KEY, 103, 3)]);
    assert_eq!(
        KeyNormalizer::new().normalize(&complete, InputPipeline::new().epoch()),
        Err(InputError::InvalidKeyValue(3))
    );
}

#[test]
fn a_rejected_report_does_not_mutate_key_state() {
    let complete = report(&[raw(EV_KEY, 103, 1), raw(EV_KEY, 106, 3)]);
    let mut normalizer = KeyNormalizer::new();
    assert!(matches!(
        normalizer.normalize(&complete, InputPipeline::new().epoch()),
        Err(InputError::InvalidKeyValue(3))
    ));
    assert!(!normalizer.is_down(KeyCode::Up));
}

#[test]
fn capabilities_name_only_normalized_keys() {
    let available = capabilities();
    assert!(available.contains(Capability::InputKeys));
    assert!(!available.contains(Capability::ExtendedInput));
}

#[test]
fn an_event_from_an_earlier_epoch_is_not_delivered() {
    let mut pipeline = InputPipeline::new();
    let old = push_report(&mut pipeline, &[raw(EV_KEY, 103, 1)]);
    let old_event = old.events()[0];
    assert!(pipeline.advance(EpochChange::Routing).is_ok());

    assert!(pipeline.accept(old_event).is_none());
}

#[test]
fn every_boundary_kind_advances_the_epoch() {
    let mut pipeline = InputPipeline::new();
    let initial = pipeline.epoch();
    let seat = pipeline.advance(EpochChange::Seat);
    let device = pipeline.advance(EpochChange::Device);
    let routing = pipeline.advance(EpochChange::Routing);

    assert!(matches!(seat, Ok(epoch) if epoch.value() == initial.value() + 1));
    assert!(matches!(device, Ok(epoch) if epoch.value() == initial.value() + 2));
    assert!(matches!(routing, Ok(epoch) if epoch.value() == initial.value() + 3));
}

#[test]
fn a_key_held_before_focus_change_is_not_observable_after_it() {
    let mut pipeline = InputPipeline::new();
    let _pressed = push_report(&mut pipeline, &[raw(EV_KEY, 103, 1)]);
    assert!(pipeline.advance(EpochChange::Routing).is_ok());

    let repeated = push_report(&mut pipeline, &[raw(EV_KEY, 103, 2)]);
    let released = push_report(&mut pipeline, &[raw(EV_KEY, 103, 0)]);
    assert!(repeated.events().is_empty());
    assert!(released.events().is_empty());
}

#[test]
fn syn_dropped_discards_the_report_and_reacquires_a_suppressed_baseline() {
    const SYN_DROPPED: u16 = 3;
    let mut pipeline = InputPipeline::new();
    assert!(matches!(
        pipeline.push(raw(EV_KEY, 103, 1)),
        Ok(PipelineStatus::Pending)
    ));
    assert!(matches!(
        pipeline.push(raw(EV_SYN, SYN_DROPPED, 0)),
        Ok(PipelineStatus::Pending)
    ));
    assert!(matches!(
        pipeline.push(raw(EV_KEY, 106, 1)),
        Ok(PipelineStatus::Pending)
    ));
    assert!(matches!(
        pipeline.push(raw(EV_SYN, SYN_REPORT, 0)),
        Ok(PipelineStatus::ReacquireKeyState)
    ));

    let mut bitmap = [0_u8; 96];
    bitmap[103 / 8] |= 1 << (103 % 8);
    let discontinuity = match pipeline.reacquire(&bitmap) {
        Ok(batch) => batch,
        Err(error) => panic!("reacquisition failed: {error}"),
    };
    assert!(matches!(discontinuity, InputBatch::Discontinuity { .. }));
    let repeat = push_report(&mut pipeline, &[raw(EV_KEY, 103, 2)]);
    let release = push_report(&mut pipeline, &[raw(EV_KEY, 103, 0)]);
    assert!(repeat.events().is_empty());
    assert!(release.events().is_empty());
}
