mod process;

use crate::process::{load_files_in_memory, process_log_files};
use clap::Parser;
use regex::Regex;
use std::error::Error;
use std::process::ExitCode;

// default value is [Qdrant](https://github.com/qdrant/qdrant) specific to please the devs :)
const DEFAULT_REGEX: &str = r"(?:newRaft, raft_id: )(\d+)";

const DEFAULT_DATE_FORMAT: &str = "%Y-%m-%dT%H:%M:%S%.6f%Z";

#[derive(Parser, Debug)]
#[command(author, version, about, long_about = None)]
struct Args {
    /// Path to the directory holding the '.log' files
    #[arg(short, long)]
    path: String,

    /// Regex to extract an identifier to color logs, the identifier being the first
    /// capture group. Pass an empty string to disable identifier detection.
    #[arg(long, default_value_t = DEFAULT_REGEX.to_string())]
    id_detection_regex: String,

    /// Disable colored output
    #[arg(long, default_value_t = false)]
    no_color: bool,

    /// Date format of the timestamp starting each log line. Formats without a UTC
    /// offset are assumed to be UTC.
    #[arg(long, default_value_t = DEFAULT_DATE_FORMAT.to_string())]
    date_format: String,
}

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("error: {error}");
            ExitCode::FAILURE
        }
    }
}

fn run() -> Result<(), Box<dyn Error>> {
    let args = Args::parse();

    if args.no_color {
        colored::control::set_override(false);
    }
    // otherwise leave the auto-detection alone so that colors are dropped when piped

    let id_detection_regex = if args.id_detection_regex.is_empty() {
        None
    } else {
        Some(
            Regex::new(&args.id_detection_regex)
                .map_err(|e| format!("invalid --id-detection-regex: {e}"))?,
        )
    };

    let log_files =
        load_files_in_memory(&args.path, id_detection_regex.as_ref(), &args.date_format)?;
    process_log_files(log_files)?;
    Ok(())
}
