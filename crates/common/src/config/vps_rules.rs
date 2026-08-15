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
pub const VPS_RULE_KEY_NETWORK_RATE_INTERFACES: &str = "network.rate.interfaces";
pub const NETWORK_RATE_TRAFFIC_SELECTOR_REFERENCE_SYNTAX: &str = "[traffic.selectors]";

pub const SUPPORTED_VPS_RULE_KEYS: [&str; 9] = [
    VPS_RULE_KEY_BILLING_PRICE,
    VPS_RULE_KEY_BILLING_CYCLE,
    VPS_RULE_KEY_NETWORK_PORT_SPEED,
    VPS_RULE_KEY_NETWORK_RATE_INTERFACES,
    VPS_RULE_KEY_TRAFFIC_RESET_DAY,
    VPS_RULE_KEY_TRAFFIC_QUOTA_TOTAL,
    VPS_RULE_KEY_TRAFFIC_QUOTA_RX,
    VPS_RULE_KEY_TRAFFIC_QUOTA_TX,
    VPS_RULE_KEY_TRAFFIC_SELECTORS,
];

const MAX_VPS_RULE_VALUE_BYTES: usize = 4096;
const MAX_TRAFFIC_SELECTOR_ITEMS: usize = 16;
const MAX_TRAFFIC_INTERFACE_BYTES: usize = 128;
const MAX_BILLING_PRICE_WHOLE_DIGITS: usize = 9;

#[derive(Clone, Debug, PartialEq)]
pub struct ParsedVpsRuleValue {
    pub raw: String,
    pub json: Value,
    pub display: String,
}

#[derive(Clone, Debug, Eq, PartialEq, Ord, PartialOrd)]
struct TrafficSelector {
    source: String,
    interface: String,
    direction: String,
    canonical: String,
}

pub fn parse_vps_rule_value(key: &str, value: &str) -> Result<ParsedVpsRuleValue, String> {
    parse_vps_rule_value_with_options(key, value, false)
}

pub fn parse_persisted_vps_rule_value(
    key: &str,
    value: &str,
) -> Result<ParsedVpsRuleValue, String> {
    parse_vps_rule_value_with_options(key, value, true)
}

fn parse_vps_rule_value_with_options(
    key: &str,
    value: &str,
    allow_direction_overlap: bool,
) -> Result<ParsedVpsRuleValue, String> {
    let key = normalize_vps_rule_key(key)?;
    let raw = value.trim();
    ensure(
        !raw.is_empty() || key == VPS_RULE_KEY_NETWORK_RATE_INTERFACES,
        "vps_rules_empty_value_invalid",
    )?;
    ensure(
        raw.len() <= MAX_VPS_RULE_VALUE_BYTES,
        "vps_rules_value_too_long",
    )?;
    match key {
        VPS_RULE_KEY_TRAFFIC_RESET_DAY => parse_traffic_reset_day(raw),
        VPS_RULE_KEY_TRAFFIC_QUOTA_TOTAL
        | VPS_RULE_KEY_TRAFFIC_QUOTA_RX
        | VPS_RULE_KEY_TRAFFIC_QUOTA_TX => parse_traffic_quota(raw),
        VPS_RULE_KEY_TRAFFIC_SELECTORS => parse_traffic_selectors(raw, allow_direction_overlap),
        VPS_RULE_KEY_NETWORK_RATE_INTERFACES => parse_network_rate_interfaces(raw),
        VPS_RULE_KEY_BILLING_PRICE => parse_billing_price(raw),
        VPS_RULE_KEY_BILLING_CYCLE => parse_billing_cycle(raw),
        VPS_RULE_KEY_NETWORK_PORT_SPEED => parse_port_speed(raw),
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
    let day = raw
        .parse::<i32>()
        .map_err(|_| "traffic.reset_day must be an integer".to_string())?;
    ensure(
        day == -1 || (1..=31).contains(&day),
        "traffic_reset_day_invalid",
    )?;
    let canonical = day.to_string();
    Ok(ParsedVpsRuleValue {
        raw: canonical,
        json: json!({"day": day}),
        display: if day == -1 {
            "-".to_string()
        } else {
            format!("{day} UTC")
        },
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

fn parse_traffic_selectors(
    raw: &str,
    allow_direction_overlap: bool,
) -> Result<ParsedVpsRuleValue, String> {
    let selectors = parse_traffic_selector_list(raw, allow_direction_overlap)?;
    Ok(ParsedVpsRuleValue {
        raw: canonical_selector_list(&selectors),
        json: json!({
            "selectors": selectors.iter().map(traffic_selector_json).collect::<Vec<_>>()
        }),
        display: format!("{} selectors", selectors.len()),
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
    if raw.is_empty() || raw == "[]" {
        return Ok(ParsedVpsRuleValue {
            raw: "[]".to_string(),
            json: json!({"mode": "all"}),
            display: "All reported interfaces".to_string(),
        });
    }
    let selectors = parse_traffic_selector_list(raw, false)?;
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

fn parse_traffic_selector_list(
    input: &str,
    allow_direction_overlap: bool,
) -> Result<Vec<TrafficSelector>, String> {
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
        if !allow_direction_overlap {
            ensure(
                *selected & requested_directions == 0,
                "traffic_selector_direction_overlap",
            )?;
        }
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
            && !interface.chars().any(|character| {
                character == ','
                    || character == '+'
                    || character == ':'
                    || character.is_whitespace()
                    || character.is_control()
            }),
        "traffic_selector_interface_invalid",
    )?;
    ensure(
        matches!(direction.as_str(), "rx" | "tx" | "total"),
        "traffic_selector_direction_invalid",
    )?;
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
            (VPS_RULE_KEY_TRAFFIC_RESET_DAY, "014", "14"),
            (VPS_RULE_KEY_TRAFFIC_QUOTA_TOTAL, " 04.00 tb ", "4TB"),
            (VPS_RULE_KEY_NETWORK_PORT_SPEED, "1.500gbps", "1.5 Gbps"),
            (VPS_RULE_KEY_BILLING_PRICE, "29.9 cny / M", "29.90 CNY/m"),
            (VPS_RULE_KEY_BILLING_CYCLE, "6-15", "06-15"),
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
