use std::io::{self, Read, Write};
use std::path::PathBuf;

use gix_upload_pack::{Server, ServerOptions};

#[cfg(feature = "async")]
fn main() -> Result<(), Box<dyn std::error::Error>> {
    futures_lite::future::block_on(async_main())
}

#[cfg(not(feature = "async"))]
fn main() -> Result<(), Box<dyn std::error::Error>> {
    sync_main()
}

#[cfg(feature = "async")]
async fn async_main() -> Result<(), Box<dyn std::error::Error>> {
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
    
    println!("Initializing upload-pack server for repository: {}", repo_path.display());
    
    // Initialize server with repository
    let options = ServerOptions::default();
    let mut server = match Server::new(repo_path, options) {
        Ok(server) => {
            println!("✓ Server initialized successfully");
            server
        },
        Err(e) => {
            eprintln!("✗ Failed to initialize server: {}", e);
            std::process::exit(1);
        }
    };
    
    println!("Ready to serve Git protocol requests on stdin/stdout");
    println!("Listening for Git client connections...");
    
    // Handle the protocol on stdin/stdout using async-compatible approach
    let mut stdin_buf = Vec::new();
    io::stdin().read_to_end(&mut stdin_buf)?;
    let stdin_slice = stdin_buf.as_slice();
    
    let mut stdout_buf = Vec::new();
    
    // Process the upload-pack protocol
    match server.serve(stdin_slice, &mut stdout_buf).await {
        Ok(()) => {
            // Write the output to stdout
            io::stdout().write_all(&stdout_buf)?;
            println!("✓ Protocol session completed successfully");
        },
        Err(e) => {
            eprintln!("✗ Protocol error: {}", e);
            std::process::exit(1);
        }
    }
    
    Ok(())
}

#[cfg(not(feature = "async"))]
fn sync_main() -> Result<(), Box<dyn std::error::Error>> {
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
    
    println!("Initializing upload-pack server for repository: {}", repo_path.display());
    
    // Initialize server with repository
    let options = ServerOptions::default();
    let mut server = match Server::new(repo_path, options) {
        Ok(server) => {
            println!("✓ Server initialized successfully");
            server
        },
        Err(e) => {
            eprintln!("✗ Failed to initialize server: {}", e);
            std::process::exit(1);
        }
    };
    
    println!("Ready to serve Git protocol requests on stdin/stdout");
    println!("Listening for Git client connections...");
    
    // Handle the protocol on stdin/stdout  
    let stdin = std::io::stdin();
    let stdout = std::io::stdout();
    let mut stdin_lock = stdin.lock();
    let mut stdout_lock = stdout.lock();
    
    // Process the upload-pack protocol
    match server.serve(&mut stdin_lock, &mut stdout_lock) {
        Ok(()) => {
            println!("✓ Protocol session completed successfully");
        },
        Err(e) => {
            eprintln!("✗ Protocol error: {}", e);
            std::process::exit(1);
        }
    }
    
    Ok(())
}
