# patricia-ip-prefix

IPv4 longest-prefix matching in Rust, implemented manually with a linear routing
 table, an ordinary binary trie, and a compressed Patricia (radix) trie. No external
 dependencies. The Cargo package is `patricia-ip-prefix`; the GitHub repository
 retains its original name `practicia-ip-prefix`.

## Run

Install a Rust toolchain supporting edition 2024, then:

```sh
cargo test
cargo run --release -- demo
cargo run --release -- trace --ip 10.16.33.200
cargo run --release -- verify --seed 42
cargo run --release -- bench --seed 42 --out artifacts
```

`demo` prints decision traces, demonstrates replacement and removal fallback, and
writes `artifacts/trie.svg`. `bench` writes that diagram plus `lookup-cost.svg`,
`throughput.svg`, and `results.csv`. Open SVGs in a browser. Output directories are
created automatically. Generated artifacts and Cargo build files are gitignored.
`--help` lists the commands. Invalid IPv4 addresses, lengths, options, and commands
produce errors and a nonzero exit status.

## Routing behavior

`Prefix::new` and CIDR parsing validate lengths 0 through 32 and normalize host
bits: `10.1.2.3/8` becomes `10.0.0.0/8`. IPv6 is intentionally unsupported.
`RoutingTable::insert` inserts or replaces an exact prefix, returning its previous
numeric next hop. `remove` removes only an exact prefix. Lookup returns both the
winning prefix and next hop, or `None` when nothing matches. `/0` is the default
route; `/32` is an individual host. Next-hop numbers are opaque demonstration IDs.

The demo contains overlapping routes from /8 through /32. For `10.16.33.200`,
`10.16.33.200/32` wins. Replacing that route changes its next hop; removing it
falls back to `10.16.33.192/26`. `192.168.0.0/16` adds a separate branch.

## How the structures differ

- **Linear:** scans every route and keeps the longest match. Lookup O(N), vector
  storage O(N); exact-prefix mutations also scan the vector.
- **Binary:** starts at /0 and stores each bit position on inserted paths. Lookup
  takes at most 32 branch decisions; storage can approach 32 nodes per route.
- **Patricia:** stores route endpoints and branching points. An insertion splits
  at the common prefix; removal collapses route-free single-child nodes. Lookup
  validates compressed spans and remembers the last matching route. A child may
  skip many bit positions, so its branch bit alone does not establish a match.

The two trie types share mutation and traversal machinery through a const generic;
the uncompressed specialization retains all intermediate nodes. Both are manually
implemented, with no collection library implementing trie behavior for them.
Patricia lookup still has a worst-case bound proportional to the 32-bit address
width. Compression reduces nodes and pointer traversals, not that width bound.
Route-bearing single-child nodes cannot be removed: they are fallback routes.

A hash map supports exact-prefix keys, but an address lookup does not know the
winning prefix length. One alternative is probing all 33 possible lengths. Tries
instead organize the prefix relationships directly; this project measures their
tradeoffs rather than claiming one structure always wins on every workload.

## Measurement contract

Benchmarks use 16, 64, 256, and 1,024 unique prefixes in three deterministic shapes:
chains of nested /8..32 prefixes, dispersed networks, and shared 10/8 networks.
Each size has 4,096 queries mixing addresses within routes, addresses differing in
the last prefix bit, and random addresses. A near miss can still match another
route. All implementations receive identical routes and queries. Dataset generation
is seeded and occurs outside timing. Every query result is checked against linear
lookup before timing. Timings use five samples and report medians; each throughput
sample runs for at least 20ms. Results depend on the machine and build configuration.

CSV fields:

| Field | Meaning |
| --- | --- |
| nodes | Live heap trie nodes; zero for the linear vector |
| allocations | Cumulative Box allocations; vector capacity growth events for linear |
| depth | Maximum number of trie nodes on a root-to-leaf path; zero for linear |
| bit_depth | Deepest stored prefix length; zero for linear |
| estimated_bytes | Live node payload bytes, or vector capacity times route size |
| build_ns | Median time inserting the pre-generated routes into an empty table |
| bits | Mean logical bits examined, including repeated linear prefix checks |
| prefix_comparisons | Mean masked prefix/span checks |
| branches | Mean address-bit branch decisions |
| lookups_per_sec | Median uninstrumented lookup throughput |

Memory excludes allocator metadata, allocation rounding, stack table headers,
traces, and benchmark data. Counts describe allocations requested by these data
structures, not global allocator instrumentation. The linear implementation has
entries rather than trie nodes; its memory estimate includes spare vector capacity.

A compressed span uses a single masked integer comparison, while its logical bit
count includes the span width. Already validated parent and branch bits are not
counted again. Zero-width spans count as comparisons in the implementation. Traces
list each bit in a compressed span for explanation; this does not mean the fast
lookup performs separate comparisons per bit. Binary traversal uses branch bits
without prefix comparisons. These separate counters avoid equating a 24-bit masked
comparison with 24 processor instructions.

Timing uses an uninstrumented const-generic lookup specialization, `black_box`,
and no trace allocation. Parsing, validation, SVG rendering, and operation counting
are excluded. Lookup-cost curves show logical bits, prefix comparisons, and branch
decisions separately. Their horizontal axis is logarithmic in route count; vertical
axes are linear and independently scaled per panel.

## Validation and layout

`src/lib.rs` contains prefix parsing, routing implementations, traces, statistics,
SVG trie rendering, deterministic verification, and tests. `src/main.rs` contains
the CLI, workloads, benchmark runner, and SVG curves.

Tests cover normalization, invalid input, no match, /0, /32, replacement, fallback,
branch splitting, compressed-span mismatch, node collapse, and four seeded mutation
runs. `verify` checks 10,000 insert/replace/remove operations and 40,000 boundary and
random address queries, including agreement between traced and fast lookup.

```sh
cargo fmt --check
cargo clippy --all-targets -- -D warnings
cargo test
```
