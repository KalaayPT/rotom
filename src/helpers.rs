pub fn parse_number_str(s: &str) -> i32 {
    if s.starts_with("0x") {
        i32::from_str_radix(s.trim_start_matches("0x"), 16).unwrap()
    } else {
        s.parse().unwrap()
    }
}
