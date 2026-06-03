//! CLI argument definitions using clap derive macros.

use clap::{Parser, Subcommand};

#[derive(Parser, Debug, Clone)]
#[command(
    name = "ledger",
    version,
    about = "Local HTTP proxy that captures, replays, and inspects API traffic",
    long_about = "ledger — an API request logger & replay engine.\n\
        Spins up a local HTTP proxy, captures every request/response into SQLite,\n\
        and gives you a terminal-native interface to search, replay, and export."
)]
pub struct Cli {
    #[command(subcommand)]
    pub command: Commands,

    #[arg(
        long,
        global = true,
        default_value = "127.0.0.1:8080",
        env = "LEDGER_ADDR"
    )]
    pub addr: String,

    #[arg(
        long,
        global = true,
        default_value = "~/.config/ledger/config.toml",
        env = "LEDGER_CONFIG"
    )]
    pub config: String,

    #[arg(long, global = true, default_value = "info")]
    pub log_level: String,
}

#[derive(Subcommand, Debug, Clone)]
pub enum Commands {
    #[command(about = "Start the HTTP proxy and capture traffic")]
    Capture {
        #[arg(short, long, default_value = "default")]
        session: String,

        #[arg(long, default_value_t = false)]
        verbose: bool,

        #[arg(long, default_value_t = false)]
        intercept: bool,

        #[arg(long)]
        intercept_rule: Option<String>,
    },

    #[command(about = "Replay a previously captured request")]
    Replay {
        #[arg(short, long)]
        id: Option<String>,

        #[arg(long, default_value_t = 1)]
        count: u32,

        #[arg(long, default_value_t = false)]
        dry_run: bool,

        #[arg(long, default_value_t = false)]
        diff: bool,

        #[arg(long, default_value_t = false)]
        edit: bool,

        #[arg(short, long)]
        filter: Option<String>,

        #[arg(long)]
        chain: Option<String>,

        #[arg(long)]
        pre_script: Option<String>,

        #[arg(long)]
        post_script: Option<String>,
    },

    #[command(about = "List captured requests in a session")]
    List {
        #[arg(short, long, default_value = "default")]
        session: String,

        #[arg(short, long, default_value = "50")]
        limit: usize,

        #[arg(long, default_value_t = false)]
        headers: bool,

        #[arg(long, default_value_t = false)]
        bodies: bool,
    },

    #[command(about = "Search captured requests by pattern")]
    Search {
        #[arg(short, long)]
        query: String,

        #[arg(short, long, default_value = "default")]
        session: String,

        #[arg(long, default_value = "path")]
        field: String,
    },

    #[command(about = "Export captured traffic")]
    Export {
        #[arg(short, long)]
        format: ExportFormat,

        #[arg(short, long, default_value = "default")]
        session: String,

        #[arg(short, long)]
        output: Option<String>,
    },

    #[command(about = "Launch the interactive terminal UI")]
    Tui {
        #[arg(short, long, default_value = "default")]
        session: String,
    },

    #[command(about = "Show session statistics and metrics")]
    Stats {
        #[arg(short, long, default_value = "default")]
        session: String,
    },

    #[command(about = "Initialize configuration file")]
    Init,

    #[command(about = "Certificate authority management")]
    Ca {
        #[command(subcommand)]
        command: CaCommands,
    },
}

#[derive(clap::Subcommand, Debug, Clone)]
pub enum CaCommands {
    #[command(about = "Generate a new CA certificate (or show existing)")]
    Generate,

    #[command(about = "Print the CA certificate PEM")]
    Show,
}

#[derive(clap::ValueEnum, Debug, Clone, Copy)]
pub enum ExportFormat {
    Har,
    Curl,
    Raw,
    Postman,
}
