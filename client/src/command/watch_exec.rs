use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::{anyhow, Result};
use clap::Parser;
use indicatif::MultiProgress;
use notify::{EventKind, RecursiveMode, Watcher};
use tokio::process::Command;
use tokio::sync::mpsc;

use crate::api::ApiClient;
use crate::cache::CacheRef;
use crate::cli::Opts;
use crate::config::Config;
use crate::push::{PushConfig, PushSessionConfig, Pusher};
use attic::nix_store::{NixStore, StorePath};

/// Run a command, watching the Nix Store and uploading new paths to a binary cache.
///
/// This is essentially `watch-store` but scoped to the lifetime of a subprocess.
/// After the command exits, any remaining paths are uploaded before exiting.
#[derive(Debug, Parser)]
pub struct WatchExec {
    /// The cache to push to.
    ///
    /// This can be either `servername:cachename` or `cachename`
    /// when using the default server.
    cache: CacheRef,

    /// Push the new paths only and do not compute closures.
    #[clap(long, hide = true)]
    no_closure: bool,

    /// Ignore the upstream cache filter.
    #[clap(long)]
    ignore_upstream_cache_filter: bool,

    /// The maximum number of parallel upload processes.
    #[clap(short = 'j', long, default_value = "5")]
    jobs: usize,

    /// Always send the upload info as part of the payload.
    #[clap(long, hide = true)]
    force_preamble: bool,

    /// The command to run.
    #[clap(last = true, required = true)]
    command: Vec<String>,
}

pub async fn run(opts: Opts) -> Result<i32> {
    let sub = opts.command.as_watch_exec().unwrap();
    if sub.jobs == 0 {
        return Err(anyhow!("The number of jobs cannot be 0"));
    }

    if sub.command.is_empty() {
        return Err(anyhow!("No command specified"));
    }

    let config = Config::load()?;

    let store = Arc::new(NixStore::connect()?);
    let store_dir = store.store_dir().to_owned();

    let (server_name, server, cache) = config.resolve_cache(&sub.cache)?;
    let mut api = ApiClient::from_server_config(server.clone())?;

    // Confirm remote cache validity, query cache config
    let cache_config = api.get_cache_config(cache).await?;

    if let Some(api_endpoint) = &cache_config.api_endpoint {
        // Use delegated API endpoint
        api.set_endpoint(api_endpoint)?;
    }

    let push_config = PushConfig {
        num_workers: sub.jobs,
        force_preamble: sub.force_preamble,
    };

    let push_session_config = PushSessionConfig {
        no_closure: sub.no_closure,
        ignore_upstream_cache_filter: sub.ignore_upstream_cache_filter,
    };

    let mp = MultiProgress::new();
    let session = Pusher::new(
        store.clone(),
        api,
        cache.to_owned(),
        cache_config,
        mp,
        push_config,
    )
    .into_push_session(push_session_config);

    let (tx, mut rx) = mpsc::unbounded_channel();

    let mut watcher = notify::recommended_watcher(move |res: notify::Result<notify::Event>| {
        tx.send(res).unwrap();
    })?;

    watcher.watch(&store_dir, RecursiveMode::NonRecursive)?;

    eprintln!(
        "👀 Pushing store paths to \"{cache}\" on \"{server}\"",
        cache = cache.as_str(),
        server = server_name.as_str(),
    );

    // Spawn the child process
    let program = &sub.command[0];
    let args = &sub.command[1..];

    let mut child = Command::new(program)
        .args(args)
        .spawn()
        .map_err(|e| anyhow!("Failed to spawn command '{}': {}", program, e))?;

    // Watch for store paths until the child process exits
    let exit_code = loop {
        tokio::select! {
            // Check if child process has exited
            status = child.wait() => {
                let status = status?;
                break status.code().unwrap_or(1);
            }

            // Process filesystem events
            res = rx.recv() => {
                match res {
                    Some(Ok(event)) => {
                        if let EventKind::Remove(_) = event.kind {
                            let paths = event
                                .paths
                                .iter()
                                .filter_map(|p| {
                                    let base = strip_lock_file(p)?;
                                    store.parse_store_path(base).ok()
                                })
                                .collect::<Vec<StorePath>>();

                            if !paths.is_empty() {
                                session.queue_many(paths).unwrap();
                            }
                        }
                    }
                    Some(Err(e)) => eprintln!("Error during watch: {:?}", e),
                    None => break 1, // Channel closed unexpectedly
                }
            }
        }
    };

    // Stop watching
    drop(watcher);

    // Process any remaining events in the channel
    while let Ok(res) = rx.try_recv() {
        if let Ok(event) = res {
            if let EventKind::Remove(_) = event.kind {
                let paths = event
                    .paths
                    .iter()
                    .filter_map(|p| {
                        let base = strip_lock_file(p)?;
                        store.parse_store_path(base).ok()
                    })
                    .collect::<Vec<StorePath>>();

                if !paths.is_empty() {
                    session.queue_many(paths).unwrap();
                }
            }
        }
    }

    // Wait for all uploads to complete
    let results = session.wait().await?;
    let upload_errors: Vec<_> = results.into_values().filter_map(|r| r.err()).collect();

    if !upload_errors.is_empty() {
        eprintln!("Some uploads failed:");
        for e in &upload_errors {
            eprintln!("  - {}", e);
        }
    }

    Ok(exit_code)
}

fn strip_lock_file(p: &Path) -> Option<PathBuf> {
    p.to_str()
        .and_then(|p| p.strip_suffix(".lock"))
        .filter(|t| !t.ends_with(".drv") && !t.ends_with("-source"))
        .map(PathBuf::from)
}
