use anyexe::analyze;
use anyexe::launch::{self, RunOptions};
use anyexe::pe::{Layout, Pe};
use std::process::ExitCode;

fn usage() {
    eprintln!("usage: anyexe inspect [--json] [--verbose] <app.exe>");
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.first().map(String::as_str) {
        Some("run") => {
            let mut opts = RunOptions::default();
            let mut rest = &args[1..];
            while let Some(a) = rest.first() {
                match a.as_str() {
                    "--strict" => opts.strict = true,
                    "--prefix" if rest.len() > 1 => {
                        opts.prefix = Some(rest[1].clone().into());
                        rest = &rest[1..];
                    }
                    _ => break,
                }
                rest = &rest[1..];
            }
            let Some(exe) = rest.first() else {
                usage();
                return ExitCode::from(2);
            };
            let err = launch::run_native(std::path::Path::new(exe), rest[1..].to_vec(), &opts);
            eprintln!("anyexe: {exe}: {err}");
            ExitCode::from(126)
        }
        Some("inspect") => {
            let mut json = false;
            let mut verbose = false;
            let mut path = None;
            for a in &args[1..] {
                match a.as_str() {
                    "--json" => json = true,
                    "--verbose" | "-v" => verbose = true,
                    p => path = Some(p.to_string()),
                }
            }
            let Some(path) = path else {
                usage();
                return ExitCode::from(2);
            };
            let bytes = match std::fs::read(&path) {
                Ok(b) => b,
                Err(e) => {
                    eprintln!("anyexe: {path}: {e}");
                    return ExitCode::from(1);
                }
            };
            let pe = match Pe::parse(&bytes, Layout::File) {
                Ok(p) => p,
                Err(e) => {
                    eprintln!("anyexe: {path}: {e}");
                    return ExitCode::from(1);
                }
            };
            let report = analyze::analyze(&pe, Some(std::path::Path::new(&path)));
            if json {
                print!("{}", analyze::to_json(&path, &report));
            } else {
                analyze::print_report(&path, &report, verbose);
            }
            ExitCode::SUCCESS
        }
        _ => {
            usage();
            ExitCode::from(2)
        }
    }
}
