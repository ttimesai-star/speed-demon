// Speed Demon 2026, track D (Traverse). Spec: specs/d-traverse.md in hackathon-raptors/speed-demon-2026.
use std::cmp::Reverse;
use std::collections::BinaryHeap;
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
    let q = fs::read(format!("{din}/queries.txt")).unwrap();
    let (mut qs, mut p) = (vec![], 0);
    while p < q.len() {
        qs.push((num(&q, &mut p) as usize, num(&q, &mut p) as usize));
    }
    let ans: Vec<AtomicU64> = qs.iter().map(|_| AtomicU64::new(0)).collect();
    let next = AtomicUsize::new(0);
    let (off, adj, ind) = (&off, &adj, &ind);
    thread::scope(|sc| {
        for _ in 0..nt {
            let (qs, ans, next) = (&qs, &ans, &next);
            sc.spawn(move || {
                let (mut dist, mut seen, mut heap) = (vec![u64::MAX; n], vec![], BinaryHeap::new());
                loop {
                    let i = next.fetch_add(1, Relaxed);
                    let Some(&(s, t)) = qs.get(i) else { break };
                    let mut r = u64::MAX; // unreachable
                    if s == t {
                        r = 0;
                    } else if ind[t].load(Relaxed) > 0 && off[s] < off[s + 1] {
                        dist[s] = 0;
                        seen.push(s as u32);
                        heap.push(Reverse((0u64, s as u32)));
                        while let Some(Reverse((du, u))) = heap.pop() {
                            let u = u as usize;
                            if du > dist[u] {
                                continue;
                            }
                            if u == t {
                                r = du;
                                break;
                            }
                            for &(v, w) in &adj[off[u] as usize..off[u + 1] as usize] {
                                let (v, nd) = (v as usize, du + w as u64);
                                if nd < dist[v] {
                                    if dist[v] == u64::MAX {
                                        seen.push(v as u32);
                                    }
                                    dist[v] = nd;
                                    heap.push(Reverse((nd, v as u32)));
                                }
                            }
                        }
                        for &v in &seen {
                            dist[v as usize] = u64::MAX;
                        }
                        (seen.clear(), heap.clear());
                    }
                    ans[i].store(r, Relaxed);
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
