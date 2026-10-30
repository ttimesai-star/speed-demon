// Speed Demon 2026, track D (Traverse). Spec: specs/d-traverse.md in hackathon-raptors/speed-demon-2026.
use std::cmp::Reverse;
use std::collections::{BinaryHeap, HashMap};
use std::io::Write;
use std::sync::atomic::{AtomicU32, AtomicU64, AtomicUsize, Ordering::Relaxed};
use std::{env, fs, thread};

fn load(path: &str) -> &'static [u8] {
    use std::os::unix::io::AsRawFd;
    extern "C" {
        fn mmap(a: *mut u8, len: usize, prot: i32, flags: i32, fd: i32, off: i64) -> *mut u8;
    }
    let f = fs::File::open(path).unwrap();
    let n = f.metadata().unwrap().len() as usize;
    if n == 0 {
        return &[];
    }
    unsafe { std::slice::from_raw_parts(mmap(std::ptr::null_mut(), n, 1, 2, f.as_raw_fd(), 0), n) }
}


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

// Calls f(u, v, w) for every edge line in d[a..b] that is not a self-loop.
fn edges(d: &[u8], a: usize, b: usize, mut f: impl FnMut(usize, usize, u32)) {
    let mut p = a;
    while p < b {
        let (u, v, w) = (num(d, &mut p), num(d, &mut p), num(d, &mut p));
        if u != v {
            f(u as usize, v as usize, w as u32);
        }
    }
}

// One half (k = 0 forward, 1 backward) of a bidirectional Dijkstra search: touched nodes and a heap of
// dist << 32 | node. Both halves' u32 labels share one array of pairs, so the meeting check at a settled
// node reads the cache line its own label is in.
struct Side {
    k: usize,
    seen: Vec<u32>,
    h: BinaryHeap<Reverse<u64>>,
}

impl Side {
    fn top(&self) -> u64 {
        self.h.peek().map_or(u64::MAX, |r| r.0 >> 32)
    }
    fn start(&mut self, d: &mut [[u32; 2]], s: usize) {
        (d[s][self.k] = 0, self.seen.push(s as u32), self.h.push(Reverse(s as u64)));
    }
    // Settles the nearest node; `best` takes its sum with the other half's label (checking at settle time
    // is enough for the stop rule top + top >= best). False if a sum does
    // not fit in u32 (the weights near 2^32 on the special nodes): the query then goes to `wide`.
    fn step(&mut self, off: &[u32], adj: &[(u32, u32)], d: &mut [[u32; 2]], best: &mut u64) -> bool {
        let Reverse(e) = self.h.pop().unwrap();
        let (du, u, k) = (e >> 32, e as u32 as usize, self.k);
        if du > d[u][k] as u64 {
            return true;
        }
        if d[u][1 - k] != u32::MAX {
            *best = (*best).min(du + d[u][1 - k] as u64);
        }
        for &(v, w) in &adj[off[u] as usize..off[u + 1] as usize] {
            let (v, nd) = (v as usize, du + w as u64);
            if nd >= u32::MAX as u64 {
                return false;
            }
            if nd < d[v][k] as u64 {
                if d[v][k] == u32::MAX {
                    self.seen.push(v as u32);
                }
                (d[v][k] = nd as u32, self.h.push(Reverse(nd << 32 | v as u64)));
            }
        }
        true
    }
    fn reset(&mut self, d: &mut [[u32; 2]]) {
        for &v in &self.seen {
            d[v as usize][self.k] = u32::MAX;
        }
        (self.seen.clear(), self.h.clear());
    }
}

// The same search with u64 distances in a hash map, for the few queries whose sums pass 2^32 - 2.
fn wide(off: &[u32], adj: &[(u32, u32)], s: usize, t: usize) -> u64 {
    let (mut dist, mut heap) = (HashMap::from([(s, 0u64)]), BinaryHeap::from([Reverse((0u64, s))]));
    while let Some(Reverse((du, u))) = heap.pop() {
        if u == t {
            return du;
        }
        if du > dist[&u] {
            continue;
        }
        for &(v, w) in &adj[off[u] as usize..off[u + 1] as usize] {
            let (v, nd) = (v as usize, du + w as u64);
            if dist.get(&v).map_or(true, |&x| nd < x) {
                (dist.insert(v, nd), heap.push(Reverse((nd, v))));
            }
        }
    }
    u64::MAX
}

// Runs f(0..nt) on nt threads and returns the results in thread order.
fn par<T: Send>(nt: usize, f: impl Fn(usize) -> T + Sync) -> Vec<T> {
    let f = &f;
    thread::scope(|sc| {
        let hs: Vec<_> = (0..nt).map(|t| sc.spawn(move || f(t))).collect();
        hs.into_iter().map(|h| h.join().unwrap()).collect()
    })
}

// CSR from edge lists (tail, head, weight) cut by tail range: list k of every part holds the tails in
// [k * c, (k + 1) * c), c = ceil(n / k_max). Thread k counts, sums and fills its own range.
// adj is a buffer to reuse: its pages are already faulted in, and every slot is written below.
fn csr(n: usize, parts: &[Vec<Vec<(u32, u32, u32)>>], mut adj: Vec<(u32, u32)>) -> (Vec<u32>, Vec<(u32, u32)>) {
    let c = n.div_ceil(parts[0].len());
    let sizes: Vec<usize> = (0..parts[0].len()).map(|k| parts.iter().map(|l| l[k].len()).sum()).collect();
    let mut off = vec![0u32; n + 1];
    (adj.clear(), adj.reserve(sizes.iter().sum()));
    unsafe { adj.set_len(sizes.iter().sum()) };
    thread::scope(|sc| {
        let (mut rest, mut base) = (&mut adj[..], 0);
        for (k, o) in off[1..].chunks_mut(c).enumerate() {
            let (a, r) = std::mem::take(&mut rest).split_at_mut(sizes[k]);
            rest = r;
            sc.spawn(move || {
                for &(u, _, _) in parts.iter().flat_map(|l| &l[k]) {
                    o[u as usize - k * c] += 1;
                }
                let mut s = base as u32;
                for x in o.iter_mut() {
                    (s += *x, *x = s); // end of each node
                }
                for &(u, v, w) in parts.iter().flat_map(|l| &l[k]) {
                    let j = &mut o[u as usize - k * c];
                    *j -= 1; // ends up at the node's start
                    a[*j as usize - base] = (v, w);
                }
            });
            base += sizes[k];
        }
    });
    off.copy_within(1.., 0); // off[x + 1] held the start of x
    off[n] = adj.len() as u32;
    (off, adj)
}

fn main() {
    let arg = |i: usize, def: &str| env::args().nth(i).unwrap_or(def.into());
    let (din, dout) = (arg(1, "/data"), arg(2, "/out"));
    let d = load(&format!("{din}/graph.txt"));
    let mut p = 0;
    let (n, _m) = (num(d, &mut p) as usize, num(d, &mut p));
    let nt = thread::available_parallelism().map_or(12, |n| n.get());
    let mut bs: Vec<usize> = (0..nt).map(|k| p + k * (d.len() - p) / nt).collect();
    for b in &mut bs[1..] {
        while d[*b - 1] != b'\n' {
            *b += 1; // to a line start
        }
    }
    bs.push(d.len());
    // Edges cut by tail range per thread, then a CSR built range by range: no atomic in either pass.
    let c = n.div_ceil(nt);
    let bs = &bs;
    let parts = par(nt, |t| {
        let mut l: Vec<Vec<_>> = (0..nt).map(|_| Vec::with_capacity((bs[t + 1] - bs[t]) / 16 / nt)).collect();
        edges(d, bs[t], bs[t + 1], |u, v, w| l[u / c].push((u as u32, v as u32, w)));
        l
    });
    let (off, adj) = csr(n, &parts, vec![]);
    // Renumber in BFS order so that nodes near each other in the graph are near each other in memory:
    // one BFS per thread from its own seed, nodes claimed by CAS, then the leftovers.
    let own: Vec<AtomicU32> = (0..n).map(|_| AtomicU32::new(u32::MAX)).collect();
    let bfs = |r: usize, t: u32, q: &mut Vec<u32>| {
        if own[r].compare_exchange(u32::MAX, t, Relaxed, Relaxed).is_err() {
            return;
        }
        let mut h = q.len();
        q.push(r as u32);
        while h < q.len() {
            let u = q[h] as usize;
            h += 1;
            for &(v, _) in &adj[off[u] as usize..off[u + 1] as usize] {
                if own[v as usize].load(Relaxed) == u32::MAX
                    && own[v as usize].compare_exchange(u32::MAX, t, Relaxed, Relaxed).is_ok()
                {
                    q.push(v);
                }
            }
        }
    };
    let mut lists = par(nt, |t| {
        let mut q = Vec::with_capacity(2 * n / nt);
        bfs(t * n / nt, t as u32, &mut q);
        q
    });
    // The leftovers (nodes no BFS reached) are found in parallel, then claimed in one thread.
    let cand = par(nt, |t| (t * n / nt..(t + 1) * n / nt).filter(|&r| own[r].load(Relaxed) == u32::MAX).collect::<Vec<_>>());
    let mut rest = vec![];
    for r in cand.concat() {
        bfs(r, nt as u32, &mut rest);
    }
    lists.push(rest);
    // The BFS orders laid end to end, each list copied by its own thread.
    let mut ord = vec![0u32; n];
    let (op, at) = (ord.as_mut_ptr() as usize, lists.iter().scan(0, |a, l| (*a, *a += l.len()).0.into()).collect::<Vec<usize>>());
    par(lists.len(), |k| unsafe { std::ptr::copy_nonoverlapping(lists[k].as_ptr(), (op as *mut u32).add(at[k]), lists[k].len()) });
    drop((lists, own));
    // New id of each node and the degrees in the new order, then the offsets, in parallel by position.
    let mut id = vec![0u32; n]; // every slot is written below
    let (ip, ordr, offr) = (id.as_mut_ptr() as usize, &ord, &off);
    let degs = par(nt, |t| {
        (t * n / nt..(t + 1) * n / nt)
            .map(|i| {
                let o = ordr[i] as usize;
                unsafe { *(ip as *mut u32).add(o) = i as u32 };
                offr[o + 1] - offr[o]
            })
            .collect::<Vec<u32>>()
    });
    let mut off2 = vec![0u32; n + 1];
    let (o2, degr) = (off2.as_mut_ptr() as usize, &degs);
    let base: Vec<u32> = degs.iter().scan(0, |a, c| (*a, *a += c.iter().sum::<u32>()).0.into()).collect();
    par(nt, |t| {
        let mut a = base[t];
        for (k, &g) in degr[t].iter().enumerate() {
            a += g;
            unsafe { *(o2 as *mut u32).add(t * n / nt + k + 1) = a };
        }
    });
    drop(degs);
    let mut adj2: Vec<(u32, u32)> = vec![(0, 0); adj.len()];
    let ap2 = adj2.as_mut_ptr() as usize;
    thread::scope(|sc| {
        for t in 0..nt {
            let (ord, id, off, off2, adj) = (&ord, &id, &off, &off2, &adj);
            sc.spawn(move || {
                for i in t * n / nt..(t + 1) * n / nt {
                    let o = ord[i] as usize;
                    for (k, &(v, w)) in adj[off[o] as usize..off[o + 1] as usize].iter().enumerate() {
                        unsafe { *(ap2 as *mut (u32, u32)).add(off2[i] as usize + k) = (id[v as usize], w) };
                    }
                }
            });
        }
    });
    drop(off);
    let (spare, (off, adj)) = (adj, (off2, adj2));
    // Reverse CSR for the backward half of the search, the same way, in the forward pass's buckets and edge
    // array (already faulted in: fresh pages are costly on the box).
    let mut parts = parts;
    thread::scope(|sc| {
        for (t, l) in parts.iter_mut().enumerate() {
            let (adj, off) = (&adj, &off);
            sc.spawn(move || {
                l.iter_mut().for_each(Vec::clear);
                for u in t * n / nt..(t + 1) * n / nt {
                    for &(v, w) in &adj[off[u] as usize..off[u + 1] as usize] {
                        l[v as usize / c].push((v, u as u32, w));
                    }
                }
            });
        }
    });
    let (roff, radj) = csr(n, &parts, spare);
    drop((parts, ord));
    let q = fs::read(format!("{din}/queries.txt")).unwrap();
    let (mut qs, mut p) = (vec![], 0);
    while p < q.len() {
        qs.push((id[num(&q, &mut p) as usize] as usize, id[num(&q, &mut p) as usize] as usize));
    }
    drop(id);
    let ans: Vec<AtomicU64> = qs.iter().map(|_| AtomicU64::new(0)).collect();
    let next = AtomicUsize::new(0);
    let (off, adj, roff, radj) = (&off, &adj, &roff, &radj);
    thread::scope(|sc| {
        for _ in 0..nt {
            let (qs, ans, next) = (&qs, &ans, &next);
            sc.spawn(move || {
                let mut d = vec![[u32::MAX; 2]; n];
                let (mut f, mut b) = (Side { k: 0, seen: vec![], h: BinaryHeap::new() }, Side { k: 1, seen: vec![], h: BinaryHeap::new() });
                loop {
                    let i = next.fetch_add(1, Relaxed);
                    let Some(&(s, t)) = qs.get(i) else { break };
                    let mut best = u64::MAX; // unreachable
                    if s == t {
                        best = 0;
                    } else if roff[t] < roff[t + 1] && off[s] < off[s + 1] {
                        (f.start(&mut d, s), b.start(&mut d, t));
                        loop {
                            let (x, y) = (f.top(), b.top());
                            if x.saturating_add(y) >= best {
                                break;
                            }
                            let ok = match x <= y {
                                true => f.step(off, adj, &mut d, &mut best),
                                false => b.step(roff, radj, &mut d, &mut best),
                            };
                            if !ok {
                                best = wide(off, adj, s, t);
                                break;
                            }
                        }
                        (f.reset(&mut d), b.reset(&mut d));
                    }
                    ans[i].store(best, Relaxed);
                }
            });
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
