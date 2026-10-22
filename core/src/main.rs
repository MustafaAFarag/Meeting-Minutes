use std::path::PathBuf;
use std::process::ExitCode;
use std::sync::atomic::Ordering;

use anyhow::Result;
use clap::{Parser, Subcommand};
use mn_core::{Config, RecordOptions, Status};

#[derive(Parser)]
#[command(
    version,
    about = "Record a call as two tracks (Me, Them), transcribe it locally, and publish a meeting card"
)]
struct Cli {
    /// Use this config file instead of ~/.config/meeting-notes/config.toml
    #[arg(long, global = true)]
    config: Option<PathBuf>,
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Record until the call goes silent, STOP appears in the session dir, or Ctrl+C
    Record {
        /// Stop after this many seconds
        #[arg(long)]
        max_seconds: Option<u64>,
    },
    /// Transcribe a recorded session dir, write transcript.md, delete the audio
    Transcribe { session_dir: PathBuf },
    /// Write the meeting card (Claude, then Codex) for a session with a transcript
    Summarize { session_dir: PathBuf },
    /// Publish every pending meeting to the "My Meetings" artifact
    Publish,
    /// Transcribe, summarize and publish a session, skipping stages that are already done
    Process { session_dir: PathBuf },
    /// Record, then process
    Run {
        /// Stop recording after this many seconds
        #[arg(long)]
        max_seconds: Option<u64>,
    },
    /// Print the effective config
    Config,
}

/// One JSON line per status, on stdout. The GUI parses these.
fn emit(status: Status) {
    println!(
        "{}",
        serde_json::to_string(&status).expect("status serializes")
    );
}

fn record_options(max_seconds: Option<u64>) -> Result<RecordOptions> {
    let opts = RecordOptions {
        max_seconds,
        ..RecordOptions::default()
    };
    let flag = opts.interrupt.clone();
    ctrlc::set_handler(move || flag.store(true, Ordering::SeqCst))?;
    Ok(opts)
}

fn run(cli: Cli) -> Result<()> {
    let cfg = Config::load(cli.config.as_deref())?;
    match cli.command {
        Command::Record { max_seconds } => {
            mn_core::record(&cfg, &record_options(max_seconds)?, emit)?;
        }
        Command::Transcribe { session_dir } => {
            mn_core::transcribe(&cfg, &session_dir, emit)?;
        }
        Command::Summarize { session_dir } => {
            mn_core::summarize(&cfg, &session_dir, emit)?;
        }
        Command::Publish => mn_core::publish(&cfg, emit)?,
        Command::Process { session_dir } => mn_core::process(&cfg, &session_dir, emit)?,
        Command::Run { max_seconds } => {
            let session = mn_core::record(&cfg, &record_options(max_seconds)?, emit)?;
            mn_core::process(&cfg, &session, emit)?;
        }
        Command::Config => {
            print!("{}", cfg.to_toml()?);
            if cfg.whisper_model.is_none() {
                println!("# whisper_model is not set (required for transcribe)");
            }
            if cfg.claude_config_dir.is_none() {
                println!("# claude_config_dir is not set (claude uses its normal login)");
            }
            if cfg.my_name.is_none() {
                println!("# my_name is not set (the card prompt says \"unknown\")");
            }
            if cfg.artifact_url.is_none() {
                println!("# artifact_url is not set (read from artifact_url.txt next to the config file)");
            }
            if cfg.capture_me.is_none() {
                println!("# capture_me: computed at runtime from `pactl get-default-source`");
            }
            if cfg.capture_them.is_none() {
                println!("# capture_them: computed at runtime from `pactl get-default-sink`");
            }
        }
    }
    Ok(())
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    let machine_readable = !matches!(cli.command, Command::Config);
    match run(cli) {
        Ok(()) => ExitCode::SUCCESS,
        Err(err) => {
            let message = format!("{err:#}");
            eprintln!("error: {message}");
            if machine_readable {
                emit(Status::Error { message });
            }
            ExitCode::FAILURE
        }
    }
}
