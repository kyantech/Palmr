use std::borrow::Cow;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};

use http::header::{HeaderMap, HeaderName, FORWARDED};
use ipnet::IpNet;

use crate::config::TrustProxy;

const X_FORWARDED_FOR: HeaderName = HeaderName::from_static("x-forwarded-for");
const X_FORWARDED_PROTO: HeaderName = HeaderName::from_static("x-forwarded-proto");

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ForwardedProto {
    Http,
    Https,
}

impl ForwardedProto {
    fn from_token(token: &str) -> Option<Self> {
        if token.eq_ignore_ascii_case("http") {
            Some(Self::Http)
        } else if token.eq_ignore_ascii_case("https") {
            Some(Self::Https)
        } else {
            None
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ResolvedClient {
    ip: IpAddr,
    via_trusted_proxy: bool,
    forwarded_proto: Option<ForwardedProto>,
}

impl ResolvedClient {
    pub const fn ip(&self) -> IpAddr {
        self.ip
    }

    pub const fn via_trusted_proxy(&self) -> bool {
        self.via_trusted_proxy
    }

    /// Informational only: generated URLs and cookie flags come from the configured public base URL.
    pub const fn forwarded_proto(&self) -> Option<ForwardedProto> {
        self.forwarded_proto
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TrustedProxies {
    networks: Vec<IpNet>,
}

impl TrustedProxies {
    pub fn new(config: &TrustProxy) -> Self {
        let networks = match config {
            TrustProxy::Off => Vec::new(),
            TrustProxy::AllowList(networks) => networks.clone(),
        };
        Self { networks }
    }

    pub fn is_trusted(&self, addr: IpAddr) -> bool {
        let addr = addr.to_canonical();
        self.networks.iter().any(|network| network.contains(&addr))
    }

    pub fn resolve(&self, peer: IpAddr, headers: &HeaderMap) -> ResolvedClient {
        let peer = peer.to_canonical();
        if !self.is_trusted(peer) {
            return ResolvedClient {
                ip: peer,
                via_trusted_proxy: false,
                forwarded_proto: None,
            };
        }

        let (walk, forwarded_proto) = match forwarded_elements(headers) {
            Ok(elements) => {
                let addresses: Vec<_> = elements.iter().map(|element| element.address).collect();
                let walk = self.walk(&addresses);
                let proto = walk
                    .outermost_hop()
                    .and_then(|index| elements.get(index))
                    .and_then(|element| element.proto);
                (walk, proto)
            }
            Err(Malformed) => match x_forwarded_for(headers) {
                Some(addresses) => {
                    let walk = self.walk(&addresses);
                    let proto = walk
                        .outermost_hop()
                        .and_then(|index| x_forwarded_proto(headers, addresses.len() - index));
                    (walk, proto)
                }
                None => (Walk::Unresolvable, None),
            },
        };

        ResolvedClient {
            ip: walk.client().unwrap_or(peer),
            via_trusted_proxy: true,
            forwarded_proto,
        }
    }

    // Walking inward from the trusted peer means every entry a client prepended sits
    // behind the first untrusted hop and can never be selected.
    fn walk(&self, chain: &[Option<IpAddr>]) -> Walk {
        for (index, entry) in chain.iter().enumerate().rev() {
            match entry {
                Some(ip) if self.is_trusted(*ip) => {}
                Some(ip) => return Walk::Client { index, ip: *ip },
                None => return Walk::Unresolvable,
            }
        }
        Walk::AllTrusted
    }
}

enum Walk {
    Client { index: usize, ip: IpAddr },
    AllTrusted,
    Unresolvable,
}

impl Walk {
    const fn client(&self) -> Option<IpAddr> {
        match self {
            Self::Client { ip, .. } => Some(*ip),
            Self::AllTrusted | Self::Unresolvable => None,
        }
    }

    const fn outermost_hop(&self) -> Option<usize> {
        match self {
            Self::Client { index, .. } => Some(*index),
            Self::AllTrusted => Some(0),
            Self::Unresolvable => None,
        }
    }
}

fn header_list<'a>(headers: &'a HeaderMap, name: &HeaderName) -> Option<Vec<&'a str>> {
    let mut entries = Vec::new();
    for value in headers.get_all(name) {
        entries.extend(value.to_str().ok()?.split(',').map(trim_ows));
    }
    Some(entries)
}

fn trim_ows(text: &str) -> &str {
    text.trim_matches([' ', '\t'])
}

fn x_forwarded_for(headers: &HeaderMap) -> Option<Vec<Option<IpAddr>>> {
    let entries = header_list(headers, &X_FORWARDED_FOR)?;
    Some(
        entries
            .into_iter()
            .map(|entry| entry.parse::<IpAddr>().ok().map(|ip| ip.to_canonical()))
            .collect(),
    )
}

fn x_forwarded_proto(headers: &HeaderMap, trusted_hops: usize) -> Option<ForwardedProto> {
    let entries = header_list(headers, &X_FORWARDED_PROTO)?;
    let outermost = entries.len().checked_sub(1)?;
    let index = entries.len().saturating_sub(trusted_hops).min(outermost);
    ForwardedProto::from_token(entries[index])
}

struct Malformed;

#[derive(Debug, Clone, Copy)]
struct ForwardedElement {
    address: Option<IpAddr>,
    proto: Option<ForwardedProto>,
}

fn forwarded_elements(headers: &HeaderMap) -> Result<Vec<ForwardedElement>, Malformed> {
    let mut elements = Vec::new();
    for value in headers.get_all(FORWARDED) {
        let value = value.to_str().map_err(|_| Malformed)?;
        parse_forwarded_line(value, &mut elements)?;
    }
    if elements.is_empty() {
        return Err(Malformed);
    }
    Ok(elements)
}

fn parse_forwarded_line(
    value: &str,
    elements: &mut Vec<ForwardedElement>,
) -> Result<(), Malformed> {
    let mut scanner = Scanner::new(value);
    loop {
        scanner.skip_ows();
        if scanner.at_end() {
            return Ok(());
        }
        if scanner.eat(b',') {
            continue;
        }
        elements.push(parse_forwarded_element(&mut scanner)?);
        scanner.skip_ows();
        if !scanner.at_end() && !scanner.eat(b',') {
            return Err(Malformed);
        }
    }
}

fn parse_forwarded_element(scanner: &mut Scanner<'_>) -> Result<ForwardedElement, Malformed> {
    let mut element = ForwardedElement {
        address: None,
        proto: None,
    };
    let mut seen: Vec<&str> = Vec::new();
    loop {
        let name = scanner.token()?;
        if seen.iter().any(|other| other.eq_ignore_ascii_case(name)) {
            return Err(Malformed);
        }
        seen.push(name);
        if !scanner.eat(b'=') {
            return Err(Malformed);
        }
        let value = if scanner.peek() == Some(b'"') {
            Cow::Owned(scanner.quoted_string()?)
        } else {
            Cow::Borrowed(scanner.token()?)
        };
        if name.eq_ignore_ascii_case("for") {
            element.address = parse_node(&value)?;
        } else if name.eq_ignore_ascii_case("proto") {
            element.proto = ForwardedProto::from_token(&value);
        }
        scanner.skip_ows();
        if !scanner.eat(b';') {
            return Ok(element);
        }
        scanner.skip_ows();
    }
}

fn parse_node(node: &str) -> Result<Option<IpAddr>, Malformed> {
    if let Some(bracketed) = node.strip_prefix('[') {
        let (address, port) = bracketed.split_once(']').ok_or(Malformed)?;
        if !port.is_empty() {
            validate_port(port.strip_prefix(':').ok_or(Malformed)?)?;
        }
        let address = address.parse::<Ipv6Addr>().map_err(|_| Malformed)?;
        return Ok(Some(IpAddr::V6(address).to_canonical()));
    }

    let name = match node.split_once(':') {
        Some((name, port)) => {
            validate_port(port)?;
            name
        }
        None => node,
    };
    if name.eq_ignore_ascii_case("unknown") || is_obfuscated(name) {
        return Ok(None);
    }
    let address = name.parse::<Ipv4Addr>().map_err(|_| Malformed)?;
    Ok(Some(IpAddr::V4(address)))
}

fn validate_port(port: &str) -> Result<(), Malformed> {
    let numeric = (1..=5).contains(&port.len()) && port.bytes().all(|byte| byte.is_ascii_digit());
    if numeric || is_obfuscated(port) {
        Ok(())
    } else {
        Err(Malformed)
    }
}

fn is_obfuscated(identifier: &str) -> bool {
    identifier.strip_prefix('_').is_some_and(|rest| {
        !rest.is_empty()
            && rest
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-'))
    })
}

struct Scanner<'a> {
    text: &'a str,
    position: usize,
}

impl<'a> Scanner<'a> {
    const fn new(text: &'a str) -> Self {
        Self { text, position: 0 }
    }

    fn peek(&self) -> Option<u8> {
        self.text.as_bytes().get(self.position).copied()
    }

    fn at_end(&self) -> bool {
        self.position >= self.text.len()
    }

    fn eat(&mut self, expected: u8) -> bool {
        let matched = self.peek() == Some(expected);
        if matched {
            self.position += 1;
        }
        matched
    }

    fn skip_ows(&mut self) {
        while matches!(self.peek(), Some(b' ' | b'\t')) {
            self.position += 1;
        }
    }

    fn token(&mut self) -> Result<&'a str, Malformed> {
        let start = self.position;
        while self.peek().is_some_and(is_tchar) {
            self.position += 1;
        }
        if self.position == start {
            return Err(Malformed);
        }
        Ok(&self.text[start..self.position])
    }

    fn quoted_string(&mut self) -> Result<String, Malformed> {
        if !self.eat(b'"') {
            return Err(Malformed);
        }
        let mut value = String::new();
        loop {
            match self.peek().ok_or(Malformed)? {
                b'"' => {
                    self.position += 1;
                    return Ok(value);
                }
                b'\\' => {
                    self.position += 1;
                    value.push(char::from(self.peek().ok_or(Malformed)?));
                }
                byte => value.push(char::from(byte)),
            }
            self.position += 1;
        }
    }
}

const fn is_tchar(byte: u8) -> bool {
    byte.is_ascii_alphanumeric()
        || matches!(
            byte,
            b'!' | b'#'
                | b'$'
                | b'%'
                | b'&'
                | b'\''
                | b'*'
                | b'+'
                | b'-'
                | b'.'
                | b'^'
                | b'_'
                | b'`'
                | b'|'
                | b'~'
        )
}

#[cfg(test)]
mod tests {
    use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};

    use http::header::{HeaderMap, HeaderName, HeaderValue};
    use proptest::prelude::*;
    use rstest::rstest;

    use super::{ForwardedProto, ResolvedClient, TrustedProxies};
    use crate::config::{EnvironmentSource, OperatorConfig, TrustProxy};

    fn allow_list(value: &str) -> TrustedProxies {
        TrustedProxies::new(&TrustProxy::AllowList(
            value
                .split(',')
                .map(|entry| entry.parse().unwrap())
                .collect(),
        ))
    }

    fn trusted() -> TrustedProxies {
        allow_list("10.0.0.0/8,fd00::/8,198.51.100.7/32")
    }

    fn ip(text: &str) -> IpAddr {
        text.parse().unwrap()
    }

    fn headers(pairs: &[(&str, &str)]) -> HeaderMap {
        let mut map = HeaderMap::new();
        for (name, value) in pairs {
            map.append(
                HeaderName::from_bytes(name.as_bytes()).unwrap(),
                HeaderValue::from_str(value).unwrap(),
            );
        }
        map
    }

    fn resolve(peer: &str, pairs: &[(&str, &str)]) -> ResolvedClient {
        trusted().resolve(ip(peer), &headers(pairs))
    }

    #[rstest]
    #[case::ipv4_inside_cidr("10.1.2.3", true)]
    #[case::ipv4_cidr_lower_edge("10.0.0.0", true)]
    #[case::ipv4_cidr_upper_edge("10.255.255.255", true)]
    #[case::ipv4_just_outside_cidr("11.0.0.0", false)]
    #[case::ipv4_single_host("198.51.100.7", true)]
    #[case::ipv4_single_host_neighbour("198.51.100.8", false)]
    #[case::ipv6_inside_cidr("fd12:3456::1", true)]
    #[case::ipv6_outside_cidr("fe80::1", false)]
    #[case::ipv4_mapped_ipv6_inside_cidr("::ffff:10.0.0.2", true)]
    #[case::loopback_not_implicitly_trusted("127.0.0.1", false)]
    #[case::ipv6_loopback_not_implicitly_trusted("::1", false)]
    #[case::private_range_not_implicitly_trusted("192.168.1.1", false)]
    #[case::docker_bridge_not_implicitly_trusted("172.17.0.1", false)]
    fn unit_trusted_proxies_cidr_membership(#[case] addr: &str, #[case] expected: bool) {
        assert_eq!(trusted().is_trusted(ip(addr)), expected);
    }

    #[rstest]
    #[case::direct_connection("203.0.113.9", &[], "203.0.113.9")]
    #[case::untrusted_peer_ignores_x_forwarded_for(
        "203.0.113.9",
        &[("x-forwarded-for", "10.0.0.1")],
        "203.0.113.9"
    )]
    #[case::untrusted_peer_ignores_forwarded(
        "203.0.113.9",
        &[("forwarded", "for=10.0.0.1")],
        "203.0.113.9"
    )]
    #[case::trusted_peer_without_headers("10.0.0.2", &[], "10.0.0.2")]
    #[case::one_trusted_proxy("10.0.0.2", &[("x-forwarded-for", "203.0.113.7")], "203.0.113.7")]
    #[case::multiple_trusted_proxies(
        "10.0.0.3",
        &[("x-forwarded-for", "203.0.113.7, 10.0.0.2, 198.51.100.7")],
        "203.0.113.7"
    )]
    #[case::client_prepended_spoof(
        "10.0.0.2",
        &[("x-forwarded-for", "198.51.100.200, 203.0.113.7")],
        "203.0.113.7"
    )]
    #[case::client_prepended_trusted_looking_spoof(
        "10.0.0.2",
        &[("x-forwarded-for", "10.0.0.5, 203.0.113.7")],
        "203.0.113.7"
    )]
    #[case::untrusted_middle_hop_stops_the_walk(
        "10.0.0.3",
        &[("x-forwarded-for", "203.0.113.7, 198.51.100.50, 10.0.0.2")],
        "198.51.100.50"
    )]
    #[case::separate_header_lines_form_one_list(
        "10.0.0.2",
        &[("x-forwarded-for", "198.51.100.200"), ("x-forwarded-for", "203.0.113.7")],
        "203.0.113.7"
    )]
    #[case::whitespace_around_entries(
        "10.0.0.2",
        &[("x-forwarded-for", " 198.51.100.200 ,\t203.0.113.7 ")],
        "203.0.113.7"
    )]
    #[case::trusted_ipv6_proxy("fd00::2", &[("x-forwarded-for", "2001:db8::7")], "2001:db8::7")]
    #[case::ipv4_mapped_trusted_peer(
        "::ffff:10.0.0.2",
        &[("x-forwarded-for", "203.0.113.7")],
        "203.0.113.7"
    )]
    #[case::ipv4_mapped_client_is_canonicalized(
        "10.0.0.2",
        &[("x-forwarded-for", "::ffff:203.0.113.7")],
        "203.0.113.7"
    )]
    #[case::trusted_single_host_proxy(
        "198.51.100.7",
        &[("x-forwarded-for", "203.0.113.7")],
        "203.0.113.7"
    )]
    #[case::neighbour_of_single_host_is_untrusted(
        "198.51.100.8",
        &[("x-forwarded-for", "203.0.113.7")],
        "198.51.100.8"
    )]
    #[case::every_entry_trusted_falls_back_to_peer(
        "10.0.0.2",
        &[("x-forwarded-for", "10.0.0.1")],
        "10.0.0.2"
    )]
    #[case::x_forwarded_for_empty_value("10.0.0.2", &[("x-forwarded-for", "")], "10.0.0.2")]
    #[case::x_forwarded_for_malformed_nearest_entry(
        "10.0.0.2",
        &[("x-forwarded-for", "203.0.113.7, not-an-ip")],
        "10.0.0.2"
    )]
    #[case::x_forwarded_for_trailing_empty_entry(
        "10.0.0.2",
        &[("x-forwarded-for", "203.0.113.7, ")],
        "10.0.0.2"
    )]
    #[case::x_forwarded_for_entry_with_port(
        "10.0.0.2",
        &[("x-forwarded-for", "203.0.113.7:8080")],
        "10.0.0.2"
    )]
    #[case::x_forwarded_for_bracketed_ipv6(
        "10.0.0.2",
        &[("x-forwarded-for", "[2001:db8::7]")],
        "10.0.0.2"
    )]
    #[case::x_forwarded_for_malformed_entry_left_of_client_is_irrelevant(
        "10.0.0.2",
        &[("x-forwarded-for", "garbage, 203.0.113.7")],
        "203.0.113.7"
    )]
    #[case::forwarded_one_hop("10.0.0.2", &[("forwarded", "for=203.0.113.7;proto=https")], "203.0.113.7")]
    #[case::forwarded_multiple_hops(
        "10.0.0.3",
        &[("forwarded", "for=203.0.113.7, for=10.0.0.2")],
        "203.0.113.7"
    )]
    #[case::forwarded_client_prepended_spoof(
        "10.0.0.2",
        &[("forwarded", "for=198.51.100.200, for=203.0.113.7")],
        "203.0.113.7"
    )]
    #[case::forwarded_separate_header_lines(
        "10.0.0.3",
        &[("forwarded", "for=203.0.113.7"), ("forwarded", "for=10.0.0.2")],
        "203.0.113.7"
    )]
    #[case::forwarded_quoted_ipv6_with_port(
        "fd00::2",
        &[("forwarded", "for=\"[2001:db8::7]:4711\"")],
        "2001:db8::7"
    )]
    #[case::forwarded_quoted_ipv4_with_port(
        "10.0.0.2",
        &[("forwarded", "for=\"203.0.113.7:8080\"")],
        "203.0.113.7"
    )]
    #[case::forwarded_quoted_plain_ipv4("10.0.0.2", &[("forwarded", "for=\"203.0.113.7\"")], "203.0.113.7")]
    #[case::forwarded_parameter_names_are_case_insensitive(
        "10.0.0.2",
        &[("forwarded", "For=203.0.113.7;PROTO=https")],
        "203.0.113.7"
    )]
    #[case::forwarded_extra_parameters_ignored(
        "10.0.0.2",
        &[("forwarded", "by=10.0.0.2;for=203.0.113.7;host=example.test;ext=\"a;b,c\"")],
        "203.0.113.7"
    )]
    #[case::forwarded_empty_list_elements_skipped(
        "10.0.0.3",
        &[("forwarded", ", for=203.0.113.7 ,, for=10.0.0.2,")],
        "203.0.113.7"
    )]
    #[case::forwarded_preferred_over_x_forwarded_for(
        "10.0.0.2",
        &[("forwarded", "for=203.0.113.7"), ("x-forwarded-for", "203.0.113.8")],
        "203.0.113.7"
    )]
    #[case::forwarded_unknown_node("10.0.0.2", &[("forwarded", "for=unknown")], "10.0.0.2")]
    #[case::forwarded_obfuscated_node("10.0.0.2", &[("forwarded", "for=_hidden")], "10.0.0.2")]
    #[case::forwarded_element_without_for("10.0.0.2", &[("forwarded", "proto=https")], "10.0.0.2")]
    #[case::forwarded_malformed_falls_back_to_x_forwarded_for(
        "10.0.0.2",
        &[("forwarded", "for=not-an-ip"), ("x-forwarded-for", "203.0.113.8")],
        "203.0.113.8"
    )]
    #[case::forwarded_malformed_never_partially_trusted(
        "10.0.0.2",
        &[("forwarded", "for=203.0.113.7, for=garbage")],
        "10.0.0.2"
    )]
    #[case::forwarded_unquoted_ipv6("10.0.0.2", &[("forwarded", "for=2001:db8::7")], "10.0.0.2")]
    #[case::forwarded_bracketed_ipv4("10.0.0.2", &[("forwarded", "for=\"[203.0.113.7]\"")], "10.0.0.2")]
    #[case::forwarded_duplicate_parameter(
        "10.0.0.2",
        &[("forwarded", "for=203.0.113.7;for=203.0.113.8")],
        "10.0.0.2"
    )]
    #[case::forwarded_unterminated_quote("10.0.0.2", &[("forwarded", "for=\"203.0.113.7")], "10.0.0.2")]
    #[case::forwarded_missing_value("10.0.0.2", &[("forwarded", "for=")], "10.0.0.2")]
    #[case::forwarded_bad_port("10.0.0.2", &[("forwarded", "for=\"203.0.113.7:123456\"")], "10.0.0.2")]
    #[case::forwarded_missing_separator(
        "10.0.0.2",
        &[("forwarded", "for=203.0.113.7 for=203.0.113.8")],
        "10.0.0.2"
    )]
    #[case::forwarded_empty_value("10.0.0.2", &[("forwarded", "")], "10.0.0.2")]
    fn unit_forwarded_right_to_left_trusted_hops(
        #[case] peer: &str,
        #[case] pairs: &[(&str, &str)],
        #[case] expected: &str,
    ) {
        let resolved = resolve(peer, pairs);
        assert_eq!(resolved.ip(), ip(expected));
        assert_eq!(resolved.via_trusted_proxy(), trusted().is_trusted(ip(peer)));
    }

    #[test]
    fn unit_non_ascii_forwarding_headers_are_not_trusted() {
        let mut map = HeaderMap::new();
        map.insert(
            HeaderName::from_static("x-forwarded-for"),
            HeaderValue::from_bytes(b"203.0.113.7, 198.51.100.\xff").unwrap(),
        );
        map.insert(
            HeaderName::from_static("x-forwarded-proto"),
            HeaderValue::from_bytes(b"https\xff").unwrap(),
        );
        let resolved = trusted().resolve(ip("10.0.0.2"), &map);
        assert_eq!(resolved.ip(), ip("10.0.0.2"));
        assert_eq!(resolved.forwarded_proto(), None);

        map.insert(
            HeaderName::from_static("forwarded"),
            HeaderValue::from_bytes(b"for=\"203.0.113.9\xff\"").unwrap(),
        );
        map.insert(
            HeaderName::from_static("x-forwarded-for"),
            HeaderValue::from_static("203.0.113.8"),
        );
        assert_eq!(
            trusted().resolve(ip("10.0.0.2"), &map).ip(),
            ip("203.0.113.8")
        );
    }

    #[rstest]
    #[case::single_hop(&[("x-forwarded-for", "203.0.113.7"), ("x-forwarded-proto", "https")], Some(ForwardedProto::Https))]
    #[case::single_hop_http(&[("x-forwarded-for", "203.0.113.7"), ("x-forwarded-proto", "http")], Some(ForwardedProto::Http))]
    #[case::chained_https_https(
        &[("x-forwarded-for", "203.0.113.7, 10.0.0.2"), ("x-forwarded-proto", "https, https")],
        Some(ForwardedProto::Https)
    )]
    #[case::chained_takes_outermost_trusted_value(
        &[("x-forwarded-for", "203.0.113.7, 10.0.0.2"), ("x-forwarded-proto", "http, https")],
        Some(ForwardedProto::Http)
    )]
    #[case::chained_on_separate_lines(
        &[
            ("x-forwarded-for", "203.0.113.7, 10.0.0.2"),
            ("x-forwarded-proto", "https"),
            ("x-forwarded-proto", "http"),
        ],
        Some(ForwardedProto::Https)
    )]
    #[case::client_prepended_proto_ignored(
        &[("x-forwarded-for", "198.51.100.200, 203.0.113.7"), ("x-forwarded-proto", "http, https")],
        Some(ForwardedProto::Https)
    )]
    #[case::overwritten_by_inner_proxy(
        &[("x-forwarded-for", "203.0.113.7, 10.0.0.2"), ("x-forwarded-proto", "https")],
        Some(ForwardedProto::Https)
    )]
    #[case::whitespace_trimmed(
        &[("x-forwarded-for", "203.0.113.7"), ("x-forwarded-proto", "\thttps ")],
        Some(ForwardedProto::Https)
    )]
    #[case::case_normalized(&[("x-forwarded-for", "203.0.113.7"), ("x-forwarded-proto", "HTTPS")], Some(ForwardedProto::Https))]
    #[case::invalid_selected_token(
        &[("x-forwarded-for", "203.0.113.7"), ("x-forwarded-proto", "https, ftp")],
        None
    )]
    #[case::invalid_token_with_suffix(&[("x-forwarded-for", "203.0.113.7"), ("x-forwarded-proto", "https;")], None)]
    #[case::invalid_token_scheme_like(&[("x-forwarded-for", "203.0.113.7"), ("x-forwarded-proto", "https://")], None)]
    #[case::empty_value(&[("x-forwarded-for", "203.0.113.7"), ("x-forwarded-proto", "")], None)]
    #[case::missing_proto(&[("x-forwarded-for", "203.0.113.7")], None)]
    #[case::unresolvable_client(
        &[("x-forwarded-for", "not-an-ip"), ("x-forwarded-proto", "https")],
        None
    )]
    #[case::forwarded_proto_of_selected_element(
        &[("forwarded", "for=203.0.113.7;proto=https, for=10.0.0.2;proto=http")],
        Some(ForwardedProto::Https)
    )]
    #[case::forwarded_quoted_proto(&[("forwarded", "for=203.0.113.7;proto=\"https\"")], Some(ForwardedProto::Https))]
    #[case::forwarded_invalid_proto(&[("forwarded", "for=203.0.113.7;proto=ftp")], None)]
    #[case::forwarded_quoted_chained_proto(&[("forwarded", "for=203.0.113.7;proto=\"https, https\"")], None)]
    #[case::forwarded_ignores_x_forwarded_proto(
        &[("forwarded", "for=203.0.113.7"), ("x-forwarded-proto", "https")],
        None
    )]
    fn unit_chained_forwarded_proto(
        #[case] pairs: &[(&str, &str)],
        #[case] expected: Option<ForwardedProto>,
    ) {
        assert_eq!(resolve("10.0.0.3", pairs).forwarded_proto(), expected);
    }

    #[test]
    fn unit_untrusted_peer_has_no_forwarded_proto() {
        let resolved = resolve(
            "203.0.113.9",
            &[
                ("x-forwarded-for", "203.0.113.7"),
                ("x-forwarded-proto", "https"),
            ],
        );
        assert_eq!(resolved.forwarded_proto(), None);
        assert!(!resolved.via_trusted_proxy());
    }

    #[test]
    fn unit_trust_proxy_off_ignores_forwarding_headers() {
        let off = TrustedProxies::new(&TrustProxy::Off);
        for peer in ["203.0.113.9", "10.0.0.2", "127.0.0.1", "::1", "172.17.0.1"] {
            let resolved = off.resolve(
                ip(peer),
                &headers(&[
                    ("x-forwarded-for", "10.0.0.1"),
                    ("forwarded", "for=10.0.0.1;proto=https"),
                    ("x-forwarded-proto", "https"),
                    ("x-real-ip", "10.0.0.1"),
                ]),
            );
            assert_eq!(
                resolved,
                ResolvedClient {
                    ip: ip(peer),
                    via_trusted_proxy: false,
                    forwarded_proto: None,
                }
            );
        }
    }

    #[test]
    fn unit_host_and_origin_headers_do_not_affect_resolution() {
        let base = [
            ("x-forwarded-for", "203.0.113.7"),
            ("x-forwarded-proto", "https"),
        ];
        let hostile = [
            ("x-forwarded-for", "203.0.113.7"),
            ("x-forwarded-proto", "https"),
            ("host", "evil.test"),
            ("x-forwarded-host", "evil.test"),
            ("origin", "https://evil.test"),
            ("referer", "https://evil.test/reset"),
            ("x-real-ip", "198.51.100.200"),
        ];
        assert_eq!(resolve("10.0.0.2", &hostile), resolve("10.0.0.2", &base));
    }

    #[test]
    fn it_trust_proxy_has_no_all_mode() {
        let loaded =
            OperatorConfig::load(&EnvironmentSource::from_vars(Vec::<(String, String)>::new()))
                .unwrap();
        assert_eq!(loaded.config.trust_proxy, TrustProxy::Off);
        let proxies = TrustedProxies::new(&loaded.config.trust_proxy);
        let socket_peer = ip("192.0.2.44");
        let resolved = proxies.resolve(
            socket_peer,
            &headers(&[
                ("x-forwarded-for", "10.0.0.1"),
                ("forwarded", "for=10.0.0.1"),
            ]),
        );
        assert_eq!(resolved.ip(), socket_peer);
        assert!(!resolved.via_trusted_proxy());

        for value in ["all", "0.0.0.0/0", "::/0"] {
            let source = EnvironmentSource::from_vars([("PALMR_TRUST_PROXY", value)]);
            assert!(
                OperatorConfig::load(&source).is_err(),
                "{value} must be rejected"
            );
        }
    }

    fn trusted_hop() -> impl Strategy<Value = IpAddr> {
        prop_oneof![
            any::<[u8; 3]>().prop_map(|[a, b, c]| IpAddr::from([10, a, b, c])),
            any::<u128>()
                .prop_map(|bits| IpAddr::from(Ipv6Addr::from((0xfd_u128 << 120) | (bits >> 8)))),
        ]
    }

    fn any_ip() -> impl Strategy<Value = IpAddr> {
        prop_oneof![
            any::<u32>().prop_map(|bits| IpAddr::from(Ipv4Addr::from(bits))),
            any::<u128>().prop_map(|bits| IpAddr::from(Ipv6Addr::from(bits))),
        ]
    }

    fn untrusted_client() -> impl Strategy<Value = IpAddr> {
        any_ip().prop_filter("client must not be a trusted hop", |addr| {
            !trusted().is_trusted(*addr)
        })
    }

    fn forwarded_node(addr: &IpAddr) -> String {
        match addr {
            IpAddr::V4(v4) => format!("for={v4}"),
            IpAddr::V6(v6) => format!("for=\"[{v6}]\""),
        }
    }

    fn hostile_text() -> impl Strategy<Value = String> {
        let fragments = prop::sample::select(vec![
            "for=",
            "proto=",
            "by=",
            ";",
            ",",
            " ",
            "\t",
            "\"",
            "\\",
            "[",
            "]",
            ":",
            "=",
            "_x",
            "unknown",
            "https",
            "http",
            "10.0.0.1",
            "203.0.113.7",
            "2001:db8::1",
            "fd00::1",
            "::ffff:10.0.0.1",
            "65536",
            "x",
        ]);
        prop_oneof![
            prop::collection::vec(fragments, 0..16).prop_map(|parts| parts.concat()),
            prop::collection::vec(0x20_u8..0x7f, 0..48)
                .prop_map(|bytes| bytes.into_iter().map(char::from).collect()),
        ]
    }

    proptest! {
        #[test]
        fn prop_client_prepended_entries_never_win(
            spoofed in prop::collection::vec(any_ip(), 0..6),
            client in untrusted_client(),
            hops in prop::collection::vec(trusted_hop(), 0..4),
            peer in trusted_hop(),
            use_forwarded in any::<bool>(),
        ) {
            let chain: Vec<IpAddr> = spoofed.iter().chain([&client]).chain(&hops).copied().collect();
            let header = if use_forwarded {
                let value = chain.iter().map(forwarded_node).collect::<Vec<_>>().join(", ");
                headers(&[("forwarded", &value)])
            } else {
                let value = chain.iter().map(ToString::to_string).collect::<Vec<_>>().join(", ");
                headers(&[("x-forwarded-for", &value)])
            };
            let resolved = trusted().resolve(peer, &header);
            prop_assert_eq!(resolved.ip(), client.to_canonical());
            prop_assert!(resolved.via_trusted_proxy());
        }

        #[test]
        fn prop_trust_proxy_off_uses_socket_peer(
            peer in any_ip(),
            forwarded in hostile_text(),
            x_forwarded_for in hostile_text(),
            x_forwarded_proto in hostile_text(),
        ) {
            let header = headers(&[
                ("forwarded", &forwarded),
                ("x-forwarded-for", &x_forwarded_for),
                ("x-forwarded-proto", &x_forwarded_proto),
            ]);
            let resolved = TrustedProxies::new(&TrustProxy::Off).resolve(peer, &header);
            prop_assert_eq!(resolved.ip(), peer.to_canonical());
            prop_assert!(!resolved.via_trusted_proxy());
            prop_assert_eq!(resolved.forwarded_proto(), None);
        }

        #[test]
        fn prop_hostile_headers_never_select_a_trusted_hop_as_client(
            peer in trusted_hop(),
            forwarded in hostile_text(),
            x_forwarded_for in hostile_text(),
            x_forwarded_proto in hostile_text(),
        ) {
            let header = headers(&[
                ("forwarded", &forwarded),
                ("x-forwarded-for", &x_forwarded_for),
                ("x-forwarded-proto", &x_forwarded_proto),
            ]);
            let resolved = trusted().resolve(peer, &header);
            prop_assert!(resolved.ip() == peer.to_canonical() || !trusted().is_trusted(resolved.ip()));
            prop_assert!(resolved.via_trusted_proxy());
        }
    }
}
