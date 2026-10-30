// Speed Demon 2026, track D (Traverse). Spec: specs/d-traverse.md in hackathon-raptors/speed-demon-2026.
use std::cmp::Reverse;
use std::collections::{BinaryHeap, HashMap};
use std::io::Write;
use std::sync::atomic::{AtomicU32, AtomicU64, AtomicUsize, Ordering::Relaxed};
use std::{env, fs, thread};

#[cfg(unix)]
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

#[cfg(not(unix))]
fn load(path: &str) -> &'static [u8] {
    fs::read(path).unwrap().leak()
}

// Reads the decimal number at d[*p..] and steps over it and the one separator after it.
fn num(d: &[u8], p: &mut usize) -> u64 {
    let mut x = 0;
    while d[*p] >= b'0' {
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

fn main() {
    let t0 = std::time::Instant::now();
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
    // CSR in two parallel passes: out-degrees (and in-degrees), then edges placed by atomic cursors.
    let deg: Vec<AtomicU32> = (0..n + 1).map(|_| AtomicU32::new(0)).collect();
    let ind: Vec<AtomicU32> = (0..n).map(|_| AtomicU32::new(0)).collect();
    let bs = &bs;
    thread::scope(|sc| {
        for t in 0..nt {
            let (deg, ind) = (&deg, &ind);
            sc.spawn(move || {
                edges(d, bs[t], bs[t + 1], |u, v, _| {
                    deg[u + 1].fetch_add(1, Relaxed);
                    ind[v].fetch_add(1, Relaxed);
                })
            });
        }
    });
    eprintln!("pass1 {:?}", t0.elapsed());
    let mut off: Vec<u32> = deg.into_iter().map(|x| x.into_inner()).collect();
    for i in 0..n {
        off[i + 1] += off[i];
    }
    let cur: Vec<AtomicU32> = off[..n].iter().map(|&x| AtomicU32::new(x)).collect();
    let mut adj: Vec<(u32, u32)> = vec![(0, 0); off[n] as usize];
    let ap = adj.as_mut_ptr() as usize;
    thread::scope(|sc| {
        for t in 0..nt {
            let cur = &cur;
            sc.spawn(move || {
                edges(d, bs[t], bs[t + 1], |u, v, w| {
                    let i = cur[u].fetch_add(1, Relaxed) as usize;
                    unsafe { *(ap as *mut (u32, u32)).add(i) = (v as u32, w) };
                })
            });
        }
    });
    drop(cur);
    eprintln!("csr {:?}", t0.elapsed());
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
    let mut lists: Vec<Vec<u32>> = thread::scope(|sc| {
        let hs: Vec<_> = (0..nt)
            .map(|t| {
                let bfs = &bfs;
                sc.spawn(move || {
                    let mut q = Vec::with_capacity(2 * n / nt);
                    bfs(t * n / nt, t as u32, &mut q);
                    q
                })
            })
            .collect();
        hs.into_iter().map(|h| h.join().unwrap()).collect()
    });
    let mut rest = vec![];
    for r in 0..n {
        bfs(r, nt as u32, &mut rest);
    }
    lists.push(rest);
    let ord: Vec<u32> = lists.concat();
    drop((lists, own));
    let mut id = vec![u32::MAX; n];
    for (i, &o) in ord.iter().enumerate() {
        id[o as usize] = i as u32;
    }
    eprintln!("bfs {:?}", t0.elapsed());
    let mut off2 = vec![0u32; n + 1];
    for i in 0..n {
        let o = ord[i] as usize;
        off2[i + 1] = off2[i] + off[o + 1] - off[o];
    }
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
    eprintln!("renum {:?}", t0.elapsed());
    drop((off, adj));
    let (off, adj) = (off2, adj2);
    let q = fs::read(format!("{din}/queries.txt")).unwrap();
    let (mut qs, mut p) = (vec![], 0);
    while p < q.len() {
        qs.push((num(&q, &mut p) as usize, num(&q, &mut p) as usize));
    }
    let ans: Vec<AtomicU64> = qs.iter().map(|_| AtomicU64::new(0)).collect();
    let next = AtomicUsize::new(0);
    let (off, adj, ind, id) = (&off, &adj, &ind, &id);
    thread::scope(|sc| {
        for _ in 0..nt {
            let (qs, ans, next) = (&qs, &ans, &next);
            sc.spawn(move || {
                let (mut dist, mut seen, mut heap) = (vec![u32::MAX; n], vec![], BinaryHeap::new());
                loop {
                    let i = next.fetch_add(1, Relaxed);
                    let Some(&(s0, q0)) = qs.get(i) else { break };
                    let (s, t) = (id[s0] as usize, id[q0] as usize);
                    let mut r = u64::MAX; // unreachable
                    if s == t {
                        r = 0;
                    } else if ind[q0].load(Relaxed) > 0 && off[s] < off[s + 1] {
                        // u32 distances, heap entries dist << 32 | node; a sum that does not fit goes to `wide`.
                        (dist[s] = 0, seen.push(s as u32), heap.push(Reverse(s as u64)));
                        while let Some(Reverse(e)) = heap.pop() {
                            let (du, u) = (e >> 32, e as u32 as usize);
                            if du > dist[u] as u64 {
                                continue;
                            }
                            if u == t {
                                r = du;
                                break;
                            }
                            for &(v, w) in &adj[off[u] as usize..off[u + 1] as usize] {
                                let (v, nd) = (v as usize, du + w as u64);
                                if nd >= u32::MAX as u64 {
                                    r = wide(off, adj, s, t);
                                    heap.clear();
                                    break;
                                }
                                if nd < dist[v] as u64 {
                                    if dist[v] == u32::MAX {
                                        seen.push(v as u32);
                                    }
                                    dist[v] = nd as u32;
                                    heap.push(Reverse(nd << 32 | v as u64));
                                }
                            }
                        }
                        for &v in &seen {
                            dist[v as usize] = u32::MAX;
                        }
                        (seen.clear(), heap.clear());
                    }
                    ans[i].store(r, Relaxed);
                }
            });
        }
    });
    eprintln!("queries {:?}", t0.elapsed());
    let mut out = Vec::with_capacity(qs.len() * 12);
    for a in &ans {
        match a.load(Relaxed) {
            u64::MAX => out.extend_from_slice(b"-1\n"),
            x => writeln!(out, "{x}").unwrap(),
        }
    }
    fs::create_dir_all(&dout).unwrap();
    fs::write(format!("{dout}/distances.txt"), out).unwrap();
    eprintln!("end {:?}", t0.elapsed());
}
