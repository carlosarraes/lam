mod client;
mod commands;
mod config;
mod notify;
mod watch;

use anyhow::Result;
use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(
    name = "lam",
    version,
    about = "Look At Me — queue a blocker for Carlos and wait for his answer"
)]
struct Cli {
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// Write ~/.config/lam/config.toml
    Init {
        #[arg(long)]
        server: String,
        #[arg(long)]
        token: String,
        #[arg(long)]
        topic: String,
    },
    /// Queue an item; prints its id
    Push {
        title: String,
        #[arg(short, long, default_value = "")]
        body: String,
        #[arg(short, long, default_value = "normal", value_parser = ["low", "normal", "critical"])]
        priority: String,
        /// Up to 3 choices shown as buttons
        #[arg(short, long = "choice")]
        choices: Vec<String>,
        /// Block until answered (same as `lam wait`)
        #[arg(short, long)]
        wait: bool,
    },
    /// Block until the item is resolved/dismissed; prints the item as JSON.
    /// Exit 0 resolved, 2 dismissed, 3 timeout
    Wait {
        id: String,
        /// e.g. 30m, 2h, 90s
        #[arg(long, default_value = "2h")]
        timeout: String,
    },
    /// List items (open by default)
    List {
        #[arg(short, long)]
        all: bool,
        #[arg(long)]
        json: bool,
    },
    /// Show one item as JSON
    Show { id: String },
    /// Resolve an item from this machine
    Done {
        id: String,
        choice: Option<String>,
        #[arg(short, long)]
        message: Option<String>,
    },
    /// Dismiss an item without answering
    Dismiss { id: String },
    /// Subscribe to ntfy and mirror pushes as desktop notifications
    Watch,
}

fn main() {
    let cli = Cli::parse();
    let code = match run(cli.cmd) {
        Ok(code) => code,
        Err(e) => {
            eprintln!("lam: {e:#}");
            1
        }
    };
    std::process::exit(code);
}

fn run(cmd: Cmd) -> Result<i32> {
    match cmd {
        Cmd::Init {
            server,
            token,
            topic,
        } => commands::init(server, token, topic),
        Cmd::Push {
            title,
            body,
            priority,
            choices,
            wait,
        } => commands::push(title, body, priority, choices, wait),
        Cmd::Wait { id, timeout } => commands::wait(&id, &timeout),
        Cmd::List { all, json } => commands::list(all, json),
        Cmd::Show { id } => commands::show(&id),
        Cmd::Done {
            id,
            choice,
            message,
        } => commands::done(&id, choice, message),
        Cmd::Dismiss { id } => commands::dismiss(&id),
        Cmd::Watch => watch::run(),
    }
}
