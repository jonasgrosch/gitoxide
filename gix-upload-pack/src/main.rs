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
    let args: Vec<String> = std::env::args().collect();
    
    if args.len() < 2 {
        eprintln!("Usage: {} [--stateless-rpc] <repository-path>", args[0]);
        std::process::exit(1);
    }
    
    let mut stateless_rpc = false;
    let mut repo_path_arg = 1;
    
    // Parse arguments
    if args.len() > 2 && args[1] == "--stateless-rpc" {
        stateless_rpc = true;
        repo_path_arg = 2;
    }
    
    if args.len() <= repo_path_arg {
        eprintln!("Usage: {} [--stateless-rpc] <repository-path>", args[0]);
        std::process::exit(1);
    }
    
    let repo_path = PathBuf::from(&args[repo_path_arg]);

    // Initialize server with repository
    let options = ServerOptions::default();
    let mut server = Server::new(repo_path, options)?
        .stateless_rpc(stateless_rpc);

    // Handle the protocol on stdin/stdout using async-compatible wrappers
    // For now, we'll use a simple buffer approach since we need AsyncRead/AsyncWrite
    let mut stdin_buf = Vec::new();
    io::stdin().read_to_end(&mut stdin_buf)?;
    let stdin_slice = stdin_buf.as_slice();
    
    let mut stdout_buf = Vec::new();
    
    // Process the upload-pack protocol
    match server.serve(stdin_slice, &mut stdout_buf).await {
        Ok(()) => {
            // Write the output to stdout
            io::stdout().write_all(&stdout_buf)?;
            Ok(())
        }
        Err(e) => {
            eprintln!("Debug: server.serve() failed with error: {:?}", e);
            Err(Box::new(e))
        }
    }
}

#[cfg(not(feature = "async"))]
fn sync_main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<String> = std::env::args().collect();
    
    if args.len() < 2 {
        eprintln!("Usage: {} [--stateless-rpc] <repository-path>", args[0]);
        std::process::exit(1);
    }
    
    let mut stateless_rpc = false;
    let mut repo_path_arg = 1;
    
    // Parse arguments
    if args.len() > 2 && args[1] == "--stateless-rpc" {
        stateless_rpc = true;
        repo_path_arg = 2;
    }
    
    if args.len() <= repo_path_arg {
        eprintln!("Usage: {} [--stateless-rpc] <repository-path>", args[0]);
        std::process::exit(1);
    }
    
    let repo_path = PathBuf::from(&args[repo_path_arg]);

    // Initialize server with repository
    let options = ServerOptions::default();
    let mut server = Server::new(repo_path, options)?
        .stateless_rpc(stateless_rpc);

    // Handle the protocol on stdin/stdout  
    let stdin = io::stdin();
    let stdout = io::stdout();
    let mut stdin_lock = stdin.lock();
    let mut stdout_lock = stdout.lock();
    
    // Process the upload-pack protocol
    match server.serve(&mut stdin_lock, &mut stdout_lock) {
        Ok(()) => {
            Ok(())
        }
        Err(e) => {
            eprintln!("Debug: server.serve() failed with error: {:?}", e);
            Err(Box::new(e))
        }
    }
}
