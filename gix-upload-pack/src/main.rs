use std::io;
use std::path::PathBuf;

use gix_upload_pack::{Server, ServerOptions};

fn main() -> Result<(), Box<dyn std::error::Error>> {
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
