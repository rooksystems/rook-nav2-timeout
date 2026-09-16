use rook_adapter_ref::{
    bodies::{CancelResponse, CancelSend, GoalResponse, GoalResult, GoalSend},
    encode_hex,
};
use rook_native::{EventHeader, EventType, Frame};
use rook_verify::{
    case::{self, Artifact, Build},
    normalization::{self, Goals},
};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::Command;

struct CopyCase(PathBuf);
impl CopyCase {
    fn new(scenario: &str) -> Self {
        static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let path = std::env::temp_dir().join(format!(
            "rook-runner-test-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
        ));
        std::fs::create_dir(&path).expect("create temporary case");
        let source = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../fixtures/native-ref")
            .join(scenario);
        for file in std::fs::read_dir(source).expect("fixture directory") {
            let file = file.expect("fixture file");
            std::fs::copy(file.path(), path.join(file.file_name())).expect("copy fixture");
        }
        Self(path)
    }
    fn edit(&self, name: &str, from: &str, to: &str) {
        let path = self.0.join(name);
        let text = std::fs::read_to_string(&path).expect("read file");
        assert!(text.contains(from), "missing edit target {from}");
        std::fs::write(path, text.replace(from, to)).expect("write edit");
    }
    fn manifest(&self) {
        let original = std::fs::read_to_string(self.0.join("MANIFEST.sha256")).expect("manifest");
        let mut text = original.lines().next().expect("header").to_owned() + "\n";
        let mut names = original
            .lines()
            .skip(1)
            .filter_map(|line| line.split_once("  ").map(|(_, name)| name.to_owned()))
            .collect::<std::collections::BTreeSet<_>>();
        if self.0.join("build.json").is_file() {
            names.insert("build.json".into());
        }
        for name in names {
            let bytes = std::fs::read(self.0.join(&name)).expect("manifest entry");
            text += &format!("{}  {name}\n", encode_hex(&Sha256::digest(bytes)));
        }
        std::fs::write(self.0.join("MANIFEST.sha256"), text).expect("manifest update");
    }
    fn run(&self, candidate: Option<&str>) -> (i32, Value, String) {
        let mut command = Command::new(env!("CARGO_BIN_EXE_rook"));
        command
            .arg(if candidate.is_some() {
                "test"
            } else {
                "verify"
            })
            .arg(&self.0);
        if let Some(candidate) = candidate {
            command.args(["--candidate", candidate]);
        }
        let output = command.output().expect("run CLI");
        let report = serde_json::from_slice(&output.stdout)
            .unwrap_or_else(|e| panic!("ndjson {e}: {:?}", output));
        (
            output.status.code().expect("exit status"),
            report,
            String::from_utf8(output.stderr).expect("readable report"),
        )
    }
}
impl Drop for CopyCase {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

#[test]
fn reference_baseline_and_candidate_matrix_separate_behavior_from_agreement() {
    for (scenario, variant, code, predicate) in [
        ("timeout", "fixed", 0, None),
        ("timeout", "old", 1, Some("cancel_request_issued")),
        ("timeout", "noop", 1, Some("goal_submitted")),
        (
            "timeout",
            "always-cancel",
            1,
            Some("cancel_after_deadline_expiry"),
        ),
        ("timely", "fixed", 0, None),
        ("timely", "old", 0, None),
        ("timely", "always-cancel", 1, Some("no_cancel_in_interval")),
        ("timely", "noop", 1, Some("goal_submitted")),
        ("timeout-gap", "fixed", 3, Some("deadline_expired")),
        ("command-gap", "fixed", 3, Some("goal_submitted")),
        ("command-gap", "noop", 1, Some("goal_submitted")),
    ] {
        let case = CopyCase::new(scenario);
        let (actual, report, text) = case.run(Some(variant));
        assert_eq!(actual, code, "{scenario}/{variant}: {report}");
        assert_eq!(report["exit_code"], code);
        assert_eq!(report["schema"], "rook-run-report@1");
        if let Some(predicate) = predicate {
            assert_eq!(report["property_result"]["predicate"], predicate);
        }
        for field in [
            "file_integrity",
            "execution_agreement",
            "capture_completeness",
            "incident_authenticity",
        ] {
            assert!(!report[field].is_null(), "{field}");
        }
        assert!(text.contains("Source identities"));
        if scenario == "timeout" && variant == "fixed" {
            assert_eq!(report["baseline_reproduced"], true);
            assert_eq!(report["scope_completed"], true);
            assert_eq!(
                report["property_result"]["unavailable"],
                json!(["cancel_acknowledged", "goal_terminated"])
            );
            assert!(!report["first_differing_effect"].is_null());
            assert_eq!(report["raw_effects_equal"], false);
            assert!(
                report["evidence"]["pending_at_finish"]
                    .as_array()
                    .expect("pending")
                    .iter()
                    .any(|p| p["name"] == "cancel_response")
            );
        }
        if scenario == "timeout-gap" && variant == "fixed" {
            assert!(
                report["property_result"]["missing"]
                    .as_str()
                    .expect("missing")
                    .contains("recorded ordinal 6")
            );
        }
    }
    let case = CopyCase::new("timeout");
    let (code, report, _) = case.run(None);
    assert_eq!(code, 0, "{report}");
    assert_eq!(
        report["raw_record_blake3"],
        "9114547d95c6f7cf48c93b3d09d5dd0daec0c68fd47e0210c97edf121eede0f3"
    );
    assert_eq!(
        report["recorded_execution_hashes"],
        report["raw_replay_execution_hashes"]
    );
    assert_eq!(report["property_result"]["result"], "fail");
}

#[test]
fn identity_drift_names_the_first_entry_and_never_executes() {
    let sources = rook_adapter_ref::source_identity();
    for (key, old, new) in [
        (
            "executable",
            "built from public source; no ROS, no plugins, no shared libraries beyond the Rust toolchain",
            "changed",
        ),
        ("adapter", "rook-adapter-ref", "other-adapter"),
        (
            "property_source_blake3",
            sources["property_source_blake3"].as_str(),
            "changed",
        ),
        (
            "normalization_source_blake3",
            sources["normalization_source_blake3"].as_str(),
            "changed",
        ),
        ("starting_state", "fresh", "snapshot"),
    ] {
        let case = CopyCase::new("timeout");
        case.edit(
            "identity",
            &format!("{key} = {old}"),
            &format!("{key} = {new}"),
        );
        case.manifest();
        for candidate in [None, Some("fixed")] {
            let (code, report, _) = case.run(candidate);
            assert_eq!(code, 2, "{report}");
            assert!(
                report["error"].as_str().expect("error").contains(key),
                "{report}"
            );
        }
    }
    for key in ["plugin.foo", "configuration"] {
        let case = CopyCase::new("timeout");
        let mut text = std::fs::read_to_string(case.0.join("identity")).expect("identity");
        text += &format!("{key} = changed\n");
        std::fs::write(case.0.join("identity"), text).expect("write");
        case.manifest();
        let (code, report, _) = case.run(None);
        assert_eq!(code, 2);
        assert!(report["error"].as_str().expect("error").contains(key));
    }
    for (key, old, new) in [
        (
            "scope.completion",
            "cancel_request_issued",
            "goal_terminated",
        ),
        (
            "scope.observation_end",
            "end_of_recording",
            "cancel_request",
        ),
        ("goal_response_deadline_ns", "1000000000", "1"),
    ] {
        let case = CopyCase::new("timeout");
        case.edit("case", &format!("{key} = {old}"), &format!("{key} = {new}"));
        case.manifest();
        let (code, report, _) = case.run(Some("fixed"));
        assert_eq!(code, 2, "{report}");
        assert!(report["error"].as_str().expect("error").contains(key));
    }
}

#[test]
fn malformed_unknown_and_corrupt_cases_have_distinct_exit_codes() {
    let case = CopyCase::new("timeout");
    case.edit("case", "scenario = timeout", "scenario = forged");
    assert_eq!(case.run(None).0, 1);
    case.manifest();
    assert_eq!(case.run(None).0, 2);
    let case = CopyCase::new("timeout");
    case.edit(
        "MANIFEST.sha256",
        "kind=native-adapter",
        "kind=unknown-robot",
    );
    let (code, report, _) = case.run(None);
    assert_eq!(code, 2);
    assert!(
        report["error"]
            .as_str()
            .expect("error")
            .contains("unknown-robot")
    );
    let case = CopyCase::new("timeout");
    std::fs::write(case.0.join("events.bin"), b"broken").expect("break events");
    case.manifest();
    // Recomputed file manifest cannot conceal disagreement with raw evidence hashes.
    assert_eq!(case.run(None).0, 1);
    let raw = case::hash(b"broken");
    case.edit(
        "expected",
        "9114547d95c6f7cf48c93b3d09d5dd0daec0c68fd47e0210c97edf121eede0f3",
        &raw,
    );
    case.manifest();
    assert_eq!(case.run(None).0, 2);
    let output = Command::new(env!("CARGO_BIN_EXE_rook"))
        .args(["test", "missing"])
        .output()
        .expect("CLI");
    assert_eq!(output.status.code(), Some(2));
    assert_eq!(
        serde_json::from_slice::<Value>(&output.stdout).expect("json")["exit_code"],
        2
    );
}

fn goal(ordinal: u64, channel: u32, id: u8) -> Frame {
    Frame {
        ordinal,
        src_actor: 1,
        dst_actor: 3,
        channel_id: channel,
        src_seq: 0,
        observed_ns: 0,
        header: EventHeader {
            event_type: EventType::GoalSend,
            schema: 1,
            flags: 3,
        },
        body: GoalSend {
            goal_id: [id; 16],
            goal: vec![7],
        }
        .encode(),
    }
}
fn normalizer(frames: &[Frame]) -> Goals {
    let identity = BTreeMap::from([
        ("normalization".into(), normalization::POLICY.into()),
        (
            "normalization_source_blake3".into(),
            normalization::source_hash(),
        ),
        ("normalization.client.20".into(), "first".into()),
        ("normalization.client.21".into(), "first".into()),
        ("normalization.client.30".into(), "second".into()),
        ("normalization.client.31".into(), "second".into()),
    ]);
    Goals::new(frames, &identity, "goal-ids-in-order@1").expect("normalizer")
}
#[test]
fn overlapping_and_multiple_clients_keep_issuance_bijections_after_results() {
    let a = goal(2, 20, 1);
    let b = goal(4, 20, 2);
    let c = goal(6, 30, 1);
    let mut goals = normalizer(&[a.clone(), b.clone(), c.clone()]);
    assert_eq!(goals.effect(&goal(2, 20, 11)).expect("a").body, a.body);
    assert_eq!(goals.effect(&goal(4, 20, 12)).expect("b").body, b.body);
    assert_eq!(goals.effect(&goal(6, 30, 13)).expect("c").body, c.body);
    for (request, id, mapped) in [(&b, 2, 12), (&a, 1, 11), (&c, 1, 13), (&a, 1, 11)] {
        let mut response = request.clone();
        response.header.event_type = EventType::Result;
        response.body = GoalResult {
            goal_send_ordinal: request.ordinal,
            goal_id: [id; 16],
            status: 4,
            result: vec![42],
        }
        .encode();
        let translated = goals.input(&response, request).expect("result correlation");
        assert_eq!(&translated.body[8..24], &[mapped; 16]);
        assert_eq!(&response.body[8..24], &[id; 16]);
        assert_eq!(&translated.body[24..], &response.body[24..]);
        response.header.event_type = EventType::Feedback;
        assert_eq!(
            &goals.input(&response, request).expect("feedback").body[8..24],
            &[mapped; 16]
        );
    }
    let mut stale = a.clone();
    stale.header.event_type = EventType::GoalResponse;
    stale.body = GoalResponse {
        goal_send_ordinal: 4,
        goal_id: [1; 16],
        accepted: true,
        stamp_ns: 27,
    }
    .encode();
    assert!(
        goals
            .input(&stale, &b)
            .expect_err("stale response")
            .to_string()
            .contains("wrong or stale")
    );
    assert!(goals.effect(&goal(8, 20, 11)).is_err());
    let mut cancel = a.clone();
    cancel.channel_id = 21;
    cancel.header.event_type = EventType::CancelSend;
    for (raw, canonical, stamp) in [(11, 1, 0), (12, 2, 999), (0, 0, 0), (0, 0, 57)] {
        cancel.body = CancelSend {
            goal_id: [raw; 16],
            stamp_ns: stamp,
        }
        .encode();
        assert_eq!(
            goals.effect(&cancel).expect("cancel").body,
            CancelSend {
                goal_id: [canonical; 16],
                stamp_ns: stamp
            }
            .encode()
        );
    }
    cancel.body = CancelSend {
        goal_id: [99; 16],
        stamp_ns: 0,
    }
    .encode();
    assert!(goals.effect(&cancel).is_err());
    cancel.body = CancelSend {
        goal_id: [0; 16],
        stamp_ns: 0,
    }
    .encode();
    let mut response = cancel.clone();
    response.header.event_type = EventType::CancelResponse;
    response.body = CancelResponse {
        cancel_send_ordinal: 8,
        return_code: 0,
        goals: vec![[1; 16], [2; 16]],
    }
    .encode()
    .expect("encode");
    assert_eq!(
        CancelResponse::decode(&goals.input(&response, &cancel).expect("cancel ack").body)
            .expect("decode")
            .goals,
        vec![[11; 16], [12; 16]]
    );
    response.body = CancelResponse {
        cancel_send_ordinal: 8,
        return_code: 0,
        goals: vec![[1; 16], [1; 16]],
    }
    .encode()
    .expect("encode");
    assert!(goals.input(&response, &cancel).is_err());
    response.body = CancelResponse {
        cancel_send_ordinal: 8,
        return_code: 0,
        goals: vec![[99; 16]],
    }
    .encode()
    .expect("encode");
    assert!(goals.input(&response, &cancel).is_err());
}

#[test]
fn missing_extra_and_unknown_goal_correspondence_are_not_repaired() {
    let a = goal(2, 20, 1);
    let b = goal(4, 20, 2);
    let mut goals = normalizer(&[a.clone(), b.clone()]);
    goals.effect(&goal(2, 20, 11)).expect("first");
    let mut response = b.clone();
    response.header.event_type = EventType::Result;
    response.body = GoalResult {
        goal_send_ordinal: 4,
        goal_id: [2; 16],
        status: 4,
        result: vec![],
    }
    .encode();
    assert!(goals.input(&response, &b).is_err());
    goals.effect(&goal(4, 20, 12)).expect("second");
    let extra = goal(6, 20, 13);
    assert_eq!(
        goals.effect(&extra).expect("extra visible").body,
        extra.body
    );
    response.body = GoalResult {
        goal_send_ordinal: 4,
        goal_id: [99; 16],
        status: 4,
        result: vec![],
    }
    .encode();
    assert!(goals.input(&response, &b).is_err());
}

const PROXY: &str = r#"import sys,json,subprocess
rook,config=sys.argv[1:]
config=json.load(open(config))
p=subprocess.Popen([rook,'__adapter',config['variant']],stdin=subprocess.PIPE,stdout=subprocess.PIPE,text=True,bufsize=1)
original_id=None
for line in sys.stdin:
 request=json.loads(line)
 if request['type']=='step':
  with open(config['log'],'a') as log: log.write(str(request['input']['ordinal'])+'\n')
 if config.get('different_id') and request['type']=='step' and request['input']['event_type'] in ['GoalResponse','Feedback','Result']:
  body=bytearray.fromhex(request['input']['body_hex']);body[8:24]=original_id;request['input']['body_hex']=body.hex()
 p.stdin.write(json.dumps(request)+'\n');p.stdin.flush()
 while True:
  response=json.loads(p.stdout.readline())
  if response['type']=='effect':
   wire=response['effect']; body=bytearray.fromhex(wire['body_hex'])
   if wire['event_type']=='GoalSend': original_id=bytes(body[:16])
   if config.get('different_id') and wire['event_type'] in ['GoalSend','CancelSend'] and any(body[:16]): body[:16]=bytes([99])*16
   if config.get('different_goal') and wire['event_type']=='GoalSend': body[-1]^=1
   if config.get('different_status') and wire['event_type']=='Status': body+=b' extra diagnostic'
   wire['body_hex']=body.hex()
   if config.get('unknown_channel') and wire['event_type']=='CancelSend': wire['channel_id']=99
  suppress=(config.get('pending_step') and response['type']=='stepped' and request['type']=='step' and request['input']['ordinal']==6)
  if not suppress: print(json.dumps(response),flush=True)
  if response['type'] in ['started','stepped','finished','refused']: break
 if request['type']=='finish': break
p.stdin.close();sys.exit(p.wait())
"#;

fn artifact(path: &Path) -> Artifact {
    Artifact {
        path: path.canonicalize().expect("artifact path"),
        blake3: case::hash(&std::fs::read(path).expect("artifact bytes")),
    }
}
fn external(case: &CopyCase, options: Value, normalization: bool) -> PathBuf {
    let rook = PathBuf::from(env!("CARGO_BIN_EXE_rook"));
    let python = Command::new("python3")
        .args(["-c", "import sys;print(sys.executable)"])
        .output()
        .expect("python");
    assert!(python.status.success());
    let python = PathBuf::from(
        String::from_utf8(python.stdout)
            .expect("python path")
            .trim(),
    );
    let script = case.0.join("proxy.py");
    std::fs::write(&script, PROXY).expect("proxy");
    let baseline = case.0.join("old.json");
    std::fs::write(
        &baseline,
        serde_json::to_vec(&json!({"variant":"old","log":case.0.join("baseline-inputs")}))
            .expect("json"),
    )
    .expect("baseline");
    let mut options = options;
    options["log"] = json!(case.0.join("candidate-inputs"));
    let component = case.0.join("candidate.json");
    std::fs::write(&component, serde_json::to_vec(&options).expect("json")).expect("candidate");
    let mut identity =
        case::values(&std::fs::read_to_string(case.0.join("identity")).expect("identity"))
            .expect("parse");
    if normalization {
        identity.insert("normalization".into(), normalization::POLICY.into());
        identity.insert("normalization_program".into(), "rook normalization".into());
        identity.insert(
            "normalization_source_blake3".into(),
            normalization::source_hash(),
        );
        identity.insert("normalization.client.20".into(), "client".into());
        identity.insert("normalization.client.21".into(), "client".into());
        case.edit("case", "raw-bytes-in-order@1", "goal-ids-in-order@1");
    }
    let mut build = Build {
        identity,
        adapter: artifact(&python),
        component: artifact(&baseline),
        property: artifact(&rook),
        dependencies: BTreeMap::from([("proxy_source_blake3".into(), artifact(&script))]),
        adapter_args: vec![
            script.to_string_lossy().into(),
            rook.to_string_lossy().into(),
            "{component}".into(),
        ],
        property_args: vec!["__property".into()],
    };
    for (key, artifact) in [
        ("adapter_binary_blake3", &build.adapter),
        ("component_binary_blake3", &build.component),
        ("property_binary_blake3", &build.property),
        (
            "proxy_source_blake3",
            &build.dependencies["proxy_source_blake3"],
        ),
    ] {
        build.identity.insert(key.into(), artifact.blake3.clone());
    }
    let text = build
        .identity
        .iter()
        .map(|(k, v)| format!("{k} = {v}\n"))
        .collect::<String>();
    std::fs::write(case.0.join("identity"), text).expect("identity");
    std::fs::write(
        case.0.join("build.json"),
        serde_json::to_vec_pretty(&build).expect("build"),
    )
    .expect("write build");
    build.component = artifact(&component);
    build.identity.insert(
        "component_binary_blake3".into(),
        build.component.blake3.clone(),
    );
    build.identity.insert(
        "component_variant".into(),
        options["variant"].as_str().expect("variant").into(),
    );
    let candidate = case.0.join("candidate-build.json");
    std::fs::write(
        &candidate,
        serde_json::to_vec_pretty(&build).expect("candidate build"),
    )
    .expect("write");
    case.manifest();
    candidate
}

#[test]
fn external_candidate_finishes_at_effect_without_stepped_or_cancel_reply() {
    let case = CopyCase::new("timeout");
    let candidate = external(&case, json!({"variant":"fixed","pending_step":true}), false);
    let (code, report, _) = case.run(Some(candidate.to_str().expect("path")));
    assert_eq!(code, 0, "{report}");
    assert_eq!(report["scope_completed"], true);
    assert_eq!(
        report["property_result"]["unavailable"],
        json!(["cancel_acknowledged", "goal_terminated"])
    );
    assert!(
        !report["observed_events"]
            .as_array()
            .expect("events")
            .iter()
            .any(|e| e["event_type"] == "CancelResponse")
    );
}

#[test]
fn unrelated_output_difference_does_not_invalidate_a_completed_property() {
    let case = CopyCase::new("timely");
    let candidate = external(
        &case,
        json!({"variant":"fixed","different_status":true}),
        false,
    );
    let (code, report, _) = case.run(Some(candidate.to_str().expect("path")));
    assert_eq!(code, 0, "{report}");
    assert_eq!(report["property_result"]["result"], "pass");
    assert!(!report["first_differing_effect"].is_null());
    assert_eq!(report["raw_effects_equal"], false);
}

#[test]
fn invalidated_response_is_stopped_before_adapter_delivery() {
    let case = CopyCase::new("timely");
    let candidate = external(
        &case,
        json!({"variant":"fixed","different_goal":true}),
        false,
    );
    let (code, report, _) = case.run(Some(candidate.to_str().expect("path")));
    assert_eq!(code, 3, "{report}");
    assert!(
        report["property_result"]["missing"]
            .as_str()
            .expect("missing")
            .contains("request")
    );
    assert_eq!(report["evidence"]["refusal"]["ordinal"], 6);
    let delivered =
        std::fs::read_to_string(case.0.join("candidate-inputs")).expect("adapter delivery log");
    assert!(!delivered.lines().any(|line| line == "6"), "{delivered}");
}

#[test]
fn external_candidate_cannot_change_property_normalizer_configuration_or_scope() {
    for key in [
        "property_source_blake3",
        "property_input_schema",
        "normalization_source_blake3",
        "normalization.client.20",
        "scope.completion",
        "configuration",
        "plugin.foo",
    ] {
        let case = CopyCase::new("timeout");
        let candidate = external(&case, json!({"variant":"fixed"}), false);
        let mut build: Value =
            serde_json::from_slice(&std::fs::read(&candidate).expect("build")).expect("json");
        build["identity"][key] = json!("changed");
        std::fs::write(&candidate, serde_json::to_vec(&build).expect("json")).expect("write");
        let (code, report, _) = case.run(Some(candidate.to_str().expect("path")));
        assert_eq!(code, 2, "{report}");
        assert!(
            report["error"].as_str().expect("error").contains(key),
            "{report}"
        );
        assert!(!case.0.join("baseline-inputs").exists());
    }
}

#[test]
fn normalized_agreement_does_not_claim_raw_equality() {
    // The candidate emits a different raw goal ID. The unchanged reference component
    // only consumes recorded IDs, so use timeout, which needs no response translation.
    let case = CopyCase::new("timeout");
    let candidate = external(&case, json!({"variant":"old","different_id":true}), true);
    let (code, report, _) = case.run(Some(candidate.to_str().expect("path")));
    assert_eq!(code, 1, "{report}"); // Old still fails the immutable timeout property.
    assert_eq!(report["raw_effects_equal"], false);
    assert_eq!(report["execution_agreement"], "measured agreement");
    assert_eq!(
        report["recorded_execution_hashes"],
        report["normalized_replay_execution_hashes"]
    );
    assert_ne!(
        report["raw_replay_execution_hashes"],
        report["normalized_replay_execution_hashes"]
    );
    assert!(
        report["observed_events"]
            .as_array()
            .expect("events")
            .iter()
            .any(|e| e["event_type"] == "GoalSend"
                && e["body_hex"]
                    .as_str()
                    .expect("body")
                    .starts_with(&"63".repeat(16)))
    );
}

#[test]
fn binary_artifact_drift_is_refused_before_execution() {
    let case = CopyCase::new("timeout");
    let candidate = external(&case, json!({"variant":"fixed"}), false);
    std::fs::write(case.0.join("candidate.json"), b"different bytes").expect("tamper artifact");
    let (code, report, _) = case.run(Some(candidate.to_str().expect("path")));
    assert_eq!(code, 2, "{report}");
    assert!(
        report["error"]
            .as_str()
            .expect("error")
            .contains("component_binary_blake3")
    );
    assert!(!case.0.join("baseline-inputs").exists());
}

#[test]
fn invalid_normalization_contract_is_refused() {
    let case = CopyCase::new("timeout");
    external(&case, json!({"variant":"fixed"}), true);
    case.edit("case", "goal-ids-in-order@1", "raw-bytes-in-order@1");
    case.manifest();
    let (code, report, _) = case.run(None);
    assert_eq!(code, 2, "{report}");
    assert!(
        report["error"]
            .as_str()
            .expect("error")
            .contains("normalization contract")
    );
    assert!(!case.0.join("baseline-inputs").exists());
}

#[cfg(unix)]
#[test]
fn invalid_utf8_invocation_returns_structured_exit_two() {
    use std::os::unix::ffi::OsStringExt;
    let output = Command::new(env!("CARGO_BIN_EXE_rook"))
        .arg(std::ffi::OsString::from_vec(vec![255]))
        .output()
        .expect("CLI");
    assert_eq!(output.status.code(), Some(2));
    assert_eq!(
        serde_json::from_slice::<Value>(&output.stdout).expect("report")["exit_code"],
        2
    );
}

#[test]
fn duplicate_execution_identity_keeps_raw_integrity_separate() {
    let mut goals = normalizer(&[goal(2, 20, 1), goal(4, 20, 2)]);
    goals.effect(&goal(2, 20, 11)).expect("first issuance");
    let error = goals
        .effect(&goal(4, 20, 11))
        .expect_err("duplicate issuance");
    let report = rook_verify::error_report(&error);
    assert_eq!(report["exit_code"], 1);
    assert_eq!(report["file_integrity"], "passed");
    assert_eq!(report["execution_agreement"], "diverged");
}

#[test]
fn eligible_responses_follow_declared_id_mapping_through_the_cli() {
    let case = CopyCase::new("timely");
    let candidate = external(&case, json!({"variant":"fixed","different_id":true}), true);
    let (code, report, _) = case.run(Some(candidate.to_str().expect("path")));
    assert_eq!(code, 0, "{report}");
    assert_eq!(report["property_result"]["result"], "pass");
    assert_eq!(report["raw_effects_equal"], false);
    assert_eq!(report["execution_agreement"], "measured agreement");
    assert_eq!(
        report["recorded_execution_hashes"],
        report["normalized_replay_execution_hashes"]
    );
    assert!(
        report["observed_events"]
            .as_array()
            .expect("events")
            .iter()
            .any(|e| e["event_type"] == "GoalResponse"
                && e["consumed"] == true
                && e["body_hex"].as_str().expect("body")[16..48] == "63".repeat(16))
    );
}

#[test]
fn property_process_exit_must_match_its_verdict() {
    let case = CopyCase::new("timeout");
    external(&case, json!({"variant":"fixed"}), false);
    let path = case.0.join("build.json");
    let mut build: Build =
        serde_json::from_slice(&std::fs::read(&path).expect("build")).expect("json");
    build.property = build.adapter.clone();
    build.property_args=vec!["-c".into(),"import json;print(json.dumps({'schema':'rook-property-result@1','property':'goal-response-timeout-cancel@1','result':'inconclusive','missing':'clock','unavailable':[],'detail':'missing clock'}));raise SystemExit(0)".into()];
    build.identity.insert(
        "property_binary_blake3".into(),
        build.property.blake3.clone(),
    );
    std::fs::write(
        case.0.join("identity"),
        build
            .identity
            .iter()
            .map(|(k, v)| format!("{k} = {v}\n"))
            .collect::<String>(),
    )
    .expect("identity");
    std::fs::write(path, serde_json::to_vec(&build).expect("json")).expect("write");
    case.manifest();
    let (code, report, _) = case.run(None);
    assert_eq!(code, 2, "{report}");
    assert!(
        report["error"]
            .as_str()
            .expect("error")
            .contains("property result and process exit disagree")
    );
}

#[test]
fn a_gap_with_matching_effects_leaves_execution_agreement_unavailable() {
    for scenario in ["timeout-gap", "command-gap"] {
        let case = CopyCase::new(scenario);
        for candidate in [None, Some("fixed")] {
            let (code, report, _) = case.run(candidate);
            assert_eq!(code, if candidate.is_some() { 3 } else { 2 }, "{report}");
            assert_eq!(report["baseline_reproduced"], Value::Null);
            for comparison in [&report, &report["baseline_comparison"]] {
                assert_eq!(comparison["raw_effects_equal"], true);
                assert_eq!(comparison["first_differing_effect"], Value::Null);
                assert_eq!(comparison["execution_agreement"], "unavailable", "{report}");
            }
        }
    }
}

#[test]
fn a_later_gap_does_not_erase_a_known_baseline_mismatch() {
    let case = CopyCase::new("timeout-gap");
    let candidate = external(&case, json!({"variant":"fixed"}), false);
    let config = case.0.join("old.json");
    let mut options: Value =
        serde_json::from_slice(&std::fs::read(&config).expect("config")).expect("json");
    options["different_goal"] = json!(true);
    std::fs::write(&config, serde_json::to_vec(&options).expect("json")).expect("write");
    let path = case.0.join("build.json");
    let mut build: Build =
        serde_json::from_slice(&std::fs::read(&path).expect("build")).expect("json");
    build.component = artifact(&config);
    build.identity.insert(
        "component_binary_blake3".into(),
        build.component.blake3.clone(),
    );
    std::fs::write(
        case.0.join("identity"),
        build
            .identity
            .iter()
            .map(|(k, v)| format!("{k} = {v}\n"))
            .collect::<String>(),
    )
    .expect("identity");
    std::fs::write(path, serde_json::to_vec(&build).expect("json")).expect("write");
    case.manifest();
    let (code, report, _) = case.run(Some(candidate.to_str().expect("path")));
    assert_eq!(code, 1, "{report}");
    assert_eq!(report["baseline_reproduced"], false);
    assert_eq!(report["property_result"]["result"], "inconclusive");
    assert_eq!(
        report["baseline_comparison"]["execution_agreement"],
        "diverged"
    );
    let (code, report, _) = case.run(None);
    assert_eq!(code, 1, "{report}");
    assert_eq!(report["execution_agreement"], "diverged");
    assert!(!report["first_differing_effect"].is_null());
}

#[test]
fn a_cancel_on_an_undeclared_channel_cannot_complete_the_property() {
    let case = CopyCase::new("timeout");
    let candidate = external(
        &case,
        json!({"variant":"fixed","unknown_channel":true}),
        false,
    );
    let (code, report, _) = case.run(Some(candidate.to_str().expect("path")));
    assert_eq!(code, 2, "{report}");
    assert!(
        report["error"]
            .as_str()
            .expect("error")
            .contains("channel.99")
    );
}

#[test]
fn duplicate_channel_aliases_cannot_overwrite_client_correspondence() {
    let identity = BTreeMap::from([
        ("normalization".into(), normalization::POLICY.into()),
        (
            "normalization_source_blake3".into(),
            normalization::source_hash(),
        ),
        ("normalization.client.020".into(), "first".into()),
        ("normalization.client.20".into(), "second".into()),
    ]);
    let error = match Goals::new(&[goal(2, 20, 1)], &identity, "goal-ids-in-order@1") {
        Ok(_) => panic!("ambiguous client configuration was accepted"),
        Err(error) => error,
    };
    assert!(error.to_string().contains("ambiguous normalization client"));
    assert_eq!(case::exit_code(&error), 2);
}

#[test]
fn unchanged_artifacts_can_be_relocated_without_changing_their_identities() {
    let case = CopyCase::new("timeout");
    let candidate = external(&case, json!({"variant":"fixed"}), false);
    let baseline = Build::load(&case.0.join("build.json")).expect("baseline build");
    let mut relocated = Build::load(&candidate).expect("candidate build");
    let directory = case.0.join("relocated");
    std::fs::create_dir(&directory).expect("relocation directory");
    for (name, artifact) in [
        ("adapter", &mut relocated.adapter),
        ("property", &mut relocated.property),
    ]
    .into_iter()
    .chain(
        relocated
            .dependencies
            .iter_mut()
            .map(|(name, artifact)| (name.as_str(), artifact)),
    ) {
        let path = directory.join(name);
        std::fs::copy(&artifact.path, &path).expect("relocate artifact");
        artifact.path = path;
    }
    std::fs::write(&candidate, serde_json::to_vec(&relocated).expect("JSON"))
        .expect("build description");
    let relocated = Build::load(&candidate).expect("rehash relocated artifacts");
    baseline
        .candidate(&relocated)
        .expect("byte-identical protected artifacts");
    let mut changed = relocated;
    changed
        .dependencies
        .get_mut("proxy_source_blake3")
        .expect("dependency")
        .blake3 = "changed".into();
    assert!(baseline.candidate(&changed).is_err());
}

#[test]
fn a_missing_case_reports_the_manifest_path() {
    let case = CopyCase::new("timeout");
    let missing = case.0.join("absent");
    let output = Command::new(env!("CARGO_BIN_EXE_rook"))
        .arg("verify")
        .arg(&missing)
        .output()
        .expect("CLI");
    assert_eq!(output.status.code(), Some(2));
    let report: Value = serde_json::from_slice(&output.stdout).expect("report");
    assert!(
        report["error"]
            .as_str()
            .expect("diagnostic")
            .contains(missing.join("MANIFEST.sha256").to_str().expect("path"))
    );
}

#[test]
fn invalid_property_input_cannot_pass_baseline_verification() {
    let case = CopyCase::new("timeout");
    external(&case, json!({"variant":"fixed"}), false);
    let path = case.0.join("build.json");
    let mut build: Build =
        serde_json::from_slice(&std::fs::read(&path).expect("build")).expect("json");
    build.property = build.adapter.clone();
    build.property_args=vec!["-c".into(),"import json;print(json.dumps({'schema':'rook-property-result@1','property':'goal-response-timeout-cancel@1','result':'invalid','missing':'clock','unavailable':[],'detail':'unsupported scope'}));raise SystemExit(2)".into()];
    build.identity.insert(
        "property_binary_blake3".into(),
        build.property.blake3.clone(),
    );
    std::fs::write(
        case.0.join("identity"),
        build
            .identity
            .iter()
            .map(|(k, v)| format!("{k} = {v}\n"))
            .collect::<String>(),
    )
    .expect("identity");
    std::fs::write(path, serde_json::to_vec(&build).expect("json")).expect("write");
    case.manifest();
    let (code, report, _) = case.run(None);
    assert_eq!(code, 2, "{report}");
    assert!(
        report["error"]
            .as_str()
            .expect("error")
            .contains("invalid property input")
    );
}
