use super::*;

#[test]
fn parses_proc_stat_start_time_with_spaces_in_comm() {
    let stat = "123 (name with spaces) S 1 2 3 4 5 6 7 8 9 10 11 12 13 14 15 16 17 18 987654 20";

    assert_eq!(parse_proc_stat_start_time_ticks(stat).unwrap(), 987654);
    assert_eq!(parse_proc_stat_state(stat).unwrap(), 'S');
}
