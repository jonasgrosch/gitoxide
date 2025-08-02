//! Simple compatibility tests comparing gix-upload-pack with native git upload-pack
//!
//! This test suite sends client packet fixtures to both implementations and compares
//! the outputs byte-for-byte to ensure 100% compatibility.

use std::fs;
use std::io::Write;
use std::path::Path;
use std::process::{Command, Stdio};

/// Test repository path (should be set up with some test data)
const TEST_REPO_PATH: &str = "../tokio-test";

/// Test fixture representing a client packet file
struct ClientPacketFixture {
    name: String,
    path: String,
    data: Vec<u8>,
}

impl ClientPacketFixture {
    /// Load a fixture from file
    fn load(name: &str, path: &str) -> Result<Self, Box<dyn std::error::Error>> {
        let data = fs::read(path)?;
        Ok(Self {
            name: name.to_string(),
            path: path.to_string(),
            data,
        })
    }
}

/// Run native git upload-pack with the given input
fn run_native_git_upload_pack(
    input: &[u8],
    repo_path: &str,
    protocol_version: Option<&str>,
) -> Result<Vec<u8>, Box<dyn std::error::Error>> {
    let mut cmd = Command::new("git");
    cmd.arg("upload-pack")
        .arg("--stateless-rpc")
        .arg(repo_path)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());

    // Set protocol version environment variable if specified
    if let Some(version) = protocol_version {
        cmd.env("GIT_PROTOCOL", format!("version={}", version));
    }

    let mut child = cmd.spawn()?;

    // Write input to stdin
    if let Some(stdin) = child.stdin.as_mut() {
        stdin.write_all(input)?;
    }

    // Wait for completion and collect output
    let output = child.wait_with_output()?;

    if !output.status.success() {
        return Err(format!(
            "Native git upload-pack failed with exit code {:?}. Stderr: {}",
            output.status.code(),
            String::from_utf8_lossy(&output.stderr)
        )
        .into());
    }

    Ok(output.stdout)
}

/// Run our gix-upload-pack implementation with the given input
fn run_gix_upload_pack(input: &[u8], repo_path: &str) -> Result<Vec<u8>, Box<dyn std::error::Error>> {
    use gix_upload_pack::config::ServerOptions;
    use gix_upload_pack::server::Server;
    use std::io::Cursor;

    // Create server with repository path
    let options = ServerOptions::default();
    let mut server = Server::new(repo_path, options)?;

    // Create input/output streams
    let input_stream = Cursor::new(input);
    let mut output = Vec::new();

    // Run the server
    server.serve(input_stream, &mut output)?;

    Ok(output)
}

/// Compare two byte arrays and return detailed diff information
fn compare_outputs(native: &[u8], gix: &[u8], fixture_name: &str) -> Result<(), String> {
    if native == gix {
        println!("✓ {}: Outputs match perfectly ({} bytes)", fixture_name, native.len());
        return Ok(());
    }

    // Outputs differ - provide detailed information
    let mut error_msg = format!(
        "✗ {}: Outputs differ!\n  Native: {} bytes\n  Gix: {} bytes\n",
        fixture_name,
        native.len(),
        gix.len()
    );

    // Show first few bytes where they differ
    let min_len = native.len().min(gix.len());
    let mut first_diff = None;

    for i in 0..min_len {
        if native[i] != gix[i] {
            first_diff = Some(i);
            break;
        }
    }

    if let Some(diff_pos) = first_diff {
        error_msg.push_str(&format!(
            "  First difference at byte {}: native=0x{:02x} gix=0x{:02x}\n",
            diff_pos, native[diff_pos], gix[diff_pos]
        ));

        // Show context around the difference
        let start = diff_pos.saturating_sub(10);
        let end = (diff_pos + 10).min(min_len);

        error_msg.push_str("  Context (native): ");
        for i in start..end {
            if i == diff_pos {
                error_msg.push_str(&format!("[{:02x}]", native[i]));
            } else {
                error_msg.push_str(&format!("{:02x}", native[i]));
            }
            error_msg.push(' ');
        }
        error_msg.push('\n');

        error_msg.push_str("  Context (gix):    ");
        for i in start..end {
            if i == diff_pos {
                error_msg.push_str(&format!("[{:02x}]", gix[i]));
            } else {
                error_msg.push_str(&format!("{:02x}", gix[i]));
            }
            error_msg.push(' ');
        }
        error_msg.push('\n');
    } else if native.len() != gix.len() {
        error_msg.push_str("  Outputs have different lengths but matching prefixes\n");
    }

    Err(error_msg)
}

/// Test a single fixture against both implementations
fn test_fixture(fixture: &ClientPacketFixture) -> Result<(), Box<dyn std::error::Error>> {
    println!("Testing fixture: {} ({} bytes)", fixture.name, fixture.data.len());

    // Check if test repository exists
    if !Path::new(TEST_REPO_PATH).exists() {
        return Err(format!("Test repository not found at: {}", TEST_REPO_PATH).into());
    }

    // Determine protocol version from fixture name
    let protocol_version = if fixture.name.starts_with("v2") {
        Some("2")
    } else if fixture.name.starts_with("v1") {
        Some("1")
    } else if fixture.name.starts_with("v0") {
        None // v0 uses no GIT_PROTOCOL environment variable
    } else {
        None // Default protocol (v0)
    };

    println!("  Using protocol version: {:?}", protocol_version);

    // Run native git upload-pack
    let native_output = run_native_git_upload_pack(&fixture.data, TEST_REPO_PATH, protocol_version)?;

    // Run our gix-upload-pack implementation
    let gix_output = run_gix_upload_pack(&fixture.data, TEST_REPO_PATH)?;

    // Compare outputs
    compare_outputs(&native_output, &gix_output, &fixture.name).map_err(|e| e.into())
}

/// Load all test fixtures
fn load_fixtures() -> Result<Vec<ClientPacketFixture>, Box<dyn std::error::Error>> {
    let mut fixtures = Vec::new();

    // Load v0 packet fixture (same client packets as v1, but no GIT_PROTOCOL env var)
    if Path::new("tests/fixtures/v1.pkt").exists() {
        fixtures.push(ClientPacketFixture::load("v0", "tests/fixtures/v1.pkt")?);
    }

    // Load v1 packet fixture
    if Path::new("tests/fixtures/v1.pkt").exists() {
        fixtures.push(ClientPacketFixture::load("v1", "tests/fixtures/v1.pkt")?);
    }

    // Load v2 ls-refs fixture
    if Path::new("tests/fixtures/v2-ls-refs.tcp").exists() {
        fixtures.push(ClientPacketFixture::load(
            "v2-ls-refs",
            "tests/fixtures/v2-ls-refs.tcp",
        )?);
    }

    // Load v2 fetch fixture
    if Path::new("tests/fixtures/v2-fetch.tcp").exists() {
        fixtures.push(ClientPacketFixture::load("v2-fetch", "tests/fixtures/v2-fetch.tcp")?);
    }

    if fixtures.is_empty() {
        return Err("No test fixtures found in tests/fixtures/".into());
    }

    Ok(fixtures)
}

#[test]
fn test_compatibility_with_fixtures() {
    println!("=== gix-upload-pack Compatibility Test ===");

    // Load all fixtures
    let fixtures = match load_fixtures() {
        Ok(f) => f,
        Err(e) => {
            panic!("Failed to load fixtures: {}", e);
        }
    };

    println!("Loaded {} test fixtures", fixtures.len());

    let mut passed = 0;
    let mut failed = 0;

    // Test each fixture
    for fixture in &fixtures {
        match test_fixture(fixture) {
            Ok(()) => {
                passed += 1;
            }
            Err(e) => {
                eprintln!("{}", e);
                failed += 1;
            }
        }
    }

    println!("\n=== Test Results ===");
    println!("Passed: {}", passed);
    println!("Failed: {}", failed);
    println!("Total:  {}", passed + failed);

    if failed > 0 {
        panic!("{} fixture tests failed! See output above for details.", failed);
    }

    println!("All compatibility tests passed! 🎉");
}

#[test]
fn test_repository_setup() {
    // Verify that the test repository exists and is accessible
    if !Path::new(TEST_REPO_PATH).exists() {
        panic!(
            "Test repository not found at: {}\n\
             Please ensure the tokio test repository is available at this path.",
            TEST_REPO_PATH
        );
    }

    // Try to run git on the repository to verify it's valid
    let output = Command::new("git")
        .arg("rev-parse")
        .arg("--git-dir")
        .current_dir(TEST_REPO_PATH)
        .output();

    match output {
        Ok(result) if result.status.success() => {
            println!("✓ Test repository is valid: {}", TEST_REPO_PATH);
        }
        Ok(result) => {
            panic!(
                "Test repository exists but is not a valid git repository: {}\n\
                 Git error: {}",
                TEST_REPO_PATH,
                String::from_utf8_lossy(&result.stderr)
            );
        }
        Err(e) => {
            panic!("Failed to check test repository: {}", e);
        }
    }
}

#[test]
fn test_native_git_available() {
    // Verify that native git upload-pack is available
    let output = Command::new("git").arg("upload-pack").arg("--help").output();

    match output {
        Ok(result) if result.status.success() => {
            println!("✓ Native git upload-pack is available");
        }
        Ok(result) => {
            panic!(
                "git upload-pack command failed: {}\n\
                 Stderr: {}",
                result.status,
                String::from_utf8_lossy(&result.stderr)
            );
        }
        Err(e) => {
            panic!("git command not found: {}", e);
        }
    }
}
