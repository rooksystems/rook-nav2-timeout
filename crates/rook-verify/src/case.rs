//! Case loading checks evidence before any executable is started. Build descriptions
//! pin local artifacts; candidate substitution can change only the component.
use anyhow::{Context, Result, ensure};
use rook_adapter_ref::{encode_hex, fixture, source_identity};
use rook_native::{EnvelopeKind, Frame};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Component, Path, PathBuf};

pub type Identity = BTreeMap<String, String>;

#[derive(Debug)]
pub struct Failure {
    pub code: i32,
    pub integrity: bool,
    pub detail: String,
}
impl std::fmt::Display for Failure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.detail)
    }
}
impl std::error::Error for Failure {}
pub fn mismatch(detail: impl Into<String>) -> anyhow::Error {
    Failure {
        code: 1,
        integrity: true,
        detail: detail.into(),
    }
    .into()
}
pub fn execution_mismatch(detail: impl Into<String>) -> anyhow::Error {
    Failure {
        code: 1,
        integrity: false,
        detail: detail.into(),
    }
    .into()
}
pub fn exit_code(error: &anyhow::Error) -> i32 {
    error
        .downcast_ref::<Failure>()
        .map_or(2, |failure| failure.code)
}
pub fn hash(bytes: &[u8]) -> String {
    encode_hex(blake3::hash(bytes).as_bytes())
}
pub fn values(text: &str) -> Result<Identity> {
    let mut values = Identity::new();
    for line in text
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty() && !l.starts_with('#'))
    {
        let (key, value) = line.split_once('=').context("expected key=value")?;
        ensure!(
            !key.trim().is_empty() && !value.trim().is_empty(),
            "empty key or value"
        );
        ensure!(
            values
                .insert(key.trim().into(), value.trim().into())
                .is_none(),
            "duplicate key {}",
            key.trim()
        );
    }
    Ok(values)
}
pub fn entry<'a>(values: &'a Identity, key: &str) -> Result<&'a str> {
    values
        .get(key)
        .map(String::as_str)
        .with_context(|| format!("missing manifest entry {key}"))
}
/// Compare the whole map, including unexpected dependencies, in lexical key order.
pub fn same_identity(expected: &Identity, actual: &Identity, candidate: bool) -> Result<()> {
    for key in expected
        .keys()
        .chain(actual.keys())
        .collect::<BTreeSet<_>>()
    {
        if candidate
            && matches!(
                key.as_str(),
                "component_variant" | "component_source_blake3" | "component_binary_blake3"
            )
        {
            continue;
        }
        ensure!(
            expected.get(key) == actual.get(key),
            "identity drift at {key}, recorded {:?}, build {:?}",
            expected.get(key),
            actual.get(key)
        );
    }
    Ok(())
}

/// Channel and endpoint declarations are part of the supported boundary. A typed
/// effect on an undeclared channel cannot become valid property evidence.
pub fn validate_event(frame: &Frame, identity: &Identity) -> Result<()> {
    if matches!(frame.kind(), EnvelopeKind::Marker | EnvelopeKind::Gap) {
        return Ok(());
    }
    entry(identity, &format!("endpoint.{}", frame.src_actor))?;
    entry(identity, &format!("endpoint.{}", frame.dst_actor))?;
    let key = format!("channel.{}", frame.channel_id);
    let declaration = entry(identity, &key)?;
    let fields = declaration.rsplitn(3, ' ').collect::<Vec<_>>();
    let direction = if frame.kind() == EnvelopeKind::Emit {
        "effect"
    } else {
        "input"
    };
    ensure!(
        fields.len() == 3
            && fields[1] == direction
            && fields[0]
                .split(',')
                .any(|name| name == rook_adapter_ref::event_type_name(frame.header.event_type)),
        "event at ordinal {} is outside declared {key}",
        frame.ordinal
    );
    Ok(())
}

pub struct Case {
    pub root: PathBuf,
    pub identity: Identity,
    pub declaration: Identity,
    pub expected: Identity,
    pub frames: Vec<Frame>,
    pub case_id: String,
    pub raw_hash: String,
    pub captured_hash: String,
    pub covered: BTreeSet<String>,
}
impl Case {
    pub fn load(root: &Path) -> Result<Self> {
        let root = root.canonicalize().context("open case directory")?;
        let manifest = std::fs::read_to_string(root.join("MANIFEST.sha256"))
            .context("read MANIFEST.sha256")?;
        let mut lines = manifest.lines();
        let kind = lines
            .next()
            .and_then(|l| l.strip_prefix("# rook-capsule-v1 kind="))
            .context("malformed capsule header")?;
        ensure!(
            kind == "native-adapter",
            "capsule kind {kind} is not executable by this runner"
        );
        let mut covered = BTreeSet::new();
        for line in lines.filter(|l| !l.trim().is_empty() && !l.starts_with('#')) {
            let (digest, name) = line
                .split_once("  ")
                .context("malformed SHA-256 manifest line")?;
            ensure!(
                digest.len() == 64 && digest.bytes().all(|b| b.is_ascii_hexdigit()),
                "malformed SHA-256 for {name}"
            );
            ensure!(
                Path::new(name)
                    .components()
                    .all(|c| matches!(c, Component::Normal(_)))
                    && !name.is_empty(),
                "invalid manifest path {name}"
            );
            ensure!(
                covered.insert(name.to_owned()),
                "duplicate manifest file {name}"
            );
            let path = root
                .join(name)
                .canonicalize()
                .with_context(|| format!("missing evidence file {name}"))?;
            ensure!(
                path.starts_with(&root),
                "manifest file escapes case directory, {name}"
            );
            let bytes = std::fs::read(path)?;
            if encode_hex(&Sha256::digest(&bytes)) != digest.to_ascii_lowercase() {
                return Err(mismatch(format!("file integrity mismatch at {name}")));
            }
        }
        for name in [
            "case",
            "identity",
            "expected",
            "events.bin",
            "effects_captured.bin",
        ] {
            ensure!(
                covered.contains(name),
                "manifest missing required file {name}"
            );
        }
        let read =
            |name| -> Result<Identity> { values(&std::fs::read_to_string(root.join(name))?) };
        let declaration = read("case")?;
        let identity = read("identity")?;
        let expected = read("expected")?;
        ensure!(
            entry(&expected, "claim")? == "measured",
            "native claim must be measured"
        );
        ensure!(
            ["Complete", "Rebuilt", "Inferred", "Watch-only"].contains(&entry(&expected, "grade")?),
            "unknown grade"
        );
        for key in [
            "corpus",
            "scenario",
            "property",
            "scope.completion",
            "scope.observation_end",
            "comparison_policy",
            "origin",
            "grade_reason",
            "goal_response_deadline_ns",
        ] {
            entry(&declaration, key)?;
        }
        for key in [
            "component",
            "component_variant",
            "adapter",
            "adapter_protocol",
            "starting_state",
            "starting_state_blake3",
            "normalization",
            "normalization_source_blake3",
            "property",
            "property_input_schema",
            "property_source_blake3",
        ] {
            entry(&identity, key)?;
        }
        ensure!(
            entry(&identity, "property")? == entry(&declaration, "property")?,
            "identity drift at property"
        );
        ensure!(
            entry(&identity, "property_input_schema")? == rook_adapter_ref::property::INPUT_SCHEMA,
            "unsupported property_input_schema"
        );
        ensure!(
            entry(&declaration, "goal_response_deadline_ns")?.parse::<i64>()? > 0,
            "invalid property deadline configuration"
        );
        let events = std::fs::read(root.join("events.bin"))?;
        let captured = std::fs::read(root.join("effects_captured.bin"))?;
        let raw_hash = hash(&events);
        let captured_hash = hash(&captured);
        for (key, actual) in [
            ("raw_record_blake3", &raw_hash),
            ("effects_captured_blake3", &captured_hash),
        ] {
            if entry(&expected, key)? != actual {
                return Err(mismatch(format!("integrity mismatch at {key}")));
            }
        }
        let frames = rook_native::decode_stream(&events)?;
        for frame in &frames {
            validate_event(frame, &identity)?;
        }
        let states = frames
            .iter()
            .filter(|f| f.header.event_type == rook_native::EventType::StartingState)
            .collect::<Vec<_>>();
        ensure!(
            states.len() == 1 && states[0].ordinal == 1,
            "starting state must occur once before inputs"
        );
        let method = match entry(&identity, "starting_state")? {
            "fresh" => 1_u16,
            "snapshot" => 2,
            other => anyhow::bail!("unsupported starting_state {other}"),
        };
        ensure!(
            states[0].body[..2] == method.to_le_bytes()
                && encode_hex(&states[0].body[2..]) == entry(&identity, "starting_state_blake3")?,
            "identity drift at starting_state"
        );
        let captured = fixture::decode_captured(&captured)?;
        let effects = frames
            .iter()
            .filter(|f| f.kind() == EnvelopeKind::Emit)
            .map(Frame::payload)
            .collect::<Vec<_>>();
        if effects != captured {
            return Err(mismatch(
                "independently captured effects differ from events.bin",
            ));
        }
        let hashes = rook_native::hash_frames(entry(&declaration, "corpus")?.as_bytes(), &frames);
        for (key, actual) in [
            ("trace_hash", hashes.trace_hash),
            ("run_hash", hashes.run_hash),
            ("output_digest", hashes.output_digest),
        ] {
            if entry(&expected, key)? != encode_hex(&actual) {
                return Err(mismatch(format!("recorded {key} mismatch")));
            }
        }
        let case_id = hash(manifest.as_bytes());
        Ok(Self {
            root,
            identity,
            declaration,
            expected,
            frames,
            case_id,
            raw_hash,
            captured_hash,
            covered,
        })
    }

    pub fn reference_identity(&self) -> Result<Identity> {
        ensure!(
            entry(&self.declaration, "corpus")? == rook_adapter_ref::CORPUS,
            "unsupported corpus, supply a manifest-covered build.json"
        );
        let scenario = fixture::scenarios()
            .into_iter()
            .find(|s| s.name == self.declaration["scenario"])
            .context("unsupported reference scenario")?;
        // Read only the source-pinned identity template. No fixture is regenerated.
        let mut identity = values(include_str!(
            "../../../fixtures/native-ref/timeout/identity"
        ))?;
        identity.insert("property".into(), scenario.property.into());
        for (key, value) in source_identity() {
            identity.insert(key.into(), value);
        }
        same_identity(&self.identity, &identity, false)?;
        ensure!(
            entry(&self.declaration, "property")? == scenario.property,
            "identity drift at property"
        );
        ensure!(
            entry(&self.declaration, "scope.completion")? == scenario.completion,
            "identity drift at scope.completion"
        );
        ensure!(
            entry(&self.declaration, "scope.observation_end")? == "end_of_recording",
            "identity drift at scope.observation_end"
        );
        ensure!(
            entry(&self.declaration, "goal_response_deadline_ns")?
                == rook_adapter_ref::GOAL_RESPONSE_DEADLINE_NS.to_string(),
            "identity drift at goal_response_deadline_ns"
        );
        Ok(identity)
    }
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Artifact {
    pub path: PathBuf,
    pub blake3: String,
}
impl Artifact {
    fn check(&mut self, root: &Path, name: &str) -> Result<()> {
        self.path = root
            .join(&self.path)
            .canonicalize()
            .with_context(|| format!("open artifact {name}"))?;
        ensure!(
            hash(&std::fs::read(&self.path)?) == self.blake3,
            "identity drift at {name}, artifact bytes differ"
        );
        Ok(())
    }
}

/// External adapters use a build.json covered by the case manifest. Arguments are
/// literal argv entries. Only {component} is substituted, without invoking a shell.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Build {
    pub identity: Identity,
    pub adapter: Artifact,
    pub component: Artifact,
    pub property: Artifact,
    pub dependencies: BTreeMap<String, Artifact>,
    pub adapter_args: Vec<String>,
    pub property_args: Vec<String>,
}
impl Build {
    pub fn load(path: &Path) -> Result<Self> {
        let mut build: Self = serde_json::from_slice(&std::fs::read(path)?)?;
        let root = path.parent().context("build file has no parent")?;
        build.adapter.check(root, "adapter_binary_blake3")?;
        build.component.check(root, "component_binary_blake3")?;
        build.property.check(root, "property_binary_blake3")?;
        for (key, artifact) in &mut build.dependencies {
            artifact.check(root, key)?;
        }
        for (key, artifact) in [
            ("adapter_binary_blake3", &build.adapter),
            ("component_binary_blake3", &build.component),
            ("property_binary_blake3", &build.property),
        ]
        .into_iter()
        .chain(
            build
                .dependencies
                .iter()
                .map(|(key, artifact)| (key.as_str(), artifact)),
        ) {
            ensure!(
                entry(&build.identity, key)? == artifact.blake3,
                "identity drift at {key}"
            );
        }
        ensure!(
            build.adapter_args.iter().any(|a| a == "{component}"),
            "adapter_args must name {{component}}"
        );
        Ok(build)
    }
    pub fn candidate(&self, candidate: &Self) -> Result<()> {
        same_identity(&self.identity, &candidate.identity, true)?;
        ensure!(
            self.adapter.blake3 == candidate.adapter.blake3,
            "identity drift at adapter artifact"
        );
        ensure!(
            self.property.blake3 == candidate.property.blake3,
            "identity drift at property artifact"
        );
        ensure!(
            self.dependencies
                .iter()
                .map(|(key, artifact)| (key, &artifact.blake3))
                .eq(candidate
                    .dependencies
                    .iter()
                    .map(|(key, artifact)| (key, &artifact.blake3))),
            "identity drift at dependency artifacts"
        );
        ensure!(
            self.adapter_args == candidate.adapter_args,
            "identity drift at adapter_args"
        );
        ensure!(
            self.property_args == candidate.property_args,
            "identity drift at property_args"
        );
        Ok(())
    }
    pub fn command(&self) -> std::process::Command {
        let mut command = std::process::Command::new(&self.adapter.path);
        for arg in &self.adapter_args {
            if arg == "{component}" {
                command.arg(&self.component.path);
            } else {
                command.arg(arg);
            }
        }
        command
    }
}
