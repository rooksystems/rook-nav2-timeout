//! usage: rook-property-ref evaluate <input.json>
//! Prints one rook-property-result@1 line, invalid input included. Exit 0
//! pass, 1 fail, 2 invalid input, 3 inconclusive.

use rook_adapter_ref::property::{PropertyInput, PropertyResult, evaluate};

const USAGE: &str = "usage: rook-property-ref evaluate <input.json>";

fn main() {
    let arguments: Vec<String> = std::env::args().skip(1).collect();
    let code = match arguments
        .iter()
        .map(String::as_str)
        .collect::<Vec<_>>()
        .as_slice()
    {
        ["evaluate", path] => match run(path) {
            Ok(code) => code,
            Err(error) => {
                let result = PropertyResult::invalid("unknown", format!("{error:#}"));
                println!("{}", serde_json::to_string(&result).unwrap_or_default());
                result.exit_code()
            }
        },
        _ => {
            eprintln!("{USAGE}");
            2
        }
    };
    std::process::exit(code);
}

fn run(path: &str) -> anyhow::Result<i32> {
    let text = std::fs::read_to_string(path)?;
    let input: PropertyInput = serde_json::from_str(&text)?;
    let result = evaluate(&input)?;
    println!("{}", serde_json::to_string(&result)?);
    Ok(result.exit_code())
}
