use super::VtyJobSelection;

#[test]
fn parses_explicit_vty_job_targets_and_flags() {
    let selection = VtyJobSelection::parse(&[
        "id:client-a",
        "name:edge-a",
        "pool:edge",
        "provider:alpha",
        "country:US",
        "tag:bgp",
        "edge",
        "--destructive",
        "--confirmed",
        "id:client-a",
    ])
    .unwrap();

    assert!(selection.clients.is_empty());
    assert_eq!(
        selection.tags,
        vec![
            "bgp",
            "country:US",
            "edge",
            "id:client-a",
            "name:edge-a",
            "pool:edge",
            "provider:alpha"
        ]
    );
    assert!(selection.destructive);
    assert!(selection.confirmed);
}

#[test]
fn treats_namespaced_values_as_tags_and_rejects_empty_selectors() {
    let selection = VtyJobSelection::parse(&["client:edge-a", "role:edge"]).unwrap();
    assert_eq!(selection.tags, vec!["client:edge-a", "role:edge"]);
    assert!(VtyJobSelection::parse(&["tag:"]).is_err());
    assert!(VtyJobSelection::parse(&["--destructive"]).is_err());
}
