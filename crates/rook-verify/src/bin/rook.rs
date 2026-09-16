//! User commands. Machine-readable ndjson goes to stdout; the local report goes to stderr.
use anyhow::{Context, Result, bail};
use rook_verify::{error_report, readable, run_case};
use std::path::Path;
fn run() -> Result<serde_json::Value> {
    let args = std::env::args_os()
        .skip(1)
        .map(|arg| arg.into_string())
        .collect::<Result<Vec<_>, _>>()
        .map_err(|_| anyhow::anyhow!("arguments must be UTF-8"))
        .context("invalid invocation")?;
    match args
        .iter()
        .map(String::as_str)
        .collect::<Vec<_>>()
        .as_slice()
    {
        ["verify", path] if !path.starts_with('-') => {
            let manifest_path = Path::new(path).join("MANIFEST.sha256");
            let manifest = std::fs::read_to_string(&manifest_path)
                .with_context(|| format!("read {}", manifest_path.display()))?;
            if manifest.lines().next() == Some("# rook-capsule-v1 kind=wasm-rmf-blockade") {
                rook_verify::wasm_verify(Path::new(path))
            } else {
                run_case(Path::new(path), None)
            }
        }
        ["test", path, "--candidate", candidate] if !path.starts_with('-') => {
            run_case(Path::new(path), Some(candidate))
        }
        ["__adapter", variant] => {
            let variant = rook_adapter_ref::component::Variant::parse(variant)
                .ok_or_else(|| anyhow::anyhow!("unknown variant"))?;
            rook_adapter_ref::adapter::serve(
                variant,
                std::io::stdin().lock(),
                std::io::stdout().lock(),
            )?;
            std::process::exit(0);
        }
        ["__property", path] => {
            std::process::exit(rook_verify::property_subcommand(Path::new(path))?)
        }
        _ => bail!("usage: rook verify CASE | rook test CASE --candidate BUILD"),
    }
}
fn main() {
    let report = match run() {
        Ok(report) => report,
        Err(error) => error_report(&error),
    };
    if let Some(text) = report["report"].as_str() {
        eprint!("{text}");
    } else {
        eprint!("{}", readable(&report));
    }
    println!("{report}");
    std::process::exit(report["exit_code"].as_i64().unwrap_or(2) as i32);
}
