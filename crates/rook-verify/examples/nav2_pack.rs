//! Build a case from an independently captured Nav2 transcript. This example
//! serializes evidence; it neither executes decision code nor derives effects.
use anyhow::{Context, Result, ensure};
use rook_adapter_ref::{decode_hex, encode_hex, fixture, parse_event_type, protocol::WireFrame};
use rook_native::{End, EventHeader, EventType, Frame, Session};
use rook_verify::{
    case::{self, Artifact, Build, Identity},
    normalization,
};
use serde::Deserialize;
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
};

#[derive(Deserialize)]
struct Capture {
    scenario: String,
    frames: Vec<WireFrame>,
    captured: Vec<WireFrame>,
}
fn frame(w: WireFrame) -> Result<Frame> {
    Ok(Frame {
        ordinal: w.ordinal.context("missing ordinal")?,
        src_actor: w.src_actor,
        dst_actor: w.dst_actor,
        channel_id: w.channel_id,
        src_seq: w.src_seq,
        observed_ns: 0,
        header: EventHeader {
            event_type: parse_event_type(&w.event_type).context("unknown event")?,
            schema: w.schema,
            flags: w.flags,
        },
        body: decode_hex(&w.body_hex)?,
    })
}
fn artifact(path: &Path) -> Result<Artifact> {
    Ok(Artifact {
        path: path.canonicalize()?,
        blake3: case::hash(&std::fs::read(path)?),
    })
}
fn write_values(path: &Path, values: &Identity) -> Result<()> {
    std::fs::write(
        path,
        values
            .iter()
            .map(|(k, v)| format!("{k} = {v}\n"))
            .collect::<String>(),
    )?;
    Ok(())
}
fn main() -> Result<()> {
    let args: Vec<_> = std::env::args_os().skip(1).map(PathBuf::from).collect();
    ensure!(
        args.len() == 4,
        "usage: nav2_pack CAPTURE SOURCE_DIR BUILD_DIR CASE_DIR"
    );
    let [capture_path, source, binaries, root] = &args[..] else {
        unreachable!()
    };
    ensure!(
        !root.exists(),
        "refuse to replace existing case or expected outputs"
    );
    let capture: Capture = serde_json::from_slice(&std::fs::read(capture_path)?)?;
    std::fs::create_dir_all(root)?;
    let uuid: [u8; 16] = blake3::hash(capture.scenario.as_bytes()).as_bytes()[..16].try_into()?;
    let marker = |ordinal, event_type, src_seq, body| Frame {
        ordinal,
        src_actor: 0,
        dst_actor: 0,
        channel_id: 0,
        src_seq,
        observed_ns: 0,
        header: EventHeader {
            event_type,
            schema: 1,
            flags: 0,
        },
        body,
    };
    let mut frames = vec![marker(
        0,
        EventType::Session,
        0,
        Session {
            uuid,
            record_utc_ns: 0,
            pid: 0,
        }
        .encode(),
    )];
    for wire in capture.frames {
        let mut f = frame(wire)?;
        ensure!(
            f.ordinal == frames.len() as u64,
            "capture ordinal is not contiguous"
        );
        if f.header.event_type == EventType::StartingState {
            f.src_seq = 1;
        }
        frames.push(f);
    }
    let count = frames.len() as u64;
    frames.push(marker(
        count,
        EventType::End,
        2,
        End {
            uuid,
            frame_count: count,
        }
        .encode(),
    ));
    let events = rook_native::encode_stream(&frames);
    rook_native::decode_stream(&events)?;
    let captured = capture
        .captured
        .into_iter()
        .map(|w| frame(w).map(|f| f.payload()))
        .collect::<Result<Vec<_>>>()?;
    ensure!(
        captured
            == frames
                .iter()
                .filter(|f| f.kind() == rook_native::EnvelopeKind::Emit)
                .map(Frame::payload)
                .collect::<Vec<_>>(),
        "capture channels differ"
    );
    let captured = fixture::encode_captured(&captured);
    std::fs::write(root.join("events.bin"), &events)?;
    std::fs::write(root.join("effects_captured.bin"), &captured)?;
    std::fs::copy(capture_path, root.join("capture.json"))?;
    let (property, completion) = match capture.scenario.as_str() {
        "timely" => (
            "nav2-timely-ack-no-cancel@1",
            "observation_interval_elapsed",
        ),
        "overlap" | "wrong-result" => ("nav2-overlap-results@1", "matching_result_processed"),
        "cancel-ack" => ("nav2-cancel-ack@1", "cancel_acknowledged"),
        "cancel-before-goal" => ("nav2-goal-termination@1", "goal_terminated"),
        "timeout" | "missing-clock" | "frozen-clock" => (
            "nav2-goal-response-timeout-cancel@1",
            "cancel_request_issued",
        ),
        _ => anyhow::bail!("unknown Nav2 capture scenario"),
    };
    let declaration = Identity::from([
        ("corpus".into(), "nav2-action@1".into()), ("scenario".into(), capture.scenario),
        ("property".into(), property.into()), ("scope.completion".into(), completion.into()),
        ("scope.observation_end".into(), "end_of_recording".into()),
        ("comparison_policy".into(), "goal-ids-in-order@1".into()),
        ("goal_response_deadline_ns".into(), "20000000".into()),
        ("origin".into(), "upstream-issue recreation; scripted environment; no field incident claim".into()),
        ("grade_reason".into(), "Complete within the scripted class boundary; substituted executor/action client; DDS, lifecycle and server behavior outside scope".into()),
    ]);
    write_values(&root.join("case"), &declaration)?;
    let hashes = rook_native::hash_frames(b"nav2-action@1", &frames);
    write_values(
        &root.join("expected"),
        &Identity::from([
            ("claim".into(), "measured".into()),
            ("grade".into(), "Complete".into()),
            ("raw_record_blake3".into(), case::hash(&events)),
            ("effects_captured_blake3".into(), case::hash(&captured)),
            ("trace_hash".into(), encode_hex(&hashes.trace_hash)),
            ("run_hash".into(), encode_hex(&hashes.run_hash)),
            ("output_digest".into(), encode_hex(&hashes.output_digest)),
        ]),
    )?;
    let adapter = artifact(&source.join("adapter.py"))?;
    let property_artifact = artifact(&source.join("property.py"))?;
    let mut identity = Identity::from([
        ("component".into(), "nav2-bt-action".into()),
        ("adapter".into(), "rook-nav2-adapter".into()),
        ("adapter_protocol".into(), "1".into()),
        ("starting_state".into(), "fresh".into()),
        ("starting_state_blake3".into(), case::hash(b"")),
        ("normalization".into(), normalization::POLICY.into()),
        (
            "normalization_source_blake3".into(),
            normalization::source_hash(),
        ),
        ("normalization.client.20".into(), "nav2-wait".into()),
        ("normalization.client.21".into(), "nav2-wait".into()),
        ("property".into(), property.into()),
        (
            "property_input_schema".into(),
            "rook-property-input@1".into(),
        ),
        (
            "property_source_blake3".into(),
            property_artifact.blake3.clone(),
        ),
        ("adapter_binary_blake3".into(), adapter.blake3.clone()),
        (
            "property_binary_blake3".into(),
            property_artifact.blake3.clone(),
        ),
        (
            "effect_record_order".into(),
            "attempted API call at dependency seam; diagnostic status on tick return".into(),
        ),
    ]);
    for (actor, name) in [
        "session",
        "component",
        "clock",
        "action_server",
        "command",
        "diagnostics",
    ]
    .iter()
    .enumerate()
    {
        identity.insert(format!("endpoint.{actor}"), (*name).into());
    }
    for (channel, declaration) in [
        (10, "tick input Message"),
        (11, "clock input Clock"),
        (
            12,
            "action_response input GoalResponse,Feedback,Result,CancelResponse",
        ),
        (20, "action_goal effect GoalSend"),
        (21, "action_cancel effect CancelSend"),
        (22, "status effect Status"),
    ] {
        identity.insert(format!("channel.{channel}"), declaration.into());
    }
    let mut dependencies = BTreeMap::new();
    for name in [
        "component.cpp",
        "control.hpp",
        "harness.hpp",
        "integration.diff",
        "prepare.py",
        "CMakeLists.txt",
    ] {
        dependencies.insert(
            format!("dependency.source.{name}"),
            artifact(&source.join(name))?,
        );
    }
    // CI enumerates the actual loader dependencies and interpreter. Each path
    // has a content identity; a changed installed package is refused on replay.
    for path in std::fs::read_to_string(binaries.join("runtime-paths.txt"))?.lines() {
        dependencies.insert(
            format!("dependency.runtime.{path}"),
            artifact(Path::new(path))?,
        );
    }
    for (key, value) in &dependencies {
        identity.insert(key.clone(), value.blake3.clone());
    }
    for variant in ["old", "fixed", "noop", "always-cancel"] {
        let component = artifact(&binaries.join(format!("nav2_component_{variant}")))?;
        identity.insert("component_variant".into(), variant.into());
        identity.insert("component_binary_blake3".into(), component.blake3.clone());
        identity.insert(
            "component_source_blake3".into(),
            case::hash(&std::fs::read(binaries.join("upstream").join(
                if variant == "old" {
                    "old.hpp"
                } else {
                    "fixed.hpp"
                },
            ))?),
        );
        let build = Build {
            identity: identity.clone(),
            adapter: adapter.clone(),
            component,
            property: property_artifact.clone(),
            dependencies: dependencies.clone(),
            adapter_args: vec!["{component}".into()],
            property_args: vec![],
        };
        std::fs::write(
            root.join(format!("{variant}.json")),
            serde_json::to_vec_pretty(&build)?,
        )?;
        if variant == "old" {
            write_values(&root.join("identity"), &identity)?;
            std::fs::write(root.join("build.json"), serde_json::to_vec_pretty(&build)?)?;
        }
    }
    let mut manifest = "# rook-capsule-v1 kind=native-adapter\n".to_owned();
    let mut paths = std::fs::read_dir(root)?
        .map(|e| e.map(|e| e.path()))
        .collect::<std::io::Result<Vec<_>>>()?;
    paths.sort();
    for path in paths {
        manifest += &format!(
            "{}  {}\n",
            encode_hex(&Sha256::digest(std::fs::read(&path)?)),
            path.file_name()
                .context("filename")?
                .to_str()
                .context("UTF-8 filename")?
        );
    }
    std::fs::write(root.join("MANIFEST.sha256"), manifest)?;
    case::Case::load(root)?;
    Ok(())
}
