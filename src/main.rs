use patricia_ip_prefix::*;
use std::{env, fs, hint::black_box, net::Ipv4Addr, path::Path, time::Instant};

fn main() {
    if let Err(e) = run() {
        eprintln!("error: {e}");
        std::process::exit(1);
    }
}
fn run() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<_> = env::args().skip(1).collect();
    let command = args.first().map(String::as_str).unwrap_or("demo");
    let mut seed = 42;
    let mut out = "artifacts".to_string();
    let mut ip = "10.16.33.200".parse::<Ipv4Addr>()?;
    let mut i = 1;
    while i < args.len() {
        let value = args.get(i + 1).ok_or("option requires a value")?;
        match args[i].as_str() {
            "--seed" => seed = value.parse()?,
            "--out" => out = value.clone(),
            "--ip" => ip = value.parse()?,
            other => return Err(format!("unknown option {other}").into()),
        }
        i += 2;
    }
    match command {
        "demo" | "trace" => {
            let mut ts = tables();
            for (_, t) in &mut ts {
                for r in demo_routes() {
                    t.insert(r.prefix, r.hop);
                }
            }
            let expected = ts[0].1.lookup(ip.into());
            for (name, t) in &ts {
                let (result, trace) = t.trace(ip.into());
                assert_eq!(result, expected);
                println!(
                    "\n{name}: {ip} -> {result:?}\n  {:?}\n  bits={}, prefix comparisons={}, branches={}",
                    t.stats(),
                    trace.bits,
                    trace.comparisons,
                    trace.branches
                );
                for event in trace.events {
                    println!("  {event}");
                }
            }
            if command == "demo" {
                let p = "10.16.33.200/32".parse()?;
                for (name, t) in &mut ts {
                    assert_eq!(t.insert(p, 99), Some(7));
                    assert_eq!(t.remove(p), Some(99));
                    println!(
                        "{name}: replaced then removed {p}; fallback {:?}",
                        t.lookup(u32::from(Ipv4Addr::new(10, 16, 33, 200)))
                    );
                }
                fs::create_dir_all(&out)?;
                let mut t = PatriciaTrie::default();
                for r in demo_routes() {
                    t.insert(r.prefix, r.hop);
                }
                fs::write(Path::new(&out).join("trie.svg"), t.svg())?;
                println!("Diagram: {out}/trie.svg");
            }
        }
        "verify" => {
            verify(seed, 10_000);
            println!(
                "Verified 10,000 mutations and 40,000 queries across all tables (seed {seed})."
            );
        }
        "bench" => benchmark(seed, Path::new(&out))?,
        "help" | "--help" | "-h" => println!(
            "patricia-ip-prefix [demo|trace|verify|bench] [--ip IPv4] [--seed integer] [--out directory]"
        ),
        _ => return Err(format!("unknown command {command}; use --help").into()),
    }
    Ok(())
}

struct Measurement {
    shape: &'static str,
    name: &'static str,
    n: usize,
    bits: f64,
    comparisons: f64,
    branches: f64,
    throughput: f64,
}
fn dataset(shape: &str, n: usize, rng: &mut Rng) -> Vec<Route> {
    let mut routes = Vec::new();
    while routes.len() < n {
        let ip = rng.next_u32();
        let (ip, len) = match shape {
            "shared" => (
                (ip & 0x00ff_ffff) | 0x0a00_0000,
                16 + (rng.next_u32() % 17) as u8,
            ),
            "nested" => {
                // Chains /8..32 per independently selected /8 anchor.
                let group = routes.len() / 25;
                let len = 8 + (routes.len() % 25) as u8;
                (((group as u32) << 24) | 0x0010_21c8, len)
            }
            _ => (ip, 8 + (rng.next_u32() % 25) as u8),
        };
        let p = Prefix::new(ip, len).unwrap();
        if !routes.iter().any(|r: &Route| r.prefix == p) {
            routes.push(Route {
                prefix: p,
                hop: routes.len() as u32,
            });
        }
    }
    routes
}
fn median(values: &mut [f64]) -> f64 {
    values.sort_by(f64::total_cmp);
    values[values.len() / 2]
}
fn benchmark(seed: u64, out: &Path) -> Result<(), Box<dyn std::error::Error>> {
    if cfg!(debug_assertions) {
        eprintln!("For meaningful timings, use cargo run --release -- bench");
    }
    fs::create_dir_all(out)?;
    let mut csv = String::from(
        "shape,routes,implementation,nodes,allocations,depth,bit_depth,estimated_bytes,build_ns,bits,prefix_comparisons,branches,lookups_per_sec\n",
    );
    let mut measurements = Vec::new();
    let mut rng = Rng(seed);
    for shape in ["nested", "dispersed", "shared"] {
        for n in [16, 64, 256, 1024] {
            let routes = dataset(shape, n, &mut rng);
            let queries: Vec<_> = (0..4096)
                .map(|i| {
                    let r = routes[rng.next_u32() as usize % n];
                    let random = rng.next_u32();
                    match i % 3 {
                        0 => {
                            r.prefix.network()
                                | (random
                                    & if r.prefix.length() == 32 {
                                        0
                                    } else {
                                        u32::MAX >> r.prefix.length()
                                    })
                        }
                        1 => r.prefix.network() ^ (1u32 << (32 - r.prefix.length())),
                        _ => random,
                    }
                })
                .collect();
            let mut ts = tables();
            for (_, t) in &mut ts {
                for r in &routes {
                    t.insert(r.prefix, r.hop);
                }
            }
            let expected: Vec<_> = queries.iter().map(|&ip| ts[0].1.lookup(ip)).collect();
            for (index, (name, t)) in ts.iter().enumerate() {
                let mut totals = (0, 0, 0);
                for (&ip, &expected) in queries.iter().zip(&expected) {
                    assert_eq!(t.lookup(ip), expected, "benchmark {name}");
                    let (result, trace) = t.trace(ip);
                    assert_eq!(result, expected);
                    totals.0 += trace.bits;
                    totals.1 += trace.comparisons;
                    totals.2 += trace.branches;
                }
                let mut builds = Vec::new();
                let mut times = Vec::new();
                for _ in 0..5 {
                    let mut fresh = tables().swap_remove(index).1;
                    let start = Instant::now();
                    for r in &routes {
                        black_box(fresh.insert(r.prefix, r.hop));
                    }
                    builds.push(start.elapsed().as_nanos() as f64);
                    black_box(&fresh);
                    // At least 20ms per sample reduces clock granularity effects.
                    let start = Instant::now();
                    let mut count = 0;
                    loop {
                        for &ip in &queries {
                            black_box(t.lookup(black_box(ip)));
                        }
                        count += queries.len();
                        if start.elapsed().as_millis() >= 20 {
                            break;
                        }
                    }
                    times.push(count as f64 / start.elapsed().as_secs_f64());
                }
                let s = t.stats();
                let build = median(&mut builds);
                let throughput = median(&mut times);
                let q = queries.len() as f64;
                let m = Measurement {
                    shape,
                    name,
                    n,
                    bits: totals.0 as f64 / q,
                    comparisons: totals.1 as f64 / q,
                    branches: totals.2 as f64 / q,
                    throughput,
                };
                csv.push_str(&format!("{shape},{n},{name},{},{},{},{},{},{build:.0},{:.3},{:.3},{:.3},{throughput:.0}\n",s.nodes,s.allocations,s.depth,s.bit_depth,s.bytes,m.bits,m.comparisons,m.branches));
                println!(
                    "{shape:9} {n:4} {name:8}: {:5} nodes, {:9.0} lookups/sec",
                    s.nodes, throughput
                );
                measurements.push(m);
            }
        }
    }
    fs::write(out.join("results.csv"), csv)?;
    fs::write(out.join("lookup-cost.svg"), chart(&measurements, false))?;
    fs::write(out.join("throughput.svg"), chart(&measurements, true))?;
    let mut t = PatriciaTrie::default();
    for r in demo_routes() {
        t.insert(r.prefix, r.hop);
    }
    fs::write(out.join("trie.svg"), t.svg())?;
    println!("Wrote results and SVGs to {} (seed {seed}).", out.display());
    Ok(())
}
fn chart(ms: &[Measurement], throughput: bool) -> String {
    let rows = if throughput { 1 } else { 3 };
    let mut s = format!(
        "<svg xmlns='http://www.w3.org/2000/svg' width='1200' height='{}' viewBox='0 0 1200 {}' font-family='sans-serif'><rect width='100%' height='100%' fill='white'/>",
        rows * 310 + 55,
        rows * 310 + 55
    );
    for row in 0..rows {
        for (column, shape) in ["nested", "dispersed", "shared"].iter().enumerate() {
            let value = |m: &Measurement| {
                if throughput {
                    m.throughput / 1e6
                } else {
                    match row {
                        0 => m.bits,
                        1 => m.comparisons,
                        _ => m.branches,
                    }
                }
            };
            let title = if throughput {
                "million lookups/sec"
            } else {
                match row {
                    0 => "logical bits inspected",
                    1 => "prefix comparisons",
                    _ => "branch decisions",
                }
            };
            let max = ms
                .iter()
                .filter(|m| m.shape == *shape)
                .map(value)
                .fold(1f64, f64::max)
                * 1.1;
            let x = column * 400 + 65;
            let y = row * 310 + 40;
            s.push_str(&format!("<text x='{x}' y='{}' font-size='14'>{shape}: {title}</text><path d='M{x},{y} v210 h300' fill='none' stroke='#334155'/>",y-15));
            for tick in 0..=4 {
                let ty = y + 210 - tick * 210 / 4;
                s.push_str(&format!(
                    "<text x='{}' y='{ty}' text-anchor='end' font-size='10'>{:.1}</text>",
                    x - 6,
                    max * tick as f64 / 4.
                ));
            }
            for (name, color) in [
                ("linear", "#dc2626"),
                ("binary", "#2563eb"),
                ("patricia", "#059669"),
            ] {
                let points: Vec<_> = ms
                    .iter()
                    .filter(|m| m.shape == *shape && m.name == name)
                    .map(|m| {
                        format!(
                            "{},{}",
                            x + ((m.n as f64 / 16.).log2() * 50.) as usize,
                            y + 210 - (value(m) / max * 210.) as usize
                        )
                    })
                    .collect();
                s.push_str(&format!(
                    "<polyline points='{}' fill='none' stroke='{color}' stroke-width='2'/>",
                    points.join(" ")
                ));
            }
            for (i, n) in [16, 64, 256, 1024].iter().enumerate() {
                s.push_str(&format!(
                    "<text x='{}' y='{}' text-anchor='middle' font-size='11'>{n}</text>",
                    x + i * 100,
                    y + 230
                ));
            }
            s.push_str(&format!(
                "<text x='{}' y='{}' font-size='11'>routes (logarithmic spacing)</text>",
                x + 65,
                y + 250
            ));
        }
    }
    for (i, (name, color)) in [
        ("linear", "#dc2626"),
        ("binary", "#2563eb"),
        ("patricia", "#059669"),
    ]
    .iter()
    .enumerate()
    {
        s.push_str(&format!(
            "<text x='{}' y='{}' fill='{color}'>{name}</text>",
            60 + i * 150,
            rows * 310 + 30
        ));
    }
    s.push_str("</svg>");
    s
}
