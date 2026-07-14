//! tda CLI (spec §9 / M3): JSON output for agents and scripts. TUI is for humans.
//!
//! `main` is thin: parse args, handle the `tui`/`db` subcommands, otherwise
//! build a [`Request`] and run it. Command execution lives in [`command`];
//! Phase 4 will try a running TUI server first (see [`ipc`]) before the direct
//! path taken here.

mod command;
mod ipc;
mod svc;
#[cfg(feature = "tui")]
mod tui;

use std::io::{self, Read as _, Write as _};
use std::path::PathBuf;

use anyhow::Context as _;
use clap::Parser;

use crate::command::{Cmd, DbCmd, run_command};
use crate::ipc::Request;
use crate::svc::{SystemClock, UlidGen, make_svc};

// ---- Clap struct ------------------------------------------------------------

#[derive(Parser)]
#[command(
    name = "tda",
    about = "Task and dependency manager — JSON output for agents/scripts",
    after_help = "Config: ~/.config/tda/tui.toml. Data: --db path if given, else the nearest ancestor .tda/tda.db (see `tda db init`), else the OS data dir (e.g. ~/.local/share/tda/tda.db on Linux)."
)]
struct Cli {
    /// Database file to use (overrides local/global discovery).
    #[arg(long, global = true)]
    db: Option<PathBuf>,
    #[command(subcommand)]
    cmd: Cmd,
}

// ---- main -------------------------------------------------------------------

#[tokio::main(flavor = "current_thread")]
async fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();

    if let Cmd::Tui = cli.cmd {
        #[cfg(feature = "tui")]
        return tui::run(cli.db).await;
        #[cfg(not(feature = "tui"))]
        anyhow::bail!("this `tda` was built without the `tui` feature");
    }

    if let Cmd::Db { cmd } = &cli.cmd {
        let cwd = std::env::current_dir().context("current dir")?;
        match cmd {
            DbCmd::Init => {
                let path = cwd.join(".tda/tda.db");
                // open_store creates the .tda dir and initializes the schema
                todoapp_config::open_store(Some(path.clone())).await?;
                writeln!(io::stdout(), "{}", path.display())?;
            }
            DbCmd::Path => {
                let path = todoapp_config::resolve_db_path(&cwd, cli.db);
                writeln!(io::stdout(), "{}", path.display())?;
            }
        }
        return Ok(());
    }

    let cwd = std::env::current_dir().context("current dir")?;
    // Only `add --batch` consumes stdin; reading it unconditionally would hang
    // on a tty for every other command.
    let stdin = if matches!(cli.cmd, Cmd::Add { batch: true, .. }) {
        let mut buf = Vec::new();
        io::stdin().read_to_end(&mut buf).context("read stdin")?;
        buf
    } else {
        Vec::new()
    };
    let req = Request {
        cmd: cli.cmd,
        cwd: cwd.clone(),
        stdin,
    };

    // If a `tda tui` is running on this db it holds the exclusive lock, so we
    // can't open the file — send the command to it over the socket instead. No
    // server (or a non-unix build) falls through to opening the db directly.
    #[cfg(unix)]
    {
        let sock = ipc::sock_path_for(&todoapp_config::resolve_db_path(&cwd, cli.db.clone()));
        if let Some(reply) = ipc::send(&sock, &req)? {
            return emit(reply);
        }
    }

    let store = todoapp_config::open_store(cli.db).await?;
    let clock = SystemClock;
    let ids = UlidGen;
    let svc = make_svc(&store, &clock, &ids);
    let reply = run_command(&svc, &req).await;
    emit(reply)
}

/// Print a reply's stdout, then fail (non-zero exit) if it carried an error.
fn emit(reply: ipc::Reply) -> anyhow::Result<()> {
    io::stdout().write_all(&reply.out).context("write stdout")?;
    if let Some(err) = reply.err {
        anyhow::bail!(err);
    }
    Ok(())
}
