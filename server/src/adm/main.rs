mod command;

use std::path::PathBuf;

use anyhow::Result;
use clap::{Parser, Subcommand};
use enum_as_inner::EnumAsInner;

use attic_server::config;
use command::make_token::{self, MakeToken};

/// Attic server administration utilities.
#[derive(Debug, Parser)]
#[clap(version, author = "Zhaofeng Li <hello@zhaofeng.li>")]
#[clap(propagate_version = true)]
pub struct Opts {
    /// Path to the config file.
    ///
    /// Not required if --secret-base64 is provided for make-token.
    #[clap(short = 'f', long, global = true)]
    config: Option<PathBuf>,

    /// The sub-command.
    #[clap(subcommand)]
    pub command: Command,
}

#[derive(Debug, Subcommand, EnumAsInner)]
pub enum Command {
    MakeToken(MakeToken),
}

#[tokio::main]
async fn main() -> Result<()> {
    let opts = Opts::parse();

    // For make-token with --secret-base64, config is optional
    let needs_config = match &opts.command {
        Command::MakeToken(sub) => sub.secret_base64.is_none() && !sub.dump_claims,
    };

    let config = if needs_config || opts.config.is_some() {
        Some(config::load_config(opts.config.as_deref(), false).await?)
    } else {
        None
    };

    match opts.command {
        Command::MakeToken(_) => make_token::run(config, opts).await?,
    }

    Ok(())
}
