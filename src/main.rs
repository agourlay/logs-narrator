mod process;

use crate::process::{load_files_in_memory, process_log_files};
use clap::Parser;
use regex::Regex;
use std::io;

// default value is [Qdrant](https://github.com/qdrant/qdrant) specific to please the devs :)
const DEFAULT_REGEX: &str = r"(?:newRaft, raft_id: )(\d+)";

#[derive(Parser, Debug)]
#[command(author, version, about, long_about = None)]
struct Args {
    /// Path to the log files
    #[arg(short, long)]
    path: String,

    /// Regex to extract an identifier to color logs
    #[arg(long, default_value_t = DEFAULT_REGEX.to_string())]
    id_detection_regex: String,

    /// Whether to always color the output
    #[arg(long, default_value_t = false)]
    always_color: bool,
}

fn main() -> io::Result<()> {
    let args = Args::parse();

    if args.always_color {
        colored::control::set_override(true);
    } else {
        colored::control::set_override(false);
    }

    let id_detection_regex = Regex::new(&args.id_detection_regex).unwrap();

    let log_files = load_files_in_memory(&args.path, Some(id_detection_regex))?;
    process_log_files(log_files);
    Ok(())
}
