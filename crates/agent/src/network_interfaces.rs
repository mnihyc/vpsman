use std::{
    collections::{BTreeMap, BTreeSet, HashMap},
    ffi::CStr,
    net::{Ipv4Addr, Ipv6Addr},
    path::Path,
    ptr,
    time::{SystemTime, UNIX_EPOCH},
};

use anyhow::{Context, Result};
use serde::Serialize;
use tokio::time::{self, Duration};
use vpsman_common::{AgentConfig, CommandOutput, OutputStream};

const MAX_INTERFACE_COUNT: usize = 256;
const MAX_INTERFACE_NAME_BYTES: usize = 64;
const READ_SMALL_LIMIT_BYTES: u64 = 4096;

pub(crate) struct NetworkInterfacesInput<'a> {
    pub(crate) job_id: uuid::Uuid,
    pub(crate) config: &'a AgentConfig,
    pub(crate) max_timeout_secs: u64,
}

#[derive(Clone, Debug, Default, Serialize)]
struct NetworkInterfaceSnapshot {
    name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    ifindex: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    operstate: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    mtu: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    mac: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    link_type: Option<u32>,
    flags: Vec<String>,
    addresses: Vec<NetworkInterfaceAddress>,
    rx_bytes: u64,
    tx_bytes: u64,
    metadata_sources: Vec<String>,
}

#[derive(Clone, Debug, Eq, PartialEq, Ord, PartialOrd, Serialize)]
struct NetworkInterfaceAddress {
    family: String,
    address: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    prefix_len: Option<u8>,
    #[serde(skip_serializing_if = "Option::is_none")]
    scope: Option<String>,
}

#[derive(Clone, Debug, Default)]
struct InterfaceCounters {
    rx_bytes: u64,
    tx_bytes: u64,
}

pub(crate) async fn execute_network_interfaces_command(
    input: NetworkInterfacesInput<'_>,
) -> Result<Vec<CommandOutput>> {
    time::timeout(
        Duration::from_secs(input.max_timeout_secs.max(1)),
        inspect_network_interfaces(input),
    )
    .await
    .context("network interface inspection timed out")?
}

async fn inspect_network_interfaces(
    input: NetworkInterfacesInput<'_>,
) -> Result<Vec<CommandOutput>> {
    let proc_root = Path::new(&input.config.telemetry.proc_root);
    let sys_class_net = Path::new(&input.config.telemetry.sys_class_net_dir);
    let counters = network_counters(proc_root).unwrap_or_default();
    let address_result = interface_addresses_from_getifaddrs();
    let (addresses, address_status, address_error) = match address_result {
        Ok(addresses) => (addresses, "ok", None),
        Err(error) => (
            HashMap::new(),
            "error",
            Some(truncate_string(&error.to_string(), 240)),
        ),
    };
    let sysfs_result = collect_sysfs_interfaces(sys_class_net, &counters, &addresses);
    let (interfaces, sysfs_status, sysfs_error) = match sysfs_result {
        Ok(interfaces) => (interfaces, "ok", None),
        Err(error) => {
            let mut interfaces = interfaces_from_addresses(&addresses, &counters);
            sort_and_limit_interfaces(&mut interfaces);
            (
                interfaces,
                "error",
                Some(truncate_string(&error.to_string(), 240)),
            )
        }
    };
    let status = serde_json::json!({
        "type": "network_interfaces",
        "client_id": input.config.client_id,
        "observed_unix": unix_now(),
        "interface_count": interfaces.len(),
        "sysfs_source": {
            "status": sysfs_status,
            "path": sys_class_net,
            "error": sysfs_error,
        },
        "counter_source": {
            "status": if counters.is_empty() { "empty" } else { "ok" },
            "path": proc_root.join("net/dev"),
        },
        "address_source": {
            "status": address_status,
            "source": "getifaddrs",
            "error": address_error,
        },
        "interfaces": interfaces,
    });
    Ok(vec![CommandOutput {
        job_id: input.job_id,
        stream: OutputStream::Status,
        data: serde_json::to_vec(&status)?,
        exit_code: Some(if sysfs_status == "ok" && address_status == "ok" {
            0
        } else {
            1
        }),
        done: true,
    }])
}

fn collect_sysfs_interfaces(
    sys_class_net: &Path,
    counters: &HashMap<String, InterfaceCounters>,
    addresses: &HashMap<String, InterfaceAddressData>,
) -> Result<Vec<NetworkInterfaceSnapshot>> {
    let mut interfaces = Vec::new();
    for entry in std::fs::read_dir(sys_class_net)
        .with_context(|| format!("failed to read {}", sys_class_net.display()))?
    {
        let entry = entry?;
        let name = entry.file_name().to_string_lossy().to_string();
        if !valid_interface_name(&name) {
            continue;
        }
        let interface_path = entry.path();
        let counter = counters.get(&name);
        let mut snapshot = NetworkInterfaceSnapshot {
            name: name.clone(),
            ifindex: read_small_trimmed(&interface_path.join("ifindex"))
                .and_then(|value| value.parse().ok()),
            operstate: read_small_trimmed(&interface_path.join("operstate")),
            mtu: read_small_trimmed(&interface_path.join("mtu"))
                .and_then(|value| value.parse().ok()),
            mac: read_small_trimmed(&interface_path.join("address")),
            link_type: read_small_trimmed(&interface_path.join("type"))
                .and_then(|value| value.parse().ok()),
            flags: addresses
                .get(&name)
                .map(|value| value.flags.clone())
                .unwrap_or_default(),
            addresses: addresses
                .get(&name)
                .map(|value| value.addresses.clone())
                .unwrap_or_default(),
            rx_bytes: counter.map(|value| value.rx_bytes).unwrap_or_default(),
            tx_bytes: counter.map(|value| value.tx_bytes).unwrap_or_default(),
            metadata_sources: vec!["sysfs".to_string()],
        };
        if counter.is_some() {
            snapshot.metadata_sources.push("proc_net_dev".to_string());
        }
        if !snapshot.addresses.is_empty() {
            snapshot.metadata_sources.push("getifaddrs".to_string());
        }
        interfaces.push(snapshot);
    }
    for (name, iface_addresses) in addresses {
        if interfaces.iter().any(|interface| interface.name == *name) {
            continue;
        }
        let counter = counters.get(name);
        interfaces.push(NetworkInterfaceSnapshot {
            name: name.clone(),
            flags: iface_addresses.flags.clone(),
            addresses: iface_addresses.addresses.clone(),
            rx_bytes: counter.map(|value| value.rx_bytes).unwrap_or_default(),
            tx_bytes: counter.map(|value| value.tx_bytes).unwrap_or_default(),
            metadata_sources: vec!["getifaddrs".to_string()],
            ..NetworkInterfaceSnapshot::default()
        });
    }
    sort_and_limit_interfaces(&mut interfaces);
    Ok(interfaces)
}

fn interfaces_from_addresses(
    addresses: &HashMap<String, InterfaceAddressData>,
    counters: &HashMap<String, InterfaceCounters>,
) -> Vec<NetworkInterfaceSnapshot> {
    let mut interfaces = Vec::new();
    for (name, iface_addresses) in addresses {
        if !valid_interface_name(name) {
            continue;
        }
        let counter = counters.get(name);
        interfaces.push(NetworkInterfaceSnapshot {
            name: name.clone(),
            flags: iface_addresses.flags.clone(),
            addresses: iface_addresses.addresses.clone(),
            rx_bytes: counter.map(|value| value.rx_bytes).unwrap_or_default(),
            tx_bytes: counter.map(|value| value.tx_bytes).unwrap_or_default(),
            metadata_sources: vec!["getifaddrs".to_string()],
            ..NetworkInterfaceSnapshot::default()
        });
    }
    interfaces
}

fn sort_and_limit_interfaces(interfaces: &mut Vec<NetworkInterfaceSnapshot>) {
    for interface in interfaces.iter_mut() {
        interface.flags.sort();
        interface.flags.dedup();
        interface.addresses.sort();
        interface.addresses.dedup();
        interface.metadata_sources.sort();
        interface.metadata_sources.dedup();
    }
    interfaces.sort_by(|left, right| left.name.cmp(&right.name));
    interfaces.truncate(MAX_INTERFACE_COUNT);
}

fn interface_addresses_from_getifaddrs() -> Result<HashMap<String, InterfaceAddressData>> {
    let mut addrs: *mut libc::ifaddrs = ptr::null_mut();
    let rc = unsafe { libc::getifaddrs(&mut addrs) };
    if rc != 0 {
        return Err(std::io::Error::last_os_error()).context("getifaddrs failed");
    }
    let _guard = IfAddrsGuard(addrs);
    let mut by_interface = BTreeMap::<String, InterfaceAddressAccumulator>::new();
    let mut cursor = addrs;
    while !cursor.is_null() {
        let ifaddr = unsafe { &*cursor };
        if !ifaddr.ifa_name.is_null() {
            let name = unsafe { CStr::from_ptr(ifaddr.ifa_name) }
                .to_string_lossy()
                .to_string();
            if valid_interface_name(&name) {
                let entry = by_interface.entry(name).or_default();
                merge_flags(entry, ifaddr.ifa_flags);
                if let Some(address) = unsafe {
                    sockaddr_to_interface_address(
                        ifaddr.ifa_addr,
                        ifaddr.ifa_netmask,
                        ifaddr.ifa_flags,
                    )
                } {
                    entry.addresses.insert(address);
                }
            }
        }
        cursor = ifaddr.ifa_next;
    }

    Ok(by_interface
        .into_iter()
        .map(|(name, accumulator)| {
            let mut addresses = accumulator.addresses.into_iter().collect::<Vec<_>>();
            addresses.sort();
            let mut flags = accumulator.flags.into_iter().collect::<Vec<_>>();
            flags.sort();
            (name, InterfaceAddressData { addresses, flags })
        })
        .collect())
}

#[derive(Default)]
struct InterfaceAddressAccumulator {
    addresses: BTreeSet<NetworkInterfaceAddress>,
    flags: BTreeSet<String>,
}

#[derive(Clone, Debug, Default)]
struct InterfaceAddressData {
    addresses: Vec<NetworkInterfaceAddress>,
    flags: Vec<String>,
}

struct IfAddrsGuard(*mut libc::ifaddrs);

impl Drop for IfAddrsGuard {
    fn drop(&mut self) {
        unsafe {
            libc::freeifaddrs(self.0);
        }
    }
}

unsafe fn sockaddr_to_interface_address(
    addr: *const libc::sockaddr,
    netmask: *const libc::sockaddr,
    flags: libc::c_uint,
) -> Option<NetworkInterfaceAddress> {
    if addr.is_null() {
        return None;
    }
    match unsafe { (*addr).sa_family as i32 } {
        libc::AF_INET => {
            let socket_addr = unsafe { &*(addr as *const libc::sockaddr_in) };
            let ip = Ipv4Addr::from(u32::from_be(socket_addr.sin_addr.s_addr));
            Some(NetworkInterfaceAddress {
                family: "inet".to_string(),
                address: ip.to_string(),
                prefix_len: ipv4_prefix_len(netmask),
                scope: Some(ipv4_scope(ip, flags).to_string()),
            })
        }
        libc::AF_INET6 => {
            let socket_addr = unsafe { &*(addr as *const libc::sockaddr_in6) };
            let ip = Ipv6Addr::from(socket_addr.sin6_addr.s6_addr);
            Some(NetworkInterfaceAddress {
                family: "inet6".to_string(),
                address: ip.to_string(),
                prefix_len: ipv6_prefix_len(netmask),
                scope: Some(ipv6_scope(ip, flags).to_string()),
            })
        }
        _ => None,
    }
}

fn merge_flags(accumulator: &mut InterfaceAddressAccumulator, flags: libc::c_uint) {
    for (bit, label) in [
        (libc::IFF_UP, "up"),
        (libc::IFF_BROADCAST, "broadcast"),
        (libc::IFF_LOOPBACK, "loopback"),
        (libc::IFF_POINTOPOINT, "point_to_point"),
        (libc::IFF_RUNNING, "running"),
        (libc::IFF_MULTICAST, "multicast"),
    ] {
        if flags & bit as libc::c_uint != 0 {
            accumulator.flags.insert(label.to_string());
        }
    }
}

fn ipv4_prefix_len(netmask: *const libc::sockaddr) -> Option<u8> {
    if netmask.is_null() {
        return None;
    }
    let socket_addr = unsafe { &*(netmask as *const libc::sockaddr_in) };
    Some(u32::from_be(socket_addr.sin_addr.s_addr).count_ones() as u8)
}

fn ipv6_prefix_len(netmask: *const libc::sockaddr) -> Option<u8> {
    if netmask.is_null() {
        return None;
    }
    let socket_addr = unsafe { &*(netmask as *const libc::sockaddr_in6) };
    Some(
        socket_addr
            .sin6_addr
            .s6_addr
            .iter()
            .map(|byte| byte.count_ones())
            .sum::<u32>() as u8,
    )
}

fn ipv4_scope(ip: Ipv4Addr, flags: libc::c_uint) -> &'static str {
    if flags & libc::IFF_LOOPBACK as libc::c_uint != 0 || ip.is_loopback() {
        "host"
    } else if ip.octets()[0] == 169 && ip.octets()[1] == 254 {
        "link"
    } else {
        "global"
    }
}

fn ipv6_scope(ip: Ipv6Addr, flags: libc::c_uint) -> &'static str {
    if flags & libc::IFF_LOOPBACK as libc::c_uint != 0 || ip.is_loopback() {
        "host"
    } else if ip.is_unicast_link_local() {
        "link"
    } else {
        "global"
    }
}

fn network_counters(proc_root: &Path) -> Result<HashMap<String, InterfaceCounters>> {
    let contents = std::fs::read_to_string(proc_root.join("net/dev"))
        .with_context(|| format!("failed to read {}", proc_root.join("net/dev").display()))?;
    Ok(network_counters_from_proc_net_dev(&contents))
}

fn network_counters_from_proc_net_dev(contents: &str) -> HashMap<String, InterfaceCounters> {
    let mut counters = HashMap::new();
    for line in contents.lines().skip(2) {
        let Some((name, values)) = line.split_once(':') else {
            continue;
        };
        let name = name.trim();
        if !valid_interface_name(name) {
            continue;
        }
        let fields = values.split_whitespace().collect::<Vec<_>>();
        if fields.len() < 16 {
            continue;
        }
        counters.insert(
            name.to_string(),
            InterfaceCounters {
                rx_bytes: fields[0].parse().unwrap_or_default(),
                tx_bytes: fields[8].parse().unwrap_or_default(),
            },
        );
    }
    counters
}

fn read_small_trimmed(path: &Path) -> Option<String> {
    let metadata = std::fs::metadata(path).ok()?;
    if !metadata.is_file() || metadata.len() > READ_SMALL_LIMIT_BYTES {
        return None;
    }
    let value = std::fs::read_to_string(path).ok()?;
    let value = value.trim();
    if value.is_empty() || value.chars().any(char::is_control) {
        None
    } else {
        Some(value.to_string())
    }
}

fn valid_interface_name(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= MAX_INTERFACE_NAME_BYTES
        && !value
            .chars()
            .any(|character| character.is_control() || matches!(character, '/' | '\\' | '\0'))
}

fn unix_now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

fn truncate_string(value: &str, max_chars: usize) -> String {
    value.chars().take(max_chars).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_proc_net_dev_counters() {
        let counters = network_counters_from_proc_net_dev(
            "Inter-| Receive | Transmit\n face |bytes packets errs drop fifo frame compressed multicast|bytes packets errs drop fifo colls carrier compressed\n  lo: 12 0 0 0 0 0 0 0 34 0 0 0 0 0 0 0\neth0: 1000 0 0 0 0 0 0 0 2000 0 0 0 0 0 0 0\n",
        );

        assert_eq!(counters.get("lo").unwrap().rx_bytes, 12);
        assert_eq!(counters.get("lo").unwrap().tx_bytes, 34);
        assert_eq!(counters.get("eth0").unwrap().rx_bytes, 1000);
        assert_eq!(counters.get("eth0").unwrap().tx_bytes, 2000);
    }

    #[test]
    fn validates_interface_names_before_reporting() {
        assert!(valid_interface_name("eth0"));
        assert!(valid_interface_name("wg-east"));
        assert!(!valid_interface_name(""));
        assert!(!valid_interface_name("../eth0"));
        assert!(!valid_interface_name("bad\nname"));
    }
}
