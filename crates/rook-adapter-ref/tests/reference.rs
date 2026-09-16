//! The reference fixture end to end: the committed capsules match a fresh
//! recording byte for byte, the old component reproduces its own baseline
//! through the adapter protocol, and the candidate matrix gives the verdicts
//! the contract promises. Set ROOK_WRITE_FIXTURES=1 to rewrite the
//! fixtures; that is a golden change.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::Command;

use rook_adapter_ref::component::Variant;
use rook_adapter_ref::driver::{Run, Stop, compare, replay};
use rook_adapter_ref::fixture::{Scenario, capsule_files, decode_captured, scenarios};
use rook_adapter_ref::property::{
    PROPERTY_TIMELY_ACK, PROPERTY_TIMEOUT_CANCEL, PropertyResult, Scope, Verdict, property_input,
};
use rook_adapter_ref::protocol::{FinishReason, Pending, StepStatus};
use rook_adapter_ref::{CORPUS, GOAL_RESPONSE_DEADLINE_NS, encode_hex};
use rook_native::{EventType, Frame, decode_stream, raw_record_hash};

fn fixture_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/native-ref")
}

fn scenario(name: &str) -> Scenario {
    scenarios()
        .into_iter()
        .find(|scenario| scenario.name == name)
        .expect("scenario")
}

fn recording(name: &str) -> Vec<Frame> {
    let bytes = std::fs::read(fixture_root().join(name).join("events.bin")).expect("events.bin");
    decode_stream(&bytes).expect("committed recording decodes")
}

fn key_values(path: &Path) -> BTreeMap<String, String> {
    std::fs::read_to_string(path)
        .expect("read key=value file")
        .lines()
        .filter(|line| !line.trim().is_empty() && !line.starts_with('#'))
        .map(|line| {
            let (key, value) = line.split_once('=').expect("key = value");
            (key.trim().to_string(), value.trim().to_string())
        })
        .collect()
}

fn adapter(variant: Variant) -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_rook-adapter-ref"));
    command.args(["serve", "--component", variant.name()]);
    command
}

fn run(name: &str, variant: Variant) -> (Vec<Frame>, Run) {
    let frames = recording(name);
    let run = replay(&mut adapter(variant), &frames, CORPUS, name).expect("replay");
    (frames, run)
}

/// Runs the property program binary on the run and returns its result and
/// exit code.
fn property(name: &str, run: &Run) -> (PropertyResult, i32) {
    let scenario = scenario(name);
    let input = property_input(
        scenario.property,
        name,
        Scope {
            completion: scenario.completion.to_string(),
            observation_end: "end_of_recording".to_string(),
        },
        GOAL_RESPONSE_DEADLINE_NS,
        run,
        key_values(&fixture_root().join(name).join("identity")),
    );
    let path = std::env::temp_dir().join(format!(
        "rook-property-input-{name}-{}-{}.json",
        std::process::id(),
        run.frames.len()
    ));
    std::fs::write(&path, serde_json::to_string_pretty(&input).unwrap()).unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_rook-property-ref"))
        .args(["evaluate", path.to_str().unwrap()])
        .output()
        .expect("run rook-property-ref");
    std::fs::remove_file(&path).ok();
    let stdout = String::from_utf8(output.stdout).unwrap();
    let result: PropertyResult = serde_json::from_str(stdout.trim()).expect("result line");
    (result, output.status.code().unwrap())
}

#[test]
fn committed_capsules_match_a_fresh_recording_byte_for_byte() {
    for scenario in scenarios() {
        let dir = fixture_root().join(scenario.name);
        let files = capsule_files(&scenario).expect("capsule files");
        if std::env::var_os("ROOK_WRITE_FIXTURES").is_some() {
            std::fs::create_dir_all(&dir).unwrap();
            for (name, bytes) in &files {
                std::fs::write(dir.join(name), bytes).unwrap();
            }
        }
        for (name, bytes) in &files {
            let committed = std::fs::read(dir.join(name))
                .unwrap_or_else(|_| panic!("{}/{name} missing", scenario.name));
            assert_eq!(&committed, bytes, "{}/{name} drifted", scenario.name);
        }
    }
}

#[test]
fn old_component_reproduces_every_baseline_through_the_protocol() {
    for scenario in scenarios() {
        let (frames, run) = run(scenario.name, Variant::Old);
        let comparison = compare(CORPUS, &frames, &run);
        assert!(
            comparison.effects_agree(),
            "{}: first differing effect {:?}",
            scenario.name,
            comparison.first_difference
        );
        let expected = key_values(&fixture_root().join(scenario.name).join("expected"));
        assert_eq!(
            encode_hex(&comparison.recorded_hashes.run_hash),
            expected["run_hash"]
        );
        if let Stop::Gap { ordinal, .. } = run.stop {
            let expected_gap = match scenario.name {
                "timeout-gap" => 6,
                "command-gap" => 3,
                "command-end" => unreachable!("command-end has no gap"),
                other => panic!("{other} stopped at an unexpected gap"),
            };
            assert_eq!(ordinal, expected_gap, "{}", scenario.name);
            continue;
        }
        assert_eq!(run.stop, Stop::Exhausted);
        assert_eq!(run.finish_reason, FinishReason::Exhausted);
        assert!(
            comparison.run_agrees(),
            "{}: replayed run hash differs from the recording",
            scenario.name
        );
        assert_eq!(
            encode_hex(&comparison.replayed_hashes.output_digest),
            expected["output_digest"]
        );
        // The independently captured effects are the replayed effect payloads.
        let captured = decode_captured(
            &std::fs::read(
                fixture_root()
                    .join(scenario.name)
                    .join("effects_captured.bin"),
            )
            .unwrap(),
        )
        .unwrap();
        let replayed: Vec<Vec<u8>> = run
            .frames
            .iter()
            .filter(|frame| frame.kind() == rook_native::EnvelopeKind::Emit)
            .map(Frame::payload)
            .collect();
        assert_eq!(captured, replayed, "{}: captured effects", scenario.name);
        assert_eq!(
            encode_hex(&raw_record_hash(
                &std::fs::read(fixture_root().join(scenario.name).join("events.bin")).unwrap()
            )),
            expected["raw_record_blake3"]
        );
    }
}

#[test]
fn timeout_scenario_matrix() {
    let (frames, run) = self::run("timeout", Variant::Fixed);
    let comparison = compare(CORPUS, &frames, &run);
    // The candidate's cancel replaces the old component's status line: the
    // difference is visible at the first effect after the goal send.
    assert_eq!(comparison.first_difference, Some(1));
    assert_eq!(run.stop, Stop::Exhausted);
    // The cancel was emitted while its response future is still pending,
    // and finish reports that pending future rather than inventing a reply.
    let cancel_ordinal = run
        .frames
        .iter()
        .find(|frame| frame.header.event_type == EventType::CancelSend)
        .expect("fixed cancels")
        .ordinal;
    assert!(
        run.effects
            .iter()
            .any(|(_, ordinal)| *ordinal == cancel_ordinal)
    );
    assert_eq!(
        run.pending_at_finish,
        vec![Pending::Future {
            callback: 2,
            name: "cancel_response".to_string()
        }]
    );
    let (result, code) = property("timeout", &run);
    assert_eq!(
        (result.result, code),
        (Verdict::Pass, 0),
        "{}",
        result.detail
    );
    assert_eq!(result.completion_ordinal, Some(cancel_ordinal));
    assert!(cancel_ordinal < run.frames.last().unwrap().ordinal);
    assert_eq!(
        result.unavailable,
        vec!["cancel_acknowledged", "goal_terminated"]
    );

    let (_, run) = self::run("timeout", Variant::Old);
    let (result, code) = property("timeout", &run);
    assert_eq!(
        (result.result, code),
        (Verdict::Fail, 1),
        "{}",
        result.detail
    );
    assert_eq!(result.predicate.as_deref(), Some("cancel_request_issued"));
    assert_eq!(result.ordinal, Some(6));

    let (_, run) = self::run("timeout", Variant::Noop);
    assert!(run.effects.is_empty());
    assert!(
        run.steps
            .iter()
            .any(|step| step.status == StepStatus::NotConsumed),
        "unconsumed clock observations are reported"
    );
    let (result, code) = property("timeout", &run);
    assert_eq!(
        (result.result, code),
        (Verdict::Fail, 1),
        "{}",
        result.detail
    );
    assert_eq!(result.predicate.as_deref(), Some("goal_submitted"));

    let (_, run) = self::run("timeout", Variant::AlwaysCancel);
    let (result, code) = property("timeout", &run);
    assert_eq!(
        (result.result, code),
        (Verdict::Fail, 1),
        "{}",
        result.detail
    );
    assert_eq!(
        result.predicate.as_deref(),
        Some("cancel_after_deadline_expiry")
    );
}

#[test]
fn timely_scenario_matrix() {
    for variant in [Variant::Old, Variant::Fixed] {
        let (frames, run) = self::run("timely", variant);
        assert!(
            compare(CORPUS, &frames, &run).run_agrees(),
            "{}",
            variant.name()
        );
        let (result, code) = property("timely", &run);
        assert_eq!(
            (result.result, code),
            (Verdict::Pass, 0),
            "{}",
            result.detail
        );
    }

    let (_, run) = self::run("timely", Variant::Noop);
    let (result, code) = property("timely", &run);
    assert_eq!(
        (result.result, code),
        (Verdict::Fail, 1),
        "{}",
        result.detail
    );
    assert_eq!(result.predicate.as_deref(), Some("goal_submitted"));

    // Always-cancel issues a novel cancel, so the adapter refuses the
    // recorded acknowledgment as no longer valid; the property still fails
    // on the observed cancel, which is a demonstrated wrong behavior and
    // never an inconclusive result.
    let (_, run) = self::run("timely", Variant::AlwaysCancel);
    assert!(
        matches!(run.stop, Stop::Refused { ordinal: 6, .. }),
        "{:?}",
        run.stop
    );
    assert_eq!(run.finish_reason, FinishReason::Scope);
    let (result, code) = property("timely", &run);
    assert_eq!(
        (result.result, code),
        (Verdict::Fail, 1),
        "{}",
        result.detail
    );
    assert_eq!(result.predicate.as_deref(), Some("no_cancel_in_interval"));
}

#[test]
fn missing_evidence_before_completion_is_inconclusive() {
    for variant in [Variant::Fixed, Variant::Old] {
        let (_, run) = self::run("timeout-gap", variant);
        assert!(matches!(run.stop, Stop::Gap { ordinal: 6, .. }));
        let (result, code) = property("timeout-gap", &run);
        assert_eq!(
            (result.result, code),
            (Verdict::Inconclusive, 3),
            "{}: {}",
            variant.name(),
            result.detail
        );
        assert_eq!(result.predicate.as_deref(), Some("deadline_expired"));
        let missing = result.missing.expect("names the missing input");
        assert!(missing.contains("gap at recorded ordinal 6"), "{missing}");
    }
}

/// A gap right after the goal command leaves the fixed component waiting
/// for its first clock read: no submission, and no verdict on it either.
#[test]
fn a_gap_before_the_first_clock_read_is_inconclusive_not_a_failure() {
    let (_, run) = self::run("command-gap", Variant::Fixed);
    assert!(
        matches!(run.stop, Stop::Gap { ordinal: 3, .. }),
        "{:?}",
        run.stop
    );
    assert_eq!(
        run.pending_at_finish,
        vec![Pending::ClockRead {
            callback: 2,
            clock_id: 1
        }]
    );
    let (result, code) = property("command-gap", &run);
    assert_eq!(
        (result.result, code),
        (Verdict::Inconclusive, 3),
        "{}",
        result.detail
    );
    assert_eq!(result.predicate.as_deref(), Some("goal_submitted"));
    assert!(
        result
            .missing
            .as_deref()
            .unwrap()
            .contains("clock observation"),
        "{:?}",
        result.missing
    );
    // The noop component was not waiting for anything, so the same gap
    // does not excuse it.
    let (_, run) = self::run("command-gap", Variant::Noop);
    let (result, code) = property("command-gap", &run);
    assert_eq!(
        (result.result, code),
        (Verdict::Fail, 1),
        "{}",
        result.detail
    );
    assert_eq!(result.predicate.as_deref(), Some("goal_submitted"));
}

/// A closed recording that ends while the callback still waits for its
/// first clock read is no verdict on the fixed component either.
#[test]
fn a_recording_closed_before_the_first_clock_read_is_inconclusive() {
    let (_, run) = self::run("command-end", Variant::Fixed);
    assert_eq!(run.stop, Stop::Exhausted);
    assert_eq!(
        run.pending_at_finish,
        vec![Pending::ClockRead {
            callback: 2,
            clock_id: 1
        }]
    );
    let (result, code) = property("command-end", &run);
    assert_eq!(
        (result.result, code),
        (Verdict::Inconclusive, 3),
        "{}",
        result.detail
    );
    assert_eq!(result.predicate.as_deref(), Some("goal_submitted"));
    let (_, run) = self::run("command-end", Variant::Noop);
    let (result, code) = property("command-end", &run);
    assert_eq!(
        (result.result, code),
        (Verdict::Fail, 1),
        "{}",
        result.detail
    );
}

/// A recorded response answers the recorded request. When this run's
/// request differs from it (here the recorded goal payload was changed
/// after the fact), the driver refuses the response before the adapter
/// sees it, and the property reports the refusal rather than a pass.
#[test]
fn a_response_to_a_different_request_is_refused_by_the_driver() {
    let mut frames = recording("timely");
    let send = frames
        .iter_mut()
        .find(|frame| frame.header.event_type == EventType::GoalSend)
        .expect("timely has a goal send");
    send.body.extend_from_slice(b" but different");
    let run = replay(&mut adapter(Variant::Fixed), &frames, CORPUS, "timely").expect("replay");
    match &run.stop {
        Stop::Refused { ordinal, reason } => {
            assert_eq!(*ordinal, 6);
            assert!(reason.contains("differs from it"), "{reason}");
        }
        other => panic!("expected a refusal, got {other:?}"),
    }
    assert!(
        !run.frames
            .iter()
            .any(|frame| frame.header.event_type == EventType::GoalResponse)
    );
    let (result, code) = property("timely", &run);
    assert_eq!(
        (result.result, code),
        (Verdict::Inconclusive, 3),
        "{}",
        result.detail
    );
}

/// An adapter that labels an effect with a callback the current input
/// neither starts nor resumes is rejected by the driver, so a stale
/// attribution never reaches the property. The mock adapter is a shell
/// script that answers the protocol by hand.
#[test]
fn a_stale_callback_attribution_is_rejected_by_the_driver() {
    let script = std::env::temp_dir().join(format!(
        "rook-stale-callback-adapter-{}.sh",
        std::process::id()
    ));
    // Step 1 (starting state) and step 2 (command) are consumed; step 3
    // (the clock read for callback 2) emits an effect claimed for
    // callback 1, which the clock observation does not name.
    std::fs::write(
        &script,
        r#"#!/bin/sh
while IFS= read -r line; do
  case "$line" in
    *'"type":"start"'*) echo '{"type":"started","protocol":1,"identity":{"component":"mock","variant":"stale","adapter":"mock","protocol":1},"endpoints":[],"channels":[],"state":{"method":"fresh","blake3":"af1349b9f5f9a1a6a0404dea36dcc9499bcb25c9adc112b7cc9a93cae41f3262"}}' ;;
    *'"ordinal":1,'*) echo '{"type":"stepped","ordinal":1,"status":"ok","pending":[]}' ;;
    *'"ordinal":2,'*) echo '{"type":"stepped","ordinal":2,"status":"ok","pending":[]}' ;;
    *'"ordinal":3,'*) echo '{"type":"effect","step":3,"callback":1,"effect":{"src_actor":1,"dst_actor":3,"channel_id":20,"src_seq":0,"event_type":"GoalSend","schema":1,"flags":3,"body_hex":"00000000000000000000000000000000"}}'; echo '{"type":"stepped","ordinal":3,"status":"ok","pending":[]}' ;;
    *'"type":"finish"'*) echo '{"type":"finished","reason":"exhausted","pending":[],"effects":1}'; exit 0 ;;
  esac
done
"#,
    )
    .unwrap();
    let mut command = Command::new("sh");
    command.arg(&script);
    let error = replay(&mut command, &recording("timeout"), CORPUS, "timeout")
        .expect_err("the driver rejects the stale attribution");
    let text = format!("{error:#}");
    assert!(text.contains("callback 1"), "{text}");
    assert!(text.contains("authorized callbacks [3, 2]"), "{text}");
    std::fs::remove_file(&script).ok();
}

/// An effect the adapter numbers wrong would make a replayed stream that
/// no reader decodes; the driver rejects it at the protocol.
#[test]
fn a_misnumbered_effect_is_rejected_by_the_driver() {
    let script = std::env::temp_dir().join(format!(
        "rook-misnumbered-effect-adapter-{}.sh",
        std::process::id()
    ));
    std::fs::write(
        &script,
        r#"#!/bin/sh
while IFS= read -r line; do
  case "$line" in
    *'"type":"start"'*) echo '{"type":"started","protocol":1,"identity":{"component":"mock","variant":"seq","adapter":"mock","protocol":1},"endpoints":[],"channels":[],"state":{"method":"fresh","blake3":"af1349b9f5f9a1a6a0404dea36dcc9499bcb25c9adc112b7cc9a93cae41f3262"}}' ;;
    *'"ordinal":1,'*) echo '{"type":"stepped","ordinal":1,"status":"ok","pending":[]}' ;;
    *'"ordinal":2,'*) echo '{"type":"effect","step":2,"callback":2,"effect":{"src_actor":1,"dst_actor":3,"channel_id":20,"src_seq":5,"event_type":"GoalSend","schema":1,"flags":3,"body_hex":"00000000000000000000000000000000"}}'; echo '{"type":"stepped","ordinal":2,"status":"ok","pending":[]}' ;;
    *'"type":"finish"'*) echo '{"type":"finished","reason":"exhausted","pending":[],"effects":1}'; exit 0 ;;
  esac
done
"#,
    )
    .unwrap();
    let mut command = Command::new("sh");
    command.arg(&script);
    let error = replay(&mut command, &recording("timeout"), CORPUS, "timeout")
        .expect_err("the driver rejects the misnumbered effect");
    let text = format!("{error:#}");
    assert!(text.contains("has sequence 5, expected 0"), "{text}");
    std::fs::remove_file(&script).ok();
}

/// The adapter's finish line must agree with the run: the reason the
/// runner sent and the number of effects the runner collected.
#[test]
fn a_finish_line_that_disagrees_with_the_run_is_rejected() {
    let script = std::env::temp_dir().join(format!(
        "rook-finish-mismatch-adapter-{}.sh",
        std::process::id()
    ));
    std::fs::write(
        &script,
        r#"#!/bin/sh
while IFS= read -r line; do
  case "$line" in
    *'"type":"start"'*) echo '{"type":"started","protocol":1,"identity":{"component":"mock","variant":"finish","adapter":"mock","protocol":1},"endpoints":[],"channels":[],"state":{"method":"fresh","blake3":"af1349b9f5f9a1a6a0404dea36dcc9499bcb25c9adc112b7cc9a93cae41f3262"}}' ;;
    *'"ordinal":1,'*) echo '{"type":"stepped","ordinal":1,"status":"ok","pending":[]}' ;;
    *'"ordinal":2,'*) echo '{"type":"stepped","ordinal":2,"status":"ok","pending":[]}' ;;
    *'"ordinal":3,'*) echo '{"type":"stepped","ordinal":3,"status":"ok","pending":[]}' ;;
    *'"ordinal":5,'*) echo '{"type":"stepped","ordinal":5,"status":"ok","pending":[]}' ;;
    *'"ordinal":6,'*) echo '{"type":"stepped","ordinal":6,"status":"ok","pending":[]}' ;;
    *'"type":"finish"'*) echo '{"type":"finished","reason":"scope","pending":[],"effects":7}'; exit 0 ;;
  esac
done
"#,
    )
    .unwrap();
    let mut command = Command::new("sh");
    command.arg(&script);
    let error = replay(&mut command, &recording("timeout"), CORPUS, "timeout")
        .expect_err("the driver rejects the finish line");
    let text = format!("{error:#}");
    assert!(text.contains("finished with reason Scope"), "{text}");
    std::fs::remove_file(&script).ok();
}

/// A refused session-channel input (the starting state here) leaves no
/// hole in the session counter: the replayed stream still decodes.
#[test]
fn a_refused_starting_state_leaves_a_decodable_stream() {
    let script = std::env::temp_dir().join(format!(
        "rook-refuse-state-adapter-{}.sh",
        std::process::id()
    ));
    std::fs::write(
        &script,
        r#"#!/bin/sh
while IFS= read -r line; do
  case "$line" in
    *'"type":"start"'*) echo '{"type":"started","protocol":1,"identity":{"component":"mock","variant":"state","adapter":"mock","protocol":1},"endpoints":[],"channels":[],"state":{"method":"fresh","blake3":"af1349b9f5f9a1a6a0404dea36dcc9499bcb25c9adc112b7cc9a93cae41f3262"}}' ;;
    *'"ordinal":1,'*) echo '{"type":"stepped","ordinal":1,"status":"refused","reason":"mock refuses the starting state","pending":[]}' ;;
    *'"type":"finish"'*) echo '{"type":"finished","reason":"scope","pending":[],"effects":0}'; exit 0 ;;
  esac
done
"#,
    )
    .unwrap();
    let mut command = Command::new("sh");
    command.arg(&script);
    let run = replay(&mut command, &recording("timeout"), CORPUS, "timeout").expect("replay");
    assert!(
        matches!(run.stop, Stop::Refused { ordinal: 1, .. }),
        "{:?}",
        run.stop
    );
    let bytes = rook_native::encode_stream(&run.frames);
    decode_stream(&bytes).expect("the replayed stream decodes");
    std::fs::remove_file(&script).ok();
}

/// Inputs that do not bind to what the component is waiting for are
/// reported as not consumed and change nothing: a clock observation from
/// the wrong clock or actor, and a response whose request reference does
/// not name this run's send.
#[test]
fn unbound_inputs_are_reported_and_not_applied() {
    use rook_adapter_ref::bodies::{Clock, GoalResponse};
    use rook_adapter_ref::component::Consumed;
    use rook_adapter_ref::component::{Component, Input};
    use rook_adapter_ref::{
        ACTOR_ACTION_SERVER, ACTOR_CLOCK, CH_ACTION_GOAL, CH_ACTION_RESPONSE, CH_CLOCK,
        CH_GOAL_COMMAND,
    };

    let mut component = Component::new(Variant::Fixed);
    let command = rook_adapter_ref::bodies::message(b"goal 1");
    // Addressed to someone else: not consumed.
    let outcome = component.step(Input {
        ordinal: 2,
        src_actor: 4,
        dst_actor: 7,
        channel_id: CH_GOAL_COMMAND,
        event_type: EventType::Message,
        body: &command,
        request: None,
    });
    assert!(matches!(outcome.consumed, Some(Consumed::No(_))));
    let outcome = component.step(Input {
        ordinal: 2,
        src_actor: 4,
        dst_actor: 1,
        channel_id: CH_GOAL_COMMAND,
        event_type: EventType::Message,
        body: &command,
        request: None,
    });
    assert_eq!(outcome.consumed, Some(Consumed::Yes));

    // System clock instead of the ROS clock the callback asked for.
    let wrong_clock = Clock {
        clock_id: 3,
        value_ns: 0,
        callback_ordinal: 2,
    }
    .encode();
    let outcome = component.step(Input {
        ordinal: 3,
        src_actor: ACTOR_CLOCK,
        dst_actor: 1,
        channel_id: CH_CLOCK,
        event_type: EventType::Clock,
        body: &wrong_clock,
        request: None,
    });
    assert!(
        matches!(outcome.consumed, Some(Consumed::No(_))),
        "{:?}",
        outcome.consumed
    );
    assert!(outcome.effects.is_empty());

    // Right clock, wrong endpoint.
    let clock = Clock {
        clock_id: 1,
        value_ns: 0,
        callback_ordinal: 2,
    }
    .encode();
    let outcome = component.step(Input {
        ordinal: 3,
        src_actor: 9,
        dst_actor: 1,
        channel_id: CH_CLOCK,
        event_type: EventType::Clock,
        body: &clock,
        request: None,
    });
    assert!(
        matches!(outcome.consumed, Some(Consumed::No(_))),
        "{:?}",
        outcome.consumed
    );

    // The real observation sends the goal.
    let outcome = component.step(Input {
        ordinal: 3,
        src_actor: ACTOR_CLOCK,
        dst_actor: 1,
        channel_id: CH_CLOCK,
        event_type: EventType::Clock,
        body: &clock,
        request: None,
    });
    assert_eq!(outcome.effects.len(), 1);
    let goal_id = Component::goal_id(0);

    // Right goal id, but the request reference names another send.
    let response = GoalResponse {
        goal_send_ordinal: 4,
        goal_id,
        accepted: true,
        stamp_ns: 1,
    }
    .encode();
    for request in [
        None,
        Some((CH_ACTION_GOAL, 7)),
        Some((CH_ACTION_RESPONSE, 0)),
    ] {
        let outcome = component.step(Input {
            ordinal: 6,
            src_actor: ACTOR_ACTION_SERVER,
            dst_actor: 1,
            channel_id: CH_ACTION_RESPONSE,
            event_type: EventType::GoalResponse,
            body: &response,
            request,
        });
        assert!(
            matches!(outcome.consumed, Some(Consumed::No(_))),
            "{request:?}"
        );
        assert!(outcome.effects.is_empty());
    }
    // Bound, but from the wrong endpoint.
    let outcome = component.step(Input {
        ordinal: 6,
        src_actor: 9,
        dst_actor: 1,
        channel_id: CH_ACTION_RESPONSE,
        event_type: EventType::GoalResponse,
        body: &response,
        request: Some((CH_ACTION_GOAL, 0)),
    });
    assert!(matches!(outcome.consumed, Some(Consumed::No(_))));
    // Bound and from the action server: consumed.
    let outcome = component.step(Input {
        ordinal: 6,
        src_actor: ACTOR_ACTION_SERVER,
        dst_actor: 1,
        channel_id: CH_ACTION_RESPONSE,
        event_type: EventType::GoalResponse,
        body: &response,
        request: Some((CH_ACTION_GOAL, 0)),
    });
    assert_eq!(outcome.consumed, Some(Consumed::Yes));
    assert_eq!(outcome.progress, vec!["goal_acknowledged"]);
}

#[test]
fn a_property_change_is_a_new_case_identity() {
    let identity = key_values(&fixture_root().join("timeout").join("identity"));
    let sources = rook_adapter_ref::source_identity();
    for key in [
        "component_source_blake3",
        "adapter_source_blake3",
        "property_source_blake3",
        "normalization_source_blake3",
    ] {
        assert_eq!(identity[key], sources[key], "{key} drifted from the source");
    }
}

#[test]
fn the_wrong_property_names_are_refused_by_the_program() {
    let (_, run) = self::run("timeout", Variant::Fixed);
    let mut input = property_input(
        "not-a-property@1",
        "timeout",
        Scope {
            completion: "cancel_request_issued".into(),
            observation_end: "end_of_recording".into(),
        },
        GOAL_RESPONSE_DEADLINE_NS,
        &run,
        BTreeMap::new(),
    );
    assert!(rook_adapter_ref::property::evaluate(&input).is_err());
    input.property = PROPERTY_TIMEOUT_CANCEL.to_string();
    input.schema = "rook-property-input@0".to_string();
    assert!(rook_adapter_ref::property::evaluate(&input).is_err());
    input.schema = rook_adapter_ref::property::INPUT_SCHEMA.to_string();
    input.scope.completion = "observation_interval_elapsed".to_string();
    assert!(rook_adapter_ref::property::evaluate(&input).is_err());
    input.scope.completion = "cancel_request_issued".to_string();
    input.scope.observation_end = "ordinal:5".to_string();
    assert!(rook_adapter_ref::property::evaluate(&input).is_err());
    input.scope.observation_end = "end_of_recording".to_string();
    input.events[0].origin = "replayd".to_string();
    assert!(rook_adapter_ref::property::evaluate(&input).is_err());
    let _ = PROPERTY_TIMELY_ACK;
}
