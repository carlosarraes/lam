mod articles;
mod client;
mod commands;
mod config;
mod name;
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
    /// Publish and read private HTML articles
    #[command(subcommand)]
    Article(ArticleCmd),
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
        /// Request an answer or publish an informational update
        #[arg(long, value_enum, default_value = "request")]
        kind: client::ItemKind,
        /// Who is asking; inferred from tmux/zellij/screen as session:window, or $LAM_NAME
        #[arg(short = 'n', long)]
        name: Option<String>,
        #[arg(short, long, default_value = "")]
        body: String,
        #[arg(short, long, default_value = "normal", value_parser = ["low", "normal", "warning", "critical"])]
        priority: String,
        /// Up to 3 choices shown as buttons
        #[arg(short, long = "choice")]
        choices: Vec<String>,
        /// Recommended action and rationale for a decision request
        #[arg(long)]
        recommendation: Option<String>,
        /// The exact recommended --choice value
        #[arg(long)]
        recommended_choice: Option<String>,
        /// Sub-item the human ticks off; the item resolves when all are done (exclusive with --choice)
        #[arg(long = "check")]
        checks: Vec<String>,
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
    /// Block until an item changes (a check ticked) or closes; prints it as JSON.
    /// Exit 0 resolved/changed, 2 dismissed, 3 timeout, 4 expired, 5 retracted
    Wait {
        /// One id, or several to return whichever closes first
        #[arg(required_unless_present = "any")]
        ids: Vec<String>,
        /// Wait on every open item pushed under this agent's name
        #[arg(long, conflicts_with = "ids")]
        any: bool,
        /// Name to scope --any to; defaults to this agent's inferred name
        #[arg(short = 'n', long)]
        name: Option<String>,
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
    /// Manage checks on an open checklist item
    #[command(subcommand)]
    Check(CheckCmd),
    /// Subscribe to ntfy and mirror pushes as desktop notifications
    Watch,
    /// Pair an Android device using a five-minute QR code
    Pair,
    /// List paired Android devices
    Devices,
    /// Manage a paired Android device
    #[command(subcommand)]
    Device(DeviceCmd),
    /// Interactive queue: answer items from the terminal (default when no command is given)
    Tui {
        /// No bell or desktop notification when new items arrive
        #[arg(long)]
        silent: bool,
    },
}

#[derive(Subcommand)]
enum ArticleCmd {
    /// Publish an HTML file and explicitly listed local assets
    Publish {
        #[arg(long)]
        file: std::path::PathBuf,
        #[arg(long)]
        title: String,
        #[arg(long)]
        summary: String,
        #[arg(long = "asset")]
        assets: Vec<String>,
        #[arg(long)]
        silent: bool,
    },
    /// List published articles
    List {
        #[arg(long, value_enum, default_value = "all")]
        read: articles::ReadFilter,
        #[arg(long)]
        query: Option<String>,
        /// Creation day in America/Sao_Paulo (YYYY-MM-DD); omit for all dates
        #[arg(long, value_parser = articles::parse_day)]
        day: Option<chrono::NaiveDate>,
    },
    /// Mark an article read
    Read { id: String },
    /// Mark an article unread
    Unread { id: String },
    /// Open an article in the isolated browser viewer
    Open { id: String },
}

#[derive(Subcommand)]
enum CheckCmd {
    /// Append a check to an open item (agent side); re-notifies
    Add { id: String, label: String },
    /// Tick a check (1-based index) — Carlos's side
    Tick { id: String, n: usize },
    /// Untick a check (1-based index)
    Untick { id: String, n: usize },
}

#[derive(Subcommand)]
enum DeviceCmd {
    /// Rename a paired device
    Rename { id: String, name: String },
    /// Revoke a paired device
    Revoke { id: String },
}

fn main() {
    let cli = Cli::parse();
    if cli.llm {
        print!("{}", llm_guide());
        std::process::exit(0);
    }
    let code = match run(cli.cmd.unwrap_or(Cmd::Tui { silent: false })) {
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
        Cmd::Article(ArticleCmd::Publish {
            file,
            title,
            summary,
            assets,
            silent,
        }) => articles::publish(articles::PublishArgs {
            file,
            title,
            summary,
            assets,
            silent,
        }),
        Cmd::Article(ArticleCmd::List { read, query, day }) => {
            articles::list(read, query.as_deref(), day)
        }
        Cmd::Article(ArticleCmd::Read { id }) => articles::set_read(&id, true),
        Cmd::Article(ArticleCmd::Unread { id }) => articles::set_read(&id, false),
        Cmd::Article(ArticleCmd::Open { id }) => articles::open(&id),
        Cmd::Init {
            server,
            token,
            topic,
        } => commands::init(server, token, topic),
        Cmd::Push {
            title,
            kind,
            name,
            body,
            priority,
            choices,
            recommendation,
            recommended_choice,
            checks,
            link,
            ttl,
            wait,
        } => commands::push(commands::PushArgs {
            title,
            kind,
            name,
            body,
            priority,
            choices,
            recommendation,
            recommended_choice,
            checks,
            link,
            ttl,
            wait,
        }),
        Cmd::Wait {
            ids,
            any,
            name,
            timeout,
        } => commands::wait(&ids, any, name, &timeout),
        Cmd::List { all, json } => commands::list(all, json),
        Cmd::Show { id } => commands::show(&id),
        Cmd::Done {
            id,
            choice,
            message,
        } => commands::done(&id, choice, message),
        Cmd::Dismiss { id } => commands::dismiss(&id),
        Cmd::Retract { id } => commands::retract(&id),
        Cmd::Check(CheckCmd::Add { id, label }) => commands::check_add(&id, &label),
        Cmd::Check(CheckCmd::Tick { id, n }) => commands::check_set(&id, n, true),
        Cmd::Check(CheckCmd::Untick { id, n }) => commands::check_set(&id, n, false),
        Cmd::Watch => watch::run(),
        Cmd::Pair => commands::pair(),
        Cmd::Devices => commands::devices(),
        Cmd::Device(DeviceCmd::Rename { id, name }) => commands::device_rename(&id, &name),
        Cmd::Device(DeviceCmd::Revoke { id }) => commands::device_revoke(&id),
        Cmd::Tui { silent } => tui::run(silent),
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
mod article_events;
