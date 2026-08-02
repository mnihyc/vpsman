use super::{percent_encode_path_segment, percent_encode_query_value};

#[test]
fn percent_encodes_query_values_without_touching_safe_bytes() {
    assert_eq!(percent_encode_query_value("agent-a_1"), "agent-a_1");
    assert_eq!(percent_encode_query_value("agent a/b"), "agent%20a%2Fb");
    assert_eq!(percent_encode_path_segment("agent a/b"), "agent%20a%2Fb");
}
