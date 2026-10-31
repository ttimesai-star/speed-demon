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

const NO: u32 = u32::MAX;
const PG: usize = 512; // nodes per label page (4 KB of [u32; 2])

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
    // Every label is unset again (the sides reset theirs): the pages go back to the pool.
    fn release(&mut self) {
        for i in self.3.drain(..) {
            self.2.push(std::mem::replace(&mut self.0[i as usize], NO));
        }
    }
}

// One half of a bidirectional Dijkstra search: .0 = 0 forward, 1 backward, .1 the touched nodes, .2 a heap
// of dist << 32 | node. Both halves' u32 labels share one pair per node, so the meeting check at a settled
// node reads the cache line its own label is in.
struct Side(usize, Vec<u32>, BinaryHeap<Reverse<u64>>);

impl Side {
    fn top(&self) -> u64 {
        self.2.peek().map_or(u64::MAX, |r| r.0 >> 32)
    }
    fn start(&mut self, d: &mut Labels, s: usize) {
        d.at(s)[self.0] = 0;
        (self.1.push(s as u32), self.2.push(Reverse(s as u64)));
    }
    // Settles the nearest node; `best` takes its sum with the other half's label (checking at settle time
    // is enough for the stop rule top + top >= best). False if a sum does not fit in u32 (the weights near
    // 2^32 on the special nodes): the query then goes to `wide`.
    fn step(&mut self, off: &[u32], adj: &[(u32, u32)], d: &mut Labels, best: &mut u64) -> bool {
        let Reverse(e) = self.2.pop().unwrap();
        let (du, u, k) = (e >> 32, e as u32 as usize, self.0);
        let lu = d.get(u);
        if du > lu[k] as u64 {
            return true;
        }
        if lu[1 - k] != NO {
            *best = (*best).min(du + lu[1 - k] as u64);
        }
        for &(v, w) in &adj[off[u] as usize..off[u + 1] as usize] {
            let (v, nd) = (v as usize, du + w as u64);
            if nd >= NO as u64 {
                return false;
            }
            let l = d.at(v); // a missing page means an unset label, which nd improves anyway
            if nd < l[k] as u64 {
                if l[k] == NO {
                    self.1.push(v as u32);
                }
                (l[k] = nd as u32, self.2.push(Reverse(nd << 32 | v as u64)));
            }
        }
        true
    }
    fn reset(&mut self, d: &mut Labels) {
        for &v in &self.1 {
            d.at(v as usize)[self.0] = NO;
        }
        (self.1.clear(), self.2.clear());
    }
}

// The same search with u64 distances in a hash map, for the few queries whose sums pass 2^32 - 2.
fn wide(off: &[u32], adj: &[(u32, u32)], s: usize, t: usize) -> u64 {
    let mut dist = HashMap::from([(s, 0u64)]);
    let mut heap = BinaryHeap::from([Reverse((0u64, s))]);
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

// CSR from edge lists (tail, head, weight) cut by tail range: list k of every part holds the tails in
// [k * c, (k + 1) * c), c = ceil(n / k_max). Thread k counts, sums and fills its own range.
fn csr(n: usize, parts: &[Vec<Vec<(u32, u32, u32)>>]) -> (Vec<u32>, Vec<(u32, u32)>) {
    let c = n.div_ceil(parts[0].len());
    let sizes: Vec<usize> = (0..parts[0].len()).map(|k| parts.iter().map(|l| l[k].len()).sum()).collect();
    let mut off = vec![0u32; n + 1];
    let mut adj = Vec::with_capacity(sizes.iter().sum());
    unsafe { adj.set_len(sizes.iter().sum()) }; // every slot is written below
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
    let parts = par(nt, |t| {
        let mut l: Vec<Vec<_>> = (0..nt).map(|_| Vec::with_capacity((bs[t + 1] - bs[t]) / 16 / nt)).collect();
        let (mut p, e) = (bs[t], bs[t + 1]);
        while p < e {
            let (u, v, w) = (num(d, &mut p) as u32, num(d, &mut p) as u32, num(d, &mut p) as u32);
            if u != v {
                l[u as usize / c].push((u, v, w)); // a self-loop never shortens a path
            }
        }
        l
    });
    let (off, adj) = csr(n, &parts);
    drop(parts);
    // Renumber in BFS order so that nodes near each other in the graph are near each other in memory:
    // one BFS per thread from its own seed, nodes claimed by CAS, then the leftovers.
    let own = zeroed(n); // 0: unclaimed, else 1 + the claiming thread
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
            q.get(h + 4).map(|&x| adj.get(off[x as usize] as usize).map(pf)); // its offsets came with its push
            for &(v, _) in &adj[off[u] as usize..off[u + 1] as usize] {
                if own[v as usize].load(Relaxed) == 0 && claim(v as usize, t) {
                    (q.push(v), pf(&off[v as usize]));
                }
            }
        }
    };
    let mut lists = par(nt, |t| {
        let mut q = Vec::with_capacity(2 * n / nt);
        (bfs(t * n / nt, t as u32, &mut q), q).1
    });
    // The leftovers (nodes no BFS reached) are found in parallel, then claimed in one thread.
    let cand = par(nt, |t| rng(t).filter(|&r| own[r].load(Relaxed) == 0).collect::<Vec<_>>());
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
            let o = ord[i] as usize;
            unsafe { *(ip as *mut u32).add(o) = i as u32 };
            off[o + 1] - off[o]
        };
        rng(t).map(deg).collect::<Vec<u32>>()
    });
    let mut off2 = vec![0u32; n + 1];
    let o2 = off2.as_mut_ptr() as usize;
    let base: Vec<u32> = degs.iter().scan(0, |a, c| (*a, *a += c.iter().sum::<u32>()).0.into()).collect();
    par(nt, |t| {
        let mut a = base[t];
        for (k, &g) in degs[t].iter().enumerate() {
            a += g;
            unsafe { *(o2 as *mut u32).add(t * n / nt + k + 1) = a };
        }
    });
    drop(degs);
    let mut adj2: Vec<(u32, u32)> = Vec::with_capacity(adj.len());
    unsafe { adj2.set_len(adj.len()) }; // every slot is written below
    let ap2 = adj2.as_mut_ptr() as usize;
    par(nt, |t| {
        for i in rng(t) {
            // Three stages ahead: the offsets of row i + 16, the edges of row i + 8, the new ids of row i + 4.
            ord.get(i + 16).map(|&o| pf(&off[o as usize]));
            ord.get(i + 8).map(|&o| adj.get(off[o as usize] as usize).map(pf));
            if let Some(&o) = ord.get(i + 4) {
                adj[off[o as usize] as usize..off[o as usize + 1] as usize].iter().for_each(|e| pf(&id[e.0 as usize]));
            }
            let o = ord[i] as usize;
            for (k, &(v, w)) in adj[off[o] as usize..off[o + 1] as usize].iter().enumerate() {
                unsafe { *(ap2 as *mut (u32, u32)).add(off2[i] as usize + k) = (id[v as usize], w) };
            }
        }
    });
    drop(off);
    let (spare, (off, adj)) = (adj, (off2, adj2));
    // Reverse CSR for the backward half of the search, in place of the old edge array (already faulted in:
    // fresh pages are costly on the box). In-degrees counted with atomics, their ends as a parallel prefix
    // sum, each edge placed at its head's end minus one: after the renumbering a head is near its tail, so
    // a thread working a range of tails mostly touches counters no other thread does.
    let r = zeroed(n + 1);
    let row = |u: usize| adj[off[u] as usize..off[u + 1] as usize].iter().map(move |&(v, w)| (u, v, w));
    let rows = |t: usize| rng(t).flat_map(row);
    par(nt, |t| rows(t).for_each(|(_, v, _)| _ = r[v as usize].fetch_add(1, Relaxed)));
    let sums = par(nt, |t| rng(t).map(|v| r[v].load(Relaxed)).sum::<u32>());
    par(nt, |t| {
        let mut a = sums[..t].iter().sum::<u32>();
        for v in rng(t) {
            a += r[v].load(Relaxed);
            r[v].store(a, Relaxed);
        }
    });
    let mut radj = spare;
    radj.truncate(adj.len());
    let rp = radj.as_mut_ptr() as usize;
    par(nt, |t| {
        for (u, v, w) in rows(t) {
            let j = r[v as usize].fetch_sub(1, Relaxed) - 1;
            unsafe { *(rp as *mut (u32, u32)).add(j as usize) = (u as u32, w) };
        }
    });
    r[n].store(adj.len() as u32, Relaxed);
    let roff: Vec<u32> = r.into_iter().map(AtomicU32::into_inner).collect();
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
        let (mut f, mut b) = (Side(0, vec![], BinaryHeap::new()), Side(1, vec![], BinaryHeap::new()));
        while let Some(&i) = qo.get(next.fetch_add(1, Relaxed)) {
            let ((s, t), mut best) = (qs[i], u64::MAX); // MAX: unreachable
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
                        true => f.step(&off, &adj, &mut d, &mut best),
                        false => b.step(&roff, &radj, &mut d, &mut best),
                    };
                    if !ok {
                        best = wide(&off, &adj, s, t);
                        break;
                    }
                }
                (f.reset(&mut d), b.reset(&mut d), d.release());
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
