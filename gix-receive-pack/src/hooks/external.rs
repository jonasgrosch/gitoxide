//! External hook execution via gix-command.
//!
//! This module provides hook implementations that execute external processes
//! using gix-command. It's only available when the "hooks-external" feature
//! is enabled.

use super::{Hooks, HookDecision, env::HookEnvironment};
use crate::protocol::CommandUpdate;
use crate::Error;
use std::collections::HashMap;
use std::io::Read;
use std::path::PathBuf;
use std::time::{Duration, Instant};

/// Trait for writing hook output to sideband channels.
///
/// This trait allows the engine to provide a sideband writer that can relay
/// hook output in real-time without this module needing to know about pkt-line
/// protocol details.
pub trait SidebandWriter {
    /// Write a chunk of data to the sideband channel.
    ///
    /// The data should be written as-is (binary-safe) to the appropriate
    /// sideband channel (typically channel 2 for progress/errors).
    fn write_chunk(&mut self, data: &[u8]) -> std::io::Result<()>;
    
    /// Flush any buffered data.
    fn flush(&mut self) -> std::io::Result<()>;
}

/// Configuration for external hook execution.
#[derive(Debug, Clone)]
pub struct ExternalHookConfig {
    /// Directory containing hook scripts
    pub hooks_dir: PathBuf,
    /// Timeout for hook execution
    pub timeout: Duration,
    /// Maximum output size (stdout + stderr combined)
    pub max_output_size: usize,
    /// Whether to enable sideband relay for hook output
    pub enable_sideband_relay: bool,
}

impl Default for ExternalHookConfig {
    fn default() -> Self {
        Self {
            hooks_dir: PathBuf::from("hooks"),
            timeout: Duration::from_secs(30),
            max_output_size: 1024 * 1024, // 1MB
            enable_sideband_relay: false,
        }
    }
}

/// Result of executing an external hook.
#[derive(Debug, Clone)]
pub struct HookResult {
    /// Whether the hook succeeded (exit code 0)
    pub success: bool,
    /// Exit code from the hook process
    pub exit_code: Option<i32>,
    /// Standard output from the hook (bounded)
    pub stdout: Vec<u8>,
    /// Standard error from the hook (bounded)
    pub stderr: Vec<u8>,
    /// Duration of hook execution
    pub duration: Duration,
}

/// External hook implementation using gix-command for process execution.
///
/// This implementation executes actual hook scripts as external processes,
/// managing timeouts, output limits, and environment setup.
pub struct ExternalHooks {
    config: ExternalHookConfig,
    environment: HookEnvironment,
    sideband_writer: Option<Box<dyn SidebandWriter>>,
}

impl ExternalHooks {
    /// Create a new external hooks executor with the given configuration.
    pub fn new(config: ExternalHookConfig, environment: HookEnvironment) -> Self {
        Self { 
            config, 
            environment,
            sideband_writer: None,
        }
    }

    /// Create external hooks with default configuration.
    pub fn with_defaults(environment: HookEnvironment) -> Self {
        Self::new(ExternalHookConfig::default(), environment)
    }

    /// Create a new external hooks executor with sideband writer.
    pub fn with_sideband_writer<W: SidebandWriter + 'static>(
        config: ExternalHookConfig, 
        environment: HookEnvironment,
        sideband_writer: W,
    ) -> Self {
        Self { 
            config, 
            environment,
            sideband_writer: Some(Box::new(sideband_writer)),
        }
    }
    /// Relay data to sideband writer if available and enabled.
    fn relay_to_sideband(&mut self, data: &[u8]) {
        if self.config.enable_sideband_relay {
            if let Some(ref mut writer) = self.sideband_writer {
                let _ = writer.write_chunk(data); // Best effort, don't fail on sideband errors
            }
        }
    }

    /// Flush sideband writer if available.
    fn flush_sideband(&mut self) {
        if self.config.enable_sideband_relay {
            if let Some(ref mut writer) = self.sideband_writer {
                let _ = writer.flush(); // Best effort
            }
        }
    }

    /// Execute a hook script with the given name and arguments.
    fn execute_hook(
        &mut self,
        hook_name: &str,
        args: &[String],
        stdin_data: Option<&[u8]>,
    ) -> Result<HookResult, Error> {
        let hook_path = self.config.hooks_dir.join(hook_name);
        
        // Check if hook exists and is executable
        if !hook_path.exists() {
            // Hook doesn't exist - this is not an error, just return success
            return Ok(HookResult {
                success: true,
                exit_code: Some(0),
                stdout: Vec::new(),
                stderr: Vec::new(),
                duration: Duration::from_secs(0),
            });
        }

        let start_time = Instant::now();
        
        // Build environment
        let env = self.environment.clone().build()?;
        
        // Execute the hook using gix-command
        let result = self.execute_with_gix_command(&hook_path, args, stdin_data, &env)?;
        
        let duration = start_time.elapsed();
        
        // Check timeout
        if duration > self.config.timeout {
            return Err(Error::hook_timeout(hook_name, self.config.timeout.as_secs(), None));
        }

        Ok(result)
    }

    /// Execute the hook using gix-command.
    ///
    /// This executes the hook script with proper timeout, output limits, and environment setup.
    fn execute_with_gix_command(
        &mut self,
        hook_path: &PathBuf,
        args: &[String],
        stdin_data: Option<&[u8]>,
        env: &HashMap<String, String>,
    ) -> Result<HookResult, Error> {
        use std::process::Stdio;
        use std::io::Write;
        
        let start_time = Instant::now();
        
        // Prepare the command using gix-command
        let mut prepare = gix_command::prepare(hook_path)
            .args(args.iter().map(|s| s.as_str()))
            .stdin(if stdin_data.is_some() { Stdio::piped() } else { Stdio::null() })
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        
        // Add environment variables
        for (key, value) in env {
            prepare = prepare.env(key, value);
        }
        
        // Spawn the process
        let mut child = prepare.spawn()
            .map_err(|e| Error::Io(e))?;
        
        // Write stdin data if provided
        if let Some(data) = stdin_data {
            if let Some(mut stdin) = child.stdin.take() {
                stdin.write_all(data)
                    .map_err(|e| Error::Io(e))?;
                // Close stdin to signal EOF
                drop(stdin);
            }
        }
        
        // Collect output with size limits and optional sideband relay
        let mut stdout_buffer = Vec::new();
        let mut stderr_buffer = Vec::new();
        let mut total_output_size = 0;
        
        // Read stdout
        if let Some(mut stdout) = child.stdout.take() {
            self.read_stream_with_limits(
                &mut stdout,
                &mut stdout_buffer,
                &mut total_output_size,
                start_time,
                hook_path,
            )?;
        }
        
        // Read stderr
        if let Some(mut stderr) = child.stderr.take() {
            self.read_stream_with_limits(
                &mut stderr,
                &mut stderr_buffer,
                &mut total_output_size,
                start_time,
                hook_path,
            )?;
        }
        
        // Flush sideband writer if available
        self.flush_sideband();
        
        // Wait for the process to complete
        let exit_status = child.wait()
            .map_err(|e| Error::Io(e))?;
        
        let duration = start_time.elapsed();
        
        // Final timeout check
        if duration > self.config.timeout {
            return Err(Error::hook_timeout(
                &hook_path.file_name().unwrap_or_default().to_string_lossy(), 
                self.config.timeout.as_secs(), 
                None
            ));
        }
        
        Ok(HookResult {
            success: exit_status.success(),
            exit_code: exit_status.code(),
            stdout: stdout_buffer,
            stderr: stderr_buffer,
            duration,
        })
    }

    /// Read from a stream with size limits and timeout checks.
    fn read_stream_with_limits(
        &mut self,
        stream: &mut dyn Read,
        buffer: &mut Vec<u8>,
        total_output_size: &mut usize,
        start_time: Instant,
        hook_path: &PathBuf,
    ) -> Result<(), Error> {
        let mut read_buffer = [0u8; 8192];
        loop {
            match stream.read(&mut read_buffer) {
                Ok(0) => break, // EOF
                Ok(n) => {
                    let chunk = &read_buffer[..n];
                    
                    // Relay to sideband if available and enabled
                    self.relay_to_sideband(chunk);
                    
                    // Always capture for return value (with size limits)
                    *total_output_size += n;
                    if *total_output_size > self.config.max_output_size {
                        // Truncate and add marker
                        let remaining = self.config.max_output_size.saturating_sub(buffer.len());
                        if remaining > 0 {
                            buffer.extend_from_slice(&chunk[..remaining.min(n)]);
                        }
                        let truncated_msg = format!("\n... [truncated: output exceeded {} bytes]", self.config.max_output_size);
                        buffer.extend_from_slice(truncated_msg.as_bytes());
                        return Err(Error::hook_output_exceeded(
                            &hook_path.file_name().unwrap_or_default().to_string_lossy(),
                            self.config.max_output_size,
                            None
                        ));
                    }
                    buffer.extend_from_slice(chunk);
                }
                Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => continue,
                Err(e) => return Err(Error::Io(e)),
            }
            
            // Check timeout
            if start_time.elapsed() > self.config.timeout {
                return Err(Error::hook_timeout(
                    &hook_path.file_name().unwrap_or_default().to_string_lossy(),
                    self.config.timeout.as_secs(),
                    None
                ));
            }
        }
        Ok(())
    }



    /// Format command for hook input (old new refname format).
    fn format_command_for_hook(command: &CommandUpdate) -> String {
        match command {
            CommandUpdate::Create { new, name } => {
                format!("{} {} {}", "0".repeat(40), new, name)
            }
            CommandUpdate::Update { old, new, name } => {
                format!("{} {} {}", old, new, name)
            }
            CommandUpdate::Delete { old, name } => {
                format!("{} {} {}", old, "0".repeat(40), name)
            }
        }
    }

    /// Convert HookResult to HookDecision.
    fn hook_result_to_decision(result: HookResult, hook_name: &str) -> HookDecision {
        if result.success {
            HookDecision::allow_with_output(result.stdout, result.stderr)
        } else {
            // Use the new error mapping for consistent error messages
            let exit_code = result.exit_code.unwrap_or(1);
            let message = if result.stderr.is_empty() {
                Error::hook_failed(hook_name, exit_code, None).to_string()
            } else {
                Error::hook_failed_with_output(hook_name, exit_code, &result.stderr, None).to_string()
            };
            
            HookDecision::deny_with_output(
                message,
                exit_code,
                result.stdout,
                result.stderr,
            )
        }
    }
}

impl Hooks for ExternalHooks {
    fn update(&mut self, command: &CommandUpdate) -> Result<HookDecision, Error> {
        let args = match command {
            CommandUpdate::Create { new, name } => {
                vec![name.clone(), "0".repeat(40), new.to_string()]
            }
            CommandUpdate::Update { old, new, name } => {
                vec![name.clone(), old.to_string(), new.to_string()]
            }
            CommandUpdate::Delete { old, name } => {
                vec![name.clone(), old.to_string(), "0".repeat(40)]
            }
        };

        let result = self.execute_hook("update", &args, None)?;
        Ok(Self::hook_result_to_decision(result, "update"))
    }

    fn pre_receive(&mut self, commands: &[CommandUpdate]) -> Result<HookDecision, Error> {
        // Format all commands for stdin
        let stdin_data = commands
            .iter()
            .map(Self::format_command_for_hook)
            .collect::<Vec<_>>()
            .join("\n");

        let result = self.execute_hook("pre-receive", &[], Some(stdin_data.as_bytes()))?;
        Ok(Self::hook_result_to_decision(result, "pre-receive"))
    }

    fn post_receive(&mut self, commands: &[CommandUpdate]) -> Result<(), Error> {
        // Format all commands for stdin
        let stdin_data = commands
            .iter()
            .map(Self::format_command_for_hook)
            .collect::<Vec<_>>()
            .join("\n");

        let _result = self.execute_hook("post-receive", &[], Some(stdin_data.as_bytes()))?;
        // Post-receive is fire-and-forget, so we don't check the result
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::hooks::env::Identity;
    use gix_hash::ObjectId;
    use std::sync::{Arc, Mutex};

    /// Mock sideband writer for testing.
    #[derive(Debug, Clone)]
    struct MockSidebandWriter {
        data: Arc<Mutex<Vec<u8>>>,
        flush_count: Arc<Mutex<usize>>,
    }

    impl MockSidebandWriter {
        fn new() -> Self {
            Self {
                data: Arc::new(Mutex::new(Vec::new())),
                flush_count: Arc::new(Mutex::new(0)),
            }
        }

        fn get_data(&self) -> Vec<u8> {
            self.data.lock().unwrap().clone()
        }

        fn get_flush_count(&self) -> usize {
            *self.flush_count.lock().unwrap()
        }
    }

    impl SidebandWriter for MockSidebandWriter {
        fn write_chunk(&mut self, data: &[u8]) -> std::io::Result<()> {
            self.data.lock().unwrap().extend_from_slice(data);
            Ok(())
        }

        fn flush(&mut self) -> std::io::Result<()> {
            *self.flush_count.lock().unwrap() += 1;
            Ok(())
        }
    }

    fn create_test_environment() -> HookEnvironment {
        HookEnvironment::new()
            .with_git_dir("/tmp/test-repo/.git")
            .with_push_options(vec!["test=true".to_string()])
            .with_identity(Identity::new().with_name("Test User"))
    }

    #[test]
    fn external_hooks_config_defaults() {
        let config = ExternalHookConfig::default();
        assert_eq!(config.hooks_dir, PathBuf::from("hooks"));
        assert_eq!(config.timeout, Duration::from_secs(30));
        assert_eq!(config.max_output_size, 1024 * 1024);
        assert!(!config.enable_sideband_relay);
    }

    #[test]
    fn external_hooks_creation() {
        let config = ExternalHookConfig::default();
        let env = create_test_environment();
        let hooks = ExternalHooks::new(config, env);
        
        // Should be able to create without errors
        assert!(hooks.config.timeout > Duration::from_secs(0));
    }

    #[test]
    fn external_hooks_with_defaults() {
        let env = create_test_environment();
        let hooks = ExternalHooks::with_defaults(env);
        
        assert_eq!(hooks.config.timeout, Duration::from_secs(30));
    }

    #[test]
    fn format_command_for_hook_create() {
        let command = CommandUpdate::Create {
            new: ObjectId::from_hex(b"1234567890123456789012345678901234567890").unwrap(),
            name: "refs/heads/main".to_string(),
        };

        let formatted = ExternalHooks::format_command_for_hook(&command);
        assert_eq!(
            formatted,
            "0000000000000000000000000000000000000000 1234567890123456789012345678901234567890 refs/heads/main"
        );
    }

    #[test]
    fn format_command_for_hook_update() {
        let command = CommandUpdate::Update {
            old: ObjectId::from_hex(b"1111111111111111111111111111111111111111").unwrap(),
            new: ObjectId::from_hex(b"2222222222222222222222222222222222222222").unwrap(),
            name: "refs/heads/develop".to_string(),
        };

        let formatted = ExternalHooks::format_command_for_hook(&command);
        assert_eq!(
            formatted,
            "1111111111111111111111111111111111111111 2222222222222222222222222222222222222222 refs/heads/develop"
        );
    }

    #[test]
    fn format_command_for_hook_delete() {
        let command = CommandUpdate::Delete {
            old: ObjectId::from_hex(b"3333333333333333333333333333333333333333").unwrap(),
            name: "refs/heads/feature".to_string(),
        };

        let formatted = ExternalHooks::format_command_for_hook(&command);
        assert_eq!(
            formatted,
            "3333333333333333333333333333333333333333 0000000000000000000000000000000000000000 refs/heads/feature"
        );
    }

    #[test]
    fn hook_result_to_decision_success() {
        let result = HookResult {
            success: true,
            exit_code: Some(0),
            stdout: b"Hook output".to_vec(),
            stderr: Vec::new(),
            duration: Duration::from_millis(100),
        };

        let decision = ExternalHooks::hook_result_to_decision(result, "test-hook");
        assert!(decision.allowed);
        assert_eq!(decision.exit_code, Some(0));
        assert_eq!(decision.stdout, b"Hook output");
    }

    #[test]
    fn hook_result_to_decision_failure() {
        let result = HookResult {
            success: false,
            exit_code: Some(1),
            stdout: Vec::new(),
            stderr: b"Hook error".to_vec(),
            duration: Duration::from_millis(100),
        };

        let decision = ExternalHooks::hook_result_to_decision(result, "test-hook");
        assert!(!decision.allowed);
        assert_eq!(decision.exit_code, Some(1));
        assert_eq!(decision.stderr, b"Hook error");
        assert!(decision.message.contains("Hook 'test-hook' failed"));
    }

    #[test]
    fn external_hooks_nonexistent_hook_succeeds() {
        let env = create_test_environment();
        let mut hooks = ExternalHooks::with_defaults(env);
        
        let command = CommandUpdate::Create {
            new: ObjectId::null(gix_hash::Kind::Sha1),
            name: "refs/heads/test".to_string(),
        };

        // Non-existent hooks should succeed
        let decision = hooks.update(&command).unwrap();
        assert!(decision.allowed);
    }

    #[test]
    fn external_hooks_with_sideband_writer() {
        let env = create_test_environment();
        let mock_writer = MockSidebandWriter::new();
        let writer_clone = mock_writer.clone();
        
        let mut config = ExternalHookConfig::default();
        config.enable_sideband_relay = true;
        
        let mut hooks = ExternalHooks::with_sideband_writer(config, env, mock_writer);
        
        let command = CommandUpdate::Create {
            new: ObjectId::null(gix_hash::Kind::Sha1),
            name: "refs/heads/test".to_string(),
        };

        // Non-existent hooks should succeed, and sideband writer should be available
        let decision = hooks.update(&command).unwrap();
        assert!(decision.allowed);
        
        // Since the hook doesn't exist, no data should be written to sideband
        assert_eq!(writer_clone.get_data(), Vec::<u8>::new());
        assert_eq!(writer_clone.get_flush_count(), 0);
    }

    #[test]
    fn external_hooks_sideband_relay_disabled() {
        let env = create_test_environment();
        let mock_writer = MockSidebandWriter::new();
        let writer_clone = mock_writer.clone();
        
        let mut config = ExternalHookConfig::default();
        config.enable_sideband_relay = false; // Explicitly disable
        
        let mut hooks = ExternalHooks::with_sideband_writer(config, env, mock_writer);
        
        let command = CommandUpdate::Create {
            new: ObjectId::null(gix_hash::Kind::Sha1),
            name: "refs/heads/test".to_string(),
        };

        let decision = hooks.update(&command).unwrap();
        assert!(decision.allowed);
        
        // Even with a sideband writer, no data should be written when relay is disabled
        assert_eq!(writer_clone.get_data(), Vec::<u8>::new());
        assert_eq!(writer_clone.get_flush_count(), 0);
    }
}