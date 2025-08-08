// Integration tests that generate head-info transcripts via fixture scripts using gix-testtools,
// matching conventions across the workspace (see gix-blame and gix-dir).
//
// Each fixture script writes `client-to-server.head-info.pkt` containing logical head-info lines.
// We then feed those lines into the public parsing entrypoint.

use gix_testtools::scripted_fixture_read_only;

use gix_receive_pack::{
    CapabilitySet, CommandList, CommandUpdate, Options, ReceivePackBuilder,
};

fn read_fixture_pkt(dir: &std::path::Path) -> String {
    std::fs::read_to_string(dir.join("client-to-server.head-info.pkt"))
        .expect("fixture pkt file present")
}

#[test]
fn head_info_empty_create() {
    let dir = scripted_fixture_read_only("head-info-empty-create.sh").expect("script runs");
    let text = read_fixture_pkt(&dir);

    let rp = ReceivePackBuilder::new().blocking().build();

    // Advertise agent to allow agent=... from client; otherwise Options::validate_against() would reject.
    let advertised = CapabilitySet::modern_defaults().with_agent(Some("gix/1.0".into()));

    let (list, opts) = rp.parse_head_info_from_text(&text, &advertised).expect("valid parse");
    assert_eq!(list.len(), 1);

    match list.iter().next().expect("one") {
        CommandUpdate::Create { new, name } => {
            assert_eq!(new.to_string(), "1111111111111111111111111111111111111111");
            assert_eq!(name, "refs/heads/main");
        }
        other => panic!("expected Create, got {other:?}"),
    }

    // Negotiated tokens include modern defaults and agent - we don't assert exact ordering here.
    assert!(opts.has("report-status"));
    assert!(opts.has("report-status-v2"));
    assert!(opts.has("quiet"));
    assert!(opts.has("delete-refs"));
    assert!(opts.has("ofs-delta"));
    assert!(opts.has("agent"));
}

#[test]
fn head_info_update_and_delete() {
    let dir = scripted_fixture_read_only("head-info-update-delete.sh").expect("script runs");
    let text = read_fixture_pkt(&dir);

    let rp = ReceivePackBuilder::new().blocking().build();
    let advertised = CapabilitySet::modern_defaults().with_agent(Some("gix/1.0".into()));

    let (list, _opts) = rp.parse_head_info_from_text(&text, &advertised).expect("valid parse");
    assert_eq!(list.len(), 3);

    let mut it = list.iter();

    match it.next().unwrap() {
        CommandUpdate::Create { new, name } => {
            assert_eq!(new.to_string(), "1111111111111111111111111111111111111111");
            assert_eq!(name, "refs/heads/main");
        }
        other => panic!("expected Create, got {other:?}"),
    }
    match it.next().unwrap() {
        CommandUpdate::Update { old, new, name } => {
            assert_eq!(old.to_string(), "1111111111111111111111111111111111111111");
            assert_eq!(new.to_string(), "2222222222222222222222222222222222222222");
            assert_eq!(name, "refs/heads/main");
        }
        other => panic!("expected Update, got {other:?}"),
    }
    match it.next().unwrap() {
        CommandUpdate::Delete { old, name } => {
            assert_eq!(old.to_string(), "2222222222222222222222222222222222222222");
            assert_eq!(name, "refs/tags/v1");
        }
        other => panic!("expected Delete, got {other:?}"),
    }
}

#[test]
fn head_info_push_options_and_shallow() {
    let dir =
        scripted_fixture_read_only("head-info-push-options-and-shallow.sh").expect("script runs");
    let text = read_fixture_pkt(&dir);

    let rp = ReceivePackBuilder::new().blocking().build();
    let advertised = CapabilitySet::modern_defaults().with_agent(Some("gix/1.0".into()));

    let (list, opts) = rp.parse_head_info_from_text(&text, &advertised).expect("valid parse");

    // one create command
    assert_eq!(list.len(), 1);

    // push-options captured
    assert_eq!(opts.push_options, vec!["ci-skip=true", "notify=team"]);

    // shallow captured
    assert_eq!(opts.shallow.len(), 1);
    assert_eq!(
        opts.shallow[0].to_string(),
        "3333333333333333333333333333333333333333"
    );
}