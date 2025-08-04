use std::io;
use std::path::PathBuf;

use gix_upload_pack::{Server, ServerOptions};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    println!("=== Gitoxide Upload-Pack Server ===");
    println!("A comprehensive Git upload-pack implementation using gitoxide components");

    let args: Vec<String> = std::env::args().collect();

    if args.len() != 2 {
        eprintln!("Usage: {} <repository-path>", args[0]);
        eprintln!("");
        eprintln!("This server implements the Git upload-pack protocol with full compatibility");
        eprintln!("to upstream Git, supporting both protocol v1 and v2.");
        eprintln!("");
        eprintln!("Features:");
        eprintln!("  • Complete Git wire protocol v1 and v2 support");
        eprintln!("  • Efficient pack generation using gix-pack");
        eprintln!("  • Advanced capabilities (multi-ack, shallow, filters)");
        eprintln!("  • Object negotiation and deduplication");
        eprintln!("  • Side-band communication with progress reporting");
        std::process::exit(1);
    }

    let repo_path = PathBuf::from(&args[1]);

    if !repo_path.exists() {
        eprintln!("Error: Repository path '{}' does not exist", repo_path.display());
        std::process::exit(1);
    }

    println!(
        "Initializing upload-pack server for repository: {}",
        repo_path.display()
    );

    // Initialize server with repository
    let options = ServerOptions::default();
    let mut server = match Server::new(repo_path, options) {
        Ok(server) => {
            println!("✓ Server initialized successfully");
            server
        }
        Err(e) => {
            eprintln!("✗ Failed to initialize server: {}", e);
            std::process::exit(1);
        }
    };

    println!("Ready to serve Git protocol requests on stdin/stdout");
    println!("Listening for Git client connections...");

    // Handle the protocol on stdin/stdout
    let stdin = io::stdin();
    let stdout = io::stdout();
    let mut stdin_lock = stdin.lock();
    let mut stdout_lock = stdout.lock();

    // Process the upload-pack protocol
    match server.serve(&mut stdin_lock, &mut stdout_lock) {
        Ok(()) => {
            println!("✓ Protocol session completed successfully");
        }
        Err(e) => {
            eprintln!("✗ Protocol error: {}", e);
            std::process::exit(1);
        }
    }

    Ok(())
}
