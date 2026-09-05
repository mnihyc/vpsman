use super::*;
use std::io::Write;

fn docker(args: &[&str], input: Option<&[u8]>) -> String {
    let mut child = std::process::Command::new("docker")
        .args(args)
        .stdin(if input.is_some() {
            Stdio::piped()
        } else {
            Stdio::null()
        })
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    if let Some(input) = input {
        child.stdin.take().unwrap().write_all(input).unwrap();
    }
    let output = child.wait_with_output().unwrap();
    assert!(
        output.status.success(),
        "docker {:?}: {} {}",
        args,
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).unwrap()
}

struct Fixture {
    network: String,
    container: String,
    target: String,
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::process::Command::new("docker")
            .args(["rm", "-f", &self.container, &self.target])
            .output();
        let _ = std::process::Command::new("docker")
            .args(["network", "rm", &self.network])
            .output();
    }
}

const SERVER: &str = r#"
import socket, sys, threading, time
report_peer = sys.argv[1:] == ['peer']
def serve(family, kind, port):
    sock = socket.socket(family, kind)
    if family == socket.AF_INET6: sock.setsockopt(socket.IPPROTO_IPV6, socket.IPV6_V6ONLY, 1)
    sock.bind(('0.0.0.0' if family == socket.AF_INET else '::', port))
    if kind == socket.SOCK_STREAM: sock.listen()
    def loop():
        while True:
            if kind == socket.SOCK_STREAM:
                connection, address = sock.accept()
                connection.recv(128)
                response = str(port) + (' ' + address[0] if report_peer else '')
                connection.sendall(response.encode())
                connection.close()
            else:
                _, address = sock.recvfrom(128)
                response = str(port) + (' ' + address[0] if report_peer else '')
                sock.sendto(response.encode(), address)
    threading.Thread(target=loop, daemon=True).start()
for family in (socket.AF_INET, socket.AF_INET6):
    for kind in (socket.SOCK_STREAM, socket.SOCK_DGRAM):
        for port in (18080, 18081, 18082, 18100, 18110): serve(family, kind, port)
open('/tmp/ready', 'w').close()
while True: time.sleep(3600)
"#;

const CLIENT: &str = r#"
import ipaddress, socket, sys
host, mode = sys.argv[1:]
for family in (socket.AF_INET, socket.AF_INET6):
    address = ('127.0.0.1' if family == socket.AF_INET else '::1') if mode == 'output' else socket.getaddrinfo(host, None, family)[0][4][0]
    for kind in (socket.SOCK_STREAM, socket.SOCK_DGRAM):
        for incoming, target in ((17080,18080), (17081,18081), (17082,18082), (18100,18100)):
            sock = socket.socket(family, kind)
            sock.settimeout(3)
            sock.connect((address, incoming))
            sock.send(b'test')
            assert sock.recv(128) == str(target).encode(), (family, kind, incoming, target)
            sock.close()
for family in (socket.AF_INET, socket.AF_INET6):
    address = socket.getaddrinfo(host, None, family)[0][4][0]
    for kind in (socket.SOCK_STREAM, socket.SOCK_DGRAM):
        for incoming, target in ((17280,18080), (17281,18081), (17282,18082), (18110,18110)):
            sock = socket.socket(family, kind)
            sock.settimeout(3)
            sock.connect((address, incoming))
            sock.send(b'dnat')
            received_port, peer = sock.recv(128).decode().split()
            assert int(received_port) == target, (family, kind, incoming, target)
            assert ipaddress.ip_address(peer) == ipaddress.ip_address(address), ('masquerade', family, kind, incoming, peer, address)
            sock.close()
print('TCP/UDP IPv4/IPv6 fixed, shifted, identity REDIRECT and remote DNAT with masquerade passed:', mode)
"#;

#[test]
#[ignore = "requires Docker and VPSMAN_PORT_FORWARD_TEST_IMAGE with nftables/python3"]
fn redirect_and_dnat_forward_real_packets_in_isolated_network() {
    let image = std::env::var("VPSMAN_PORT_FORWARD_TEST_IMAGE")
        .expect("set image containing nftables and python3");
    let id = uuid::Uuid::new_v4().simple().to_string();
    let fixture = Fixture {
        network: format!("vpsman-pf-test-{id}"),
        container: format!("vpsman-pf-server-{id}"),
        target: format!("vpsman-pf-target-{id}"),
    };
    let subnet = format!("fd00:{}:{}::/64", &id[..4], &id[4..8]);
    docker(
        &[
            "network",
            "create",
            "--ipv6",
            "--subnet",
            &subnet,
            &fixture.network,
        ],
        None,
    );
    docker(
        &[
            "run",
            "-d",
            "--name",
            &fixture.container,
            "--network",
            &fixture.network,
            "--cap-add",
            "NET_ADMIN",
            "--sysctl",
            "net.ipv4.ip_forward=1",
            "--sysctl",
            "net.ipv6.conf.all.forwarding=1",
            &image,
            "python3",
            "-u",
            "-c",
            SERVER,
        ],
        None,
    );
    docker(
        &[
            "run",
            "-d",
            "--name",
            &fixture.target,
            "--network",
            &fixture.network,
            &image,
            "python3",
            "-u",
            "-c",
            SERVER,
            "peer",
        ],
        None,
    );
    for container in [&fixture.container, &fixture.target] {
        docker(&["exec", container, "python3", "-c", "import os,time; deadline=time.monotonic()+5\nwhile not os.path.exists('/tmp/ready'):\n assert time.monotonic()<deadline\n time.sleep(.01)"], None);
    }
    let ip = docker(
        &[
            "inspect",
            "--format",
            "{{range .NetworkSettings.Networks}}{{.IPAddress}}{{end}}",
            &fixture.target,
        ],
        None,
    );
    let ipv6 = docker(
        &[
            "inspect",
            "--format",
            "{{range .NetworkSettings.Networks}}{{.GlobalIPv6Address}}{{end}}",
            &fixture.target,
        ],
        None,
    );
    let mut config = config();
    config.schema_version = 2;
    config.rules[0].mode = PortForwardMode::Redirect;
    config.rules[0].target_ip = None;
    config.rules[0].address_family = Some(PortForwardAddressFamily::Both);
    config.rules[0].mappings =
        pair_port_expressions("17080,17081-17082,18100", "18080,18081-18082,18100").unwrap();
    config.rules[0].masquerade = false;
    let mut dnat = config.rules[0].clone();
    dnat.id = uuid::Uuid::new_v4();
    dnat.mode = PortForwardMode::Dnat;
    dnat.address_family = None;
    dnat.target_ip = Some(ip.trim().parse().unwrap());
    dnat.mappings =
        pair_port_expressions("17280,17281-17282,18110", "18080,18081-18082,18110").unwrap();
    dnat.masquerade = true;
    let mut dnat_ipv6 = dnat.clone();
    dnat_ipv6.id = uuid::Uuid::new_v4();
    dnat_ipv6.target_ip = Some(ipv6.trim().parse().unwrap());
    config.rules.push(dnat);
    config.rules.push(dnat_ipv6);
    config.desired_hash = port_forwarding_desired_hash(&config.rules);
    let probe = render_capability_probe_script().unwrap();
    docker(
        &[
            "exec",
            "-i",
            &fixture.container,
            "nft",
            "--check",
            "--file",
            "-",
        ],
        Some(probe.as_bytes()),
    );
    let program = render_apply_script(&config, false).unwrap();
    docker(
        &[
            "exec",
            "-i",
            &fixture.container,
            "nft",
            "--check",
            "--file",
            "-",
        ],
        Some(program.as_bytes()),
    );
    docker(
        &["exec", "-i", &fixture.container, "nft", "--file", "-"],
        Some(program.as_bytes()),
    );
    let output = docker(
        &[
            "run",
            "--rm",
            "--network",
            &fixture.network,
            &image,
            "python3",
            "-c",
            CLIENT,
            &fixture.container,
            "prerouting",
        ],
        None,
    );
    assert!(output.contains("passed: prerouting"));
    println!("{output}");
    let output = docker(
        &[
            "exec",
            &fixture.container,
            "python3",
            "-c",
            CLIENT,
            &fixture.container,
            "output",
        ],
        None,
    );
    assert!(output.contains("passed: output"));
    println!("{output}");
}
