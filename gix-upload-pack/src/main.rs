use clap::Parser;
use std::io;
use std::path::PathBuf;
use std::time::Duration;

use gix_upload_pack::config::ServerOptions;
use gix_upload_pack::server::Server;

/// Git upload-pack server implementation for gitoxide
///
/// This is a drop-in replacement for Git's native upload-pack command,
/// providing the same functionality with improved performance and reliability.
///
/// The upload-pack protocol is used by Git clients to fetch objects from
/// a repository. This implementation supports all Git protocol versions
/// (v0, v1, v2) and maintains byte-for-byte compatibility with native Git.
#[derive(Parser, Debug)]
#[command(
    name = "gix-upload-pack",
    version = "0.1.0",
    about = "Git upload-pack server implementation for gitoxide",
    long_about = "A high-performance, drop-in replacement for Git's native upload-pack command.\n\
                  Supports all Git protocol versions and maintains full compatibility with Git clients.\n\
                  \n\
                  This server handles Git fetch, clone, and ls-remote operations by:\n\
                  - Advertising available references\n\
                  - Negotiating object requirements with clients\n\
                  - Generating and streaming pack files\n\
                  - Supporting shallow clones and object filtering",
    override_usage = "gix-upload-pack [OPTIONS] <DIRECTORY>",
    help_template = "{before-help}{name} {version}\n{about}\n\n{usage-heading} {usage}\n\n{all-args}{after-help}",
    after_help = "EXAMPLES:\n    \
                  gix-upload-pack /path/to/repo.git\n    \
                  gix-upload-pack --stateless-rpc --advertise-refs /path/to/repo\n    \
                  gix-upload-pack --timeout=300 --strict /path/to/repo.git\n\n\
                  For more information about the Git upload-pack protocol, see:\n    \
                  https://git-scm.com/docs/git-upload-pack"
)]
struct Args {
    /// The repository directory to serve
    ///
    /// This should be the path to a Git repository (either a bare repository
    /// ending in .git or a working directory containing a .git subdirectory).
    /// The path will be validated to ensure it contains a valid Git repository.
    #[arg(
        value_name = "DIRECTORY",
        help = "The repository directory to serve",
        long_help = "Path to the Git repository to serve. Can be either:\n\
                     - A bare repository (typically ending in .git)\n\
                     - A working directory containing a .git subdirectory\n\
                     \n\
                     The repository must be accessible and contain valid Git objects."
    )]
    directory: PathBuf,

    /// Quit after a single request/response exchange
    ///
    /// This mode is used by Git's HTTP transport and other stateless protocols.
    /// The server will process one request and then exit, rather than maintaining
    /// a persistent connection for multiple operations.
    #[arg(
        long = "stateless-rpc",
        help = "Quit after a single request/response exchange",
        long_help = "Enable stateless RPC mode for HTTP and other stateless transports.\n\
                     \n\
                     In this mode, the server:\n\
                     - Processes exactly one request\n\
                     - Sends the complete response\n\
                     - Exits immediately\n\
                     \n\
                     This is required for Git's HTTP smart transport protocol."
    )]
    stateless_rpc: bool,

    /// Serve up the info/refs for git-http-backend
    ///
    /// This mode is used by Git's HTTP transport to advertise available references.
    /// The server will output the reference advertisement and exit, without
    /// processing any fetch requests.
    #[arg(
        long = "advertise-refs",
        help = "Serve up the info/refs for git-http-backend",
        long_help = "Output reference advertisement for HTTP transport.\n\
                     \n\
                     In this mode, the server:\n\
                     - Lists all available references\n\
                     - Includes supported capabilities\n\
                     - Exits after sending the advertisement\n\
                     \n\
                     This is used by Git's HTTP smart transport for the initial\n\
                     GET /info/refs?service=git-upload-pack request."
    )]
    advertise_refs: bool,

    /// Do not try <directory>/.git/ if <directory> is no Git directory
    ///
    /// By default, if the specified directory is not a Git repository,
    /// the server will try appending /.git to find a repository.
    /// This flag disables that behavior.
    #[arg(
        long = "strict",
        help = "Do not try <directory>/.git/ if <directory> is no Git directory",
        long_help = "Disable automatic .git subdirectory detection.\n\
                     \n\
                     Normally, if the specified directory is not a Git repository,\n\
                     the server will try looking for a .git subdirectory.\n\
                     \n\
                     With --strict:\n\
                     - Only the exact specified path is checked\n\
                     - No automatic .git subdirectory lookup\n\
                     - Fails immediately if path is not a Git repository\n\
                     \n\
                     This is useful for security-sensitive environments where\n\
                     path traversal should be strictly controlled."
    )]
    strict: bool,

    /// Try <directory>/.git/ if <directory> is no Git directory
    ///
    /// This explicitly enables the default behavior of looking for a .git
    /// subdirectory. This flag overrides --strict if both are specified.
    #[arg(
        long = "no-strict",
        help = "Try <directory>/.git/ if <directory> is no Git directory",
        long_help = "Enable automatic .git subdirectory detection (default behavior).\n\
                     \n\
                     This flag explicitly enables the default behavior and\n\
                     overrides --strict if both are specified.\n\
                     \n\
                     With --no-strict (or by default):\n\
                     - If the specified path is not a Git repository\n\
                     - The server will try appending /.git\n\
                     - This allows serving working directories\n\
                     \n\
                     This is the standard Git behavior for upload-pack."
    )]
    no_strict: bool,

    /// Interrupt transfer after <n> seconds of inactivity
    ///
    /// Sets a timeout for client connections. If no data is received from
    /// the client for the specified number of seconds, the connection will
    /// be terminated. A value of 0 disables the timeout.
    #[arg(
        long = "timeout",
        value_name = "SECONDS",
        help = "Interrupt transfer after <n> seconds of inactivity",
        long_help = "Set connection timeout in seconds.\n\
                     \n\
                     If no data is received from the client for this many seconds,\n\
                     the connection will be terminated with an error.\n\
                     \n\
                     Values:\n\
                     - 0: No timeout (wait indefinitely)\n\
                     - >0: Timeout after specified seconds\n\
                     \n\
                     This helps prevent hung connections from consuming server\n\
                     resources indefinitely. Typical values are 300-3600 seconds."
    )]
    timeout: Option<u32>,

    /// Hidden flag for HTTP backend compatibility (same as --advertise-refs)
    #[arg(long = "http-backend-info-refs", hide = true)]
    http_backend_info_refs: bool,
}

impl Args {
    /// Validate argument combinations and return appropriate errors
    fn validate(&self) -> Result<(), String> {
        // Validate timeout value
        if let Some(timeout) = self.timeout {
            if timeout > 86400 {
                // 24 hours
                return Err("Timeout cannot exceed 86400 seconds (24 hours)".to_string());
            }
        }

        // Validate directory path
        if !self.directory.exists() {
            return Err(format!("Directory does not exist: {}", self.directory.display()));
        }

        Ok(())
    }

    /// Convert parsed arguments to ServerOptions
    fn to_server_options(&self) -> ServerOptions {
        let timeout = self.timeout.filter(|&t| t > 0).map(|t| Duration::from_secs(t as u64));

        let advertise_refs = self.advertise_refs || self.http_backend_info_refs;
        let strict = self.strict && !self.no_strict;

        ServerOptions {
            advertise_refs,
            stateless_rpc: self.stateless_rpc,
            strict,
            timeout,
            ..Default::default()
        }
    }
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Parse command line arguments using clap derive
    let args = Args::parse();

    // Validate argument combinations
    if let Err(msg) = args.validate() {
        eprintln!("Error: {msg}");
        std::process::exit(1);
    }

    // Convert to server options
    let options = args.to_server_options();

    // Initialize server with validated directory path
    let mut server = match Server::new(args.directory.clone(), options) {
        Ok(server) => server,
        Err(e) => {
            eprintln!("Error initializing server: {e}");
            std::process::exit(1);
        }
    };

    // Handle the protocol on stdin/stdout
    let stdin = io::stdin();
    let stdout = io::stdout();
    let mut stdin_lock = stdin.lock();
    let mut stdout_lock = stdout.lock();

    // Process the upload-pack protocol
    if let Err(e) = server.serve(&mut stdin_lock, &mut stdout_lock) {
        eprintln!("Error serving upload-pack protocol: {e}");
        std::process::exit(1);
    }

    Ok(())
}
