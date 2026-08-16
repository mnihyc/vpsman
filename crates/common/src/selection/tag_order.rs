use std::cmp::Ordering;

pub fn tag_namespace(tag: &str) -> Option<&str> {
    tag.split_once(':')
        .map(|(namespace, _)| namespace)
        .filter(|namespace| !namespace.is_empty())
}

pub fn same_tag_namespace(left: &str, right: &str) -> bool {
    match (tag_namespace(left), tag_namespace(right)) {
        (Some(left), Some(right)) => left.eq_ignore_ascii_case(right),
        _ => false,
    }
}

pub fn compare_tag_names_naturally(left: &str, right: &str) -> Ordering {
    let left_value = left.split_once(':').map_or(left, |(_, value)| value);
    let right_value = right.split_once(':').map_or(right, |(_, value)| value);
    compare_ascii_natural(left_value, right_value).then_with(|| left.cmp(right))
}

pub fn normalize_tag_namespace_blocks(tags: &mut [String]) {
    let mut start = 0;
    while start < tags.len() {
        if tag_namespace(&tags[start]).is_none() {
            start += 1;
            continue;
        }
        let mut end = start + 1;
        while end < tags.len() && same_tag_namespace(&tags[start], &tags[end]) {
            end += 1;
        }
        if end - start > 1 {
            tags[start..end].sort_by(|left, right| compare_tag_names_naturally(left, right));
        }
        start = end;
    }
}

pub fn insert_tags_into_last_namespace_blocks(
    ordered_tags: &mut Vec<String>,
    additions: &[String],
    namespace_natural_sort_enabled: bool,
) {
    for addition in additions {
        if ordered_tags.iter().any(|tag| tag == addition) {
            continue;
        }
        let insertion_index = tag_namespace(addition).and_then(|_| {
            ordered_tags
                .iter()
                .rposition(|existing| same_tag_namespace(existing, addition))
                .map(|index| index + 1)
        });
        match insertion_index {
            Some(index) => ordered_tags.insert(index, addition.clone()),
            None => ordered_tags.push(addition.clone()),
        }
    }
    if namespace_natural_sort_enabled {
        normalize_tag_namespace_blocks(ordered_tags);
    }
}

fn compare_ascii_natural(left: &str, right: &str) -> Ordering {
    let left = left.as_bytes();
    let right = right.as_bytes();
    let mut left_cursor = 0;
    let mut right_cursor = 0;
    let mut numeric_tie_break = Ordering::Equal;

    while left_cursor < left.len() && right_cursor < right.len() {
        let left_digit = left[left_cursor].is_ascii_digit();
        let right_digit = right[right_cursor].is_ascii_digit();
        if left_digit != right_digit {
            return fold_ascii(left[left_cursor]).cmp(&fold_ascii(right[right_cursor]));
        }

        let left_end = chunk_end(left, left_cursor, left_digit);
        let right_end = chunk_end(right, right_cursor, right_digit);
        let left_chunk = &left[left_cursor..left_end];
        let right_chunk = &right[right_cursor..right_end];
        let ordering = if left_digit {
            compare_digit_chunks(left_chunk, right_chunk)
        } else {
            compare_text_chunks(left_chunk, right_chunk)
        };
        if ordering != Ordering::Equal {
            return ordering;
        }
        if left_digit && numeric_tie_break == Ordering::Equal {
            numeric_tie_break = left_chunk
                .len()
                .cmp(&right_chunk.len())
                .then_with(|| left_chunk.cmp(right_chunk));
        }
        left_cursor = left_end;
        right_cursor = right_end;
    }

    match (left_cursor == left.len(), right_cursor == right.len()) {
        (true, false) => Ordering::Less,
        (false, true) => Ordering::Greater,
        _ => numeric_tie_break,
    }
}

fn chunk_end(value: &[u8], start: usize, digit: bool) -> usize {
    value[start..]
        .iter()
        .position(|byte| byte.is_ascii_digit() != digit)
        .map_or(value.len(), |offset| start + offset)
}

fn compare_digit_chunks(left: &[u8], right: &[u8]) -> Ordering {
    let left_significant = significant_digits(left);
    let right_significant = significant_digits(right);
    left_significant
        .len()
        .cmp(&right_significant.len())
        .then_with(|| left_significant.cmp(right_significant))
}

fn significant_digits(value: &[u8]) -> &[u8] {
    let first_significant = value
        .iter()
        .position(|byte| *byte != b'0')
        .unwrap_or(value.len().saturating_sub(1));
    &value[first_significant..]
}

fn compare_text_chunks(left: &[u8], right: &[u8]) -> Ordering {
    left.iter()
        .map(|byte| fold_ascii(*byte))
        .cmp(right.iter().map(|byte| fold_ascii(*byte)))
}

fn fold_ascii(byte: u8) -> u8 {
    byte.to_ascii_lowercase()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde::Deserialize;

    #[derive(Deserialize)]
    struct TagOrderFixtures {
        cases: Vec<TagOrderFixture>,
    }

    #[derive(Deserialize)]
    struct TagOrderFixture {
        name: String,
        input: Vec<String>,
        expected: Vec<String>,
    }

    #[test]
    fn natural_tag_order_matches_shared_frontend_fixtures() {
        let fixtures: TagOrderFixtures =
            serde_json::from_str(include_str!("../../tests/fixtures/tag-order-cases.json"))
                .unwrap();
        for fixture in fixtures.cases {
            let mut tags = fixture.input;
            tags.sort_by(|left, right| compare_tag_names_naturally(left, right));
            assert_eq!(tags, fixture.expected, "fixture: {}", fixture.name);
        }
    }

    #[test]
    fn namespace_normalization_never_crosses_top_level_boundaries() {
        let mut tags = vec![
            "provider:B".to_string(),
            "provider:A".to_string(),
            "country:US".to_string(),
            "plain".to_string(),
            "Provider:D".to_string(),
            "provider:C".to_string(),
        ];
        normalize_tag_namespace_blocks(&mut tags);
        assert_eq!(
            tags,
            [
                "provider:A",
                "provider:B",
                "country:US",
                "plain",
                "provider:C",
                "Provider:D",
            ]
        );
    }

    #[test]
    fn additions_extend_only_the_last_matching_namespace_block() {
        let mut tags = vec![
            "provider:A".to_string(),
            "country:US".to_string(),
            "provider:D".to_string(),
            "provider:B".to_string(),
            "plain".to_string(),
        ];
        insert_tags_into_last_namespace_blocks(
            &mut tags,
            &["Provider:C".to_string(), "standalone".to_string()],
            true,
        );
        assert_eq!(
            tags,
            [
                "provider:A",
                "country:US",
                "provider:B",
                "Provider:C",
                "provider:D",
                "plain",
                "standalone",
            ]
        );
    }
}
