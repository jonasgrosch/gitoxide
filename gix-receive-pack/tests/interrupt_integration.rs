//! Integration test for interrupt scaffolding to verify public API access.

use gix_receive_pack::{CancellationFlag, CancellationPoint};

#[test]
fn interrupt_types_are_publicly_accessible() {
    // Test that we can create and use the interrupt types from the main crate
    let flag = CancellationFlag::new();
    assert!(!flag.is_cancelled());
    
    // Test that we can use the trait
    assert!(flag.check().is_ok());
    
    // Test cancellation
    flag.cancel();
    
    #[cfg(feature = "interrupt")]
    {
        assert!(flag.is_cancelled());
        assert!(flag.check().is_err());
    }
    
    #[cfg(not(feature = "interrupt"))]
    {
        // No-op version should still work but not actually cancel
        assert!(!flag.is_cancelled());
        assert!(flag.check().is_ok());
    }
}

#[test]
fn default_flag_works() {
    let flag = CancellationFlag::default();
    assert!(!flag.is_cancelled());
    assert!(flag.check().is_ok());
}