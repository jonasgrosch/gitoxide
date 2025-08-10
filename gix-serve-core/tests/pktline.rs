#[cfg(feature = "blocking-io")]
#[test]
fn write_flush_and_delimiter() {
    use gix_serve_core::io_blocking::{pkt_writer as mk, write_advert_trailer};
    use gix_serve_core::pktline::PktWriter;
    let mut out = Vec::new();
    let mut w: PktWriter<&mut Vec<u8>> = mk(&mut out);
    write_advert_trailer(&mut w).unwrap();
    assert_eq!(&out, b"00010000");
}


