/// Test to verify feature gate build matrix sanity.
/// 
/// This test ensures that:
/// 1. Default features (blocking-io) work correctly
/// 2. Explicit blocking-io feature works correctly  
/// 3. Async-io feature compiles but returns Unimplemented for M1
/// 4. Both features enabled prioritizes blocking path

use gix_receive_pack::protocol::{Advertiser, CapabilitySet, RefRecord};
use gix_hash::ObjectId;

fn oid(hex40: &str) -> ObjectId {
    ObjectId::from_hex(hex40.as_bytes()).expect("valid hex")
}

#[cfg(feature = "blocking-io")]
#[test]
fn blocking_io_works() {
    let refs = vec![
        RefRecord::new(oid("1111111111111111111111111111111111111111"), "refs/heads/main"),
    ];
    let caps = CapabilitySet::modern_defaults();
    let mut buf = Vec::new();
    let mut adv = Advertiser::new(&mut buf);
    
    // Should work with blocking-io
    let result = adv.write_advertisement(&refs, &caps, None);
    assert!(result.is_ok());
    assert!(!buf.is_empty());
}

#[cfg(all(feature = "async-io", not(feature = "blocking-io")))]
#[test]
fn async_io_returns_unimplemented() {
    let refs = vec![
        RefRecord::new(oid("1111111111111111111111111111111111111111"), "refs/heads/main"),
    ];
    let caps = CapabilitySet::modern_defaults();
    let mut buf = Vec::new();
    let mut adv = Advertiser::new(&mut buf);
    
    // Should return Unimplemented with async-io only
    let result = adv.write_advertisement(&refs, &caps, None);
    assert!(result.is_err());
    match result.unwrap_err() {
        gix_receive_pack::Error::Unimplemented => {
            // Expected for async-only builds in M1
        }
        other => panic!("Expected Unimplemented, got: {:?}", other),
    }
}

#[cfg(all(feature = "blocking-io", feature = "async-io"))]
#[test]
fn both_features_uses_blocking() {
    let refs = vec![
        RefRecord::new(oid("1111111111111111111111111111111111111111"), "refs/heads/main"),
    ];
    let caps = CapabilitySet::modern_defaults();
    let mut buf = Vec::new();
    let mut adv = Advertiser::new(&mut buf);
    
    // Should work when both features are enabled (blocking takes priority)
    let result = adv.write_advertisement(&refs, &caps, None);
    assert!(result.is_ok());
    assert!(!buf.is_empty());
}