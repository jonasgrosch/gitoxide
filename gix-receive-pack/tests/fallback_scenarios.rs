//! Integration tests for pack ingestion fallback scenarios.
//!
//! These tests verify that the pack ingestion controller correctly handles
//! fallback strategies when the primary strategy fails.

use gix_receive_pack::pack::{
    IngestionPolicy, PackIngestPath, PackIngestor, PackIngestionController,
};
use gix_receive_pack::error::{ErrorKind, PackIngestionError};



#[test]
fn test_fallback_policy_strategy_selection() {
    let policy = IngestionPolicy::with_fallback(Some(100));
    
    // Test primary strategy selection
    assert_eq!(policy.choose_path(Some(50)), PackIngestPath::UnpackObjects);
    assert_eq!(policy.choose_path(Some(150)), PackIngestPath::IndexPack);
    
    // Test fallback strategy availability
    assert!(policy.has_fallback(PackIngestPath::IndexPack));
    assert!(policy.has_fallback(PackIngestPath::UnpackObjects));
    
    // Test fallback strategy selection
    assert_eq!(
        policy.get_fallback_strategy(PackIngestPath::IndexPack),
        Some(PackIngestPath::UnpackObjects)
    );
    assert_eq!(
        policy.get_fallback_strategy(PackIngestPath::UnpackObjects),
        Some(PackIngestPath::IndexPack)
    );
}

#[test]
fn test_fallback_disabled_policy() {
    let policy = IngestionPolicy::without_fallback(Some(100));
    
    // Test fallback disabled
    assert!(!policy.has_fallback(PackIngestPath::IndexPack));
    assert!(!policy.has_fallback(PackIngestPath::UnpackObjects));
    
    // Test no fallback strategies
    assert_eq!(policy.get_fallback_strategy(PackIngestPath::IndexPack), None);
    assert_eq!(policy.get_fallback_strategy(PackIngestPath::UnpackObjects), None);
    
    // Test strategy sequence contains only primary strategy
    let sequence = policy.get_strategy_sequence(Some(50));
    assert_eq!(sequence.len(), 1);
    assert_eq!(sequence[0], PackIngestPath::UnpackObjects);
}

#[test]
fn test_pack_ingestion_controller_creation() {
    let ingestor = PackIngestor::default();
    let policy = IngestionPolicy::with_fallback(Some(100));
    let controller = PackIngestionController::new(ingestor, policy);
    
    assert_eq!(controller.max_fallback_attempts, 2);
    assert!(controller.policy().enable_fallback);
    
    // Test custom max attempts
    let controller = controller.with_max_fallback_attempts(5);
    assert_eq!(controller.max_fallback_attempts, 5);
}

#[test]
fn test_should_attempt_fallback_logic() {
    let ingestor = PackIngestor::default();
    let policy = IngestionPolicy::with_fallback(Some(100));
    let controller = PackIngestionController::new(ingestor, policy);
    
    // Create different types of errors
    let recoverable_error = PackIngestionError::io(
        "temporary I/O failure",
        gix_receive_pack::error::ErrorContext::new("test"),
        std::io::Error::new(std::io::ErrorKind::Interrupted, "test"),
    );
    
    let non_recoverable_error = PackIngestionError::object_validation(
        "validation failed",
        gix_receive_pack::error::ErrorContext::new("test"),
        None,
        vec!["object is corrupted".to_string()],
        None,
    );
    
    // Test recoverable error scenarios
    assert!(controller.should_attempt_fallback(&recoverable_error, 0, 0)); // First attempt
    assert!(!controller.should_attempt_fallback(&recoverable_error, 0, 2)); // Exceeded max attempts
    assert!(!controller.should_attempt_fallback(&recoverable_error, 1, 0)); // Already fallback
    
    // Test non-recoverable error
    assert!(!controller.should_attempt_fallback(&non_recoverable_error, 0, 0));
}

#[test]
fn test_should_attempt_fallback_disabled_policy() {
    let ingestor = PackIngestor::default();
    let policy = IngestionPolicy::without_fallback(Some(100));
    let controller = PackIngestionController::new(ingestor, policy);
    
    let recoverable_error = PackIngestionError::io(
        "temporary I/O failure",
        gix_receive_pack::error::ErrorContext::new("test"),
        std::io::Error::new(std::io::ErrorKind::Interrupted, "test"),
    );
    
    // Should not attempt fallback when policy disables it
    assert!(!controller.should_attempt_fallback(&recoverable_error, 0, 0));
}

#[test]
fn test_error_kind_recovery_properties() {
    // Test recoverable error kinds
    assert!(ErrorKind::Io.is_recoverable());
    assert!(ErrorKind::Resource.is_recoverable());
    assert!(ErrorKind::Cancelled.is_recoverable());
    
    // Test non-recoverable error kinds
    assert!(!ErrorKind::Protocol.is_recoverable());
    assert!(!ErrorKind::Validation.is_recoverable());
    assert!(!ErrorKind::Permission.is_recoverable());
    assert!(!ErrorKind::NotFound.is_recoverable());
    assert!(!ErrorKind::Bug.is_recoverable());
    assert!(!ErrorKind::Other.is_recoverable());
}

#[test]
fn test_error_kind_temporary_properties() {
    // Test temporary error kinds
    assert!(ErrorKind::Io.is_temporary());
    assert!(ErrorKind::Resource.is_temporary());
    assert!(ErrorKind::Cancelled.is_temporary());
    
    // Test non-temporary error kinds
    assert!(!ErrorKind::Protocol.is_temporary());
    assert!(!ErrorKind::Validation.is_temporary());
    assert!(!ErrorKind::Permission.is_temporary());
    assert!(!ErrorKind::NotFound.is_temporary());
    assert!(!ErrorKind::Bug.is_temporary());
    assert!(!ErrorKind::Other.is_temporary());
}

#[test]
fn test_pack_ingestion_result_structure() {
    use gix_receive_pack::pack::{PackIngestionResult, FsckResults};
    
    let result = PackIngestionResult {
        strategy_used: PackIngestPath::IndexPack,
        fsck_results: FsckResults {
            validated_objects: vec![],
            warnings: vec![],
            errors: vec![],
            missing_objects: vec![],
        },
        attempts_made: 2,
        fallback_used: true,
        errors_encountered: vec![],
    };
    
    assert_eq!(result.strategy_used, PackIngestPath::IndexPack);
    assert_eq!(result.attempts_made, 2);
    assert!(result.fallback_used);
    assert!(result.errors_encountered.is_empty());
}

#[cfg(all(feature = "progress", feature = "pack-streaming"))]
#[test]
fn test_pack_ingestion_streaming_result_structure() {
    use gix_receive_pack::pack::{PackIngestionStreamingResult, FsckResults, StreamingStats, MemoryStats};
    
    let result = PackIngestionStreamingResult {
        strategy_used: PackIngestPath::UnpackObjects,
        fsck_results: FsckResults {
            validated_objects: vec![],
            warnings: vec![],
            errors: vec![],
            missing_objects: vec![],
        },
        streaming_stats: StreamingStats {
            bytes_read: 1024,
            memory_stats: MemoryStats {
                peak_usage: 512,
                current_usage: 256,
                allocations: 10,
                deallocations: 5,
            },
            buffer_size: 8192,
        },
        attempts_made: 1,
        fallback_used: false,
        errors_encountered: vec![],
    };
    
    assert_eq!(result.strategy_used, PackIngestPath::UnpackObjects);
    assert_eq!(result.attempts_made, 1);
    assert!(!result.fallback_used);
    assert_eq!(result.streaming_stats.bytes_read, 1024);
}

#[test]
fn test_multiple_error_creation() {
    let error1 = PackIngestionError::io(
        "first error",
        gix_receive_pack::error::ErrorContext::new("test1"),
        std::io::Error::new(std::io::ErrorKind::Interrupted, "test1"),
    );
    
    let error2 = PackIngestionError::resource_limit_exceeded(
        "memory",
        1024,
        512,
        gix_receive_pack::error::ErrorContext::new("test2"),
    );
    
    let multiple_error = PackIngestionError::Multiple {
        errors: vec![error1, error2],
        context: gix_receive_pack::error::ErrorContext::new("multiple-test"),
    };
    
    // Test that multiple error returns the most severe error kind
    assert_eq!(multiple_error.kind(), ErrorKind::Resource);
    
    // Test that multiple error is recoverable if any component is recoverable
    assert!(multiple_error.is_recoverable());
}

#[test]
fn test_fallback_error_user_messages() {
    let io_error = PackIngestionError::io(
        "disk full",
        gix_receive_pack::error::ErrorContext::new("test"),
        std::io::Error::new(std::io::ErrorKind::Other, "disk full"),
    );
    
    let user_msg = io_error.user_message();
    assert!(user_msg.contains("I/O error"));
    assert!(user_msg.contains("disk space"));
    
    let validation_error = PackIngestionError::object_validation(
        "corrupt object",
        gix_receive_pack::error::ErrorContext::new("test"),
        None,
        vec!["object header invalid".to_string()],
        None,
    );
    
    let user_msg = validation_error.user_message();
    assert!(user_msg.contains("Object validation failed"));
    assert!(user_msg.contains("object header invalid"));
}

#[test]
fn test_strategy_sequence_generation() {
    let policy = IngestionPolicy::with_fallback(Some(100));
    
    // Test sequence for small pack (should prefer UnpackObjects)
    let sequence = policy.get_strategy_sequence(Some(50));
    assert_eq!(sequence.len(), 2);
    assert_eq!(sequence[0], PackIngestPath::UnpackObjects);
    assert_eq!(sequence[1], PackIngestPath::IndexPack);
    
    // Test sequence for large pack (should prefer IndexPack)
    let sequence = policy.get_strategy_sequence(Some(150));
    assert_eq!(sequence.len(), 2);
    assert_eq!(sequence[0], PackIngestPath::IndexPack);
    assert_eq!(sequence[1], PackIngestPath::UnpackObjects);
    
    // Test sequence with no fallback
    let policy_no_fallback = IngestionPolicy::without_fallback(Some(100));
    let sequence = policy_no_fallback.get_strategy_sequence(Some(50));
    assert_eq!(sequence.len(), 1);
    assert_eq!(sequence[0], PackIngestPath::UnpackObjects);
}

#[test]
fn test_fallback_integration_example() {
    // This test demonstrates how the fallback system would work in practice
    let ingestor = PackIngestor::default();
    let policy = IngestionPolicy::with_fallback(Some(100));
    let controller = PackIngestionController::new(ingestor, policy);
    
    // Test that the controller is properly configured for fallback
    assert!(controller.policy().enable_fallback);
    assert_eq!(controller.max_fallback_attempts, 2);
    
    // Test strategy sequence generation
    let strategies = controller.policy().get_strategy_sequence(Some(50));
    assert_eq!(strategies.len(), 2);
    assert_eq!(strategies[0], PackIngestPath::UnpackObjects); // Primary for small pack
    assert_eq!(strategies[1], PackIngestPath::IndexPack);     // Fallback
    
    let strategies = controller.policy().get_strategy_sequence(Some(200));
    assert_eq!(strategies.len(), 2);
    assert_eq!(strategies[0], PackIngestPath::IndexPack);     // Primary for large pack
    assert_eq!(strategies[1], PackIngestPath::UnpackObjects); // Fallback
}

#[test]
fn test_error_recovery_strategies() {
    use gix_receive_pack::error::{ErrorRecovery, RecoveryAction};
    
    // Test recovery strategy for I/O errors (should be recoverable)
    let io_error = PackIngestionError::io(
        "disk full",
        gix_receive_pack::error::ErrorContext::new("test"),
        std::io::Error::new(std::io::ErrorKind::Other, "disk full"),
    );
    
    let recovery = ErrorRecovery::for_error(&io_error);
    assert!(recovery.should_auto_recover());
    assert!(recovery.recovery_actions.contains(&RecoveryAction::Retry));
    
    // Test recovery strategy for validation errors (should not be auto-recoverable)
    let validation_error = PackIngestionError::object_validation(
        "corrupt object",
        gix_receive_pack::error::ErrorContext::new("test"),
        None,
        vec!["object header invalid".to_string()],
        None,
    );
    
    let recovery = ErrorRecovery::for_error(&validation_error);
    assert!(!recovery.should_auto_recover());
    assert!(recovery.recovery_actions.contains(&RecoveryAction::ManualIntervention));
}