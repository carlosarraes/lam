mod client;
mod commands;
mod config;
mod notify;
mod tui;
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
    /// Print the usage guide written for AI agents (same content as the `lam` skill)
    #[arg(long)]
    llm: bool,
    #[command(subcommand)]
    cmd: Option<Cmd>,
}

const SKILL: &str = include_str!("../../skill/lam/SKILL.md");

/// The skill's markdown body without its YAML frontmatter.
fn llm_guide() -> &'static str {
    SKILL
        .strip_prefix("---")
        .and_then(|rest| rest.split_once("\n---\n"))
        .map(|(_, body)| body.trim_start())
        .unwrap_or(SKILL)
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
        /// URL shown as an "Open" button on the phone
        #[arg(short, long)]
        link: Option<String>,
        /// Expire the item after this long (e.g. 2h); expired items leave the queue
        #[arg(long)]
        ttl: Option<String>,
        /// Block until answered (same as `lam wait`)
        #[arg(short, long)]
        wait: bool,
    },
    /// Block until an item closes; prints it as JSON.
    /// Exit 0 resolved, 2 dismissed, 3 timeout, 4 expired, 5 retracted
    Wait {
        /// One id, or several to return whichever closes first
        #[arg(required_unless_present = "any")]
        ids: Vec<String>,
        /// Wait on every open item pushed from this host and project
        #[arg(long, conflicts_with = "ids")]
        any: bool,
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
    /// Withdraw your own item because the world already resolved it
    Retract { id: String },
    /// Subscribe to ntfy and mirror pushes as desktop notifications
    Watch,
    /// Interactive queue: answer items from the terminal (default when no command is given)
    Tui,
}

fn main() {
    let cli = Cli::parse();
    if cli.llm {
        print!("{}", llm_guide());
        std::process::exit(0);
    }
    let code = match run(cli.cmd.unwrap_or(Cmd::Tui)) {
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
            link,
            ttl,
            wait,
        } => commands::push(commands::PushArgs {
            title,
            body,
            priority,
            choices,
            link,
            ttl,
            wait,
        }),
        Cmd::Wait { ids, any, timeout } => commands::wait(&ids, any, &timeout),
        Cmd::List { all, json } => commands::list(all, json),
        Cmd::Show { id } => commands::show(&id),
        Cmd::Done {
            id,
            choice,
            message,
        } => commands::done(&id, choice, message),
        Cmd::Dismiss { id } => commands::dismiss(&id),
        Cmd::Retract { id } => commands::retract(&id),
        Cmd::Watch => watch::run(),
        Cmd::Tui => tui::run(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn llm_guide_strips_frontmatter() {
        let g = llm_guide();
        assert!(g.starts_with("# lam"), "{g}");
        assert!(!g.contains("description:"));
        assert!(g.contains("lam push"));
    }
}
