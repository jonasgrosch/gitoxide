use clap::{Arg, Command};
use std::io;
use std::path::PathBuf;

use gix_upload_pack::server::Server;
use gix_upload_pack::config::ServerOptions;

const UPLOAD_PACK_USAGE: &str = "git-upload-pack [--[no-]strict] [--timeout=<n>] [--stateless-rpc] [--advertise-refs] <directory>";

fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Parse command line arguments to match native git upload-pack exactly
    let matches = Command::new("gix-upload-pack")
        .version("0.1.0")
        .about("Git upload-pack server implementation for gitoxide")
        .override_usage(UPLOAD_PACK_USAGE)
        .arg(
            Arg::new("stateless-rpc")
                .long("stateless-rpc")
                .help("quit after a single request/response exchange")
                .action(clap::ArgAction::SetTrue),
        )
        .arg(
            Arg::new("advertise-refs")
                .long("advertise-refs")
                .help("serve up the info/refs for git-http-backend")
                .action(clap::ArgAction::SetTrue),
        )
        .arg(
            Arg::new("http-backend-info-refs")
                .long("http-backend-info-refs")
                .help("serve up the info/refs for git-http-backend")
                .action(clap::ArgAction::SetTrue)
                .hide(true), // Hidden like in native git
        )
        .arg(
            Arg::new("strict")
                .long("strict")
                .help("do not try <directory>/.git/ if <directory> is no Git directory")
                .action(clap::ArgAction::SetTrue),
        )
        .arg(
            Arg::new("no-strict")
                .long("no-strict")
                .help("try <directory>/.git/ if <directory> is no Git directory")
                .action(clap::ArgAction::SetTrue),
        )
        .arg(
            Arg::new("timeout")
                .long("timeout")
                .help("interrupt transfer after <n> seconds of inactivity")
                .value_name("n")
                .value_parser(clap::value_parser!(u32)),
        )
        .arg(
            Arg::new("directory")
                .help("The repository directory to serve")
                .required(true)
                .index(1),
        )
        .get_matches();

    // Extract parsed arguments
    let directory = matches.get_one::<String>("directory").unwrap();
    let stateless_rpc = matches.get_flag("stateless-rpc");
    let advertise_refs = matches.get_flag("advertise-refs") || matches.get_flag("http-backend-info-refs");
    let strict = matches.get_flag("strict") && !matches.get_flag("no-strict");
    let timeout = matches.get_one::<u32>("timeout").copied().unwrap_or(0);

    // Create server options matching native git behavior
    let timeout_duration = if timeout > 0 {
        Some(std::time::Duration::from_secs(timeout as u64))
    } else {
        None
    };
    
    let options = ServerOptions {
        advertise_refs,
        stateless_rpc,
        strict,
        timeout: timeout_duration,
        ..Default::default()
    };

    // Initialize server with directory path (matching native git)
    let repo_path = PathBuf::from(directory);
    let mut server = Server::new(repo_path, options)?;

    // Handle the protocol on stdin/stdout  
    let stdin = io::stdin();
    let stdout = io::stdout();
    let mut stdin_lock = stdin.lock();
    let mut stdout_lock = stdout.lock();
    
    // Process the upload-pack protocol
    server.serve(&mut stdin_lock, &mut stdout_lock)?;
    
    Ok(())
}
