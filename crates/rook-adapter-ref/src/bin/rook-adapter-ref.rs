//! usage: rook-adapter-ref serve --component old|fixed|noop|always-cancel
//!        rook-adapter-ref fixtures <dir>
//!        rook-adapter-ref identity

use std::path::Path;

use rook_adapter_ref::adapter::serve;
use rook_adapter_ref::component::Variant;
use rook_adapter_ref::fixture::{scenarios, write_capsule};
use rook_adapter_ref::source_identity;

const USAGE: &str = "usage: rook-adapter-ref serve --component old|fixed|noop|always-cancel\n       rook-adapter-ref fixtures <dir>\n       rook-adapter-ref identity";

fn main() {
    let arguments: Vec<String> = std::env::args().skip(1).collect();
    let code = match arguments
        .iter()
        .map(String::as_str)
        .collect::<Vec<_>>()
        .as_slice()
    {
        ["serve", "--component", variant] => match Variant::parse(variant) {
            Some(variant) => {
                let stdin = std::io::stdin();
                let stdout = std::io::stdout();
                match serve(variant, stdin.lock(), stdout.lock()) {
                    Ok(()) => 0,
                    Err(error) => {
                        eprintln!("error: {error:#}");
                        2
                    }
                }
            }
            None => {
                eprintln!("unknown component variant {variant}\n{USAGE}");
                2
            }
        },
        ["fixtures", dir] => {
            let mut code = 0;
            for scenario in scenarios() {
                if let Err(error) = write_capsule(&Path::new(dir).join(scenario.name), &scenario) {
                    eprintln!("error: {error:#}");
                    code = 2;
                }
            }
            code
        }
        ["identity"] => {
            for (key, value) in source_identity() {
                println!("{key} = {value}");
            }
            0
        }
        _ => {
            eprintln!("{USAGE}");
            2
        }
    };
    std::process::exit(code);
}
