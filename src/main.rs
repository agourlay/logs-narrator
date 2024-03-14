mod process;

use crate::process::{load_files_in_memory, process_log_files};
use clap::Parser;
use regex::Regex;
use std::io;

// default value is [Qdrant](https://github.com/qdrant/qdrant) specific to please the devs :)
const DEFAULT_REGEX: &str = r"(?:newRaft, raft_id: )(\d+)";

const DEFAULT_DATE_FORMAT: &str = "%Y-%m-%dT%H:%M:%S%.6f%Z";

#[derive(Parser, Debug)]
#[command(author, version, about, long_about = None)]
struct Args {
    /// Path to the log files
    #[arg(short, long)]
    path: String,

    /// Regex to extract an identifier to color logs
    #[arg(long, default_value_t = DEFAULT_REGEX.to_string())]
    id_detection_regex: String,

    /// Whether to color the output
    #[arg(long, default_value_t = false)]
    no_color: bool,

    /// Date format to use
    #[arg(long, default_value_t = DEFAULT_DATE_FORMAT.to_string())]
    date_format: String,
}

fn main() -> io::Result<()> {
    let args = Args::parse();

    if args.no_color {
        colored::control::set_override(false);
    } else {
        colored::control::set_override(true);
    }

    let id_detection_regex = Regex::new(&args.id_detection_regex).unwrap();

    let log_files = load_files_in_memory(&args.path, Some(id_detection_regex), &args.date_format)?;
    process_log_files(log_files);
    Ok(())
}
