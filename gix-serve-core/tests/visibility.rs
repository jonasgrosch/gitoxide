use gix_serve_core::visibility::{RefRecord, VisibleRoots};

#[test]
fn ref_record_new() {
    let id = gix_hash::ObjectId::null(gix_hash::Kind::Sha1);
    let r = RefRecord::new(id, "refs/heads/main");
    assert_eq!(r.id, id);
    assert_eq!(r.name, "refs/heads/main");
}

#[test]
fn visible_roots_filters_hidden() {
    use std::sync::Arc;
    // create an empty repo in memory not easily; just ensure predicate logic is applied by constructing
    // a VisibleRoots and calling collect() can't be done without a repo. Here we just ensure constructor compiles.
    let repo_path = gix_testtools::scripted_fixture_read_only("make_basic_repo.sh").unwrap();
    let repo = gix::open(repo_path).unwrap();
    let hidden: Arc<gix_serve_core::visibility::HiddenRefPredicate> = Arc::new(|rec| rec.name.contains("hidden"));
    let vr = VisibleRoots::new(&repo, hidden);
    let roots = vr.collect().unwrap();
    for (name, _id) in roots {
        assert!(!name.contains("hidden"));
    }
}


