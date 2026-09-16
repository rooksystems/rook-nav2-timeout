//! Local baseline verification and bounded candidate testing.
pub mod case;
pub mod normalization;
pub mod runner;

use anyhow::{Context, Result, ensure};
use case::{Build, Case, entry, hash};
use rook_adapter_ref::{component::Variant, driver, encode_hex};
use rook_native::{EnvelopeKind, EventType};
use runner::{Execution, PropertyProgram};
use serde_json::{Value, json};
use std::path::Path;
use std::process::Command;

/// Execute a case locally. The caller owns rendering and translates Failure to
/// its documented exit code. Reference candidates are variants compiled into rook;
/// other adapters supply a manifest-covered build.json and a candidate build file.
pub fn run_case(path: &Path, candidate: Option<&str>) -> Result<Value> {
    let case = Case::load(path)?;
    let executable = std::env::current_exe()?;
    let runner_hash = hash(&std::fs::read(&executable)?);
    let (mut baseline_command, property, baseline_identity, baseline_artifacts, candidate_build) =
        if case.covered.contains("build.json") {
            let baseline = Build::load(&case.root.join("build.json"))?;
            case::same_identity(&case.identity, &baseline.identity, false)?;
            let tested = candidate
                .map(|name| Build::load(Path::new(name)))
                .transpose()?;
            if let Some(tested) = &tested {
                baseline.candidate(tested)?;
            }
            let property = PropertyProgram {
                executable: baseline.property.path.clone(),
                arguments: baseline.property_args.clone(),
            };
            (
                baseline.command(),
                property,
                baseline.identity.clone(),
                serde_json::to_value(&baseline)?,
                tested,
            )
        } else {
            let identity = case.reference_identity()?;
            let mut command = Command::new(&executable);
            command.args(["__adapter", entry(&identity, "component_variant")?]);
            if let Some(name) = candidate {
                ensure!(
                    Variant::parse(name).is_some(),
                    "unknown reference candidate {name}, expected old, fixed, noop or always-cancel"
                );
            }
            (
                command,
                PropertyProgram {
                    executable: executable.clone(),
                    arguments: vec!["__property".into()],
                },
                identity,
                json!({"runner_binary_blake3":runner_hash,"programs":"source-pinned reference adapter and property compiled into rook"}),
                None,
            )
        };
    // Identity/normalization refusal precedes execution, including candidate changes.
    normalization::Goals::new(
        &case.frames,
        &baseline_identity,
        entry(&case.declaration, "comparison_policy")?,
    )?;
    let baseline = runner::replay(
        &mut baseline_command,
        &case,
        &baseline_identity,
        &property,
        false,
    )?;
    ensure!(
        baseline.property.result != rook_adapter_ref::property::Verdict::Invalid,
        "invalid property input for {} ({})",
        case.declaration["property"],
        baseline.property.detail
    );
    let baseline_comparison = comparison(&case, &baseline);
    let reproduced = baseline_comparison["execution_agreement"] == "measured agreement";
    let baseline_complete = matches!(baseline.run.stop, driver::Stop::Exhausted);
    // A later evidence gap cannot erase a mismatch already established in the
    // consumed prefix. Missing effects beyond that gap remain unavailable.
    let baseline_mismatch = match &baseline.run.stop {
        driver::Stop::Exhausted => !reproduced,
        driver::Stop::Gap { ordinal, .. } | driver::Stop::Refused { ordinal, .. } => {
            let prefix = case
                .frames
                .iter()
                .take_while(|frame| frame.ordinal < *ordinal)
                .cloned()
                .collect::<Vec<_>>();
            let mut run = baseline.run.clone();
            run.frames = baseline.normalized.clone();
            !driver::compare(&case.declaration["corpus"], &prefix, &run).effects_agree()
        }
    };
    let (tested, tested_identity, candidate_artifacts) = if let Some(name) = candidate {
        let (mut command, identity, artifacts) = if let Some(build) = candidate_build {
            (
                build.command(),
                build.identity.clone(),
                serde_json::to_value(build)?,
            )
        } else {
            let mut command = Command::new(&executable);
            command.args(["__adapter", name]);
            let mut identity = baseline_identity.clone();
            identity.insert("component_variant".into(), name.into());
            (command, identity, baseline_artifacts.clone())
        };
        let tested = runner::replay(&mut command, &case, &identity, &property, true)?;
        (tested, identity, Some(artifacts))
    } else {
        (baseline, baseline_identity.clone(), None)
    };
    let comparison = comparison(&case, &tested);
    let code = if candidate.is_some() {
        if baseline_mismatch {
            1
        } else {
            tested.property.exit_code()
        }
    } else if reproduced {
        0
    } else if baseline_complete
        || baseline_mismatch
        || matches!(tested.run.stop, driver::Stop::Refused { .. })
    {
        1
    } else {
        2
    };
    let hashes = |frames: &[rook_native::Frame]| {
        let hashes = rook_native::hash_frames(case.declaration["corpus"].as_bytes(), frames);
        json!({"trace_hash":encode_hex(&hashes.trace_hash),"run_hash":encode_hex(&hashes.run_hash),"output_digest":encode_hex(&hashes.output_digest)})
    };
    let mut property_input = rook_adapter_ref::property::property_input(
        &case.declaration["property"],
        &case.declaration["scenario"],
        rook_adapter_ref::property::Scope {
            completion: case.declaration["scope.completion"].clone(),
            observation_end: case.declaration["scope.observation_end"].clone(),
        },
        case.declaration["goal_response_deadline_ns"].parse()?,
        &tested.run,
        tested_identity.clone(),
    );
    if tested.run.finish_reason == rook_adapter_ref::protocol::FinishReason::Scope {
        property_input.evidence.end_of_recording = false;
    }
    Ok(json!({
        "schema":"rook-run-report@1", "command":if candidate.is_some() {"test"} else {"verify"}, "exit_code":code,
        "case_id":case.case_id, "case_path":case.root, "recording_origin":case.declaration["origin"],
        "recording_limits":case.declaration["grade_reason"], "claim":"measured",
        "platform":format!("{}-{}",std::env::consts::OS,std::env::consts::ARCH), "runner_binary_blake3":runner_hash,
        "source_identity":baseline_identity, "candidate_identity":candidate.map(|_| &tested_identity),
        "baseline_artifacts":baseline_artifacts, "candidate_artifacts":candidate_artifacts,
        "comparison_policy":case.declaration["comparison_policy"], "normalization":case.identity["normalization"],
        "normalization_source_blake3":case.identity["normalization_source_blake3"],
        "scope":{"completion":case.declaration["scope.completion"],"observation_end":case.declaration["scope.observation_end"]},
        "scope_completed":tested.scope_completed, "baseline_reproduced":if baseline_mismatch {Some(false)} else if baseline_complete {Some(reproduced)} else {None},
        "baseline_comparison":baseline_comparison,
        "file_integrity":"passed", "raw_record_blake3":case.raw_hash, "effects_captured_blake3":case.captured_hash,
        "recorded_execution_hashes":hashes(&case.frames), "raw_replay_execution_hashes":hashes(&tested.run.frames),
        "normalized_replay_execution_hashes":hashes(&tested.normalized),
        "raw_effects_equal":comparison["raw_effects_equal"], "execution_agreement":comparison["execution_agreement"],
        "first_differing_effect":comparison["first_differing_effect"], "normalized_first_difference":comparison["normalized_first_difference"],
        "capture_completeness":{"declared_grade":case.expected["grade"],"independently_established":false,"end_marker":true,"gap_count":case.frames.iter().filter(|f|f.header.event_type==EventType::Gap).count()},
        "incident_authenticity":"outside this verifier", "property_result":tested.property,
        "observed_events":property_input.events, "evidence":property_input.evidence, "finish_reason":tested.run.finish_reason,
        "recorded_effects":case.frames.iter().filter(|f|f.kind()==EnvelopeKind::Emit).map(driver::wire_frame).collect::<Vec<_>>()
    }))
}

fn comparison(case: &Case, execution: &Execution) -> Value {
    let raw = driver::compare(&case.declaration["corpus"], &case.frames, &execution.run);
    let mut normalized_run = execution.run.clone();
    normalized_run.frames = execution.normalized.clone();
    let normalized = driver::compare(&case.declaration["corpus"], &case.frames, &normalized_run);
    let recorded = case
        .frames
        .iter()
        .filter(|f| f.kind() == EnvelopeKind::Emit)
        .collect::<Vec<_>>();
    let replayed = execution
        .run
        .frames
        .iter()
        .filter(|f| f.kind() == EnvelopeKind::Emit)
        .collect::<Vec<_>>();
    let first = raw.first_difference.map(|index|json!({"index":index,"recorded":recorded.get(index).map(|f|driver::wire_frame(f)),"replayed":replayed.get(index).map(|f|driver::wire_frame(f))}));
    let agreement = match execution.run.stop {
        driver::Stop::Gap { ordinal, .. } => {
            // A gap changes the full run hash without demonstrating a mismatch.
            // Only effects before the gap can establish disagreement.
            let end = case.frames.partition_point(|frame| frame.ordinal < ordinal);
            if driver::compare(
                &case.declaration["corpus"],
                &case.frames[..end],
                &normalized_run,
            )
            .effects_agree()
            {
                "unavailable"
            } else {
                "diverged"
            }
        }
        _ if normalized.effects_agree() && normalized.run_agrees() => "measured agreement",
        _ => "diverged",
    };
    json!({"raw_effects_equal":raw.effects_agree(),"first_differing_effect":first,"normalized_first_difference":normalized.first_difference,"execution_agreement":agreement})
}

/// Readable output accompanies the machine report on stderr, so stdout stays ndjson.
pub fn readable(report: &Value) -> String {
    if let Some(error) = report.get("error") {
        return format!(
            "{}\nExit {}\n",
            error.as_str().unwrap_or("error"),
            report["exit_code"]
        );
    }
    format!(
        "{} {}\nRecording {}\nLimits {}\nPlatform {}\nBaseline reproduced {}\nFile integrity {}\nExecution agreement {}\nRaw effects equal {}\nRaw record BLAKE3 {}\nCaptured effects BLAKE3 {}\nRecorded execution hashes {}\nRaw replay execution hashes {}\nNormalized replay execution hashes {}\nCapture completeness {}\nIncident authenticity {}\nComparison {} with {}\nScope {}\nFirst differing effect {}\nProperty {}\nSource identities {}\nCandidate identities {}\nExit {}\n",
        report["command"],
        report["case_path"],
        report["recording_origin"],
        report["recording_limits"],
        report["platform"],
        report["baseline_reproduced"],
        report["file_integrity"],
        report["execution_agreement"],
        report["raw_effects_equal"],
        report["raw_record_blake3"],
        report["effects_captured_blake3"],
        report["recorded_execution_hashes"],
        report["raw_replay_execution_hashes"],
        report["normalized_replay_execution_hashes"],
        report["capture_completeness"],
        report["incident_authenticity"],
        report["comparison_policy"],
        report["normalization"],
        report["scope"],
        report["first_differing_effect"],
        report["property_result"],
        report["source_identity"],
        report["candidate_identity"],
        report["exit_code"]
    )
}

pub fn error_report(error: &anyhow::Error) -> Value {
    let failure = error.downcast_ref::<case::Failure>();
    let integrity = match failure {
        Some(f) if f.integrity => "diverged",
        Some(_) => "passed",
        None => "unavailable",
    };
    let execution = if failure.is_some_and(|f| !f.integrity) {
        "diverged"
    } else {
        "unavailable"
    };
    json!({"schema":"rook-run-report@1","exit_code":case::exit_code(error),"error":format!("{error:#}"),"file_integrity":integrity,"execution_agreement":execution,"capture_completeness":"unavailable","incident_authenticity":"outside this verifier"})
}

pub fn property_subcommand(path: &Path) -> Result<i32> {
    let input = serde_json::from_slice(&std::fs::read(path)?)?;
    let result = rook_adapter_ref::property::evaluate(&input)?;
    println!("{}", serde_json::to_string(&result)?);
    Ok(result.exit_code())
}

/// Keep the established Wasm verifier as the owner of its replay contract.
pub fn wasm_verify(path: &Path) -> Result<Value> {
    let executable = std::env::current_exe()?.with_file_name("rook-verify");
    let output = Command::new(executable)
        .arg(path)
        .output()
        .context("start sibling rook-verify for Wasm case")?;
    Ok(
        json!({"schema":"rook-run-report@1","command":"verify","kind":"wasm-rmf-blockade","exit_code":output.status.code().unwrap_or(2),"report":String::from_utf8_lossy(&output.stdout),"diagnostic":String::from_utf8_lossy(&output.stderr)}),
    )
}
