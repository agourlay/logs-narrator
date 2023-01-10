# logs-narrator

Merge log files according to the timestamps to display the logs in a chronological order.

Alternative to [lnav](https://github.com/tstack/lnav) where an additional host id can be extracted via a regex for logs correlation.

![example](example.png)

## usage

```commandline
Merge logs to tell a story

Usage: logs-narrator [OPTIONS] --path <PATH>

Options:
  -p, --path <PATH>
          Path to the log files
      --id-detection-regex <ID_DETECTION_REGEX>
          Regex to extract an identifier to color logs [default: "(?:newRaft, raft_id: )(\\d+)"]
      --always-color
          Whether to always color the output
  -h, --help
          Print help information
  -V, --version
          Print version information
```