use std::path::PathBuf;
use std::process::ExitCode;
use std::time::Instant;

use palmr_stress::gen::{encode_hex, generate_bytes, hash_generated, GenError, Seed};
use palmr_stress::manifest::{default_manifest_path, verify_entry, Manifest};

const USAGE: &str = "Usage:
  gen-verify [--manifest <path>] [--max-size <bytes>]
  gen-verify digest --seed <64 hex> --size <bytes>
  gen-verify slice --seed <64 hex> --size <bytes> --offset <bytes> --length <bytes>";

#[derive(Debug)]
struct Args {
    command: Option<String>,
    manifest: Option<PathBuf>,
    max_size: Option<u64>,
    seed: Option<String>,
    size: Option<u64>,
    offset: Option<u64>,
    length: Option<u64>,
}

fn parse_number(flag: &str, value: Option<String>) -> Result<u64, String> {
    value
        .as_deref()
        .and_then(|text| text.bytes().all(|b| b.is_ascii_digit()).then_some(text))
        .and_then(|text| text.parse::<u64>().ok())
        .ok_or_else(|| format!("{flag} must be a non-negative integer number of bytes"))
}

fn parse_args(raw: impl Iterator<Item = String>) -> Result<Args, String> {
    let mut args = Args {
        command: None,
        manifest: None,
        max_size: None,
        seed: None,
        size: None,
        offset: None,
        length: None,
    };
    let mut raw = raw.peekable();
    if raw.peek().is_some_and(|first| !first.starts_with("--")) {
        args.command = raw.next();
    }
    while let Some(flag) = raw.next() {
        match flag.as_str() {
            "--manifest" => args.manifest = raw.next().map(PathBuf::from),
            "--max-size" => args.max_size = Some(parse_number("--max-size", raw.next())?),
            "--seed" => args.seed = raw.next(),
            "--size" => args.size = Some(parse_number("--size", raw.next())?),
            "--offset" => args.offset = Some(parse_number("--offset", raw.next())?),
            "--length" => args.length = Some(parse_number("--length", raw.next())?),
            other => return Err(format!("unexpected argument '{other}'")),
        }
    }
    Ok(args)
}

fn seed_and_size(args: &Args) -> Result<(Seed, u64), String> {
    let seed = args.seed.as_deref().ok_or("--seed is required")?;
    let size = args.size.ok_or("--size is required")?;
    Seed::from_hex(seed)
        .map(|seed| (seed, size))
        .map_err(|error: GenError| error.to_string())
}

fn run_digest(args: &Args) -> Result<ExitCode, String> {
    let (seed, size) = seed_and_size(args)?;
    let outcome = hash_generated(&seed, size).map_err(|error| error.to_string())?;
    println!(
        "{{\"seed\":\"{}\",\"size\":{size},\"bytes\":{},\"sha256\":\"{}\"}}",
        seed.to_hex(),
        outcome.bytes,
        outcome.sha256
    );
    Ok(ExitCode::SUCCESS)
}

fn run_slice(args: &Args) -> Result<ExitCode, String> {
    let (seed, size) = seed_and_size(args)?;
    let offset = args.offset.ok_or("--offset is required")?;
    let length = args.length.ok_or("--length is required")?;
    let bytes = generate_bytes(&seed, size, offset, length).map_err(|error| error.to_string())?;
    println!("{}", encode_hex(&bytes));
    Ok(ExitCode::SUCCESS)
}

fn run_verify(args: &Args) -> Result<ExitCode, String> {
    let path = args.manifest.clone().unwrap_or_else(default_manifest_path);
    let manifest = Manifest::load(&path).map_err(|error| error.to_string())?;
    let entries = manifest.entries_up_to(args.max_size);
    if entries.is_empty() {
        return Err("no manifest entry matches the selection".to_owned());
    }
    let mut failed = 0usize;
    for entry in &entries {
        let started = Instant::now();
        let report = verify_entry(entry).map_err(|error| error.to_string())?;
        if !report.passed() {
            failed += 1;
        }
        println!(
            "{} seed={} size={} bytes={} computed={} expected={} seconds={:.1}",
            if report.passed() { "PASS" } else { "FAIL" },
            report.seed,
            report.size,
            report.bytes,
            report.computed,
            report.expected,
            started.elapsed().as_secs_f64()
        );
    }
    println!(
        "{}/{} manifest entries verified",
        entries.len() - failed,
        entries.len()
    );
    Ok(if failed == 0 {
        ExitCode::SUCCESS
    } else {
        ExitCode::FAILURE
    })
}

fn main() -> ExitCode {
    let outcome =
        parse_args(std::env::args().skip(1)).and_then(|args| match args.command.as_deref() {
            None | Some("verify") => run_verify(&args),
            Some("digest") => run_digest(&args),
            Some("slice") => run_slice(&args),
            Some(other) => Err(format!("unknown command '{other}'")),
        });
    match outcome {
        Ok(code) => code,
        Err(message) => {
            eprintln!("{message}\n{USAGE}");
            ExitCode::from(2)
        }
    }
}
