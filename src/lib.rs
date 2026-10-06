//! Manually implemented IPv4 longest-prefix routing tables.
use std::{net::Ipv4Addr, str::FromStr};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Prefix {
    network: u32,
    len: u8,
}
impl Prefix {
    pub fn network(self) -> u32 {
        self.network
    }
    pub fn length(self) -> u8 {
        self.len
    }
    pub fn new(network: u32, len: u8) -> Result<Self, String> {
        if len > 32 {
            return Err("prefix length must be 0..32".into());
        }
        Ok(Self {
            network: network & mask(len),
            len,
        })
    }
    pub fn matches(self, ip: u32) -> bool {
        ip & mask(self.len) == self.network
    }
}
fn mask(len: u8) -> u32 {
    if len == 0 { 0 } else { u32::MAX << (32 - len) }
}
fn bit(ip: u32, index: u8) -> usize {
    ((ip >> (31 - index)) & 1) as usize
}
impl FromStr for Prefix {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, String> {
        let (ip, len) = s.split_once('/').ok_or("expected IPv4/length")?;
        Self::new(
            u32::from(ip.parse::<Ipv4Addr>().map_err(|e| e.to_string())?),
            len.parse::<u8>().map_err(|e| e.to_string())?,
        )
    }
}
impl std::fmt::Display for Prefix {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}/{}", Ipv4Addr::from(self.network), self.len)
    }
}
pub type NextHop = u32;
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Route {
    pub prefix: Prefix,
    pub hop: NextHop,
}
#[derive(Default, Debug)]
pub struct Trace {
    pub bits: usize,
    pub comparisons: usize,
    pub branches: usize,
    pub events: Vec<String>,
}
#[derive(Default, Debug)]
pub struct Stats {
    pub nodes: usize,
    pub allocations: usize,
    pub depth: usize,
    pub bit_depth: u8,
    pub bytes: usize,
}
pub trait RoutingTable {
    fn insert(&mut self, prefix: Prefix, hop: NextHop) -> Option<NextHop>;
    fn remove(&mut self, prefix: Prefix) -> Option<NextHop>;
    fn lookup(&self, ip: u32) -> Option<Route>;
    fn trace(&self, ip: u32) -> (Option<Route>, Trace);
    fn stats(&self) -> Stats;
}

#[derive(Default)]
pub struct Linear {
    routes: Vec<Route>,
    allocations: usize,
}
impl Linear {
    fn search<const TRACE: bool>(&self, ip: u32, t: &mut Trace) -> Option<Route> {
        let mut best: Option<Route> = None;
        for &r in &self.routes {
            let matches = r.prefix.matches(ip);
            if TRACE {
                t.bits += r.prefix.len as usize;
                t.comparisons += 1;
                t.events.push(format!("check {}: {}", r.prefix, matches));
            }
            if matches && best.is_none_or(|b| r.prefix.len > b.prefix.len) {
                best = Some(r);
                if TRACE {
                    t.events.push(format!("best {} -> {}", r.prefix, r.hop));
                }
            }
        }
        if TRACE {
            t.events.push("stop: all routes inspected".into());
        }
        best
    }
}
impl RoutingTable for Linear {
    fn insert(&mut self, prefix: Prefix, hop: NextHop) -> Option<NextHop> {
        if let Some(r) = self.routes.iter_mut().find(|r| r.prefix == prefix) {
            return Some(std::mem::replace(&mut r.hop, hop));
        }
        let capacity = self.routes.capacity();
        self.routes.push(Route { prefix, hop });
        if capacity != self.routes.capacity() {
            self.allocations += 1;
        }
        None
    }
    fn remove(&mut self, prefix: Prefix) -> Option<NextHop> {
        let i = self.routes.iter().position(|r| r.prefix == prefix)?;
        Some(self.routes.remove(i).hop)
    }
    fn lookup(&self, ip: u32) -> Option<Route> {
        self.search::<false>(ip, &mut Trace::default())
    }
    fn trace(&self, ip: u32) -> (Option<Route>, Trace) {
        let mut t = Trace::default();
        (self.search::<true>(ip, &mut t), t)
    }
    fn stats(&self) -> Stats {
        Stats {
            allocations: self.allocations,
            bytes: self.routes.capacity() * std::mem::size_of::<Route>(),
            ..Stats::default()
        }
    }
}

struct Node {
    prefix: Prefix,
    hop: Option<NextHop>,
    children: [Option<Box<Node>>; 2],
}
impl Node {
    fn new(prefix: Prefix, hop: Option<NextHop>) -> Box<Self> {
        Box::new(Self {
            prefix,
            hop,
            children: [None, None],
        })
    }
}
/// COMPRESSED=false retains every bit node; true splits and collapses paths.
pub struct Trie<const COMPRESSED: bool> {
    root: Option<Box<Node>>,
    allocations: usize,
}
pub type BinaryTrie = Trie<false>;
pub type PatriciaTrie = Trie<true>;
impl<const C: bool> Default for Trie<C> {
    fn default() -> Self {
        Self {
            root: None,
            allocations: 0,
        }
    }
}
impl<const C: bool> Trie<C> {
    fn insert_at(
        slot: &mut Option<Box<Node>>,
        p: Prefix,
        hop: NextHop,
        depth: u8,
        allocations: &mut usize,
    ) -> Option<NextHop> {
        if slot.is_none() {
            let prefix = if C {
                p
            } else {
                Prefix::new(p.network, depth).unwrap()
            };
            *slot = Some(Node::new(prefix, None));
            *allocations += 1;
        }
        let n = slot.as_mut().unwrap();
        if C {
            let common = (n.prefix.network ^ p.network)
                .leading_zeros()
                .min(n.prefix.len as u32)
                .min(p.len as u32) as u8;
            if common < n.prefix.len {
                let old = slot.take().unwrap();
                let side = bit(old.prefix.network, common);
                let mut parent = Node::new(Prefix::new(p.network, common).unwrap(), None);
                *allocations += 1;
                parent.children[side] = Some(old);
                *slot = Some(parent);
            }
        }
        let n = slot.as_mut().unwrap();
        if n.prefix.len == p.len {
            return n.hop.replace(hop);
        }
        let side = bit(p.network, n.prefix.len);
        Self::insert_at(&mut n.children[side], p, hop, n.prefix.len + 1, allocations)
    }
    fn remove_at(slot: &mut Option<Box<Node>>, p: Prefix) -> Option<NextHop> {
        let n = slot.as_mut()?;
        if n.prefix.len > p.len || !n.prefix.matches(p.network) {
            return None;
        }
        let old = if n.prefix == p {
            n.hop.take()
        } else {
            Self::remove_at(&mut n.children[bit(p.network, n.prefix.len)], p)
        };
        if n.hop.is_none() {
            match (n.children[0].is_some(), n.children[1].is_some()) {
                (false, false) => *slot = None,
                (true, false) if C => *slot = n.children[0].take(),
                (false, true) if C => *slot = n.children[1].take(),
                _ => {}
            }
        }
        old
    }
    fn search<const TRACE: bool>(&self, ip: u32, t: &mut Trace) -> Option<Route> {
        let mut current = self.root.as_deref();
        let mut best = None;
        let mut checked = 0;
        while let Some(n) = current {
            if C {
                // Ancestor and branch bits were checked already. Compare only the new span.
                let span = mask(n.prefix.len) ^ mask(checked);
                let matches = (ip ^ n.prefix.network) & span == 0;
                if TRACE {
                    t.bits += (n.prefix.len - checked) as usize;
                    t.comparisons += 1;
                    t.events.push(format!(
                        "span [{checked}..{}) at {}: {matches}",
                        n.prefix.len, n.prefix
                    ));
                    for i in checked..n.prefix.len {
                        t.events.push(format!(
                            "  bit {i}: address {}, expected {}",
                            bit(ip, i),
                            bit(n.prefix.network, i)
                        ));
                    }
                }
                if !matches {
                    if TRACE {
                        t.events.push("stop: compressed span mismatch".into());
                    }
                    return best;
                }
            }
            if let Some(hop) = n.hop {
                best = Some(Route {
                    prefix: n.prefix,
                    hop,
                });
                if TRACE {
                    t.events.push(format!("best {} -> {hop}", n.prefix));
                }
            }
            if n.prefix.len == 32 {
                if TRACE {
                    t.events.push("stop: /32 reached".into());
                }
                return best;
            }
            let side = bit(ip, n.prefix.len);
            if TRACE {
                t.bits += 1;
                t.branches += 1;
                t.events
                    .push(format!("bit {} = {side}: child {side}", n.prefix.len));
            }
            checked = n.prefix.len + 1;
            current = n.children[side].as_deref();
        }
        if TRACE {
            t.events.push("stop: missing child".into());
        }
        best
    }
    pub fn svg(&self) -> String {
        fn visit(
            n: &Node,
            parent: Option<(usize, usize, u8)>,
            rows: &mut Vec<String>,
            count: &mut usize,
            level: usize,
        ) {
            let x = 30 + level * 190;
            let y = 35 + *count * 60;
            *count += 1;
            if let Some((px, py, len)) = parent {
                let label: String = (len..n.prefix.len)
                    .map(|i| char::from(b'0' + bit(n.prefix.network, i) as u8))
                    .collect();
                rows.push(format!("<path d='M{px},{py} L{x},{y}' stroke='#64748b'/><text x='{}' y='{}' font-size='10'>{label}</text>",px+8,y-14));
            }
            rows.push(format!("<rect x='{x}' y='{}' width='175' height='32' rx='5' fill='#dbeafe'/><text x='{}' y='{}'>{} {}</text>",y-16,x+5,y+4,n.prefix,n.hop.map(|h|format!("→ {h}")).unwrap_or_default()));
            for child in n.children.iter().flatten() {
                visit(
                    child,
                    Some((x + 175, y, n.prefix.len)),
                    rows,
                    count,
                    level + 1,
                );
            }
        }
        let mut rows = Vec::new();
        let mut count = 0;
        if let Some(n) = self.root.as_deref() {
            visit(n, None, &mut rows, &mut count, 0);
        }
        format!(
            "<svg xmlns='http://www.w3.org/2000/svg' width='{}' height='{}' font-family='monospace' font-size='12'>{}</svg>",
            self.stats().depth.max(1) * 190 + 40,
            count * 60 + 40,
            rows.join("\n")
        )
    }
}
impl<const C: bool> RoutingTable for Trie<C> {
    fn insert(&mut self, p: Prefix, h: NextHop) -> Option<NextHop> {
        Self::insert_at(&mut self.root, p, h, 0, &mut self.allocations)
    }
    fn remove(&mut self, p: Prefix) -> Option<NextHop> {
        Self::remove_at(&mut self.root, p)
    }
    fn lookup(&self, ip: u32) -> Option<Route> {
        self.search::<false>(ip, &mut Trace::default())
    }
    fn trace(&self, ip: u32) -> (Option<Route>, Trace) {
        let mut t = Trace::default();
        (self.search::<true>(ip, &mut t), t)
    }
    fn stats(&self) -> Stats {
        fn walk(n: &Node, depth: usize, s: &mut Stats) {
            s.nodes += 1;
            s.depth = s.depth.max(depth);
            s.bit_depth = s.bit_depth.max(n.prefix.len);
            for c in n.children.iter().flatten() {
                walk(c, depth + 1, s);
            }
        }
        let mut s = Stats {
            allocations: self.allocations,
            ..Stats::default()
        };
        if let Some(n) = self.root.as_deref() {
            walk(n, 1, &mut s);
        }
        s.bytes = s.nodes * std::mem::size_of::<Node>();
        s
    }
}

pub fn demo_routes() -> Vec<Route> {
    [
        "10.0.0.0/8",
        "10.16.0.0/12",
        "10.16.32.0/20",
        "10.16.33.0/24",
        "10.16.33.128/25",
        "10.16.33.192/26",
        "10.16.33.200/32",
        "192.168.0.0/16",
    ]
    .iter()
    .enumerate()
    .map(|(i, p)| Route {
        prefix: p.parse().unwrap(),
        hop: i as u32 + 1,
    })
    .collect()
}
pub fn tables() -> Vec<(&'static str, Box<dyn RoutingTable>)> {
    vec![
        ("linear", Box::<Linear>::default()),
        ("binary", Box::<BinaryTrie>::default()),
        ("patricia", Box::<PatriciaTrie>::default()),
    ]
}
pub struct Rng(pub u64);
impl Rng {
    pub fn next_u32(&mut self) -> u32 {
        self.0 = self
            .0
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        (self.0 >> 32) as u32
    }
}
pub fn verify(seed: u64, steps: usize) {
    let mut rng = Rng(seed);
    let mut ts = tables();
    let mut known = Vec::new();
    for step in 0..steps {
        let p = if !known.is_empty() && step % 3 == 0 {
            known[rng.next_u32() as usize % known.len()]
        } else {
            let p = Prefix::new(rng.next_u32(), (rng.next_u32() % 33) as u8).unwrap();
            known.push(p);
            p
        };
        let remove = step % 5 == 0;
        let hop = rng.next_u32();
        let results: Vec<_> = ts
            .iter_mut()
            .map(|(_, t)| {
                if remove {
                    t.remove(p)
                } else {
                    t.insert(p, hop)
                }
            })
            .collect();
        assert!(
            results.windows(2).all(|v| v[0] == v[1]),
            "mutation step {step}"
        );
        for ip in [
            p.network,
            p.network | !mask(p.len),
            rng.next_u32(),
            rng.next_u32(),
        ] {
            let expected = ts[0].1.lookup(ip);
            for (name, t) in &ts {
                assert_eq!(expected, t.lookup(ip), "{name} step {step}");
                assert_eq!(expected, t.trace(ip).0);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn exhaustive_small_address_space() {
        // All prefixes /28..32 under one /28, inserted from leaves toward ancestors.
        let mut ts = tables();
        let base = u32::from(Ipv4Addr::new(10, 0, 0, 0));
        let mut prefixes = Vec::new();
        for len in (28..=32).rev() {
            for suffix in (0..16).step_by(1 << (32 - len)) {
                prefixes.push(Prefix::new(base + suffix, len).unwrap());
            }
        }
        for &p in &prefixes {
            for (_, t) in &mut ts {
                t.insert(p, p.len as u32);
            }
            for ip in base - 1..=base + 16 {
                let expected = ts[0].1.lookup(ip);
                for (_, t) in &ts {
                    assert_eq!(t.lookup(ip), expected);
                }
            }
        }
        for &p in prefixes.iter().rev() {
            for (_, t) in &mut ts {
                t.remove(p);
            }
            for ip in base - 1..=base + 16 {
                let expected = ts[0].1.lookup(ip);
                for (_, t) in &ts {
                    assert_eq!(t.lookup(ip), expected);
                }
            }
        }
        assert_eq!(ts[1].1.stats().nodes, 0);
        assert_eq!(ts[2].1.stats().nodes, 0);
    }
    #[test]
    fn parsing() {
        assert_eq!(
            "10.1.2.3/8".parse::<Prefix>().unwrap().to_string(),
            "10.0.0.0/8"
        );
        for bad in ["10/8", "::1/32", "1.2.3.4/33", "1.2.3.4", "1.2.3.4/-1"] {
            assert!(bad.parse::<Prefix>().is_err());
        }
    }
    #[test]
    fn fallback_and_removal() {
        for (_, mut t) in tables() {
            let ip = u32::from(Ipv4Addr::new(10, 16, 33, 200));
            assert_eq!(t.lookup(ip), None);
            for r in demo_routes() {
                assert_eq!(t.insert(r.prefix, r.hop), None);
            }
            let p = "10.16.33.200/32".parse().unwrap();
            assert_eq!(t.insert(p, 99), Some(7));
            assert_eq!(t.lookup(ip).unwrap().hop, 99);
            assert_eq!(t.remove(p), Some(99));
            assert_eq!(t.lookup(ip).unwrap().hop, 6);
            assert_eq!(t.remove(p), None);
            assert_eq!(t.lookup(0), None);
            t.insert("0.0.0.0/0".parse().unwrap(), 100);
            assert_eq!(t.lookup(0).unwrap().hop, 100);
            for r in demo_routes() {
                t.remove(r.prefix);
            }
            t.remove("0.0.0.0/0".parse().unwrap());
            assert_eq!(t.lookup(ip), None);
        }
    }
    #[test]
    fn split_mismatch_and_collapse() {
        let mut t = PatriciaTrie::default();
        let a = "10.0.0.0/8".parse().unwrap();
        let b = "11.0.0.0/8".parse().unwrap();
        t.insert(a, 1);
        t.insert(b, 2);
        assert_eq!(t.stats().nodes, 3);
        assert_eq!(t.lookup(0), None);
        t.remove(b);
        assert_eq!(t.stats().nodes, 1);
        t.remove(a);
        assert_eq!(t.stats().nodes, 0);
    }
    #[test]
    fn seeded_mutations() {
        for seed in [0, 1, 42, 123456] {
            verify(seed, 1500);
        }
    }
}
