use std::io::{self, BufRead, Write};
use std::path::PathBuf;

use gix_upload_pack::{Server, ServerOptions, Error};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<String> = std::env::args().collect();
    
    if args.len() != 2 {
        eprintln!("Usage: {} <repository-path>", args[0]);
        std::process::exit(1);
    }
    
    let repo_path = PathBuf::from(&args[1]);
    
    // Initialize server with repository
    let options = ServerOptions::default();
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
