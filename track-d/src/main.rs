// Speed Demon 2026, track D (Traverse). Spec: specs/d-traverse.md in hackathon-raptors/speed-demon-2026.
use std::cmp::Reverse;
use std::collections::{BinaryHeap, HashMap};
use std::io::Write;
use std::sync::atomic::{AtomicU32, AtomicU64, AtomicUsize, Ordering::Relaxed};
use std::{env, fs, thread};

extern "C" {
    fn mmap(a: *mut u8, len: usize, prot: i32, flags: i32, fd: i32, off: i64) -> *mut u8;
    fn madvise(a: *const u8, len: usize, advice: i32) -> i32;
}

fn load(path: &str) -> &'static [u8] {
    use std::os::unix::io::AsRawFd;
    let f = fs::File::open(path).unwrap();
    let n = f.metadata().unwrap().len() as usize;
    if n == 0 {
        return &[];
    }
    unsafe { std::slice::from_raw_parts(mmap(std::ptr::null_mut(), n, 1, 2, f.as_raw_fd(), 0), n) }
}

const NO: u32 = u32::MAX;
const PG: usize = 128; // nodes per label page

// Eight bytes at d[p..], little-endian, zero-padded past the end of d.
fn word(d: &[u8], p: usize) -> u64 {
    if p + 8 <= d.len() {
        return u64::from_le(unsafe { (d.as_ptr().add(p) as *const u64).read_unaligned() });
    }
    let mut b = [0u8; 8];
    b[..d.len() - p].copy_from_slice(&d[p..]);
    u64::from_le_bytes(b)
}

// Reads the decimal number at d[*p..] and steps over it and the one separator after it. Up to eight
// digits at once: a digit byte has bit 4 set, a space, LF or the zero padding has it clear.
fn num(d: &[u8], p: &mut usize) -> u64 {
    let w = word(d, *p);
    let k = (!w & 0x1010_1010_1010_1010).trailing_zeros() as usize / 8;
    let x = (w << (64 - 8 * k)) & 0x0F0F_0F0F_0F0F_0F0F; // the digits, most significant in the low byte
    let x = (x.wrapping_mul(10) + (x >> 8)) & 0x00FF_00FF_00FF_00FF;
    let x = (x.wrapping_mul(100) + (x >> 16)) & 0x0000_FFFF_0000_FFFF;
    let mut x = (x.wrapping_mul(10000) + (x >> 32)) & 0xFFFF_FFFF;
    *p += k;
    while k == 8 && d[*p] >= b'0' {
        x = x * 10 + (d[*p] - b'0') as u64;
        *p += 1;
    }
    *p += 1;
    x
}

// Labels of both halves of a search, paged: .0[v / PG] is a page of .1 or NO (every label unset), .2 the
// free pages, .3 the directory entries in use. A search touches a few hundred pages, so a thread keeps
// that, not n labels.
struct Labels(Vec<u32>, Vec<[[u32; 2]; PG]>, Vec<u32>, Vec<u32>);

impl Labels {
    fn get(&self, v: usize) -> [u32; 2] {
        match self.0[v / PG] {
            NO => [NO; 2],
            p => self.1[p as usize][v % PG],
        }
    }
    fn at(&mut self, v: usize) -> &mut [u32; 2] {
        if self.0[v / PG] == NO {
            let new = |l: &mut Labels| (l.1.push([[NO; 2]; PG]), l.1.len() as u32 - 1).1;
            self.0[v / PG] = self.2.pop().unwrap_or_else(|| new(self));
            self.3.push((v / PG) as u32);
        }
        &mut self.1[self.0[v / PG] as usize][v % PG]
    }
    // After a search: its pages are cleared and go back to the pool (no list of touched nodes is kept).
    fn release(&mut self) {
        for i in self.3.drain(..) {
            let p = std::mem::replace(&mut self.0[i as usize], NO);
            (self.1[p as usize].fill([NO; 2]), self.2.push(p));
        }
    }
}

// A search graph: offsets, edges as head | weight << .3 (the head takes .3 bits; a weight that does not fit
// leaves the field's top value and is kept exact in .2, sorted by edge position). Weights are mostly below
// 2^11 and n is below 2^21 at full scale, so an edge is 4 bytes instead of 8.
struct G(Vec<u32>, Vec<u32>, Vec<(u32, u32)>, u32);

impl G {
    fn row(&self, u: usize) -> std::ops::Range<usize> {
        self.0[u] as usize..self.0[u + 1] as usize
    }
    // The exact weight of edge j, whose field holds the top value.
    fn big(&self, j: usize) -> u32 {
        self.2[self.2.binary_search_by_key(&(j as u32), |b| b.0).unwrap()].1
    }
    // Heads and weights of u's edges.
    fn edges(&self, u: usize) -> impl Iterator<Item = (usize, u64)> + '_ {
        let (r, top, m) = (self.row(u), u32::MAX >> self.3, (1 << self.3) - 1);
        let w = move |j: usize, y: u32| (if y == top { self.big(j) } else { y }) as u64;
        let it = r.clone().zip(&self.1[r]);
        it.map(move |(j, &x)| ((x & m) as usize, w(j, x >> self.3)))
    }
}

// A radix heap of (dist, node): Dijkstra pops distances in order and pushes none below the last pop, so
// bucket i holds the entries whose highest bit differing from that last pop is bit i - 1; a pop from an
// empty bucket 0 moves the lowest non-empty bucket down around its minimum. No comparisons to sift.
struct Heap(u32, [Vec<(u32, u32)>; 33]);

impl Heap {
    fn push(&mut self, k: u32, v: u32) {
        self.1[(32 - (k ^ self.0).leading_zeros()) as usize].push((k, v));
    }
    // The smallest distance (u64::MAX if empty), with bucket 0 refilled.
    fn top(&mut self) -> u64 {
        if self.1[0].is_empty() {
            let Some(i) = (1..33).find(|&i| !self.1[i].is_empty()) else { return u64::MAX };
            let mut b = std::mem::take(&mut self.1[i]);
            self.0 = b.iter().map(|e| e.0).min().unwrap();
            b.drain(..).for_each(|(k, v)| self.push(k, v));
            self.1[i] = b; // keeps its capacity
        }
        self.0 as u64
    }
}

// One half of a bidirectional Dijkstra search: .0 = 0 forward, 1 backward, .1 its heap. Both halves' u32
// labels share one pair per node, so the meeting check at a settled node reads the cache line its own
// label is in.
struct Side(usize, Heap);

impl Side {
    fn top(&mut self) -> u64 {
        self.1.top()
    }
    fn start(&mut self, d: &mut Labels, s: usize) {
        (d.at(s)[self.0] = 0, self.1 .0 = 0, self.1.push(0, s as u32));
    }
    // Settles the nearest node; `best` takes its sum with the other half's label (checking at settle time
    // is enough for the stop rule top + top >= best). False if a sum does not fit in u32 (the weights near
    // 2^32 on the special nodes): the query then goes to `wide`.
    fn step(&mut self, g: &G, d: &mut Labels, best: &mut u64) -> bool {
        let (du, u) = self.1 .1[0].pop().unwrap(); // after top()
        let (du, u, k) = (du as u64, u as usize, self.0);
        let lu = d.get(u);
        if du > lu[k] as u64 {
            return true;
        }
        if lu[1 - k] != NO {
            *best = (*best).min(du + lu[1 - k] as u64);
        }
        for (v, w) in g.edges(u) {
            let nd = du + w;
            if nd >= NO as u64 {
                return false;
            }
            let l = d.at(v); // a missing page means an unset label, which nd improves anyway
            if nd < l[k] as u64 {
                (l[k] = nd as u32, self.1.push(nd as u32, v as u32));
            }
        }
        true
    }
}

// The same search with u64 distances in a hash map, for the few queries whose sums pass 2^32 - 2.
fn wide(g: &G, s: usize, t: usize) -> u64 {
    let mut dist = HashMap::from([(s, 0u64)]);
    let mut heap = BinaryHeap::from([Reverse((0u64, s))]);
    while let Some(Reverse((du, u))) = heap.pop() {
        if u == t {
            return du;
        }
        if du > dist[&u] {
            continue;
        }
        for (v, w) in g.edges(u) {
            let nd = du + w;
            if dist.get(&v).map_or(true, |&x| nd < x) {
                (dist.insert(v, nd), heap.push(Reverse((nd, v))));
            }
        }
    }
    u64::MAX
}

// n zeroed atomics from calloc: no pass over them in one thread (7 MB costs milliseconds on the box).
fn zeroed(n: usize) -> Vec<AtomicU32> {
    unsafe { std::mem::transmute::<Vec<u32>, Vec<AtomicU32>>(vec![0; n]) }
}

// A prefetch of x's cache line; the builds below walk rows in an order the hardware cannot guess.
fn pf<T>(x: &T) {
    unsafe { std::arch::x86_64::_mm_prefetch::<3>(x as *const T as *const i8) };
}

// Runs f(0..nt) on nt threads and returns the results in thread order.
fn par<T: Send>(nt: usize, f: impl Fn(usize) -> T + Sync) -> Vec<T> {
    let f = &f;
    thread::scope(|sc| {
        let hs: Vec<_> = (0..nt).map(|t| sc.spawn(move || f(t))).collect();
        hs.into_iter().map(|h| h.join().unwrap()).collect()
    })
}

// Bits for values below x.
fn bits(x: usize) -> u32 {
    usize::BITS - x.max(2).wrapping_sub(1).leading_zeros()
}

// Packed CSR from the edge lists cut by tail range: part.0[k] holds, for the tails in [k * c, (k + 1) * c),
// c = ceil(n / k_max), each edge as (tail - k * c) | head << ub | weight << (ub + vb), and part.1 the edges
// whose weight does not fit there. Thread k counts, sums and fills its own range.
fn csr(n: usize, parts: &[(Vec<Vec<u64>>, Vec<(u32, u32, u32)>)], vb: u32) -> G {
    let (kk, top) = (parts[0].0.len(), u32::MAX >> vb);
    let (c, ub) = (n.div_ceil(kk), bits(n.div_ceil(kk)));
    let each = |k: usize, f: &mut dyn FnMut(usize, u32, u32)| {
        for &x in parts.iter().flat_map(|p| &p.0[k]) {
            let u = (x & (1 << ub) - 1) as usize;
            f(u, (x >> ub) as u32 & (1 << vb) - 1, (x >> ub >> vb) as u32);
        }
        for &(u, v, w) in parts.iter().flat_map(|p| &p.1) {
            if u as usize / c == k {
                f(u as usize - k * c, v, w);
            }
        }
    };
    let mut sizes = vec![0; kk];
    for p in parts {
        (0..kk).for_each(|k| sizes[k] += p.0[k].len());
        p.1.iter().for_each(|e| sizes[e.0 as usize / c] += 1);
    }
    let mut off = vec![0u32; n + 1];
    let mut adj = Vec::with_capacity(sizes.iter().sum());
    unsafe { adj.set_len(sizes.iter().sum()) }; // every slot is written below
    let big = thread::scope(|sc| {
        let (mut rest, mut base, mut hs) = (&mut adj[..], 0, vec![]);
        for (k, o) in off[1..].chunks_mut(c).enumerate() {
            let (a, r) = std::mem::take(&mut rest).split_at_mut(sizes[k]);
            rest = r;
            hs.push(sc.spawn(move || {
                each(k, &mut |u, _, _| o[u] += 1);
                let mut s = base as u32;
                for x in o.iter_mut() {
                    (s += *x, *x = s); // end of each node
                }
                let mut big = vec![];
                each(k, &mut |u, v, w| {
                    o[u] -= 1; // ends up at the node's start
                    a[o[u] as usize - base] = v | w.min(top) << vb;
                    if w >= top {
                        big.push((o[u], w));
                    }
                });
                big
            }));
            base += sizes[k];
        }
        hs.into_iter().flat_map(|h| h.join().unwrap()).collect::<Vec<_>>()
    });
    off.copy_within(1.., 0); // off[x + 1] held the start of x
    off[n] = adj.len() as u32;
    let mut big = big;
    big.sort_unstable();
    G(off, adj, big, vb)
}

fn main() {
    let arg = |i: usize, def: &str| env::args().nth(i).unwrap_or(def.into());
    let (din, dout) = (arg(1, "/data"), arg(2, "/out"));
    let d = load(&format!("{din}/graph.txt"));
    let mut p = 0;
    let n = (num(d, &mut p) as usize, num(d, &mut p)).0;
    let nt = thread::available_parallelism().map_or(12, |n| n.get());
    let mut bs: Vec<usize> = (0..nt).map(|k| p + k * (d.len() - p) / nt).collect();
    for b in &mut bs[1..] {
        while d[*b - 1] != b'\n' {
            *b += 1; // to a line start
        }
    }
    bs.push(d.len());
    // Edges cut by tail range per thread, then a CSR built range by range: no atomic in either pass.
    let (c, bs, rng) = (n.div_ceil(nt), &bs, |t: usize| t * n / nt..(t + 1) * n / nt);
    let (vb, ub) = (bits(n), bits(c));
    let parts = par(nt, |t| {
        let cap = (bs[t + 1] - bs[t]) / 16 / nt;
        let mut l: Vec<Vec<_>> = (0..nt).map(|_| Vec::with_capacity(cap)).collect();
        let (mut p, e, mut big) = (bs[t], bs[t + 1], vec![]);
        while p < e {
            let (u, v, w) = (num(d, &mut p) as usize, num(d, &mut p), num(d, &mut p));
            if u == v as usize {
                continue; // a self-loop never shortens a path
            }
            match w < 1 << (64 - ub - vb) {
                true => l[u / c].push((u - u / c * c) as u64 | v << ub | w << ub << vb),
                false => big.push((u as u32, v as u32, w as u32)),
            }
        }
        // MADV_DONTNEED on the whole pages of this part of the file: unmapped here in parallel, not at the exit
        let (a, e) = (bs[t].next_multiple_of(4096).min(e), e / 4096 * 4096);
        unsafe { madvise(d.as_ptr().add(a.min(e)), e - a.min(e), 4) };
        (l, big)
    });
    let g0 = csr(n, &parts, vb);
    drop(parts);
    // Renumber in BFS order so that nodes near each other in the graph are near each other in memory:
    // one BFS per thread from its own seed, nodes claimed by CAS, then the leftovers.
    let own = zeroed(n); // 0: unclaimed, else 1 + the claiming thread
    let pre = |x: usize| _ = g0.1.get(g0.0[x] as usize).map(pf); // a row's edges, its offsets are cached
    let claim = |v: usize, t: u32| own[v].compare_exchange(0, t + 1, Relaxed, Relaxed).is_ok();
    let bfs = |r: usize, t: u32, q: &mut Vec<u32>| {
        if !claim(r, t) {
            return;
        }
        let mut h = q.len();
        q.push(r as u32);
        while h < q.len() {
            let u = q[h] as usize;
            h += 1;
            q.get(h + 4).map(|&x| pre(x as usize));
            q.get(h + 2).map(|&x| g0.edges(x as usize).for_each(|(v, _)| pf(&own[v]))); // their claims
            for (v, _) in g0.edges(u) {
                if own[v].load(Relaxed) == 0 && claim(v, t) {
                    (q.push(v as u32), pf(&g0.0[v]));
                }
            }
        }
    };
    let mut lists = par(nt, |t| {
        let mut q = Vec::with_capacity(2 * n / nt);
        (bfs(t * n / nt, t as u32, &mut q), q).1
    });
    // The leftovers (nodes no BFS reached) are found in parallel, then claimed in one thread.
    let free = |r: &usize| own[*r].load(Relaxed) == 0;
    let cand = par(nt, |t| rng(t).filter(free).collect::<Vec<_>>());
    let mut rest = vec![];
    for r in cand.concat() {
        bfs(r, nt as u32, &mut rest);
    }
    lists.push(rest);
    let ord: Vec<u32> = lists.concat();
    drop((lists, own));
    // New id of each node and the degrees in the new order, then the offsets, in parallel by position.
    let mut id = vec![0u32; n]; // calloc: every entry is written below
    let ip = id.as_mut_ptr() as usize;
    let degs = par(nt, |t| {
        let deg = |i: usize| {
            ord.get(i + 16).map(|&o| (pf(&g0.0[o as usize]), pf(unsafe { &*(ip as *const u32).add(o as usize) })));
            let o = ord[i] as usize;
            unsafe { *(ip as *mut u32).add(o) = i as u32 };
            g0.0[o + 1] - g0.0[o]
        };
        rng(t).map(deg).collect::<Vec<u32>>()
    });
    let mut off2 = vec![0u32; n + 1];
    let o2 = off2.as_mut_ptr() as usize;
    let mut base = vec![0u32];
    for c in &degs {
        base.push(base.last().unwrap() + c.iter().sum::<u32>());
    }
    par(nt, |t| {
        let mut a = base[t];
        for (k, &g) in degs[t].iter().enumerate() {
            a += g;
            unsafe { *(o2 as *mut u32).add(t * n / nt + k + 1) = a };
        }
    });
    drop(degs);
    let (m, top) = (g0.1.len(), u32::MAX >> vb);
    let mut adj2: Vec<u32> = Vec::with_capacity(m);
    unsafe { adj2.set_len(m) }; // every slot is written below
    let ap2 = adj2.as_mut_ptr() as usize;
    let big = par(nt, |t| {
        let mut big = vec![];
        for i in rng(t) {
            // Three stages ahead: the offsets of row i + 16, the edges of row i + 8, the new ids of row i + 4.
            ord.get(i + 16).map(|&o| pf(&g0.0[o as usize]));
            ord.get(i + 8).map(|&o| pre(o as usize));
            ord.get(i + 4).map(|&o| g0.edges(o as usize).for_each(|(v, _)| pf(&id[v])));
            for (k, (v, w)) in g0.edges(ord[i] as usize).enumerate() {
                let j = off2[i] as usize + k;
                unsafe { *(ap2 as *mut u32).add(j) = id[v] | (w as u32).min(top) << vb };
                if w >= top as u64 {
                    big.push((j as u32, w as u32));
                }
            }
        }
        big
    });
    drop(g0);
    let g = G(off2, adj2, big.concat(), vb);
    // Reverse CSR for the backward half of the search, in place of the old edge array (already faulted in:
    // fresh pages are costly on the box). In-degrees counted with atomics, their ends as a parallel prefix
    // sum, each edge placed at its head's end minus one: after the renumbering a head is near its tail, so
    // a thread working a range of tails mostly touches counters no other thread does.
    // The heads are counted in one flat pass over a thread's edges; the placement copies the weight field
    // as it is, and looks up an exact weight only for the rare edge whose field holds the top value.
    let (r, mask) = (zeroed(n + 1), (1u32 << vb) - 1);
    let span = |t: usize| &g.1[g.0[t * n / nt] as usize..g.0[(t + 1) * n / nt] as usize];
    let count = |&x: &u32| _ = r[(x & mask) as usize].fetch_add(1, Relaxed);
    par(nt, |t| span(t).iter().for_each(count));
    let sums = par(nt, |t| rng(t).map(|v| r[v].load(Relaxed)).sum::<u32>());
    par(nt, |t| {
        let mut a = sums[..t].iter().sum::<u32>();
        for v in rng(t) {
            a += r[v].load(Relaxed);
            r[v].store(a, Relaxed);
        }
    });
    let mut radj: Vec<u32> = Vec::with_capacity(m);
    unsafe { radj.set_len(m) }; // every slot is written below
    let rp = radj.as_mut_ptr() as usize;
    let mut big = par(nt, |t| {
        let mut big = vec![];
        for u in rng(t) {
            for j in g.row(u) {
                let x = g.1[j];
                let p = r[(x & mask) as usize].fetch_sub(1, Relaxed) - 1;
                unsafe { *(rp as *mut u32).add(p as usize) = u as u32 | x & !mask };
                if x >> vb == top {
                    big.push((p, g.big(j)));
                }
            }
        }
        big
    })
    .concat();
    big.sort_unstable();
    r[n].store(m as u32, Relaxed);
    let roff = r.into_iter().map(AtomicU32::into_inner).collect();
    let rg = G(roff, radj, big, vb);
    drop(ord);
    let q = fs::read(format!("{din}/queries.txt")).unwrap();
    let (mut qs, mut p) = (vec![], 0);
    while p < q.len() {
        let s = id[num(&q, &mut p) as usize] as usize;
        qs.push((s, id[num(&q, &mut p) as usize] as usize));
    }
    drop(id);
    let ans: Vec<AtomicU64> = qs.iter().map(|_| AtomicU64::new(0)).collect();
    // Queries taken in the order of their sources' new ids: searches that run at the same time work on
    // nearby parts of the graph, which the shared cache then holds for all of them.
    let mut qo: Vec<usize> = (0..qs.len()).collect();
    qo.sort_unstable_by_key(|&i| qs[i]);
    let next = AtomicUsize::new(0);
    par(nt, |_| {
        let mut d = Labels(vec![NO; n.div_ceil(PG)], vec![], vec![], vec![]);
        let side = |k| Side(k, Heap(0, std::array::from_fn(|_| vec![])));
        let (mut f, mut b) = (side(0), side(1));
        while let Some(&i) = qo.get(next.fetch_add(1, Relaxed)) {
            let ((s, t), mut best) = (qs[i], u64::MAX); // MAX: unreachable
            if s == t {
                best = 0;
            } else if !rg.row(t).is_empty() && !g.row(s).is_empty() {
                (f.start(&mut d, s), b.start(&mut d, t));
                loop {
                    let (x, y) = (f.top(), b.top());
                    if x.saturating_add(y) >= best {
                        break;
                    }
                    let ok = match x <= y {
                        true => f.step(&g, &mut d, &mut best),
                        false => b.step(&rg, &mut d, &mut best),
                    };
                    if !ok {
                        best = wide(&g, s, t);
                        break;
                    }
                }
                f.1 .1.iter_mut().chain(&mut b.1 .1).for_each(Vec::clear);
                d.release();
            }
            ans[i].store(best, Relaxed);
        }
    });
    let mut out = Vec::with_capacity(qs.len() * 12);
    for a in &ans {
        match a.load(Relaxed) {
            u64::MAX => out.extend_from_slice(b"-1\n"),
            x => writeln!(out, "{x}").unwrap(),
        }
    }
    fs::create_dir_all(&dout).unwrap();
    fs::write(format!("{dout}/distances.txt"), out).unwrap();
}
