use std::collections::{BTreeMap, BTreeSet};

use serde_json::{json, Value};

pub const VPS_RULE_KEY_TRAFFIC_RESET_DAY: &str = "traffic.reset_day";
pub const VPS_RULE_KEY_TRAFFIC_QUOTA_TOTAL: &str = "traffic.quota.total";
pub const VPS_RULE_KEY_TRAFFIC_QUOTA_RX: &str = "traffic.quota.rx";
pub const VPS_RULE_KEY_TRAFFIC_QUOTA_TX: &str = "traffic.quota.tx";
pub const VPS_RULE_KEY_TRAFFIC_SELECTORS: &str = "traffic.selectors";
pub const VPS_RULE_KEY_BILLING_PRICE: &str = "billing.price";
pub const VPS_RULE_KEY_BILLING_CYCLE: &str = "billing.cycle";
pub const VPS_RULE_KEY_NETWORK_PORT_SPEED: &str = "network.port_speed";
pub const VPS_RULE_KEY_NETWORK_INTERFACES: &str = "network.interfaces";
pub const VPS_RULE_KEY_NETWORK_RATE_INTERFACES: &str = "network.rate.interfaces";
pub const VPS_RULE_KEY_PRODUCT_NAME: &str = "product.name";
pub const NETWORK_RATE_TRAFFIC_SELECTOR_REFERENCE_SYNTAX: &str = "[traffic.selectors]";

pub const SUPPORTED_VPS_RULE_KEYS: [&str; 11] = [
    VPS_RULE_KEY_BILLING_PRICE,
    VPS_RULE_KEY_BILLING_CYCLE,
    VPS_RULE_KEY_NETWORK_PORT_SPEED,
    VPS_RULE_KEY_NETWORK_INTERFACES,
    VPS_RULE_KEY_NETWORK_RATE_INTERFACES,
    VPS_RULE_KEY_PRODUCT_NAME,
    VPS_RULE_KEY_TRAFFIC_RESET_DAY,
    VPS_RULE_KEY_TRAFFIC_QUOTA_TOTAL,
    VPS_RULE_KEY_TRAFFIC_QUOTA_RX,
    VPS_RULE_KEY_TRAFFIC_QUOTA_TX,
    VPS_RULE_KEY_TRAFFIC_SELECTORS,
];

const MAX_VPS_RULE_VALUE_BYTES: usize = 4096;
pub const MAX_PRODUCT_NAME_BYTES: usize = 160;
const MAX_TRAFFIC_SELECTOR_ITEMS: usize = 16;
const MAX_TRAFFIC_INTERFACE_BYTES: usize = 128;
const MAX_BILLING_PRICE_WHOLE_DIGITS: usize = 9;

#[derive(Clone, Debug, PartialEq)]
pub struct ParsedVpsRuleValue {
    pub raw: String,
    pub json: Value,
    pub display: String,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum NetworkInterfaceSource {
    Host,
    Tunnel,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum NetworkInterfacePolicy {
    DefaultPhysical,
    All,
    Patterns(Vec<String>),
}

impl NetworkInterfacePolicy {
    pub fn from_rule_json(value: Option<&Value>) -> Result<Self, String> {
        let Some(value) = value else {
            return Ok(Self::DefaultPhysical);
        };
        match value.get("mode").and_then(Value::as_str) {
            Some("all") => Ok(Self::All),
            Some("patterns") => {
                let patterns = value
                    .get("patterns")
                    .and_then(Value::as_array)
                    .ok_or_else(|| "network_interfaces_patterns_invalid".to_string())?;
                ensure(
                    !patterns.is_empty() && patterns.len() <= MAX_TRAFFIC_SELECTOR_ITEMS,
                    "network_interfaces_patterns_invalid",
                )?;
                let mut parsed = Vec::with_capacity(patterns.len());
                let mut seen = BTreeSet::new();
                for pattern in patterns {
                    let pattern = pattern
                        .as_str()
                        .ok_or_else(|| "network_interfaces_pattern_invalid".to_string())?;
                    validate_network_interface_pattern(pattern)?;
                    ensure(
                        pattern != "*" && seen.insert(pattern),
                        "network_interfaces_pattern_invalid",
                    )?;
                    parsed.push(pattern.to_string());
                }
                Ok(Self::Patterns(parsed))
            }
            _ => Err("network_interfaces_mode_invalid".to_string()),
        }
    }

    pub fn matches(&self, source: NetworkInterfaceSource, interface: &str) -> bool {
        match self {
            Self::DefaultPhysical => {
                source == NetworkInterfaceSource::Host
                    && (interface.starts_with('e') || interface.starts_with('w'))
            }
            Self::All => true,
            Self::Patterns(patterns) => patterns
                .iter()
                .any(|pattern| network_interface_pattern_matches(pattern, interface)),
        }
    }
}

pub fn network_interface_pattern_matches(pattern: &str, interface: &str) -> bool {
    pattern == "*"
        || pattern
            .strip_suffix('*')
            .map_or(interface == pattern, |prefix| interface.starts_with(prefix))
}

#[derive(Clone, Debug, Eq, PartialEq, Ord, PartialOrd)]
struct TrafficSelector {
    source: String,
    interface: String,
    direction: String,
    canonical: String,
}

pub fn parse_vps_rule_value(key: &str, value: &str) -> Result<ParsedVpsRuleValue, String> {
    let key = normalize_vps_rule_key(key)?;
    ensure(
        value.len() <= MAX_VPS_RULE_VALUE_BYTES,
        "vps_rules_value_too_long",
    )?;
    let raw = value.trim();
    ensure(!raw.is_empty(), "vps_rules_empty_value_invalid")?;
    match key {
        VPS_RULE_KEY_TRAFFIC_RESET_DAY => parse_traffic_reset_day(raw),
        VPS_RULE_KEY_TRAFFIC_QUOTA_TOTAL
        | VPS_RULE_KEY_TRAFFIC_QUOTA_RX
        | VPS_RULE_KEY_TRAFFIC_QUOTA_TX => parse_traffic_quota(raw),
        VPS_RULE_KEY_TRAFFIC_SELECTORS => parse_traffic_selectors(raw),
        VPS_RULE_KEY_NETWORK_INTERFACES => parse_network_interfaces(raw),
        VPS_RULE_KEY_NETWORK_RATE_INTERFACES => parse_network_rate_interfaces(raw),
        VPS_RULE_KEY_BILLING_PRICE => parse_billing_price(raw),
        VPS_RULE_KEY_BILLING_CYCLE => parse_billing_cycle(raw),
        VPS_RULE_KEY_NETWORK_PORT_SPEED => parse_port_speed(raw),
        VPS_RULE_KEY_PRODUCT_NAME => parse_product_name(raw),
        _ => unreachable!("normalize_vps_rule_key rejects unsupported keys"),
    }
}

fn normalize_vps_rule_key(key: &str) -> Result<&str, String> {
    let key = key.trim();
    if SUPPORTED_VPS_RULE_KEYS.contains(&key) {
        Ok(key)
    } else {
        Err("vps_rules_key_unsupported".to_string())
    }
}

fn parse_traffic_reset_day(raw: &str) -> Result<ParsedVpsRuleValue, String> {
    let parts = raw.split_whitespace().collect::<Vec<_>>();
    ensure((1..=2).contains(&parts.len()), "traffic_reset_day_invalid")?;
    let day = parts[0]
        .parse::<i32>()
        .map_err(|_| "traffic_reset_day_invalid".to_string())?;
    ensure(
        day == -1 || (1..=31).contains(&day),
        "traffic_reset_day_invalid",
    )?;
    if day == -1 {
        ensure(parts.len() == 1, "traffic_reset_day_invalid")?;
        return Ok(ParsedVpsRuleValue {
            raw: "-1".to_string(),
            json: json!({"day": -1, "hour": 0}),
            display: "-".to_string(),
        });
    }
    let hour = if let Some(time) = parts.get(1) {
        let time = time.split(':').collect::<Vec<_>>();
        ensure(time.len() == 2, "traffic_reset_day_invalid")?;
        ensure(
            time[0].len() == 2
                && time[1].len() == 2
                && time
                    .iter()
                    .all(|part| part.chars().all(|ch| ch.is_ascii_digit())),
            "traffic_reset_day_invalid",
        )?;
        let hour = time[0]
            .parse::<i32>()
            .map_err(|_| "traffic_reset_day_invalid".to_string())?;
        let minute = time[1]
            .parse::<i32>()
            .map_err(|_| "traffic_reset_day_invalid".to_string())?;
        ensure(
            (0..=23).contains(&hour) && (0..=59).contains(&minute),
            "traffic_reset_day_invalid",
        )?;
        hour
    } else {
        0
    };
    let canonical = format!("{day} {hour:02}:00");
    Ok(ParsedVpsRuleValue {
        raw: canonical,
        json: json!({"day": day, "hour": hour}),
        display: format!("{day} {hour:02}:00 UTC"),
    })
}

fn parse_product_name(raw: &str) -> Result<ParsedVpsRuleValue, String> {
    let canonical = raw.split_whitespace().collect::<Vec<_>>().join(" ");
    ensure(!canonical.is_empty(), "product_name_empty")?;
    ensure(
        canonical.len() <= MAX_PRODUCT_NAME_BYTES,
        "product_name_too_long",
    )?;
    ensure(
        !canonical.chars().any(char::is_control),
        "product_name_control_character_invalid",
    )?;
    Ok(ParsedVpsRuleValue {
        raw: canonical.clone(),
        json: json!({"name": canonical, "display": canonical}),
        display: canonical,
    })
}

fn parse_traffic_quota(raw: &str) -> Result<ParsedVpsRuleValue, String> {
    if raw == "-1" {
        return Ok(ParsedVpsRuleValue {
            raw: "-1".to_string(),
            json: json!({"bytes": -1, "unlimited": true, "display": "Unlimited"}),
            display: "Unlimited".to_string(),
        });
    }
    let (bytes, canonical) = parse_byte_size(raw)?;
    Ok(ParsedVpsRuleValue {
        raw: canonical,
        json: json!({"bytes": bytes, "display": display_bytes(bytes)}),
        display: format!("{bytes} bytes"),
    })
}

fn parse_traffic_selectors(raw: &str) -> Result<ParsedVpsRuleValue, String> {
    if raw == "*" {
        return Ok(ParsedVpsRuleValue {
            raw: "*".to_string(),
            json: json!({"mode": "all"}),
            display: "All eligible interfaces".to_string(),
        });
    }
    let selectors = parse_traffic_selector_list(raw)?;
    Ok(ParsedVpsRuleValue {
        raw: canonical_selector_list(&selectors),
        json: json!({
            "mode": "exact",
            "selectors": selectors.iter().map(traffic_selector_json).collect::<Vec<_>>()
        }),
        display: format!("{} selectors", selectors.len()),
    })
}

fn parse_network_interfaces(raw: &str) -> Result<ParsedVpsRuleValue, String> {
    if raw == "*" {
        return Ok(ParsedVpsRuleValue {
            raw: "*".to_string(),
            json: json!({"mode": "all"}),
            display: "All reported interfaces".to_string(),
        });
    }
    let patterns = parse_network_interface_patterns(raw)?;
    Ok(ParsedVpsRuleValue {
        raw: patterns.join(","),
        json: json!({"mode": "patterns", "patterns": patterns}),
        display: format!("{} interface patterns", patterns.len()),
    })
}

fn parse_network_rate_interfaces(raw: &str) -> Result<ParsedVpsRuleValue, String> {
    if raw == NETWORK_RATE_TRAFFIC_SELECTOR_REFERENCE_SYNTAX {
        return Ok(ParsedVpsRuleValue {
            raw: NETWORK_RATE_TRAFFIC_SELECTOR_REFERENCE_SYNTAX.to_string(),
            json: json!({
                "mode": "reference",
                "reference": {"rule": VPS_RULE_KEY_TRAFFIC_SELECTORS},
            }),
            display: "Traffic selectors (referenced)".to_string(),
        });
    }
    if raw == "*" {
        return Ok(ParsedVpsRuleValue {
            raw: "*".to_string(),
            json: json!({"mode": "all"}),
            display: "All eligible interfaces".to_string(),
        });
    }
    if raw == "[]" {
        return Ok(ParsedVpsRuleValue {
            raw: "[]".to_string(),
            json: json!({"mode": "exact", "selectors": []}),
            display: "No live-rate interfaces".to_string(),
        });
    }
    let selectors = parse_traffic_selector_list(raw)?;
    ensure(
        selectors.iter().all(|selector| selector.source == "host"),
        "network_rate_selector_source_invalid",
    )?;
    Ok(ParsedVpsRuleValue {
        raw: canonical_selector_list(&selectors),
        json: json!({
            "mode": "exact",
            "selectors": selectors.iter().map(traffic_selector_json).collect::<Vec<_>>()
        }),
        display: format!("{} live-rate selectors", selectors.len()),
    })
}

fn parse_billing_price(raw: &str) -> Result<ParsedVpsRuleValue, String> {
    if raw
        .chars()
        .filter(|character| !character.is_whitespace())
        .collect::<String>()
        == "-1"
    {
        return Ok(ParsedVpsRuleValue {
            raw: "-1".to_string(),
            json: json!({"disabled": true, "display": "-"}),
            display: "-".to_string(),
        });
    }
    let (amount_and_currency, period_input) = raw
        .split_once('/')
        .ok_or_else(|| "billing_plan_period_required".to_string())?;
    ensure(
        !period_input.contains('/') && !period_input.trim().chars().any(char::is_whitespace),
        "billing_plan_period_invalid",
    )?;
    let amount_and_currency = amount_and_currency.trim();
    let currency_start = amount_and_currency
        .char_indices()
        .find(|(_, character)| {
            !character.is_ascii_digit() && *character != '.' && !character.is_whitespace()
        })
        .map(|(index, _)| index)
        .ok_or_else(|| "billing_plan_currency_required".to_string())?;
    let amount = amount_and_currency[..currency_start].trim_end();
    let currency_input = amount_and_currency[currency_start..].trim();
    ensure(
        !amount.chars().any(char::is_whitespace)
            && !currency_input.chars().any(char::is_whitespace),
        "billing_plan_price_invalid",
    )?;
    let price = normalize_billing_price(amount)?;
    let (currency, currency_display) = normalize_billing_currency(currency_input)?;
    let (period_code, period) = match period_input.trim().to_ascii_lowercase().as_str() {
        "m" => ("m", "month"),
        "q" => ("q", "quarter"),
        "h" | "hy" => ("hy", "half_year"),
        "y" => ("y", "year"),
        _ => return Err("billing_plan_period_invalid".to_string()),
    };
    let display = format!("{price} {currency_display}/{period_code}");
    Ok(ParsedVpsRuleValue {
        raw: display.clone(),
        json: json!({
            "disabled": false,
            "price": price,
            "currency": currency,
            "currency_display": currency_display,
            "period": period,
            "period_code": period_code,
            "display": display,
        }),
        display,
    })
}

fn parse_billing_cycle(raw: &str) -> Result<ParsedVpsRuleValue, String> {
    let (day, month) = match raw.split_once('-') {
        Some((month, day)) => (parse_billing_day(day)?, Some(parse_billing_month(month)?)),
        None => (parse_billing_day(raw)?, None),
    };
    if let Some(month) = month {
        let maximum_day = match month {
            2 => 29,
            4 | 6 | 9 | 11 => 30,
            _ => 31,
        };
        ensure(day <= maximum_day, "billing_cycle_day_invalid")?;
    }
    let display = month.map_or_else(|| day.to_string(), |month| format!("{month:02}-{day:02}"));
    Ok(ParsedVpsRuleValue {
        raw: display.clone(),
        json: json!({"day": day, "month": month, "display": display}),
        display,
    })
}

fn parse_port_speed(raw: &str) -> Result<ParsedVpsRuleValue, String> {
    let raw = raw.trim();
    let unit_start = raw
        .find(|character: char| character.is_ascii_alphabetic())
        .ok_or_else(|| "port_speed_unit_required".to_string())?;
    let amount = raw[..unit_start].trim_end();
    let unit_input = raw[unit_start..].trim();
    ensure(
        !amount.chars().any(char::is_whitespace) && !unit_input.chars().any(char::is_whitespace),
        "port_speed_value_invalid",
    )?;
    let (unit, multiplier) = match unit_input.to_ascii_lowercase().as_str() {
        "bps" => ("bps", 1_u128),
        "kbps" => ("Kbps", 1_000_u128),
        "mbps" => ("Mbps", 1_000_000_u128),
        "gbps" => ("Gbps", 1_000_000_000_u128),
        "tbps" => ("Tbps", 1_000_000_000_000_u128),
        _ => return Err("port_speed_unit_invalid".to_string()),
    };
    let (whole, fraction) = amount.split_once('.').unwrap_or((amount, ""));
    ensure(
        !whole.is_empty()
            && whole.chars().all(|character| character.is_ascii_digit())
            && fraction.len() <= 3
            && fraction.chars().all(|character| character.is_ascii_digit())
            && amount.matches('.').count() <= 1,
        "port_speed_value_invalid",
    )?;
    let scale = 10_u128
        .checked_pow(fraction.len() as u32)
        .ok_or_else(|| "port_speed_value_too_large".to_string())?;
    let whole_value = whole
        .parse::<u128>()
        .map_err(|_| "port_speed_value_invalid".to_string())?;
    let fraction_value = if fraction.is_empty() {
        0
    } else {
        fraction
            .parse::<u128>()
            .map_err(|_| "port_speed_value_invalid".to_string())?
    };
    let scaled = whole_value
        .checked_mul(scale)
        .and_then(|value| value.checked_add(fraction_value))
        .ok_or_else(|| "port_speed_value_too_large".to_string())?;
    let bps = scaled
        .checked_mul(multiplier)
        .ok_or_else(|| "port_speed_value_too_large".to_string())?
        / scale;
    ensure(
        bps > 0 && bps <= i64::MAX as u128,
        "port_speed_value_invalid",
    )?;
    let normalized_amount = normalize_decimal_text(whole, fraction);
    let display = format!("{normalized_amount} {unit}");
    Ok(ParsedVpsRuleValue {
        raw: display.clone(),
        json: json!({"bps": bps as i64, "display": display}),
        display,
    })
}

fn parse_byte_size(raw: &str) -> Result<(i64, String), String> {
    let value = raw.trim();
    ensure(!value.is_empty(), "byte_size_empty")?;
    let split_at = value
        .find(|character: char| !(character.is_ascii_digit() || character == '.'))
        .unwrap_or(value.len());
    let number_raw = &value[..split_at];
    let suffix = value[split_at..].trim().to_ascii_lowercase();
    let (unit, multiplier) = match suffix.as_str() {
        "" => ("", 1_u128),
        "b" => ("B", 1_u128),
        "kb" => ("KB", 1_000_u128),
        "mb" => ("MB", 1_000_000_u128),
        "gb" => ("GB", 1_000_000_000_u128),
        "tb" => ("TB", 1_000_000_000_000_u128),
        "kib" => ("KiB", 1_024_u128),
        "mib" => ("MiB", 1_048_576_u128),
        "gib" => ("GiB", 1_073_741_824_u128),
        "tib" => ("TiB", 1_099_511_627_776_u128),
        _ => return Err("byte_size_unit_invalid".to_string()),
    };
    let (whole, fraction) = number_raw.split_once('.').unwrap_or((number_raw, ""));
    ensure(
        !whole.is_empty()
            && whole.chars().all(|character| character.is_ascii_digit())
            && fraction.chars().all(|character| character.is_ascii_digit())
            && number_raw.matches('.').count() <= 1,
        "byte_size_number_invalid",
    )?;
    ensure(
        whole
            .bytes()
            .chain(fraction.bytes())
            .any(|digit| digit != b'0'),
        "byte_size_number_invalid",
    )?;
    let bytes = checked_decimal_times_multiplier(whole, fraction, multiplier)?;
    Ok((
        bytes,
        format!("{}{}", normalize_decimal_text(whole, fraction), unit),
    ))
}

/// Converts an unsigned decimal multiplied by an integer scale to bytes using
/// decimal round-half-up. The fractional part is multiplied digit-by-digit so
/// values above JavaScript's safe-integer limit and long fractional inputs
/// never pass through a floating-point representation.
fn checked_decimal_times_multiplier(
    whole: &str,
    fraction: &str,
    multiplier: u128,
) -> Result<i64, String> {
    let normalized_whole = whole.trim_start_matches('0');
    let whole_value = if normalized_whole.is_empty() {
        0
    } else {
        normalized_whole
            .parse::<u128>()
            .map_err(|_| "byte_size_too_large".to_string())?
    };
    let whole_scaled = whole_value
        .checked_mul(multiplier)
        .ok_or_else(|| "byte_size_too_large".to_string())?;

    // Standard base-10 multiplication from least-significant digit to most.
    // After consuming every fractional digit, `carry` is the integral part of
    // (fraction * multiplier) / 10^fraction.len(). The last emitted digit is
    // the tenths digit of the remainder and therefore decides half-up rounding.
    let mut carry = 0_u128;
    let mut remainder_tenths = 0_u8;
    for digit in fraction.bytes().rev() {
        let product = u128::from(digit - b'0')
            .checked_mul(multiplier)
            .and_then(|value| value.checked_add(carry))
            .ok_or_else(|| "byte_size_too_large".to_string())?;
        remainder_tenths = (product % 10) as u8;
        carry = product / 10;
    }
    let fractional_scaled = if remainder_tenths >= 5 {
        carry
            .checked_add(1)
            .ok_or_else(|| "byte_size_too_large".to_string())?
    } else {
        carry
    };
    let scaled = whole_scaled
        .checked_add(fractional_scaled)
        .ok_or_else(|| "byte_size_too_large".to_string())?;
    i64::try_from(scaled).map_err(|_| "byte_size_too_large".to_string())
}

fn display_bytes(bytes: i64) -> String {
    const UNITS: [(&str, f64); 5] = [
        ("TB", 1_000_000_000_000.0),
        ("GB", 1_000_000_000.0),
        ("MB", 1_000_000.0),
        ("KB", 1_000.0),
        ("B", 1.0),
    ];
    for (unit, factor) in UNITS {
        if bytes as f64 >= factor || unit == "B" {
            let value = bytes as f64 / factor;
            return if unit == "B" {
                format!("{bytes} B")
            } else if value >= 10.0 {
                format!("{value:.0} {unit}")
            } else {
                format!("{value:.1} {unit}")
            };
        }
    }
    format!("{bytes} B")
}

fn normalize_decimal_text(whole: &str, fraction: &str) -> String {
    let whole = whole.trim_start_matches('0');
    let whole = if whole.is_empty() { "0" } else { whole };
    let fraction = fraction.trim_end_matches('0');
    if fraction.is_empty() {
        whole.to_string()
    } else {
        format!("{whole}.{fraction}")
    }
}

fn normalize_billing_price(input: &str) -> Result<String, String> {
    let (whole, fraction) = input.split_once('.').unwrap_or((input, ""));
    ensure(
        !whole.is_empty()
            && whole.len() <= MAX_BILLING_PRICE_WHOLE_DIGITS
            && whole.chars().all(|character| character.is_ascii_digit())
            && fraction.len() <= 2
            && fraction.chars().all(|character| character.is_ascii_digit())
            && input.matches('.').count() <= 1,
        "billing_plan_price_invalid",
    )?;
    let whole = whole.trim_start_matches('0');
    let whole = if whole.is_empty() { "0" } else { whole };
    let fraction = match fraction.len() {
        0 => "00".to_string(),
        1 => format!("{fraction}0"),
        2 => fraction.to_string(),
        _ => unreachable!("billing price fraction length was validated"),
    };
    Ok(format!("{whole}.{fraction}"))
}

fn normalize_billing_currency(input: &str) -> Result<(String, String), String> {
    let upper = input.to_ascii_uppercase();
    match input {
        "$" => Ok(("USD".to_string(), "$".to_string())),
        "¥" | "￥" => Ok(("CNY".to_string(), "¥".to_string())),
        "€" => Ok(("EUR".to_string(), "€".to_string())),
        "£" => Ok(("GBP".to_string(), "£".to_string())),
        _ if upper.len() == 3
            && upper
                .chars()
                .all(|character| character.is_ascii_alphabetic()) =>
        {
            Ok((upper.clone(), upper))
        }
        _ => Err("billing_plan_currency_invalid".to_string()),
    }
}

fn parse_billing_day(input: &str) -> Result<u8, String> {
    let day = input
        .trim()
        .parse::<u8>()
        .map_err(|_| "billing_cycle_day_invalid".to_string())?;
    ensure((1..=31).contains(&day), "billing_cycle_day_invalid")?;
    Ok(day)
}

fn parse_billing_month(input: &str) -> Result<u8, String> {
    let month = input
        .trim()
        .parse::<u8>()
        .map_err(|_| "billing_cycle_month_invalid".to_string())?;
    ensure((1..=12).contains(&month), "billing_cycle_month_invalid")?;
    Ok(month)
}

fn parse_traffic_selector_list(input: &str) -> Result<Vec<TrafficSelector>, String> {
    let raw = input.trim();
    ensure(!raw.is_empty(), "traffic_selector_empty")?;
    let mut selectors = Vec::new();
    let mut seen = BTreeSet::new();
    let mut selected_directions = BTreeMap::<(String, String), u8>::new();
    for item in raw.split(',') {
        let selector = parse_traffic_selector(item)?;
        ensure(
            seen.insert(selector.canonical.clone()),
            "traffic_selector_duplicate",
        )?;
        let requested_directions = traffic_selector_direction_mask(&selector);
        let selected = selected_directions
            .entry((selector.source.clone(), selector.interface.clone()))
            .or_default();
        ensure(
            *selected & requested_directions == 0,
            "traffic_selector_direction_overlap",
        )?;
        *selected |= requested_directions;
        selectors.push(selector);
    }
    ensure(
        selectors.len() <= MAX_TRAFFIC_SELECTOR_ITEMS,
        "traffic_selector_too_many_items",
    )?;
    Ok(selectors)
}

fn parse_traffic_selector(item: &str) -> Result<TrafficSelector, String> {
    let item = item.trim();
    ensure(!item.is_empty(), "traffic_selector_empty_item")?;
    let (source, rest) = if let Some((source, rest)) = item.split_once(':') {
        let source = source.trim().to_ascii_lowercase();
        ensure(
            source == "host" || source == "tunnel",
            "traffic_selector_source_invalid",
        )?;
        (source, rest)
    } else {
        ("host".to_string(), item)
    };
    let (interface, direction) = if let Some((interface, direction)) = rest.split_once('+') {
        (interface.trim(), direction.trim().to_ascii_lowercase())
    } else {
        (rest.trim(), "total".to_string())
    };
    ensure(!interface.is_empty(), "traffic_selector_interface_required")?;
    ensure(
        interface.len() <= MAX_TRAFFIC_INTERFACE_BYTES
            && !interface.contains('*')
            && !interface.chars().any(|character| {
                character == ','
                    || character == '+'
                    || character == ':'
                    || character.is_whitespace()
                    || character.is_control()
            }),
        "traffic_selector_interface_invalid",
    )?;
    let direction = match direction.as_str() {
        "rx" => "rx",
        "tx" => "tx",
        "total" | "rx+tx" | "tx+rx" => "total",
        "tx/rx" | "rx/tx" => "tx/rx",
        _ => return Err("traffic_selector_direction_invalid".to_string()),
    }
    .to_string();
    let canonical = if source == "host" {
        if direction == "total" {
            interface.to_string()
        } else {
            format!("{interface}+{direction}")
        }
    } else if direction == "total" {
        format!("{source}:{interface}")
    } else {
        format!("{source}:{interface}+{direction}")
    };
    Ok(TrafficSelector {
        source,
        interface: interface.to_string(),
        direction,
        canonical,
    })
}

fn parse_network_interface_patterns(input: &str) -> Result<Vec<String>, String> {
    let mut patterns = Vec::new();
    let mut seen = BTreeSet::new();
    for item in input.split(',') {
        let pattern = item.trim();
        validate_network_interface_pattern(pattern)?;
        ensure(pattern != "*", "network_interfaces_all_must_stand_alone")?;
        ensure(
            seen.insert(pattern.to_string()),
            "network_interfaces_pattern_duplicate",
        )?;
        patterns.push(pattern.to_string());
    }
    ensure(
        !patterns.is_empty() && patterns.len() <= MAX_TRAFFIC_SELECTOR_ITEMS,
        "network_interfaces_too_many_patterns",
    )?;
    Ok(patterns)
}

fn validate_network_interface_pattern(pattern: &str) -> Result<(), String> {
    ensure(
        !pattern.is_empty()
            && pattern.len() <= MAX_TRAFFIC_INTERFACE_BYTES
            && !pattern.chars().any(|character| {
                character == ','
                    || character == '+'
                    || character == ':'
                    || character.is_whitespace()
                    || character.is_control()
            })
            && match pattern.matches('*').count() {
                0 => true,
                1 => pattern.ends_with('*'),
                _ => false,
            },
        "network_interfaces_pattern_invalid",
    )
}

fn traffic_selector_direction_mask(selector: &TrafficSelector) -> u8 {
    match selector.direction.as_str() {
        "rx" => 0b01,
        "tx" => 0b10,
        _ => 0b11,
    }
}

fn traffic_selector_json(selector: &TrafficSelector) -> Value {
    json!({
        "source": selector.source,
        "interface": selector.interface,
        "direction": selector.direction,
        "canonical": selector.canonical,
    })
}

fn canonical_selector_list(selectors: &[TrafficSelector]) -> String {
    selectors
        .iter()
        .map(|selector| selector.canonical.as_str())
        .collect::<Vec<_>>()
        .join(",")
}

fn ensure(condition: bool, error: &str) -> Result<(), String> {
    if condition {
        Ok(())
    } else {
        Err(error.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalizes_operator_facing_rule_values() {
        let cases = [
            (VPS_RULE_KEY_TRAFFIC_RESET_DAY, "014", "14 00:00"),
            (VPS_RULE_KEY_TRAFFIC_QUOTA_TOTAL, " 04.00 tb ", "4TB"),
            (VPS_RULE_KEY_NETWORK_PORT_SPEED, "1.500gbps", "1.5 Gbps"),
            (VPS_RULE_KEY_BILLING_PRICE, "29.9 cny / M", "29.90 CNY/m"),
            (VPS_RULE_KEY_BILLING_CYCLE, "6-15", "06-15"),
            (
                VPS_RULE_KEY_PRODUCT_NAME,
                "  Storage-Box\t 4  ",
                "Storage-Box 4",
            ),
            (
                VPS_RULE_KEY_TRAFFIC_SELECTORS,
                " ens3, eth0+tx ",
                "ens3,eth0+tx",
            ),
            (
                VPS_RULE_KEY_NETWORK_RATE_INTERFACES,
                " host:eth0, eth1+tx ",
                "eth0,eth1+tx",
            ),
            (
                VPS_RULE_KEY_TRAFFIC_SELECTORS,
                " HOST:eth0+TX, TUNNEL:wg0+RX ",
                "eth0+tx,tunnel:wg0+rx",
            ),
            (
                VPS_RULE_KEY_TRAFFIC_SELECTORS,
                " eth0+TX+RX, ens3+RX/TX ",
                "eth0,ens3+tx/rx",
            ),
            (
                VPS_RULE_KEY_NETWORK_RATE_INTERFACES,
                " eth0+RX+TX, ens3+TX/RX ",
                "eth0,ens3+tx/rx",
            ),
        ];
        for (key, input, expected) in cases {
            let parsed = parse_vps_rule_value(key, input).unwrap();
            assert_eq!(parsed.raw, expected);
            assert_eq!(
                parse_vps_rule_value(key, &parsed.raw).unwrap().raw,
                parsed.raw,
                "canonical normalization must be idempotent for {key}"
            );
        }
    }

    #[test]
    fn billing_cycle_accepts_unpadded_shorthand_but_keeps_the_padded_standard() {
        let padded = parse_vps_rule_value(VPS_RULE_KEY_BILLING_CYCLE, "06-05").unwrap();
        let unpadded = parse_vps_rule_value(VPS_RULE_KEY_BILLING_CYCLE, "6-5").unwrap();
        assert_eq!(padded, unpadded);
        assert_eq!(padded.raw, "06-05");
        assert_eq!(padded.display, "06-05");
        assert_eq!(
            padded.json,
            json!({"day": 5, "month": 6, "display": "06-05"})
        );

        let day_only = parse_vps_rule_value(VPS_RULE_KEY_BILLING_CYCLE, "07").unwrap();
        assert_eq!(day_only.raw, "7");
        assert_eq!(day_only.display, "7");
        assert_eq!(
            day_only.json,
            json!({"day": 7, "month": null, "display": "7"})
        );
    }

    #[test]
    fn traffic_reset_accepts_hour_and_never_persists_minute_precision() {
        let parsed = parse_vps_rule_value(VPS_RULE_KEY_TRAFFIC_RESET_DAY, "29 05:37").unwrap();
        assert_eq!(parsed.raw, "29 05:00");
        assert_eq!(parsed.json, json!({"day": 29, "hour": 5}));
        assert_eq!(parsed.display, "29 05:00 UTC");

        let day_only = parse_vps_rule_value(VPS_RULE_KEY_TRAFFIC_RESET_DAY, "29").unwrap();
        assert_eq!(day_only.raw, "29 00:00");
        assert_eq!(day_only.json, json!({"day": 29, "hour": 0}));

        let no_reset = parse_vps_rule_value(VPS_RULE_KEY_TRAFFIC_RESET_DAY, "-1").unwrap();
        assert_eq!(no_reset.raw, "-1");
        assert_eq!(no_reset.json, json!({"day": -1, "hour": 0}));
        assert!(parse_vps_rule_value(VPS_RULE_KEY_TRAFFIC_RESET_DAY, "-1 05:00").is_err());
    }

    #[test]
    fn product_name_is_bounded_display_text_without_a_semantic_format() {
        let punctuation = parse_vps_rule_value(VPS_RULE_KEY_PRODUCT_NAME, " LN.V2.HKGv3 ").unwrap();
        assert_eq!(punctuation.raw, "LN.V2.HKGv3");
        assert_eq!(punctuation.json["name"], "LN.V2.HKGv3");
        assert_eq!(punctuation.json["display"], "LN.V2.HKGv3");

        let unicode = format!("{}xxxx", "界".repeat(52));
        assert_eq!(unicode.len(), MAX_PRODUCT_NAME_BYTES);
        assert_eq!(
            parse_vps_rule_value(VPS_RULE_KEY_PRODUCT_NAME, &unicode)
                .unwrap()
                .raw,
            unicode
        );
        assert_eq!(
            parse_vps_rule_value(VPS_RULE_KEY_PRODUCT_NAME, &"界".repeat(54)).unwrap_err(),
            "product_name_too_long"
        );
        assert_eq!(
            parse_vps_rule_value(
                VPS_RULE_KEY_PRODUCT_NAME,
                &"x".repeat(MAX_PRODUCT_NAME_BYTES + 1),
            )
            .unwrap_err(),
            "product_name_too_long"
        );
        assert_eq!(
            parse_vps_rule_value(VPS_RULE_KEY_PRODUCT_NAME, "box\0name").unwrap_err(),
            "product_name_control_character_invalid"
        );
    }

    #[test]
    fn quota_conversion_is_exact_across_integer_limits_and_fractional_rounding() {
        let above_safe_integer =
            parse_vps_rule_value(VPS_RULE_KEY_TRAFFIC_QUOTA_TOTAL, "9007199254740993B").unwrap();
        assert_eq!(
            above_safe_integer.json["bytes"],
            json!(9_007_199_254_740_993_i64)
        );

        let maximum =
            parse_vps_rule_value(VPS_RULE_KEY_TRAFFIC_QUOTA_TOTAL, "9223372036854775807B").unwrap();
        assert_eq!(maximum.json["bytes"], json!(i64::MAX));
        let scaled_maximum =
            parse_vps_rule_value(VPS_RULE_KEY_TRAFFIC_QUOTA_TOTAL, "9223372.036854775807TB")
                .unwrap();
        assert_eq!(scaled_maximum.json["bytes"], json!(i64::MAX));
        assert_eq!(
            parse_vps_rule_value(VPS_RULE_KEY_TRAFFIC_QUOTA_TOTAL, "9223372036854775808B")
                .unwrap_err(),
            "byte_size_too_large"
        );
        assert_eq!(
            parse_vps_rule_value(VPS_RULE_KEY_TRAFFIC_QUOTA_TOTAL, "9223372.0368547758075TB")
                .unwrap_err(),
            "byte_size_too_large"
        );
        assert_eq!(
            parse_vps_rule_value(VPS_RULE_KEY_TRAFFIC_QUOTA_TOTAL, "1 0GB").unwrap_err(),
            "byte_size_unit_invalid"
        );

        let rounds_down = parse_vps_rule_value(
            VPS_RULE_KEY_TRAFFIC_QUOTA_TOTAL,
            "0.000000000000499999999999TB",
        )
        .unwrap();
        let rounds_up = parse_vps_rule_value(
            VPS_RULE_KEY_TRAFFIC_QUOTA_TOTAL,
            "0.000000000000500000000001TB",
        )
        .unwrap();
        assert_eq!(rounds_down.json["bytes"], json!(0));
        assert_eq!(rounds_up.json["bytes"], json!(1));
        assert_eq!(
            parse_vps_rule_value(VPS_RULE_KEY_TRAFFIC_QUOTA_TOTAL, &rounds_up.raw)
                .unwrap()
                .json,
            rounds_up.json
        );
    }

    #[test]
    fn decimal_scaling_matches_exact_small_rational_arithmetic() {
        for multiplier in [1_u128, 1_000, 1_000_000_000, 1_099_511_627_776] {
            for digits in 1..=6_u32 {
                let scale = 10_u128.pow(digits);
                for numerator in [0, 1, scale / 2, scale / 2 + 1, scale - 1] {
                    let fraction = format!("{numerator:0width$}", width = digits as usize);
                    let expected = (numerator * multiplier + scale / 2) / scale;
                    assert_eq!(
                        checked_decimal_times_multiplier("0", &fraction, multiplier).unwrap(),
                        expected as i64,
                        "fraction={fraction}, multiplier={multiplier}"
                    );
                }
            }
        }
    }

    #[test]
    fn selectors_reject_duplicates_overlap_and_utf8_names_over_128_bytes() {
        assert_eq!(
            parse_vps_rule_value(VPS_RULE_KEY_TRAFFIC_SELECTORS, "eth0, eth0").unwrap_err(),
            "traffic_selector_duplicate"
        );
        assert_eq!(
            parse_vps_rule_value(VPS_RULE_KEY_TRAFFIC_SELECTORS, "eth0, eth0+rx").unwrap_err(),
            "traffic_selector_direction_overlap"
        );
        let oversized_interface = "é".repeat(65);
        assert_eq!(
            parse_vps_rule_value(VPS_RULE_KEY_TRAFFIC_SELECTORS, &oversized_interface).unwrap_err(),
            "traffic_selector_interface_invalid"
        );
        for invalid in [
            "eth0+rx+rx",
            "eth0+tx+tx",
            "eth0+rx/rx",
            "eth0+tx/tx",
            "eth0+rx/tx+rx",
        ] {
            assert_eq!(
                parse_vps_rule_value(VPS_RULE_KEY_TRAFFIC_SELECTORS, invalid).unwrap_err(),
                "traffic_selector_direction_invalid"
            );
        }
        assert_eq!(
            parse_vps_rule_value(VPS_RULE_KEY_TRAFFIC_SELECTORS, "eth0+tx/rx,eth0+rx").unwrap_err(),
            "traffic_selector_direction_overlap"
        );
    }

    #[test]
    fn interface_policy_grammar_is_bounded_and_case_sensitive() {
        let patterns =
            parse_vps_rule_value(VPS_RULE_KEY_NETWORK_INTERFACES, " w*, eth0, ens* ").unwrap();
        assert_eq!(patterns.raw, "w*,eth0,ens*");
        assert_eq!(
            patterns.json,
            json!({"mode": "patterns", "patterns": ["w*", "eth0", "ens*"]})
        );
        let policy = NetworkInterfacePolicy::from_rule_json(Some(&patterns.json)).unwrap();
        assert!(policy.matches(NetworkInterfaceSource::Host, "wlan0"));
        assert!(policy.matches(NetworkInterfaceSource::Tunnel, "ens-tunnel"));
        assert!(!policy.matches(NetworkInterfaceSource::Host, "WLAN0"));
        assert!(!policy.matches(NetworkInterfaceSource::Host, "eth1"));

        let default = NetworkInterfacePolicy::from_rule_json(None).unwrap();
        assert!(default.matches(NetworkInterfaceSource::Host, "eth0"));
        assert!(default.matches(NetworkInterfaceSource::Host, "wlan0"));
        assert!(!default.matches(NetworkInterfaceSource::Host, "lo"));
        assert!(!default.matches(NetworkInterfaceSource::Tunnel, "eth-tunnel"));

        let all = parse_vps_rule_value(VPS_RULE_KEY_NETWORK_INTERFACES, "*").unwrap();
        assert_eq!(all.json, json!({"mode": "all"}));
        assert!(NetworkInterfacePolicy::from_rule_json(Some(&all.json))
            .unwrap()
            .matches(NetworkInterfaceSource::Tunnel, "wg0"));

        for invalid in [
            "",
            "*,eth0",
            "eth*0",
            "eth**",
            "host:eth0",
            "eth0+rx",
            "e*,e*",
        ] {
            assert!(
                parse_vps_rule_value(VPS_RULE_KEY_NETWORK_INTERFACES, invalid).is_err(),
                "input={invalid:?}"
            );
        }
        assert!(parse_vps_rule_value(
            VPS_RULE_KEY_NETWORK_INTERFACES,
            &(0..17)
                .map(|index| format!("eth{index}"))
                .collect::<Vec<_>>()
                .join(",")
        )
        .is_err());
    }

    #[test]
    fn selector_all_and_rate_none_are_explicit_and_exact_cannot_use_wildcards() {
        let traffic_all = parse_vps_rule_value(VPS_RULE_KEY_TRAFFIC_SELECTORS, "*").unwrap();
        assert_eq!(traffic_all.raw, "*");
        assert_eq!(traffic_all.json, json!({"mode": "all"}));

        let traffic_exact =
            parse_vps_rule_value(VPS_RULE_KEY_TRAFFIC_SELECTORS, "eth0+rx").unwrap();
        assert_eq!(traffic_exact.json["mode"], "exact");

        let rate_all = parse_vps_rule_value(VPS_RULE_KEY_NETWORK_RATE_INTERFACES, "*").unwrap();
        assert_eq!(rate_all.raw, "*");
        assert_eq!(rate_all.json, json!({"mode": "all"}));
        let rate_none = parse_vps_rule_value(VPS_RULE_KEY_NETWORK_RATE_INTERFACES, "[]").unwrap();
        assert_eq!(rate_none.raw, "[]");
        assert_eq!(rate_none.json, json!({"mode": "exact", "selectors": []}));
        for invalid in ["", "eth*", "*,eth0"] {
            assert!(
                parse_vps_rule_value(VPS_RULE_KEY_NETWORK_RATE_INTERFACES, invalid).is_err(),
                "input={invalid:?}"
            );
        }
        for invalid in ["eth*", "*,eth0", "tunnel:wg*"] {
            assert!(
                parse_vps_rule_value(VPS_RULE_KEY_TRAFFIC_SELECTORS, invalid).is_err(),
                "input={invalid:?}"
            );
        }
    }

    #[test]
    fn selector_total_and_max_aliases_have_one_canonical_representation() {
        for input in ["eth0", "eth0+total", "eth0+rx+tx", "eth0+tx+rx"] {
            let parsed = parse_vps_rule_value(VPS_RULE_KEY_TRAFFIC_SELECTORS, input).unwrap();
            assert_eq!(parsed.raw, "eth0", "input={input}");
            assert_eq!(parsed.json["selectors"][0]["direction"], "total");
            assert_eq!(parsed.json["selectors"][0]["canonical"], "eth0");
        }
        for input in ["ens3+tx/rx", "ens3+rx/tx"] {
            let parsed = parse_vps_rule_value(VPS_RULE_KEY_TRAFFIC_SELECTORS, input).unwrap();
            assert_eq!(parsed.raw, "ens3+tx/rx", "input={input}");
            assert_eq!(parsed.json["selectors"][0]["direction"], "tx/rx");
            assert_eq!(parsed.json["selectors"][0]["canonical"], "ens3+tx/rx");
        }
    }

    #[test]
    fn preserves_rule_sentinels_and_reference_syntax() {
        assert_eq!(
            parse_vps_rule_value(VPS_RULE_KEY_TRAFFIC_QUOTA_TOTAL, "-1")
                .unwrap()
                .raw,
            "-1"
        );
        assert_eq!(
            parse_vps_rule_value(
                VPS_RULE_KEY_NETWORK_RATE_INTERFACES,
                NETWORK_RATE_TRAFFIC_SELECTOR_REFERENCE_SYNTAX,
            )
            .unwrap()
            .raw,
            NETWORK_RATE_TRAFFIC_SELECTOR_REFERENCE_SYNTAX
        );
    }
}
